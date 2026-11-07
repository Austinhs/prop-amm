// cp_fee: plain constant-product AMM with a fee, i.e. the starter with a
// tunable FEE_BPS. Use it as the baseline every new idea has to beat.
// Try: ./sweep.sh strategies/cp_fee.rs FEE_BPS 20 30 50 80 120

use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64};

const NAME: &str = "cp_fee";
const MODEL_USED: &str = "Claude Opus 5.5";

const FEE_BPS: u128 = 80;

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

pub fn compute_swap(data: &[u8]) -> u64 {
    if data.len() < 25 {
        return 0;
    }
    let side = data[0];
    let input = rd_u64(data, 1) as u128;
    let rx = rd_u64(data, 9) as u128;
    let ry = rd_u64(data, 17) as u128;
    if rx == 0 || ry == 0 {
        return 0;
    }
    let k = rx * ry;
    let net = input * (10_000 - FEE_BPS) / 10_000;
    match side {
        0 => {
            let new_ry = ry + net;
            rx.saturating_sub((k + new_ry - 1) / new_ry) as u64
        }
        1 => {
            let new_rx = rx + net;
            ry.saturating_sub((k + new_rx - 1) / new_rx) as u64
        }
        _ => 0,
    }
}
