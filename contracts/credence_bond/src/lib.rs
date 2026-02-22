#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug)]
pub struct IdentityBond {
    pub identity: Address,
    pub bonded_amount: i128,
    pub bond_start: u64,
    pub bond_duration: u64,
    pub slashed_amount: i128,
    pub active: bool,
}

#[contract]
pub struct CredenceBond;

#[contractimpl]
impl CredenceBond {
    /// Initialize the contract (admin).
    pub fn initialize(e: Env, admin: Address) {
        e.storage().instance().set(&Symbol::new(&e, "admin"), &admin);
    }

    /// Create or top-up a bond for an identity. In a full implementation this would
    /// transfer USDC from the caller and store the bond.
    pub fn create_bond(
        e: Env,
        identity: Address,
        amount: i128,
        duration: u64,
    ) -> IdentityBond {
        let bond_start = e.ledger().timestamp();
        
        // Verify the end timestamp wouldn't overflow
        let _end_timestamp = bond_start.checked_add(duration)
            .expect("bond end timestamp would overflow");

        let bond = IdentityBond {
            identity: identity.clone(),
            bonded_amount: amount,
            bond_start,
            bond_duration: duration,
            slashed_amount: 0,
            active: true,
        };
        let key = Symbol::new(&e, "bond");
        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Return current bond state for an identity (simplified: single bond per contract instance).
    pub fn get_identity_state(e: Env) -> IdentityBond {
        e.storage()
            .instance()
            .get::<_, IdentityBond>(&Symbol::new(&e, "bond"))
            .unwrap_or_else(|| {
                panic!("no bond")
            })
    }

    /// Withdraw from bond. Checks that the bond has sufficient balance after accounting for slashed amount.
    /// Returns the updated bond with reduced bonded_amount.
    pub fn withdraw(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e.storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        // Calculate available balance (bonded - slashed)
        let available = bond.bonded_amount.checked_sub(bond.slashed_amount)
            .expect("slashed amount exceeds bonded amount");

        // Verify sufficient available balance for withdrawal
        if amount > available {
            panic!("insufficient balance for withdrawal");
        }

        // Perform withdrawal with overflow protection
        bond.bonded_amount = bond.bonded_amount.checked_sub(amount)
            .expect("withdrawal caused underflow");

        // Verify invariant: slashed amount should not exceed bonded amount after withdrawal
        if bond.slashed_amount > bond.bonded_amount {
            panic!("slashed amount exceeds bonded amount");
        }

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Slash a portion of the bond. Increases slashed_amount up to the bonded_amount.
    /// Returns the updated bond with increased slashed_amount.
    pub fn slash(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e.storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        // Calculate new slashed amount, checking for overflow
        let new_slashed = bond.slashed_amount.checked_add(amount)
            .expect("slashing caused overflow");

        // Cap slashed amount at bonded amount
        bond.slashed_amount = if new_slashed > bond.bonded_amount {
            bond.bonded_amount
        } else {
            new_slashed
        };

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Top up the bond with additional amount (checks for overflow)
    pub fn top_up(e: Env, amount: i128) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e.storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        // Perform top-up with overflow protection
        bond.bonded_amount = bond.bonded_amount.checked_add(amount)
            .expect("top-up caused overflow");

        e.storage().instance().set(&key, &bond);
        bond
    }

    /// Extend bond duration (checks for u64 overflow on timestamps)
    pub fn extend_duration(e: Env, additional_duration: u64) -> IdentityBond {
        let key = Symbol::new(&e, "bond");
        let mut bond = e.storage()
            .instance()
            .get::<_, IdentityBond>(&key)
            .unwrap_or_else(|| panic!("no bond"));

        // Perform duration extension with overflow protection
        bond.bond_duration = bond.bond_duration.checked_add(additional_duration)
            .expect("duration extension caused overflow");

        // Also verify the end timestamp wouldn't overflow
        let _end_timestamp = bond.bond_start.checked_add(bond.bond_duration)
            .expect("bond end timestamp would overflow");

        e.storage().instance().set(&key, &bond);
        bond
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod security;
