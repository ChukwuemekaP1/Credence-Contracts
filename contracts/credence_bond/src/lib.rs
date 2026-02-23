#![no_std]

mod early_exit_penalty;
mod emergency;
mod rolling_bond;
mod tiered_bond;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

/// Identity tier based on bonded amount (Bronze < Silver < Gold < Platinum).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BondTier {
    Bronze,
    Silver,
    Gold,
    Platinum,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct IdentityBond {
    pub identity: Address,
    pub bonded_amount: i128,
    pub bond_start: u64,
    pub bond_duration: u64,
    pub slashed_amount: i128,
    pub active: bool,
    /// If true, bond auto-renews at period end unless withdrawal was requested.
    pub is_rolling: bool,
    /// When withdrawal was requested (0 = not requested).
    pub withdrawal_requested_at: u64,
    /// Notice period duration for rolling bonds (seconds).
    pub notice_period_duration: u64,
}

#[contract]
pub struct CredenceBond;

#[contractimpl]
impl CredenceBond {
    /// Initialize the contract (admin).
    pub fn initialize(e: Env, admin: Address) {
        e.storage()
            .instance()
            .set(&Symbol::new(&e, "admin"), &admin);
    }

    /// Set early exit penalty config (admin only). Penalty in basis points (e.g. 500 = 5%).
    pub fn set_early_exit_config(e: Env, admin: Address, treasury: Address, penalty_bps: u32) {
        let stored_admin: Address = e
            .storage()
            .instance()
            .get(&Symbol::new(&e, "admin"))
            .unwrap_or_else(|| panic!("not initialized"));
        if admin != stored_admin {
            panic!("not admin");
        }
        early_exit_penalty::set_config(&e, treasury, penalty_bps);
    }

    /// @notice Configure emergency withdrawal controls.
    /// @dev Requires admin authorization and sets governance approver, treasury, fee, and enabled mode.
    /// @param admin Admin address authorized to configure emergency settings.
    /// @param governance Governance address required for elevated approval on emergency withdrawals.
    /// @param treasury Treasury receiving emergency fees.
    /// @param emergency_fee_bps Emergency fee in basis points (max 10000).
    /// @param enabled Initial emergency mode state.
    pub fn set_emergency_config(
        e: Env,
        admin: Address,
        governance: Address,
        treasury: Address,
        emergency_fee_bps: u32,
        enabled: bool,
    ) {
        let stored_admin: Address = e
            .storage()
            .instance()
            .get(&Symbol::new(&e, "admin"))
            .unwrap_or_else(|| panic!("not initialized"));
        if admin != stored_admin {
            panic!("not admin");
        }
        admin.require_auth();
        emergency::set_config(&e, governance, treasury, emergency_fee_bps, enabled);
    }

    /// @notice Toggle emergency mode with elevated governance approval.
    /// @dev Requires both admin and configured governance approvals.
    /// @param admin Admin approver.
    /// @param governance Governance approver.
    /// @param enabled New emergency mode status.
    pub fn set_emergency_mode(e: Env, admin: Address, governance: Address, enabled: bool) {
        let stored_admin: Address = e
            .storage()
            .instance()
            .get(&Symbol::new(&e, "admin"))
            .unwrap_or_else(|| panic!("not initialized"));
        if admin != stored_admin {
            panic!("not admin");
        }
        let cfg = emergency::get_config(&e);
        if governance != cfg.governance {
            panic!("not governance");
        }
        admin.require_auth();
        governance.require_auth();
        emergency::set_enabled(&e, enabled);
        emergency::emit_emergency_mode_event(&e, enabled, &admin, &governance);
    }

    /// @notice Execute emergency withdrawal during crisis mode.
    /// @dev Requires elevated approval from both admin and governance, applies emergency fee, emits event, and writes immutable audit record.
    /// @param admin Admin approver for emergency override.
    /// @param governance Governance approver for emergency override.
    /// @param amount Gross amount withdrawn from bond.
    /// @param reason Symbolic reason code for audit trail.
    /// @return Updated bond after emergency withdrawal.
    pub fn emergency_withdraw(
        e: Env,
        admin: Address,
        governance: Address,
        amount: i128,
        reason: Symbol,
    ) -> IdentityBond {
        let stored_admin: Address = e
            .storage()
            .instance()
            .get(&Symbol::new(&e, "admin"))
            .unwrap_or_else(|| panic!("not initialized"));
        if admin != stored_admin {
            panic!("not admin");
        }

        let cfg = emergency::get_config(&e);
        if governance != cfg.governance {
            panic!("not governance");
        }
        if !cfg.enabled {
            panic!("emergency mode disabled");
        }
        if amount <= 0 {
            panic!("amount must be positive");
        }

        admin.require_auth();
        governance.require_auth();

        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        let available = bond
            .bonded_amount
            .checked_sub(bond.slashed_amount)
            .expect("slashed amount exceeds bonded amount");
        if amount > available {
            panic!("insufficient balance for withdrawal");
        }

        let fee_amount = emergency::calculate_fee(amount, cfg.emergency_fee_bps);
        let net_amount = amount
            .checked_sub(fee_amount)
            .expect("emergency fee exceeds amount");

        let old_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        bond.bonded_amount = bond
            .bonded_amount
            .checked_sub(amount)
            .expect("withdrawal caused underflow");
        if bond.slashed_amount > bond.bonded_amount {
            panic!("slashed amount exceeds bonded amount");
        }
        let new_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        tiered_bond::emit_tier_change_if_needed(&e, &bond.identity, old_tier, new_tier);

        let record_id = emergency::store_record(
            &e,
            bond.identity.clone(),
            amount,
            fee_amount,
            net_amount,
            cfg.treasury.clone(),
            admin,
            governance,
            reason.clone(),
        );

        emergency::emit_emergency_withdrawal_event(
            &e,
            record_id,
            &bond.identity,
            amount,
            fee_amount,
            net_amount,
            &reason,
        );

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// @notice Return current emergency configuration.
    pub fn get_emergency_config(e: Env) -> emergency::EmergencyConfig {
        emergency::get_config(&e)
    }

    /// @notice Return latest emergency withdrawal record id (0 when none).
    pub fn get_latest_emergency_record_id(e: Env) -> u64 {
        emergency::latest_record_id(&e)
    }

    /// @notice Return immutable emergency withdrawal record by id.
    pub fn get_emergency_record(e: Env, id: u64) -> emergency::EmergencyWithdrawalRecord {
        emergency::get_record(&e, id)
    }

    /// Create or top-up a bond for an identity. In a full implementation this would
    /// transfer USDC from the caller and store the bond.
    /// For rolling bonds, set is_rolling true and notice_period_duration > 0.
    pub fn create_bond(
        e: Env,
        identity: Address,
        amount: i128,
        duration: u64,
        is_rolling: bool,
        notice_period_duration: u64,
    ) -> IdentityBond {
        let bond_start = e.ledger().timestamp();

        let _end_timestamp = bond_start
            .checked_add(duration)
            .expect("bond end timestamp would overflow");

        let bond = IdentityBond {
            identity: identity.clone(),
            bonded_amount: amount,
            bond_start,
            bond_duration: duration,
            slashed_amount: 0,
            active: true,
            is_rolling,
            withdrawal_requested_at: 0,
            notice_period_duration,
        };
        let key = Symbol::new(&e, "bond");
        e.storage().instance().set(&key, &bond);
        let tier = tiered_bond::get_tier_for_amount(amount);
        tiered_bond::emit_tier_change_if_needed(&e, &identity, BondTier::Bronze, tier);
        bond
    }

    /// Return current bond state for an identity (simplified: single bond per contract instance).
    pub fn get_identity_state(e: Env) -> IdentityBond {
        e.storage()
            .instance()
            .get::<_, IdentityBond>(&Symbol::new(&e, "bond"))
            .unwrap_or_else(|| panic!("no bond"))
    }

    /// Withdraw from bond (no penalty). Use when lock-up has ended or after notice period for rolling bonds.
    /// Checks that the bond has sufficient balance after accounting for slashed amount.
    pub fn withdraw(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        let available = bond
            .bonded_amount
            .checked_sub(bond.slashed_amount)
            .expect("slashed amount exceeds bonded amount");
        if amount > available {
            panic!("insufficient balance for withdrawal");
        }

        let old_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        bond.bonded_amount = bond
            .bonded_amount
            .checked_sub(amount)
            .expect("withdrawal caused underflow");
        if bond.slashed_amount > bond.bonded_amount {
            panic!("slashed amount exceeds bonded amount");
        }
        let new_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        tiered_bond::emit_tier_change_if_needed(&e, &bond.identity, old_tier, new_tier);

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Withdraw before lock-up end; applies early exit penalty and transfers penalty to treasury.
    /// Net amount to user = amount - penalty. Use when lock-up has not yet ended.
    pub fn withdraw_early(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        let available = bond
            .bonded_amount
            .checked_sub(bond.slashed_amount)
            .expect("slashed amount exceeds bonded amount");
        if amount > available {
            panic!("insufficient balance for withdrawal");
        }

        let now = e.ledger().timestamp();
        let end = bond.bond_start.saturating_add(bond.bond_duration);
        if now >= end {
            panic!("use withdraw for post lock-up");
        }

        let (treasury, penalty_bps) = early_exit_penalty::get_config(&e);
        let remaining = end.saturating_sub(now);
        let penalty = early_exit_penalty::calculate_penalty(
            amount,
            remaining,
            bond.bond_duration,
            penalty_bps,
        );
        early_exit_penalty::emit_penalty_event(&e, &bond.identity, amount, penalty, &treasury);
        // In a full implementation: transfer (amount - penalty) to user, penalty to treasury.

        let old_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        bond.bonded_amount = bond
            .bonded_amount
            .checked_sub(amount)
            .expect("withdrawal caused underflow");
        if bond.slashed_amount > bond.bonded_amount {
            panic!("slashed amount exceeds bonded amount");
        }
        let new_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        tiered_bond::emit_tier_change_if_needed(&e, &bond.identity, old_tier, new_tier);

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Request withdrawal (rolling bonds). Withdrawal allowed after notice period.
    pub fn request_withdrawal(e: Env) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));
        if !bond.is_rolling {
            panic!("not a rolling bond");
        }
        if bond.withdrawal_requested_at != 0 {
            panic!("withdrawal already requested");
        }
        bond.withdrawal_requested_at = e.ledger().timestamp();
        e.storage().instance().set(&key, &bond);
        e.events().publish(
            (Symbol::new(&e, "withdrawal_requested"),),
            (bond.identity.clone(), bond.withdrawal_requested_at),
        );
        bond
    }

    /// If bond is rolling and period has ended, renew (new period start = now). Emits renewal event.
    pub fn renew_if_rolling(e: Env) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));
        if !bond.is_rolling {
            return bond;
        }
        let now = e.ledger().timestamp();
        if !rolling_bond::is_period_ended(now, bond.bond_start, bond.bond_duration) {
            return bond;
        }
        rolling_bond::apply_renewal(&mut bond, now);
        e.storage().instance().set(&key, &bond);
        e.events().publish(
            (Symbol::new(&e, "bond_renewed"),),
            (bond.identity.clone(), bond.bond_start, bond.bond_duration),
        );
        bond
    }

    /// Get current tier for the bond's bonded amount.
    pub fn get_tier(e: Env) -> BondTier {
        let bond = Self::get_identity_state(e);
        tiered_bond::get_tier_for_amount(bond.bonded_amount)
    }

    /// Slash a portion of the bond (admin only). Increases slashed_amount up to the bonded_amount.
    /// Returns the updated bond with increased slashed_amount.
    pub fn slash(e: Env, admin: Address, amount: i128) -> IdentityBond {
        let stored_admin: Address = e
            .storage()
            .instance()
            .get(&Symbol::new(&e, "admin"))
            .unwrap_or_else(|| panic!("not initialized"));
        if admin != stored_admin {
            panic!("not admin");
        }
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        let new_slashed = bond
            .slashed_amount
            .checked_add(amount)
            .expect("slashing caused overflow");
        bond.slashed_amount = if new_slashed > bond.bonded_amount {
            bond.bonded_amount
        } else {
            new_slashed
        };

        e.storage().instance().set(&key, &bond);
        e.events().publish(
            (Symbol::new(&e, "slashed"),),
            (bond.identity.clone(), amount, bond.slashed_amount),
        );
        bond
    }

    /// Top up the bond with additional amount (checks for overflow). May emit tier_changed.
    pub fn top_up(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        let old_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        bond.bonded_amount = bond
            .bonded_amount
            .checked_add(amount)
            .expect("top-up caused overflow");
        let new_tier = tiered_bond::get_tier_for_amount(bond.bonded_amount);
        tiered_bond::emit_tier_change_if_needed(&e, &bond.identity, old_tier, new_tier);

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Extend bond duration (checks for u64 overflow on timestamps)
    pub fn extend_duration(e: Env, additional_duration: u64) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e
            .storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        // Perform duration extension with overflow protection
        bond.bond_duration = bond
            .bond_duration
            .checked_add(additional_duration)
            .expect("duration extension caused overflow");

        // Also verify the end timestamp wouldn't overflow
        let _end_timestamp = bond
            .bond_start
            .checked_add(bond.bond_duration)
            .expect("bond end timestamp would overflow");

        e.storage().instance().set(&key, &bond);
        bond
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod security;

#[cfg(test)]
mod test_early_exit_penalty;

#[cfg(test)]
mod test_rolling_bond;

#[cfg(test)]
mod test_tiered_bond;

#[cfg(test)]
mod test_slashing;

#[cfg(test)]
mod test_emergency;
