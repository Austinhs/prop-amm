// v1_recenter: a "prop AMM" that quotes around its own fair-price estimate.
//
// Idea:
//   * Keep an estimate `p` of the fair price (Y per X) in storage.
//   * Quote a constant-product curve whose virtual reserves are centred at
//     ask = p*(1+s) for buys and bid = p*(1-s) for sells. Output is
//     x_v*in/(y_v+in), which is monotonic and concave, so it passes checks.
//   * Every step the arbitrageur trades first. An arb pushes our marginal
//     price exactly to the fair price, so after the FIRST trade of a step we
//     read the post-trade marginal price and re-centre `p` on it.
//     (Plain x*y=k with a fee leaves its mid a full fee away from fair after
//     an arb; re-centring removes that bias.)
//   * Retail trades carry no price information, so by default they don't
//     move `p` (RETAIL_TRUST_PCT = 0).
//
// Tunables are plain `const`s so sweep.sh can vary them.

use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "v1_recenter";
const MODEL_USED: &str = "Claude Opus 5.5";

const SPREAD_BPS: u128 = 40; // half-spread around the fair-price estimate
const DEPTH_PCT: u128 = 100; // virtual depth as % of min(reserve_x, reserve_y / p)
const ARB_TRUST_PCT: i128 = 50; // how far to move p toward the implied price on a step's first trade
const RETAIL_TRUST_PCT: i128 = 0; // same, for later trades in the step

const NANO: u128 = 1_000_000_000;
const STORAGE_SIZE: usize = 1024;
const SWAP_HEADER: usize = 25;
const AFTER_HEADER: usize = 42;
// storage layout: [0..8] p (nano Y per X), [8..16] last step + 1 (0 = none yet)

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if instruction_data.is_empty() {
        return Ok(());
    }
    match instruction_data[0] {
        0 | 1 => set_return_data_u64(compute_swap(instruction_data)),
        2 => {
            if instruction_data.len() >= AFTER_HEADER + STORAGE_SIZE {
                let mut buf = [0u8; STORAGE_SIZE];
                buf.copy_from_slice(&instruction_data[AFTER_HEADER..AFTER_HEADER + STORAGE_SIZE]);
                after_swap(instruction_data, &mut buf);
                let _ = set_storage(&buf);
            }
        }
        3 => set_return_data_bytes(NAME.as_bytes()),
        4 => set_return_data_bytes(get_model_used().as_bytes()),
        _ => {}
    }
    Ok(())
}

pub fn get_model_used() -> &'static str {
    MODEL_USED
}

#[inline]
fn rd_u64(d: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    u64::from_le_bytes(b)
}

/// Fair-price estimate from storage, falling back to the pool ratio.
#[inline]
fn load_p(storage: &[u8], rx: u128, ry: u128) -> u128 {
    let p = if storage.len() >= 8 { rd_u64(storage, 0) as u128 } else { 0 };
    if p > 0 {
        p
    } else {
        ry * NANO / rx
    }
}

/// Virtual X depth, capped so outputs can never exceed real reserves.
#[inline]
fn depth(rx: u128, ry: u128, p: u128) -> u128 {
    let base = core::cmp::min(rx, ry * NANO / p);
    base * DEPTH_PCT / 100
}

#[inline]
fn ask(p: u128) -> u128 {
    p * (10_000 + SPREAD_BPS) / 10_000
}

#[inline]
fn bid(p: u128) -> u128 {
    p * (10_000 - SPREAD_BPS) / 10_000
}

pub fn compute_swap(data: &[u8]) -> u64 {
    if data.len() < SWAP_HEADER {
        return 0;
    }
    let side = data[0];
    let input = rd_u64(data, 1) as u128;
    let rx = rd_u64(data, 9) as u128;
    let ry = rd_u64(data, 17) as u128;
    if rx == 0 || ry == 0 || input == 0 {
        return 0;
    }
    let p = load_p(&data[SWAP_HEADER..], rx, ry);
    if p == 0 {
        return 0;
    }
    let l = depth(rx, ry, p);
    if l == 0 {
        return 0;
    }

    let out = match side {
        // trader pays Y, receives X
        0 => {
            let y_v = l * ask(p) / NANO;
            l * input / (y_v + input)
        }
        // trader pays X, receives Y
        1 => {
            let y_v = l * bid(p) / NANO;
            y_v * input / (l + input)
        }
        _ => 0,
    };

    // Hard cap below real reserves (a flat cap keeps the curve concave).
    let cap = if side == 0 { rx * 99 / 100 } else { ry * 99 / 100 };
    core::cmp::min(out, cap) as u64
}

pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    if data.len() < AFTER_HEADER || storage.len() < 16 {
        return;
    }
    let side = data[1];
    let input = rd_u64(data, 2) as u128;
    let output = rd_u64(data, 10) as u128;
    let rx_post = rd_u64(data, 18) as u128;
    let ry_post = rd_u64(data, 26) as u128;
    let step = rd_u64(data, 34);

    // Reconstruct the reserves compute_swap saw when this trade was quoted.
    let (rx, ry) = if side == 0 {
        (rx_post + output, ry_post.saturating_sub(input))
    } else {
        (rx_post.saturating_sub(input), ry_post + output)
    };
    if rx == 0 || ry == 0 {
        return;
    }
    let p = load_p(storage, rx, ry);
    let l = depth(rx, ry, p);
    if l == 0 {
        return;
    }

    // Marginal price on our virtual curve right after the trade.
    let implied: u128 = if side == 0 {
        let y_v = l * ask(p) / NANO + input;
        let x_v = l.saturating_sub(output);
        if x_v == 0 {
            return;
        }
        // that marginal is an ask; the arb stops when ask == fair
        y_v * NANO / x_v
    } else {
        let y_v = (l * bid(p) / NANO).saturating_sub(output);
        let x_v = l + input;
        y_v * NANO / x_v
    };

    let last_step_plus1 = rd_u64(storage, 8);
    let first_in_step = last_step_plus1 != step + 1;
    let trust = if first_in_step { ARB_TRUST_PCT } else { RETAIL_TRUST_PCT };

    let p_i = p as i128;
    let mut p_new = p_i + trust * (implied as i128 - p_i) / 100;
    if p_new < 1 {
        p_new = 1;
    }

    storage[0..8].copy_from_slice(&(p_new as u64).to_le_bytes());
    storage[8..16].copy_from_slice(&(step + 1).to_le_bytes());
}
