//! Emergency Withdrawal Mechanism
//!
//! Enables governance-approved withdrawals in crisis scenarios with mandatory
//! fee application, event emission, and immutable audit records.

use soroban_sdk::{contracttype, Address, Env, Symbol};

/// Storage key for emergency configuration.
const KEY_EMERGENCY_CONFIG: &str = "emergency_config";
/// Storage key for latest emergency withdrawal record id.
const KEY_EMERGENCY_RECORD_SEQ: &str = "emergency_record_seq";

/// Emergency mode configuration.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyConfig {
    pub governance: Address,
    pub treasury: Address,
    pub emergency_fee_bps: u32,
    pub enabled: bool,
}

/// Immutable audit record for an emergency withdrawal execution.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyWithdrawalRecord {
    pub id: u64,
    pub identity: Address,
    pub gross_amount: i128,
    pub fee_amount: i128,
    pub net_amount: i128,
    pub treasury: Address,
    pub approved_admin: Address,
    pub approved_governance: Address,
    pub reason: Symbol,
    pub timestamp: u64,
}

/// Dynamic key for emergency audit records.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmergencyDataKey {
    Record(u64),
}

/// Set emergency configuration.
pub fn set_config(
    e: &Env,
    governance: Address,
    treasury: Address,
    emergency_fee_bps: u32,
    enabled: bool,
) {
    if emergency_fee_bps > 10_000 {
        panic!("emergency fee bps must be <= 10000 (100%)");
    }
    let cfg = EmergencyConfig {
        governance,
        treasury,
        emergency_fee_bps,
        enabled,
    };
    e.storage()
        .instance()
        .set(&Symbol::new(e, KEY_EMERGENCY_CONFIG), &cfg);
}

/// Get emergency configuration. Panics if unset.
pub fn get_config(e: &Env) -> EmergencyConfig {
    e.storage()
        .instance()
        .get::<_, EmergencyConfig>(&Symbol::new(e, KEY_EMERGENCY_CONFIG))
        .unwrap_or_else(|| panic!("emergency config not set"))
}

/// Update emergency enabled state.
pub fn set_enabled(e: &Env, enabled: bool) {
    let mut cfg = get_config(e);
    cfg.enabled = enabled;
    e.storage()
        .instance()
        .set(&Symbol::new(e, KEY_EMERGENCY_CONFIG), &cfg);
}

/// Calculates emergency fee for withdrawal amount.
#[must_use]
pub fn calculate_fee(amount: i128, fee_bps: u32) -> i128 {
    if fee_bps == 0 {
        return 0;
    }
    amount
        .checked_mul(fee_bps as i128)
        .expect("emergency fee multiplication overflow")
        / 10_000
}

/// Persist an immutable emergency withdrawal record and return record id.
pub fn store_record(
    e: &Env,
    identity: Address,
    gross_amount: i128,
    fee_amount: i128,
    net_amount: i128,
    treasury: Address,
    approved_admin: Address,
    approved_governance: Address,
    reason: Symbol,
) -> u64 {
    let next_id = e
        .storage()
        .instance()
        .get::<_, u64>(&Symbol::new(e, KEY_EMERGENCY_RECORD_SEQ))
        .unwrap_or(0)
        .checked_add(1)
        .expect("emergency record id overflow");

    let record = EmergencyWithdrawalRecord {
        id: next_id,
        identity,
        gross_amount,
        fee_amount,
        net_amount,
        treasury,
        approved_admin,
        approved_governance,
        reason,
        timestamp: e.ledger().timestamp(),
    };

    e.storage()
        .instance()
        .set(&Symbol::new(e, KEY_EMERGENCY_RECORD_SEQ), &next_id);
    e.storage()
        .instance()
        .set(&EmergencyDataKey::Record(next_id), &record);
    next_id
}

/// Get latest emergency withdrawal record id, 0 if no records.
#[must_use]
pub fn latest_record_id(e: &Env) -> u64 {
    e.storage()
        .instance()
        .get::<_, u64>(&Symbol::new(e, KEY_EMERGENCY_RECORD_SEQ))
        .unwrap_or(0)
}

/// Get emergency withdrawal record by id.
pub fn get_record(e: &Env, id: u64) -> EmergencyWithdrawalRecord {
    e.storage()
        .instance()
        .get::<_, EmergencyWithdrawalRecord>(&EmergencyDataKey::Record(id))
        .unwrap_or_else(|| panic!("emergency record not found"))
}

/// Emit emergency mode event.
pub fn emit_emergency_mode_event(e: &Env, enabled: bool, admin: &Address, governance: &Address) {
    e.events().publish(
        (Symbol::new(e, "emergency_mode"),),
        (
            enabled,
            admin.clone(),
            governance.clone(),
            e.ledger().timestamp(),
        ),
    );
}

/// Emit emergency withdrawal event.
pub fn emit_emergency_withdrawal_event(
    e: &Env,
    record_id: u64,
    identity: &Address,
    gross_amount: i128,
    fee_amount: i128,
    net_amount: i128,
    reason: &Symbol,
) {
    e.events().publish(
        (Symbol::new(e, "emergency_withdrawal"),),
        (
            record_id,
            identity.clone(),
            gross_amount,
            fee_amount,
            net_amount,
            reason.clone(),
            e.ledger().timestamp(),
        ),
    );
}
