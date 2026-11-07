// v4_flat: flat-then-curved quotes around a fair-price estimate.
//
// Curve (each side): a FLAT segment at the inside price (ask or bid) for the
// first FLAT_Y worth of Y, then a constant-product tail whose starting slope
// equals the flat price (so the whole curve stays concave). Tail depth is
// DEPTH_PCT% of min(reserve_x, reserve_y/p).
//   * Typical retail orders fit inside the flat segment: no price impact,
//     so the router sends them to us instead of the normalizer.
//   * The tail limits how much an arbitrageur can take.
//   * Classification for free: an arb always eats the whole flat segment
//     and pushes into the tail until our marginal price equals the fair price.
//     So only trades that reach the tail re-centre `p`. Trades that stay flat
//     are treated as retail (no information).
// Spread adapts to volatility measured between consecutive arb-implied prices
// (the v3 estimator).

use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "Curvance.com";
const MODEL_USED: &str = "Claude Opus 5.5";

const FLAT_Y: u128 = 0; // flat segment size, whole Y units
const DEPTH_PCT: u128 = 50; // tail virtual depth, % of min(reserve_x, reserve_y / p)
const MIN_SPREAD_BPS: u128 = 35; // floor on half-spread
const MAX_SPREAD_BPS: u128 = 400; // ceiling on half-spread
const K_PCT: u128 = 50; // half-spread = K_PCT/100 * sigma estimate (bps); 0 = constant MIN_SPREAD_BPS
const INIT_SIGMA_BPS: u128 = 30; // prior sigma before any data
const VOL_SHIFT: u32 = 5; // EWMA weight = 1/2^VOL_SHIFT
const ARB_TRUST_PCT: i128 = 100; // re-centre % for a tail trade that is first in its step
const LATE_TAIL_TRUST_PCT: i128 = 0; // re-centre % for a tail trade later in the step (big retail)

const NANO: u128 = 1_000_000_000;
const STORAGE_SIZE: usize = 1024;
const SWAP_HEADER: usize = 25;
const AFTER_HEADER: usize = 42;
// storage layout (u64 LE each):
// [0..8]   p, nano Y per X (0 = unset)
// [8..16]  last trade step + 1
// [16..24] variance estimate, ppm^2 per step (0 = unset)
// [24..32] current half-spread bps (0 = unset)
// [32..40] step of last implied-price observation + 1
// [40..48] last implied price, nano (0 = none)

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

#[inline]
fn wr_u64(d: &mut [u8], off: usize, v: u64) {
    d[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[inline]
fn spread_for_var(var_ppm2: u128) -> u128 {
    let sigma_bps = isqrt(var_ppm2) / 100;
    (K_PCT * sigma_bps / 100).max(MIN_SPREAD_BPS).min(MAX_SPREAD_BPS)
}

#[inline]
fn init_var() -> u128 {
    let s = INIT_SIGMA_BPS * 100;
    s * s
}

#[inline]
fn load_spread(storage: &[u8]) -> u128 {
    let s = if storage.len() >= 32 { rd_u64(storage, 24) as u128 } else { 0 };
    if s > 0 {
        s
    } else {
        spread_for_var(init_var())
    }
}

#[inline]
fn load_p(storage: &[u8], rx: u128, ry: u128) -> u128 {
    let p = if storage.len() >= 8 { rd_u64(storage, 0) as u128 } else { 0 };
    if p > 0 {
        p
    } else {
        ry * NANO / rx
    }
}

#[inline]
fn depth(rx: u128, ry: u128, p: u128) -> u128 {
    let base = core::cmp::min(rx, ry * NANO / p);
    (base * DEPTH_PCT / 100).max(1)
}

/// Everything compute_swap and after_swap must agree on.
struct Quote {
    ask: u128,  // nano Y per X
    bid: u128,  // nano Y per X
    q_y: u128,  // flat segment on buy side, nano Y of input
    q_x: u128,  // flat segment on sell side, nano X of input
    l: u128,    // tail depth, nano X
}

#[inline]
fn make_quote(storage: &[u8], rx: u128, ry: u128) -> Option<Quote> {
    let p = load_p(storage, rx, ry);
    if p == 0 {
        return None;
    }
    let s = load_spread(storage);
    let q_y = FLAT_Y * NANO;
    Some(Quote {
        ask: p * (10_000 + s) / 10_000,
        bid: p * (10_000 - s) / 10_000,
        q_y,
        q_x: q_y * NANO / p,
        l: depth(rx, ry, p),
    })
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
    let q = match make_quote(&data[SWAP_HEADER..], rx, ry) {
        Some(q) => q,
        None => return 0,
    };
    if q.ask == 0 || q.bid == 0 {
        return 0;
    }

    let out = match side {
        // trader pays Y, receives X
        0 => {
            if input <= q.q_y {
                input * NANO / q.ask
            } else {
                let base = q.q_y * NANO / q.ask;
                let d = input - q.q_y;
                let y_t = q.l * q.ask / NANO;
                base + q.l * d / (y_t + d)
            }
        }
        // trader pays X, receives Y
        1 => {
            if input <= q.q_x {
                input * q.bid / NANO
            } else {
                let base = q.q_x * q.bid / NANO;
                let d = input - q.q_x;
                let y_t = q.l * q.bid / NANO;
                base + y_t * d / (q.l + d)
            }
        }
        _ => 0,
    };
    let cap = if side == 0 { rx * 99 / 100 } else { ry * 99 / 100 };
    core::cmp::min(out, cap) as u64
}

pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    if data.len() < AFTER_HEADER || storage.len() < 48 {
        return;
    }
    let side = data[1];
    let input = rd_u64(data, 2) as u128;
    let output = rd_u64(data, 10) as u128;
    let rx_post = rd_u64(data, 18) as u128;
    let ry_post = rd_u64(data, 26) as u128;
    let step = rd_u64(data, 34);

    let (rx, ry) = if side == 0 {
        (rx_post + output, ry_post.saturating_sub(input))
    } else {
        (rx_post.saturating_sub(input), ry_post + output)
    };
    if rx == 0 || ry == 0 {
        return;
    }
    let p = load_p(storage, rx, ry);
    let q = match make_quote(storage, rx, ry) {
        Some(q) => q,
        None => return,
    };

    let first_in_step = rd_u64(storage, 8) != step + 1;
    wr_u64(storage, 8, step + 1);

    // Did the trade reach the tail? If so, the marginal price afterwards is informative.
    let implied: Option<u128> = if side == 0 {
        if input > q.q_y {
            let d = input - q.q_y;
            let y_t = q.l * q.ask / NANO;
            let out_t = q.l * d / (y_t + d);
            if q.l > out_t {
                Some((y_t + d) * NANO / (q.l - out_t))
            } else {
                None
            }
        } else {
            None
        }
    } else if input > q.q_x {
        let d = input - q.q_x;
        let y_t = q.l * q.bid / NANO;
        let out_t = y_t * d / (q.l + d);
        Some(y_t.saturating_sub(out_t) * NANO / (q.l + d))
    } else {
        None
    };

    let implied = match implied {
        Some(v) if v > 0 => v,
        _ => {
            // Flat-only trade: treat as retail, but make sure p is persisted.
            if rd_u64(storage, 0) == 0 {
                wr_u64(storage, 0, p as u64);
            }
            return;
        }
    };

    let trust = if first_in_step { ARB_TRUST_PCT } else { LATE_TAIL_TRUST_PCT };

    if first_in_step {
        // Volatility from consecutive arb-implied prices (independent of our spread).
        let last_upd = rd_u64(storage, 32);
        let dt: u128 = if last_upd == 0 {
            (step as u128) + 1
        } else {
            ((step + 1).saturating_sub(last_upd) as u128).max(1)
        };
        let last_implied = rd_u64(storage, 40) as u128;
        if last_implied > 0 {
            let diff = if implied > last_implied { implied - last_implied } else { last_implied - implied };
            let r_ppm = diff * 1_000_000 / last_implied;
            let sample = (r_ppm * r_ppm / dt) as i128;
            let mut var = rd_u64(storage, 16) as u128;
            if var == 0 {
                var = init_var();
            }
            let var_i = var as i128;
            let new_var = ((var_i + ((sample - var_i) >> VOL_SHIFT)).max(1) as u128).min(u64::MAX as u128);
            wr_u64(storage, 16, new_var as u64);
            wr_u64(storage, 24, spread_for_var(new_var) as u64);
        }
        wr_u64(storage, 40, implied.min(u64::MAX as u128) as u64);
        wr_u64(storage, 32, step + 1);
    }

    let p_i = p as i128;
    let mut p_new = p_i + trust * (implied as i128 - p_i) / 100;
    if p_new < 1 {
        p_new = 1;
    }
    wr_u64(storage, 0, p_new as u64);
}
