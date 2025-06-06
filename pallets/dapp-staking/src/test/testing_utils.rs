// This file is part of Astar.

// Copyright (C) Stake Technologies Pte.Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later

// Astar is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Astar is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Astar. If not, see <http://www.gnu.org/licenses/>.

use crate::test::mock::*;
use crate::types::*;
use crate::{
    pallet::Config, ActiveProtocolState, ContractStake, CurrentEraInfo, DAppId, DAppTiers,
    EraRewards, Event, FreezeReason, HistoryCleanupMarker, IntegratedDApps, Ledger, NextDAppId,
    PeriodEnd, PeriodEndInfo, StakerInfo,
};

use frame_support::{
    assert_ok,
    traits::{fungible::InspectFreeze, Currency, Get, OnIdle},
    weights::Weight,
};
use sp_runtime::{traits::Zero, Perbill};
use std::collections::HashMap;

use astar_primitives::{
    dapp_staking::{CycleConfiguration, EraNumber, PeriodNumber},
    Balance, BlockNumber,
};

/// Helper struct used to store the entire pallet state snapshot.
/// Used when comparison of before/after states is required.
#[derive(Debug, Clone)]
pub(crate) struct MemorySnapshot {
    active_protocol_state: ProtocolState,
    next_dapp_id: DAppId,
    current_era_info: EraInfo,
    integrated_dapps: HashMap<
        <Test as Config>::SmartContract,
        DAppInfo<<Test as frame_system::Config>::AccountId>,
    >,
    ledger: HashMap<<Test as frame_system::Config>::AccountId, AccountLedgerFor<Test>>,
    staker_info: HashMap<
        (
            <Test as frame_system::Config>::AccountId,
            <Test as Config>::SmartContract,
        ),
        SingularStakingInfo,
    >,
    contract_stake: HashMap<DAppId, ContractStakeAmount>,
    era_rewards: HashMap<EraNumber, EraRewardSpan<<Test as Config>::EraRewardSpanLength>>,
    period_end: HashMap<PeriodNumber, PeriodEndInfo>,
    dapp_tiers: HashMap<EraNumber, DAppTierRewardsFor<Test>>,
    cleanup_marker: CleanupMarker,
}

impl MemorySnapshot {
    /// Generate a new memory snapshot, capturing entire dApp staking pallet state.
    pub fn new() -> Self {
        Self {
            active_protocol_state: ActiveProtocolState::<Test>::get(),
            next_dapp_id: NextDAppId::<Test>::get(),
            current_era_info: CurrentEraInfo::<Test>::get(),
            integrated_dapps: IntegratedDApps::<Test>::iter().collect(),
            ledger: Ledger::<Test>::iter().collect(),
            staker_info: StakerInfo::<Test>::iter()
                .map(|(k1, k2, v)| ((k1, k2), v))
                .collect(),
            contract_stake: ContractStake::<Test>::iter().collect(),
            era_rewards: EraRewards::<Test>::iter().collect(),
            period_end: PeriodEnd::<Test>::iter().collect(),
            dapp_tiers: DAppTiers::<Test>::iter().collect(),
            cleanup_marker: HistoryCleanupMarker::<Test>::get(),
        }
    }

    /// Returns locked balance in dApp staking for the specified account.
    /// In case no balance is locked, returns zero.
    pub fn locked_balance(&self, account: &AccountId) -> Balance {
        self.ledger.get(&account).map_or(Balance::zero(), |ledger| {
            ledger.locked + ledger.unlocking.iter().fold(0, |acc, x| acc + x.amount)
        })
    }
}

/// Register contract for staking and assert success.
pub(crate) fn assert_register(owner: AccountId, smart_contract: &MockSmartContract) {
    // Init check to ensure smart contract hasn't already been integrated
    assert!(!IntegratedDApps::<Test>::contains_key(smart_contract));
    let pre_snapshot = MemorySnapshot::new();

    // Register smart contract
    assert_ok!(DappStaking::register(
        RuntimeOrigin::root(),
        owner,
        smart_contract.clone()
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::DAppRegistered {
        owner,
        smart_contract: smart_contract.clone(),
        dapp_id: pre_snapshot.next_dapp_id,
    }));

    // Verify post-state
    let dapp_info = IntegratedDApps::<Test>::get(smart_contract).unwrap();
    assert_eq!(dapp_info.owner, owner);
    assert_eq!(dapp_info.id, pre_snapshot.next_dapp_id);
    assert!(dapp_info.reward_beneficiary.is_none());

    assert_eq!(pre_snapshot.next_dapp_id + 1, NextDAppId::<Test>::get());
    assert_eq!(
        pre_snapshot.integrated_dapps.len() + 1,
        IntegratedDApps::<Test>::count() as usize
    );
}

/// Update dApp reward destination and assert success
pub(crate) fn assert_set_dapp_reward_beneficiary(
    owner: AccountId,
    smart_contract: &MockSmartContract,
    beneficiary: Option<AccountId>,
) {
    // Change reward destination
    assert_ok!(DappStaking::set_dapp_reward_beneficiary(
        RuntimeOrigin::signed(owner),
        smart_contract.clone(),
        beneficiary,
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(
        Event::DAppRewardDestinationUpdated {
            smart_contract: smart_contract.clone(),
            beneficiary: beneficiary,
        },
    ));

    // Sanity check & reward destination update
    assert_eq!(
        IntegratedDApps::<Test>::get(&smart_contract)
            .unwrap()
            .reward_beneficiary,
        beneficiary
    );
}

/// Update dApp owner and assert success.
/// if `caller` is `None`, `Root` origin is used, otherwise standard `Signed` origin is used.
pub(crate) fn assert_set_dapp_owner(
    caller: Option<AccountId>,
    smart_contract: &MockSmartContract,
    new_owner: AccountId,
) {
    let origin = caller.map_or(RuntimeOrigin::root(), |owner| RuntimeOrigin::signed(owner));

    // Change dApp owner
    assert_ok!(DappStaking::set_dapp_owner(
        origin,
        smart_contract.clone(),
        new_owner,
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::DAppOwnerChanged {
        smart_contract: smart_contract.clone(),
        new_owner,
    }));

    // Verify post-state
    assert_eq!(
        IntegratedDApps::<Test>::get(&smart_contract).unwrap().owner,
        new_owner
    );
}

/// Update dApp status to unregistered and assert success.
pub(crate) fn assert_unregister(smart_contract: &MockSmartContract) {
    let pre_snapshot = MemorySnapshot::new();

    // Unregister dApp
    assert_ok!(DappStaking::unregister(
        RuntimeOrigin::root(),
        smart_contract.clone(),
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::DAppUnregistered {
        smart_contract: smart_contract.clone(),
        era: pre_snapshot.active_protocol_state.era,
    }));

    // Verify post-state
    assert!(!IntegratedDApps::<Test>::contains_key(&smart_contract));
    assert_eq!(
        pre_snapshot.integrated_dapps.len() - 1,
        IntegratedDApps::<Test>::count() as usize
    );

    assert!(!ContractStake::<Test>::contains_key(
        &pre_snapshot.integrated_dapps[&smart_contract].id
    ));
}

/// Lock funds into dApp staking and assert success.
pub(crate) fn assert_lock(account: AccountId, amount: Balance) {
    let pre_snapshot = MemorySnapshot::new();

    let total_balance = Balances::total_balance(&account);
    let locked_balance = pre_snapshot.locked_balance(&account);
    let init_frozen_balance = Balances::balance_frozen(&FreezeReason::DAppStaking.into(), &account);

    let available_balance = total_balance
        .checked_sub(locked_balance)
        .expect("Locked amount cannot be greater than available free balance");
    let expected_lock_amount = available_balance.min(amount);
    assert!(!expected_lock_amount.is_zero());

    // Lock funds
    assert_ok!(DappStaking::lock(RuntimeOrigin::signed(account), amount,));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::Locked {
        account,
        amount: expected_lock_amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();

    assert_eq!(
        post_snapshot.locked_balance(&account),
        locked_balance + expected_lock_amount,
        "Locked balance should be increased by the amount locked."
    );

    assert_eq!(
        post_snapshot.current_era_info.total_locked,
        pre_snapshot.current_era_info.total_locked + expected_lock_amount,
        "Total locked balance should be increased by the amount locked."
    );

    let post_frozen_balance = Balances::balance_frozen(&FreezeReason::DAppStaking.into(), &account);
    assert_eq!(
        init_frozen_balance + expected_lock_amount,
        post_frozen_balance
    );
    assert!(
        Balances::total_balance(&account) >= post_frozen_balance,
        "Total balance should never be less than frozen balance."
    )
}

/// Start the unlocking process for locked funds and assert success.
pub(crate) fn assert_unlock(account: AccountId, amount: Balance) {
    let pre_snapshot = MemorySnapshot::new();
    let init_frozen_balance = Balances::balance_frozen(&FreezeReason::DAppStaking.into(), &account);

    assert!(
        pre_snapshot.ledger.contains_key(&account),
        "Cannot unlock for non-existing ledger."
    );

    // Calculate expected unlock amount
    let pre_ledger = &pre_snapshot.ledger[&account];
    let expected_unlock_amount = {
        // Cannot unlock more than is available
        let possible_unlock_amount = pre_ledger
            .unlockable_amount(pre_snapshot.active_protocol_state.period_number())
            .min(amount);

        // When unlocking would take account below the minimum lock threshold, unlock everything
        let locked_amount = pre_ledger.active_locked_amount();
        let min_locked_amount = <Test as Config>::MinimumLockedAmount::get();
        if locked_amount.saturating_sub(possible_unlock_amount) < min_locked_amount {
            locked_amount
        } else {
            possible_unlock_amount
        }
    };

    // Unlock funds
    assert_ok!(DappStaking::unlock(RuntimeOrigin::signed(account), amount,));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::Unlocking {
        account,
        amount: expected_unlock_amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();

    // Verify ledger is as expected
    let period_number = pre_snapshot.active_protocol_state.period_number();
    let post_ledger = &post_snapshot.ledger[&account];
    assert_eq!(
        pre_ledger.active_locked_amount(),
        post_ledger.active_locked_amount() + expected_unlock_amount,
        "Active locked amount should be decreased by the amount unlocked."
    );
    assert_eq!(
        pre_ledger.unlocking_amount() + expected_unlock_amount,
        post_ledger.unlocking_amount(),
        "Total unlocking amount should be increased by the amount unlocked."
    );
    assert_eq!(
        pre_ledger.total_locked_amount(),
        post_ledger.total_locked_amount(),
        "Total locked amount should remain exactly the same since the unlocking chunks are still locked."
    );
    assert_eq!(
        pre_ledger.unlockable_amount(period_number),
        post_ledger.unlockable_amount(period_number) + expected_unlock_amount,
        "Unlockable amount should be decreased by the amount unlocked."
    );

    // In case ledger is empty, it should have been removed from the storage
    if post_ledger.is_empty() {
        assert!(!Ledger::<Test>::contains_key(&account));
    }

    // Verify era info post-state
    let pre_era_info = &pre_snapshot.current_era_info;
    let post_era_info = &post_snapshot.current_era_info;
    assert_eq!(
        pre_era_info.unlocking + expected_unlock_amount,
        post_era_info.unlocking
    );
    assert_eq!(
        pre_era_info
            .total_locked
            .saturating_sub(expected_unlock_amount),
        post_era_info.total_locked
    );

    assert_eq!(
        init_frozen_balance,
        Balances::balance_frozen(&FreezeReason::DAppStaking.into(), &account),
        "Frozen balance must remain the same since the funds are still locked/frozen, only undergoing the unlocking process."
    );
}

/// Claims the unlocked funds back into free balance of the user and assert success.
pub(crate) fn assert_claim_unlocked(account: AccountId) {
    let pre_snapshot = MemorySnapshot::new();

    assert!(
        pre_snapshot.ledger.contains_key(&account),
        "Cannot claim unlocked for non-existing ledger."
    );

    let current_block = System::block_number();
    let mut consumed_chunks = 0;
    let mut amount = 0;
    for unlock_chunk in pre_snapshot.ledger[&account].clone().unlocking.into_inner() {
        if unlock_chunk.unlock_block <= current_block {
            amount += unlock_chunk.amount;
            consumed_chunks += 1;
        }
    }

    // Claim unlocked chunks
    assert_ok!(DappStaking::claim_unlocked(RuntimeOrigin::signed(account)));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::ClaimedUnlocked {
        account,
        amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();

    let post_ledger = if let Some(ledger) = post_snapshot.ledger.get(&account) {
        ledger.clone()
    } else {
        Default::default()
    };

    assert_eq!(
        post_ledger.unlocking.len(),
        pre_snapshot.ledger[&account].unlocking.len() - consumed_chunks
    );
    assert_eq!(
        post_ledger.unlocking_amount(),
        pre_snapshot.ledger[&account].unlocking_amount() - amount
    );
    assert_eq!(
        post_snapshot.current_era_info.unlocking,
        pre_snapshot.current_era_info.unlocking - amount
    );

    // In case of full withdrawal from the protocol
    if post_ledger.is_empty() {
        assert!(!Ledger::<Test>::contains_key(&account));
        assert!(
            StakerInfo::<Test>::iter_prefix_values(&account)
                .count()
                .is_zero(),
            "All stake entries need to be cleaned up."
        );
    }
}

/// Claims the unlocked funds back into free balance of the user and assert success.
pub(crate) fn assert_relock_unlocking(account: AccountId) {
    let pre_snapshot = MemorySnapshot::new();

    assert!(
        pre_snapshot.ledger.contains_key(&account),
        "Cannot relock unlocking non-existing ledger."
    );

    let amount = pre_snapshot.ledger[&account].unlocking_amount();

    // Relock unlocking chunks
    assert_ok!(DappStaking::relock_unlocking(RuntimeOrigin::signed(
        account
    )));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::Relock { account, amount }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();

    // Account ledger
    let post_ledger = &post_snapshot.ledger[&account];
    assert!(post_ledger.unlocking.is_empty());
    assert!(post_ledger.unlocking_amount().is_zero());
    assert_eq!(
        post_ledger.active_locked_amount(),
        pre_snapshot.ledger[&account].active_locked_amount() + amount
    );

    // Current era info
    assert_eq!(
        post_snapshot.current_era_info.unlocking,
        pre_snapshot.current_era_info.unlocking - amount
    );
    assert_eq!(
        post_snapshot.current_era_info.total_locked,
        pre_snapshot.current_era_info.total_locked + amount
    );
}

/// Stake some funds on the specified smart contract.
pub(crate) fn assert_stake(
    account: AccountId,
    smart_contract: &MockSmartContract,
    amount: Balance,
) {
    let pre_snapshot = MemorySnapshot::new();
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, smart_contract.clone()));
    let pre_contract_stake = pre_snapshot
        .contract_stake
        .get(&pre_snapshot.integrated_dapps[&smart_contract].id)
        .map_or(ContractStakeAmount::default(), |series| series.clone());
    let pre_era_info = pre_snapshot.current_era_info;

    let stake_era = pre_snapshot.active_protocol_state.era + 1;
    let stake_period = pre_snapshot.active_protocol_state.period_number();
    let stake_subperiod = pre_snapshot.active_protocol_state.subperiod();

    // Stake on smart contract & verify event
    assert_ok!(DappStaking::stake(
        RuntimeOrigin::signed(account),
        smart_contract.clone(),
        amount
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::Stake {
        account,
        smart_contract: smart_contract.clone(),
        amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();
    let post_staker_info = post_snapshot
        .staker_info
        .get(&(account, *smart_contract))
        .expect("Entry must exist since 'stake' operation was successful.");
    let post_contract_stake = post_snapshot
        .contract_stake
        .get(&pre_snapshot.integrated_dapps[&smart_contract].id)
        .expect("Entry must exist since 'stake' operation was successful.");
    let post_era_info = post_snapshot.current_era_info;

    // 1. verify ledger
    // =====================
    // =====================
    if is_account_ledger_expired(pre_ledger, stake_period) {
        assert!(
            post_ledger.staked.is_empty(),
            "Must be cleaned up if expired."
        );
    } else {
        match pre_ledger.staked_future {
            Some(stake_amount) => {
                if stake_amount.era == pre_snapshot.active_protocol_state.era {
                    assert_eq!(
                        post_ledger.staked, stake_amount,
                        "Future entry must be moved over to the current entry."
                    );
                } else if stake_amount.era == pre_snapshot.active_protocol_state.era + 1 {
                    assert_eq!(
                        post_ledger.staked, pre_ledger.staked,
                        "Must remain exactly the same, only future must be updated."
                    );
                } else {
                    panic!("Invalid future entry era.");
                }
            }
            None => {
                assert_eq!(
                    post_ledger.staked, pre_ledger.staked,
                    "Must remain exactly the same since there's nothing to be moved."
                );
            }
        }
    }

    assert_eq!(post_ledger.staked_future.unwrap().period, stake_period);
    assert_eq!(post_ledger.staked_future.unwrap().era, stake_era);
    assert_eq!(
        post_ledger.staked_amount(stake_period),
        pre_ledger.staked_amount(stake_period) + amount,
        "Stake amount must increase by the 'amount'"
    );
    assert_eq!(
        post_ledger.stakeable_amount(stake_period),
        pre_ledger.stakeable_amount(stake_period) - amount,
        "Stakeable amount must decrease by the 'amount'"
    );

    // 2. verify staker info
    // =====================
    // =====================

    let (stake_amount, bonus_status) = match stake_subperiod {
        Subperiod::Voting => (
            StakeAmount {
                voting: amount,
                build_and_earn: 0,
                era: stake_era,
                period: stake_period,
            },
            *BonusStatusWrapperFor::<Test>::default(),
        ),
        Subperiod::BuildAndEarn => (
            StakeAmount {
                voting: 0,
                build_and_earn: amount,
                era: stake_era,
                period: stake_period,
            },
            0,
        ),
    };

    assert_staker_info_after_stake(
        &pre_snapshot,
        &post_snapshot,
        account,
        smart_contract,
        stake_amount,
        bonus_status,
    );

    match pre_staker_info {
        // We're just updating an existing entry
        Some(pre_staker_info) if pre_staker_info.period_number() == stake_period => {
            assert_eq!(
                post_staker_info.is_bonus_eligible(),
                pre_staker_info.is_bonus_eligible(),
                "Staking operation mustn't change bonus reward
                eligibility."
            );
        }
        // A new entry is created.
        _ => {
            assert_eq!(
                post_staker_info.is_bonus_eligible(),
                stake_subperiod == Subperiod::Voting
            );
        }
    }

    // 3. verify contract stake
    // =========================
    // =========================
    assert_contract_stake_after_stake(
        &pre_contract_stake,
        &post_contract_stake,
        &pre_snapshot,
        stake_amount,
    );

    // 4. verify era info
    // =========================
    // =========================
    assert_eq!(
        post_era_info.total_staked_amount(),
        pre_era_info.total_staked_amount(),
        "Total staked amount for the current era must remain the same."
    );
    assert_eq!(
        post_era_info.total_staked_amount_next_era(),
        pre_era_info.total_staked_amount_next_era() + amount
    );
    assert_eq!(
        post_era_info.staked_amount_next_era(stake_subperiod),
        pre_era_info.staked_amount_next_era(stake_subperiod) + amount
    );
}

/// Unstake some funds from the specified smart contract.
pub(crate) fn assert_unstake(
    account: AccountId,
    smart_contract: &MockSmartContract,
    amount: Balance,
) {
    let pre_snapshot = MemorySnapshot::new();
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, smart_contract.clone()))
        .expect("Entry must exist since 'unstake' is being called.");
    let pre_contract_stake = pre_snapshot
        .contract_stake
        .get(&pre_snapshot.integrated_dapps[&smart_contract].id)
        .expect("Entry must exist since 'unstake' is being called.");
    let pre_era_info = pre_snapshot.current_era_info;

    let unstake_period = pre_snapshot.active_protocol_state.period_number();

    let minimum_stake_amount: Balance = <Test as Config>::MinimumStakeAmount::get();
    let is_full_unstake =
        pre_staker_info.total_staked_amount().saturating_sub(amount) < minimum_stake_amount;

    // Unstake all if we expect to go below the minimum stake amount
    let unstake_amount = if is_full_unstake {
        pre_staker_info.total_staked_amount()
    } else {
        amount
    };

    // Unstake from smart contract & verify event
    assert_ok!(DappStaking::unstake(
        RuntimeOrigin::signed(account),
        smart_contract.clone(),
        amount
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::Unstake {
        account,
        smart_contract: smart_contract.clone(),
        amount: unstake_amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();
    let post_contract_stake = post_snapshot
        .contract_stake
        .get(&pre_snapshot.integrated_dapps[&smart_contract].id)
        .expect("Entry must exist since 'unstake' operation was successful.");
    let post_era_info = post_snapshot.current_era_info;

    // 1. verify ledger
    // =====================
    // =====================
    assert_eq!(
        post_ledger.staked_amount(unstake_period),
        pre_ledger.staked_amount(unstake_period) - unstake_amount,
        "Stake amount must decrease by the 'amount'"
    );
    assert_eq!(
        post_ledger.stakeable_amount(unstake_period),
        pre_ledger.stakeable_amount(unstake_period) + unstake_amount,
        "Stakeable amount must increase by the 'amount'"
    );

    assert_ledger_contract_stake_count(pre_ledger, post_ledger, is_full_unstake, false);

    // 2. verify staker info
    // =====================
    // =====================
    let (unstake_amount_entries, _) = assert_staker_info_after_unstake(
        &pre_snapshot,
        &post_snapshot,
        account,
        smart_contract,
        unstake_amount,
        is_full_unstake,
    );

    // 3. verify contract stake
    // =========================
    // =========================
    assert_contract_stake_after_unstake(
        &pre_contract_stake,
        &post_contract_stake,
        &pre_snapshot,
        unstake_amount_entries.clone(),
    );

    // 4. verify era info
    // =========================
    // =========================

    assert_era_info_current(
        &pre_era_info,
        &post_era_info,
        unstake_amount_entries.clone(),
    );

    assert_eq!(
        post_era_info.total_staked_amount_next_era(),
        pre_era_info.total_staked_amount_next_era() - unstake_amount,
        "Total staked amount for the next era must decrease by 'amount'. No overflow is allowed."
    );

    // expected to be non-zero in case we're past the voting sub-period
    if !post_era_info.total_staked_amount().is_zero() {
        // Ensure no invariance occurs in the voting stake amount for unstake operations
        assert_eq!(
            post_era_info.current_stake_amount.voting,
            post_era_info.next_stake_amount.voting
        );
    }

    let unstake_amount = unstake_amount_entries
        .iter()
        .max_by(|a, b| a.total().cmp(&b.total()))
        .expect("At least one value exists, otherwise we wouldn't be here.");
    assert_eq!(
        post_era_info.staked_amount_next_era(Subperiod::Voting),
        pre_era_info
            .staked_amount_next_era(Subperiod::Voting)
            .saturating_sub(unstake_amount.for_type(Subperiod::Voting)),
        "Voting next era staked amount must decreased by the 'unstake_amount'"
    );
    assert_eq!(
        post_era_info.staked_amount_next_era(Subperiod::BuildAndEarn),
        pre_era_info
            .staked_amount_next_era(Subperiod::BuildAndEarn)
            .saturating_sub(unstake_amount.for_type(Subperiod::BuildAndEarn)),
        "BuildAndEarn next era staked amount must decreased by the 'unstake_amount'"
    );
}

/// Move stake funds from source contract to destination contract.
pub(crate) fn assert_move_stake(
    account: AccountId,
    source_contract: &MockSmartContract,
    destination_contract: &MockSmartContract,
    amount: Balance,
) {
    let pre_snapshot = MemorySnapshot::new();
    let is_source_unregistered = IntegratedDApps::<Test>::get(&source_contract).is_none();

    let pre_era_info = pre_snapshot.current_era_info;
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, source_contract.clone()))
        .expect("Entry must exist since 'move' is being called on a registered contract.");
    let maybe_pre_source_contract_stake = if is_source_unregistered {
        None
    } else {
        Some(
            pre_snapshot
                .contract_stake
                .get(&pre_snapshot.integrated_dapps[&source_contract].id)
                .expect("Entry must exist since 'move' is being called."),
        )
    };
    let pre_destination_contract_stake = pre_snapshot
        .contract_stake
        .get(&pre_snapshot.integrated_dapps[&destination_contract].id)
        .map_or(ContractStakeAmount::default(), |series| series.clone());
    let maybe_pre_destination_staker_info = pre_snapshot
        .staker_info
        .get(&(account, *destination_contract));

    let move_period = pre_snapshot.active_protocol_state.period_number();

    let minimum_stake_amount: Balance = <Test as Config>::MinimumStakeAmount::get();
    let is_source_unregistered = maybe_pre_source_contract_stake.is_none();
    let is_full_unstake =
        pre_staker_info.total_staked_amount().saturating_sub(amount) < minimum_stake_amount;

    let amount_to_move = if is_source_unregistered {
        pre_staker_info.total_staked_amount()
    } else {
        if is_full_unstake {
            pre_staker_info.total_staked_amount()
        } else {
            amount
        }
    };
    let is_full_move_from_source = pre_staker_info.total_staked_amount() == amount_to_move;

    // Move from source contract to destination contract & verify event
    assert_ok!(DappStaking::move_stake(
        RuntimeOrigin::signed(account),
        source_contract.clone(),
        destination_contract.clone(),
        amount
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::StakeMoved {
        account,
        source_contract: source_contract.clone(),
        destination_contract: destination_contract.clone(),
        amount: amount_to_move,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();
    let post_era_info = post_snapshot.current_era_info;
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();
    let maybe_post_source_contract_stake = if is_source_unregistered {
        None
    } else {
        Some(
            post_snapshot
                .contract_stake
                .get(&post_snapshot.integrated_dapps[&source_contract].id)
                .expect("Entry must exist since 'move' is being called on a registered contract."),
        )
    };
    let post_destination_contract_stake = post_snapshot
        .contract_stake
        .get(&post_snapshot.integrated_dapps[&destination_contract].id)
        .expect("Entry must exist since 'move' is being called.");

    // 1. verify staker info
    // =====================
    // =====================

    let (amount_entries, updated_bonus_status) = assert_staker_info_after_unstake(
        &pre_snapshot,
        &post_snapshot,
        account,
        source_contract,
        amount_to_move,
        is_full_move_from_source,
    );
    let bonus_status = if is_source_unregistered {
        pre_staker_info.bonus_status
    } else {
        updated_bonus_status
    };

    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, source_contract.clone()))
        .expect("Entry must exist since 'move' is being called.");

    let unstake_amount_entries = if is_source_unregistered {
        vec![pre_staker_info.previous_staked, pre_staker_info.staked]
            .into_iter()
            .filter(|stake_amount| !stake_amount.is_empty())
            .collect::<Vec<StakeAmount>>()
    } else {
        amount_entries
    };

    let mut unstake_amount_entries_clone = unstake_amount_entries.clone();
    let stake_amount = unstake_amount_entries_clone
        .iter_mut()
        .max_by(|a, b| a.total().cmp(&b.total()))
        .expect("At least one value exists, otherwise we wouldn't be here.");

    // Merge voting into b&e when bonus is lost
    let is_voting_stake_converted = bonus_status == 0 && stake_amount.voting > 0;
    if is_voting_stake_converted {
        stake_amount.convert_bonus_into_regular_stake();
    }

    assert_eq!(stake_amount.total(), amount_to_move);

    assert_staker_info_after_stake(
        &pre_snapshot,
        &post_snapshot,
        account,
        destination_contract,
        *stake_amount,
        bonus_status,
    );

    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, source_contract.clone()))
        .expect("Entry must exist since 'move' operation is being called.");
    let post_staker_info = post_snapshot
        .staker_info
        .get(&(account, destination_contract.clone()))
        .expect("Entry must exist since 'move' operation was successful.");
    if is_source_unregistered {
        assert_eq!(pre_staker_info.bonus_status, post_staker_info.bonus_status);
    }

    // 2. verify contract stake (for registered source contract)
    // =========================
    // =========================

    if let (Some(pre_source_contract_stake), Some(post_source_contract_stake)) = (
        maybe_pre_source_contract_stake,
        maybe_post_source_contract_stake,
    ) {
        assert_contract_stake_after_unstake(
            &pre_source_contract_stake,
            &post_source_contract_stake,
            &pre_snapshot,
            unstake_amount_entries.clone(),
        );
    }

    assert_contract_stake_after_stake(
        &pre_destination_contract_stake,
        &post_destination_contract_stake,
        &pre_snapshot,
        *stake_amount,
    );

    // 3. verify ledger (unchanged)
    // =====================
    // =====================
    assert_eq!(
        post_ledger.staked_amount(move_period),
        pre_ledger.staked_amount(move_period),
        "Stake amount must remain unchanged for a 'move'"
    );
    assert_eq!(
        post_ledger.stakeable_amount(move_period),
        pre_ledger.stakeable_amount(move_period),
        "Stakeable amount must remain unchanged for a 'move'"
    );

    let is_new_dest = maybe_pre_destination_staker_info.is_none();
    let is_expected_to_decrease =
        !is_new_dest && is_full_move_from_source && !is_source_unregistered;
    let is_expected_to_increase =
        is_new_dest && !(is_source_unregistered || is_full_move_from_source);
    assert_ledger_contract_stake_count(
        pre_ledger,
        post_ledger,
        is_expected_to_decrease,
        is_expected_to_increase,
    );

    // 4. verify era info
    // =========================
    // =========================

    assert_era_info_current(
        &pre_era_info,
        &post_era_info,
        unstake_amount_entries.clone(),
    );

    // expected to be non-zero in case we're past the voting sub-period
    if !post_era_info.total_staked_amount().is_zero() {
        // A move operation delay staking rewards to the next era, the next voting stake amount cannot be lower than the current era one
        assert!(
            post_era_info.current_stake_amount.voting <= post_era_info.next_stake_amount.voting
        );
    }

    assert_eq!(
        post_era_info.total_staked_amount_next_era(),
        pre_era_info.total_staked_amount_next_era(),
        "Total staked amount for the next era must remain unchanged for a 'move'."
    );
}

/// Claim staker rewards.
pub(crate) fn assert_claim_staker_rewards(account: AccountId) {
    let pre_snapshot = MemorySnapshot::new();
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let pre_total_issuance = <Test as Config>::Currency::total_issuance();
    let pre_free_balance = <Test as Config>::Currency::free_balance(&account);

    // Get the first eligible era for claiming rewards
    let first_claim_era = pre_ledger
        .earliest_staked_era()
        .expect("Entry must exist, otherwise 'claim' is invalid.");

    // Get the appropriate era rewards span for the 'first era'
    let era_span_length: EraNumber = <Test as Config>::EraRewardSpanLength::get();
    let era_span_index = first_claim_era - (first_claim_era % era_span_length);
    let era_rewards_span = pre_snapshot
        .era_rewards
        .get(&era_span_index)
        .expect("Entry must exist, otherwise 'claim' is invalid.");

    // Calculate the final era for claiming rewards. Also determine if this will fully claim all staked period rewards.
    let claim_period_end = if pre_ledger.staked_period().unwrap()
        == pre_snapshot.active_protocol_state.period_number()
    {
        None
    } else {
        Some(
            pre_snapshot
                .period_end
                .get(&pre_ledger.staked_period().unwrap())
                .expect("Entry must exist, since it's the current period.")
                .final_era,
        )
    };

    let (last_claim_era, is_full_claim) = if claim_period_end.is_none() {
        (pre_snapshot.active_protocol_state.era - 1, false)
    } else {
        let claim_period = pre_ledger.staked_period().unwrap();
        let period_end = pre_snapshot
            .period_end
            .get(&claim_period)
            .expect("Entry must exist, since it's a past period.");

        let last_claim_era = era_rewards_span.last_era().min(period_end.final_era);
        let is_full_claim = last_claim_era == period_end.final_era;
        (last_claim_era, is_full_claim)
    };

    assert!(
        last_claim_era < pre_snapshot.active_protocol_state.era,
        "Sanity check."
    );

    // Calculate the expected rewards
    let mut rewards = Vec::new();
    for (era, amount) in pre_ledger
        .clone()
        .claim_up_to_era(last_claim_era, claim_period_end)
        .unwrap()
    {
        let era_reward_info = era_rewards_span
            .get(era)
            .expect("Entry must exist, otherwise 'claim' is invalid.");

        let reward = Perbill::from_rational(amount, era_reward_info.staked)
            * era_reward_info.staker_reward_pool;
        if reward.is_zero() {
            continue;
        }

        rewards.push((era, reward));
    }
    let total_reward = rewards
        .iter()
        .fold(Balance::zero(), |acc, (_, reward)| acc + reward);

    //clean up possible leftover events
    System::reset_events();

    // Claim staker rewards & verify all events
    assert_ok!(DappStaking::claim_staker_rewards(RuntimeOrigin::signed(
        account
    ),));

    let events = dapp_staking_events();
    assert_eq!(events.len(), rewards.len());
    for (event, (era, reward)) in events.iter().zip(rewards.iter()) {
        assert_eq!(
            event,
            &Event::<Test>::Reward {
                account,
                era: *era,
                amount: *reward,
            }
        );
    }

    // Verify post state

    let post_total_issuance = <Test as Config>::Currency::total_issuance();
    assert_eq!(
        post_total_issuance,
        pre_total_issuance + total_reward,
        "Total issuance must increase by the total reward amount."
    );

    let post_free_balance = <Test as Config>::Currency::free_balance(&account);
    assert_eq!(
        post_free_balance,
        pre_free_balance + total_reward,
        "Free balance must increase by the total reward amount."
    );

    let post_snapshot = MemorySnapshot::new();
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();

    if is_full_claim {
        assert_eq!(post_ledger.staked, StakeAmount::default());
        assert!(post_ledger.staked_future.is_none());
    } else {
        assert_eq!(post_ledger.staked.era, last_claim_era + 1);
        assert!(post_ledger.staked_future.is_none());
    }
}

/// Claim staker rewards.
pub(crate) fn assert_claim_bonus_reward(account: AccountId, smart_contract: &MockSmartContract) {
    let pre_snapshot = MemorySnapshot::new();
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, *smart_contract))
        .unwrap();
    let pre_total_issuance = <Test as Config>::Currency::total_issuance();
    let pre_free_balance = <Test as Config>::Currency::free_balance(&account);

    let staked_period = pre_staker_info.period_number();
    let stake_amount = pre_staker_info.staked_amount(Subperiod::Voting);

    let period_end_info = pre_snapshot
        .period_end
        .get(&staked_period)
        .expect("Entry must exist, since it's a past period.");

    let reward = Perbill::from_rational(stake_amount, period_end_info.total_vp_stake)
        * period_end_info.bonus_reward_pool;

    // Claim bonus reward & verify event
    assert_ok!(DappStaking::claim_bonus_reward(
        RuntimeOrigin::signed(account),
        smart_contract.clone(),
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::BonusReward {
        account,
        smart_contract: *smart_contract,
        period: staked_period,
        amount: reward,
    }));

    // Verify post state

    let post_total_issuance = <Test as Config>::Currency::total_issuance();
    assert_eq!(
        post_total_issuance,
        pre_total_issuance + reward,
        "Total issuance must increase by the reward amount."
    );

    let post_free_balance = <Test as Config>::Currency::free_balance(&account);
    assert_eq!(
        post_free_balance,
        pre_free_balance + reward,
        "Free balance must increase by the reward amount."
    );

    assert!(
        !StakerInfo::<Test>::contains_key(&account, smart_contract),
        "Entry must be removed after successful reward claim."
    );
    assert_eq!(
        pre_snapshot.ledger[&account].contract_stake_count,
        Ledger::<Test>::get(&account).contract_stake_count + 1,
        "Count must be reduced since the staker info entry was removed."
    );
}

/// Claim dapp reward for a particular era.
pub(crate) fn assert_claim_dapp_reward(
    account: AccountId,
    smart_contract: &MockSmartContract,
    era: EraNumber,
) {
    let pre_snapshot = MemorySnapshot::new();
    let dapp_info = pre_snapshot.integrated_dapps.get(smart_contract).unwrap();
    let beneficiary = dapp_info.reward_beneficiary();
    let pre_total_issuance = <Test as Config>::Currency::total_issuance();
    let pre_free_balance = <Test as Config>::Currency::free_balance(beneficiary);

    let pre_reward_info = pre_snapshot
        .dapp_tiers
        .get(&era)
        .expect("Entry must exist.")
        .clone();
    let (expected_reward, expected_ranked_tier) = {
        let mut info = pre_reward_info.clone();
        info.try_claim(dapp_info.id).unwrap()
    };

    // Claim dApp reward & verify event
    assert_ok!(DappStaking::claim_dapp_reward(
        RuntimeOrigin::signed(account),
        smart_contract.clone(),
        era,
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::DAppReward {
        beneficiary: beneficiary.clone(),
        smart_contract: smart_contract.clone(),
        tier_id: expected_ranked_tier.tier(),
        rank: expected_ranked_tier.rank(),
        era,
        amount: expected_reward,
    }));

    // Verify post-state

    let post_total_issuance = <Test as Config>::Currency::total_issuance();
    assert_eq!(
        post_total_issuance,
        pre_total_issuance + expected_reward,
        "Total issuance must increase by the reward amount."
    );

    let post_free_balance = <Test as Config>::Currency::free_balance(beneficiary);
    assert_eq!(
        post_free_balance,
        pre_free_balance + expected_reward,
        "Free balance must increase by the reward amount."
    );

    let post_snapshot = MemorySnapshot::new();
    let mut post_reward_info = post_snapshot
        .dapp_tiers
        .get(&era)
        .expect("Entry must exist.")
        .clone();
    assert_eq!(
        post_reward_info.try_claim(dapp_info.id),
        Err(DAppTierError::NoDAppInTiers),
        "It must not be possible to claim the same reward twice!.",
    );
    assert_eq!(
        pre_reward_info.dapps.len(),
        post_reward_info.dapps.len() + 1,
        "Entry must have been removed after successful reward claim."
    );
}

/// Unstake some funds from the specified unregistered smart contract.
pub(crate) fn assert_unstake_from_unregistered(
    account: AccountId,
    smart_contract: &MockSmartContract,
) {
    let pre_snapshot = MemorySnapshot::new();
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, smart_contract.clone()))
        .expect("Entry must exist since 'unstake_from_unregistered' is being called.");
    let pre_era_info = pre_snapshot.current_era_info;

    let amount = pre_staker_info.total_staked_amount();

    // Unstake from smart contract & verify event
    assert_ok!(DappStaking::unstake_from_unregistered(
        RuntimeOrigin::signed(account),
        smart_contract.clone(),
    ));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::UnstakeFromUnregistered {
        account,
        smart_contract: smart_contract.clone(),
        amount,
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();
    let post_era_info = post_snapshot.current_era_info;
    let period = pre_snapshot.active_protocol_state.period_number();
    let unstake_subperiod = pre_snapshot.active_protocol_state.subperiod();

    // 1. verify ledger
    // =====================
    // =====================
    assert_eq!(
        post_ledger.staked_amount(period),
        pre_ledger.staked_amount(period) - amount,
        "Stake amount must decrease by the 'amount'"
    );
    assert_eq!(
        post_ledger.stakeable_amount(period),
        pre_ledger.stakeable_amount(period) + amount,
        "Stakeable amount must increase by the 'amount'"
    );

    assert_ledger_contract_stake_count(pre_ledger, post_ledger, true, false);

    // 2. verify staker info
    // =====================
    // =====================
    assert!(
        !StakerInfo::<Test>::contains_key(&account, smart_contract),
        "Entry must be deleted since contract is unregistered."
    );

    // 3. verify era info
    // =========================
    // =========================

    let unstake_era = pre_snapshot.active_protocol_state.era;
    let (stake_amount_entries, _) =
        pre_staker_info
            .clone()
            .unstake(amount, unstake_era, unstake_subperiod);

    // expected to be non-zero in case we're past the voting sub-period
    if !post_era_info.total_staked_amount().is_zero() {
        // Ensure no invariance occurs in the voting stake amount for unstake operations
        assert_eq!(
            post_era_info.current_stake_amount.voting,
            post_era_info.next_stake_amount.voting
        );
    }

    assert_eq!(
        post_era_info.total_staked_amount_next_era(),
        pre_era_info.total_staked_amount_next_era() - amount,
        "Total staked amount for the next era must decrease by 'amount'. No overflow is allowed."
    );

    let unstake_amount = stake_amount_entries
        .iter()
        .max_by(|a, b| a.total().cmp(&b.total()))
        .expect("At least one value exists, otherwise we wouldn't be here.");
    assert_eq!(
        post_era_info.staked_amount_next_era(Subperiod::Voting),
        pre_era_info
            .staked_amount_next_era(Subperiod::Voting)
            .saturating_sub(unstake_amount.for_type(Subperiod::Voting)),
        "Voting next era staked amount must decreased by the 'unstake_amount'"
    );
    assert_eq!(
        post_era_info.staked_amount_next_era(Subperiod::BuildAndEarn),
        pre_era_info
            .staked_amount_next_era(Subperiod::BuildAndEarn)
            .saturating_sub(unstake_amount.for_type(Subperiod::BuildAndEarn)),
        "BuildAndEarn next era staked amount must decreased by the 'unstake_amount'"
    );
}

/// Cleanup expired DB entries for the account and verify post state.
pub(crate) fn assert_cleanup_expired_entries(account: AccountId) {
    let pre_snapshot = MemorySnapshot::new();

    let current_period = pre_snapshot.active_protocol_state.period_number();
    let threshold_period = DappStaking::oldest_claimable_period(current_period);

    // Find entries which should be kept, and which should be deleted
    let mut to_be_deleted = Vec::new();
    let mut to_be_kept = Vec::new();
    pre_snapshot
        .staker_info
        .iter()
        .for_each(|((inner_account, contract), entry)| {
            if *inner_account == account {
                if entry.period_number() < current_period && !entry.is_bonus_eligible()
                    || entry.period_number() < threshold_period
                {
                    to_be_deleted.push(contract);
                } else {
                    to_be_kept.push(contract);
                }
            }
        });

    // Cleanup expired entries and verify event
    assert_ok!(DappStaking::cleanup_expired_entries(RuntimeOrigin::signed(
        account
    )));
    System::assert_last_event(RuntimeEvent::DappStaking(Event::ExpiredEntriesRemoved {
        account,
        count: to_be_deleted.len().try_into().unwrap(),
    }));

    // Verify post-state
    let post_snapshot = MemorySnapshot::new();

    // Ensure that correct entries have been kept
    assert_eq!(post_snapshot.staker_info.len(), to_be_kept.len());
    to_be_kept.iter().for_each(|contract| {
        assert!(post_snapshot
            .staker_info
            .contains_key(&(account, **contract)));
    });

    // Ensure that ledger has been correctly updated
    let pre_ledger = pre_snapshot.ledger.get(&account).unwrap();
    let post_ledger = post_snapshot.ledger.get(&account).unwrap();

    let num_of_deleted_entries: u32 = to_be_deleted.len().try_into().unwrap();
    assert_eq!(
        pre_ledger.contract_stake_count - num_of_deleted_entries,
        post_ledger.contract_stake_count
    );
}

/// Asserts correct transitions of the protocol after a block has been produced.
pub(crate) fn assert_block_bump(pre_snapshot: &MemorySnapshot) {
    let current_block_number = System::block_number();

    // No checks if era didn't change.
    if pre_snapshot.active_protocol_state.next_era_start > current_block_number {
        return;
    }

    // Verify post state
    let post_snapshot = MemorySnapshot::new();

    let is_new_subperiod = pre_snapshot
        .active_protocol_state
        .period_info
        .next_subperiod_start_era
        <= post_snapshot.active_protocol_state.era;

    // 1. Verify protocol state
    let pre_protoc_state = pre_snapshot.active_protocol_state;
    let post_protoc_state = post_snapshot.active_protocol_state;
    assert_eq!(post_protoc_state.era, pre_protoc_state.era + 1);

    match pre_protoc_state.subperiod() {
        Subperiod::Voting => {
            assert_eq!(
                post_protoc_state.subperiod(),
                Subperiod::BuildAndEarn,
                "Voting subperiod only lasts for a single era."
            );

            let eras_per_bep =
                <Test as Config>::CycleConfiguration::eras_per_build_and_earn_subperiod();
            assert_eq!(
                post_protoc_state.period_info.next_subperiod_start_era,
                post_protoc_state.era + eras_per_bep,
                "Build&earn must last for the predefined amount of standard eras."
            );

            let standard_era_length = <Test as Config>::CycleConfiguration::blocks_per_era();
            assert_eq!(
                post_protoc_state.next_era_start,
                current_block_number + standard_era_length,
                "Era in build&earn period must last for the predefined amount of blocks."
            );
        }
        Subperiod::BuildAndEarn => {
            if is_new_subperiod {
                assert_eq!(
                    post_protoc_state.subperiod(),
                    Subperiod::Voting,
                    "Since we expect a new subperiod, it must be 'Voting'."
                );
                assert_eq!(
                    post_protoc_state.period_number(),
                    pre_protoc_state.period_number() + 1,
                    "Ending 'Build&Earn' triggers a new period."
                );
                assert_eq!(
                    post_protoc_state.period_info.next_subperiod_start_era,
                    post_protoc_state.era + 1,
                    "Voting era must last for a single era."
                );

                let blocks_per_standard_era =
                    <Test as Config>::CycleConfiguration::blocks_per_era();
                let eras_per_voting_subperiod =
                    <Test as Config>::CycleConfiguration::eras_per_voting_subperiod();
                let eras_per_voting_subperiod: BlockNumber = eras_per_voting_subperiod.into();
                let era_length: BlockNumber = blocks_per_standard_era * eras_per_voting_subperiod;
                assert_eq!(
                    post_protoc_state.next_era_start,
                    current_block_number + era_length,
                    "The upcoming 'Voting' subperiod must last for the 'standard eras per voting subperiod x standard era length' amount of blocks."
                );
            } else {
                assert_eq!(
                    post_protoc_state.period_info, pre_protoc_state.period_info,
                    "New subperiod hasn't started, hence it should remain 'Build&Earn'."
                );
            }
        }
    }

    // 2. Verify current era info
    let pre_era_info = pre_snapshot.current_era_info;
    let post_era_info = post_snapshot.current_era_info;

    assert_eq!(post_era_info.total_locked, pre_era_info.total_locked);
    assert_eq!(post_era_info.unlocking, pre_era_info.unlocking);

    // New period has started
    if is_new_subperiod && pre_protoc_state.subperiod() == Subperiod::BuildAndEarn {
        assert_eq!(
            post_era_info.current_stake_amount,
            StakeAmount {
                voting: Zero::zero(),
                build_and_earn: Zero::zero(),
                era: pre_protoc_state.era + 1,
                period: pre_protoc_state.period_number() + 1,
            }
        );
        assert_eq!(
            post_era_info.next_stake_amount,
            StakeAmount {
                voting: Zero::zero(),
                build_and_earn: Zero::zero(),
                era: pre_protoc_state.era + 2,
                period: pre_protoc_state.period_number() + 1,
            }
        );
    } else {
        assert_eq!(
            post_era_info.current_stake_amount,
            pre_era_info.next_stake_amount
        );
        assert_eq!(
            post_era_info.next_stake_amount.total(),
            post_era_info.current_stake_amount.total()
        );
        assert_eq!(
            post_era_info.next_stake_amount.era,
            post_protoc_state.era + 1,
        );
        assert_eq!(
            post_era_info.next_stake_amount.period,
            pre_protoc_state.period_number(),
        );
    }

    // 3. Verify era reward
    let era_span_index = DappStaking::era_reward_span_index(pre_protoc_state.era);
    let maybe_pre_era_reward_span = pre_snapshot.era_rewards.get(&era_span_index);
    let post_era_reward_span = post_snapshot
        .era_rewards
        .get(&era_span_index)
        .expect("Era reward info must exist after era has finished.");

    // Sanity check
    if let Some(pre_era_reward_span) = maybe_pre_era_reward_span {
        assert_eq!(
            pre_era_reward_span.last_era(),
            pre_protoc_state.era - 1,
            "If entry exists, it should cover eras up to the previous one, exactly."
        );
    }

    assert_eq!(
        post_era_reward_span.last_era(),
        pre_protoc_state.era,
        "Entry must cover the current era."
    );
    assert_eq!(
        post_era_reward_span
            .get(pre_protoc_state.era)
            .expect("Above check proved it must exist.")
            .staked,
        pre_snapshot.current_era_info.total_staked_amount(),
        "Total staked amount must be equal to total amount staked at the end of the era."
    );

    // 4. Verify period end
    if is_new_subperiod && pre_protoc_state.subperiod() == Subperiod::BuildAndEarn {
        let period_end_info = post_snapshot.period_end[&pre_protoc_state.period_number()];
        assert_eq!(
            period_end_info.total_vp_stake,
            pre_snapshot
                .current_era_info
                .staked_amount(Subperiod::Voting),
        );
    }

    // 5. Verify history cleanup marker update
    let period_has_advanced = pre_protoc_state.period_number() < post_protoc_state.period_number();
    if period_has_advanced {
        let reward_retention_in_periods: PeriodNumber =
            <Test as Config>::RewardRetentionInPeriods::get();

        let pre_marker = pre_snapshot.cleanup_marker;
        let post_marker = post_snapshot.cleanup_marker;

        if let Some(expired_period) = pre_protoc_state
            .period_number()
            .checked_sub(reward_retention_in_periods)
        {
            if let Some(period_end_info) = pre_snapshot.period_end.get(&expired_period) {
                let oldest_valid_era = period_end_info.final_era + 1;

                assert_eq!(post_marker.oldest_valid_era, oldest_valid_era);
                assert_eq!(post_marker.dapp_tiers_index, pre_marker.dapp_tiers_index);
                assert_eq!(post_marker.era_reward_index, pre_marker.era_reward_index);

                assert!(
                    !post_snapshot.period_end.contains_key(&expired_period),
                    "Expired entry should have been removed."
                );
            } else {
                assert_eq!(pre_marker, post_marker, "Must remain unchanged.");
            }
        } else {
            assert_eq!(pre_marker, post_marker, "Must remain unchanged.");
        }
    }

    // 6. Verify event(s)
    if is_new_subperiod {
        let events = dapp_staking_events();
        assert!(
            events.len() >= 2,
            "At least 2 events should exist from era & subperiod change."
        );
        assert_eq!(
            events[events.len() - 2],
            Event::NewEra {
                era: post_protoc_state.era,
            }
        );
        assert_eq!(
            events[events.len() - 1],
            Event::NewSubperiod {
                subperiod: pre_protoc_state.subperiod().next(),
                number: post_protoc_state.period_number(),
            }
        )
    } else {
        System::assert_last_event(RuntimeEvent::DappStaking(Event::NewEra {
            era: post_protoc_state.era,
        }));
    }
}

/// Verify `on_idle` cleanup.
pub(crate) fn assert_on_idle_cleanup() {
    // Pre-data snapshot (limited to speed up testing)
    let pre_cleanup_marker = HistoryCleanupMarker::<Test>::get();

    // Check if any span or tier reward cleanup is needed.
    let is_era_span_cleanup_expected =
        EraRewards::<Test>::get(&pre_cleanup_marker.era_reward_index)
            .map(|span| span.last_era() < pre_cleanup_marker.oldest_valid_era)
            .unwrap_or(false);
    let is_dapp_tiers_cleanup_expected =
        pre_cleanup_marker.dapp_tiers_index < pre_cleanup_marker.oldest_valid_era;

    // If span doesn't exists, but no cleanup is expected, we should increment the era reward index anyway.
    // This is because the span was never created in the first place since dApp staking v3 wasn't active then.
    //
    // In case of cleanup, we always increment the index.
    let is_era_reward_index_increase = is_era_span_cleanup_expected
        || !EraRewards::<Test>::contains_key(&pre_cleanup_marker.era_reward_index)
            && pre_cleanup_marker.oldest_valid_era > pre_cleanup_marker.era_reward_index;

    // Cleanup and verify post state.
    DappStaking::on_idle(System::block_number(), Weight::MAX);

    // Post checks
    let post_cleanup_marker = HistoryCleanupMarker::<Test>::get();

    if is_era_span_cleanup_expected {
        assert!(!EraRewards::<Test>::contains_key(
            pre_cleanup_marker.era_reward_index
        ));
    }

    if is_era_reward_index_increase {
        let span_length: EraNumber = <Test as Config>::EraRewardSpanLength::get();
        assert_eq!(
            post_cleanup_marker.era_reward_index,
            pre_cleanup_marker.era_reward_index + span_length
        );
    }

    if is_dapp_tiers_cleanup_expected {
        assert!(!DAppTiers::<Test>::contains_key(
            pre_cleanup_marker.dapp_tiers_index
        ));
        assert_eq!(
            post_cleanup_marker.dapp_tiers_index,
            pre_cleanup_marker.dapp_tiers_index + 1
        );
    }

    assert_eq!(
        post_cleanup_marker.oldest_valid_era, pre_cleanup_marker.oldest_valid_era,
        "Sanity check, must remain unchanged."
    );
}

/// Returns from which starting era to which ending era can rewards be claimed for the specified account.
///
/// If `None` is returned, there is nothing to claim.
///
/// **NOTE:** Doesn't consider reward expiration.
pub(crate) fn claimable_reward_range(account: AccountId) -> Option<(EraNumber, EraNumber)> {
    let ledger = Ledger::<Test>::get(&account);
    let protocol_state = ActiveProtocolState::<Test>::get();

    let earliest_stake_era = if let Some(era) = ledger.earliest_staked_era() {
        era
    } else {
        return None;
    };

    let last_claim_era = if ledger.staked_period() == Some(protocol_state.period_number()) {
        protocol_state.era - 1
    } else {
        // Period finished, we can claim up to its final era
        let period_end = PeriodEnd::<Test>::get(ledger.staked_period().unwrap()).unwrap();
        period_end.final_era
    };

    Some((earliest_stake_era, last_claim_era))
}

/// Number of times it's required to call `claim_staker_rewards` to claim all pending rewards.
///
/// In case no rewards are pending, return **zero**.
pub(crate) fn required_number_of_reward_claims(account: AccountId) -> u32 {
    let range = if let Some(range) = claimable_reward_range(account) {
        range
    } else {
        return 0;
    };

    let era_span_length: EraNumber = <Test as Config>::EraRewardSpanLength::get();
    let first = DappStaking::era_reward_span_index(range.0)
        .checked_div(era_span_length)
        .unwrap();
    let second = DappStaking::era_reward_span_index(range.1)
        .checked_div(era_span_length)
        .unwrap();

    second - first + 1
}

/// Check whether the given account ledger's stake rewards have expired.
///
/// `true` if expired, `false` otherwise.
pub(crate) fn is_account_ledger_expired(
    ledger: &AccountLedgerFor<Test>,
    current_period: PeriodNumber,
) -> bool {
    let valid_threshold_period = DappStaking::oldest_claimable_period(current_period);
    match ledger.staked_period() {
        Some(staked_period) if staked_period < valid_threshold_period => true,
        _ => false,
    }
}

/// Helpers to compose StakerInfo verification

fn assert_staker_info_after_unstake(
    pre_snapshot: &MemorySnapshot,
    post_snapshot: &MemorySnapshot,
    account: AccountId,
    smart_contract: &MockSmartContract,
    amount: Balance,
    is_full_unstake: bool,
) -> (Vec<StakeAmount>, BonusStatus) {
    let unstake_era = pre_snapshot.active_protocol_state.era;
    let unstake_period = pre_snapshot.active_protocol_state.period_number();
    let unstake_subperiod = pre_snapshot.active_protocol_state.subperiod();

    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, smart_contract.clone()))
        .expect("Entry must exist since 'unstake' is being called.");

    let (stake_amount_entries, bonus_status) =
        pre_staker_info
            .clone()
            .unstake(amount, unstake_era, unstake_subperiod);
    assert!(
        stake_amount_entries.len() <= 2 && stake_amount_entries.len() > 0,
        "Sanity check"
    );

    let unstake_amount = stake_amount_entries
        .iter()
        .max_by(|a, b| a.total().cmp(&b.total()))
        .expect("At least one value exists, otherwise we wouldn't be here.");
    assert_eq!(unstake_amount.total(), amount);

    if is_full_unstake {
        assert!(
            !StakerInfo::<Test>::contains_key(&account, smart_contract),
            "Entry must be deleted since it was a full unstake."
        );
    } else {
        let post_staker_info = post_snapshot
            .staker_info
            .get(&(account, *smart_contract))
            .expect(
            "Entry must exist since 'stake' operation was successful and it wasn't a full unstake.",
        );
        let should_keep_bonus = pre_staker_info.is_bonus_eligible()
            && (pre_staker_info.bonus_status > 1
                || unstake_subperiod == Subperiod::Voting
                || post_staker_info.staked_amount(Subperiod::Voting)
                    == pre_staker_info.staked_amount(Subperiod::Voting));

        assert_eq!(post_staker_info.period_number(), unstake_period);
        assert_eq!(
            post_staker_info.total_staked_amount(),
            pre_staker_info.total_staked_amount() - amount,
            "Total staked amount must decrease by the 'amount'"
        );

        assert_eq!(
            post_staker_info.staked_amount(Subperiod::Voting),
            pre_staker_info
                .staked_amount(Subperiod::Voting)
                .saturating_sub(unstake_amount.for_type(Subperiod::Voting)),
            "Voting next era staked amount must decreased by the 'unstake_amount'"
        );
        assert_eq!(
            post_staker_info.staked_amount(Subperiod::BuildAndEarn),
            pre_staker_info
                .staked_amount(Subperiod::BuildAndEarn)
                .saturating_sub(unstake_amount.for_type(Subperiod::BuildAndEarn)),
            "BuildAndEarn next era staked amount must decreased by the 'unstake_amount'"
        );

        assert_eq!(
            post_staker_info.is_bonus_eligible(),
            should_keep_bonus,
            "If 'voting stake' amount is reduced in B&E subperiod, 'BonusStatus' must reflect this."
        );

        if unstake_subperiod == Subperiod::BuildAndEarn
            && pre_staker_info.is_bonus_eligible()
            && post_staker_info.staked_amount(Subperiod::Voting)
                < pre_staker_info.staked_amount(Subperiod::Voting)
        {
            assert_eq!(
                post_staker_info.bonus_status, pre_staker_info.bonus_status - 1,
                "'BonusStatus' must correctly decrease moves when 'voting stake' is reduced in B&E subperiod."
            );
        }
    }

    (stake_amount_entries, bonus_status)
}

fn assert_staker_info_after_stake(
    pre_snapshot: &MemorySnapshot,
    post_snapshot: &MemorySnapshot,
    account: AccountId,
    smart_contract: &MockSmartContract,
    stake_amount: StakeAmount,
    incoming_bonus_status: BonusStatus,
) {
    let pre_staker_info = pre_snapshot
        .staker_info
        .get(&(account, smart_contract.clone()));

    let stake_period = pre_snapshot.active_protocol_state.period_number();
    let stake_subperiod = pre_snapshot.active_protocol_state.subperiod();

    // Verify post-state
    let post_staker_info = post_snapshot
        .staker_info
        .get(&(account, *smart_contract))
        .expect("Entry must exist since 'stake' operation was successful.");

    // Verify staker info
    // =====================
    // =====================
    match pre_staker_info {
        // We're just updating an existing entry
        Some(pre_staker_info) if pre_staker_info.period_number() == stake_period => {
            assert_eq!(
                post_staker_info.total_staked_amount(),
                pre_staker_info.total_staked_amount() + stake_amount.total(),
                "Total staked amount must increase by the total 'stake_amount'"
            );

            if pre_staker_info.bonus_status == 0 {
                assert_eq!(
                    post_staker_info.bonus_status, incoming_bonus_status,
                    "Bonus status should be updated to incoming one"
                );
            }

            assert_eq!(
                post_staker_info.staked_amount(Subperiod::Voting),
                pre_staker_info.staked_amount(Subperiod::Voting) + stake_amount.voting,
                "Voting staked amount must increase by the voting 'stake_amount'"
            );
            assert_eq!(
                post_staker_info.staked_amount(Subperiod::BuildAndEarn),
                pre_staker_info.staked_amount(Subperiod::BuildAndEarn)
                    + stake_amount.build_and_earn,
                "B&E staked amount must increase by the B&E 'stake_amount'"
            );
            assert_eq!(post_staker_info.period_number(), stake_period);
        }
        // A new entry is created.
        _ => {
            assert_eq!(
                post_staker_info.total_staked_amount(),
                stake_amount.total(),
                "Total staked amount must be equal to exactly the 'amount'"
            );
            assert!(stake_amount.total() >= <Test as Config>::MinimumStakeAmount::get());
            assert_eq!(
                post_staker_info.staked_amount(Subperiod::Voting),
                stake_amount.voting,
                "Voting staked amount must be equal to exactly the voting 'stake_amount'"
            );
            assert_eq!(
                post_staker_info.staked_amount(Subperiod::BuildAndEarn),
                stake_amount.build_and_earn,
                "B&E staked amount must be equal to exactly the B&E 'stake_amount'"
            );
            assert_eq!(post_staker_info.period_number(), stake_period);
        }
    }

    // Verify BonusStatus value for new staking info during voting subperiods
    if stake_subperiod == Subperiod::Voting {
        assert_default_bonus_status_after_voting_stake(account, &smart_contract);
    }
}

pub(crate) fn assert_default_bonus_status_after_voting_stake(
    account: AccountId,
    smart_contract: &MockSmartContract,
) {
    let default_bonus_status = *BonusStatusWrapperFor::<Test>::default();
    let staking_info = StakerInfo::<Test>::get(account, &smart_contract)
        .expect("Should exist since stake operation was successful.");
    assert_eq!(staking_info.bonus_status, default_bonus_status);
}

/// Helpers to compose ContractStake verification

fn assert_contract_stake_after_unstake(
    pre_contract_stake: &ContractStakeAmount,
    post_contract_stake: &ContractStakeAmount,
    pre_snapshot: &MemorySnapshot,
    unstake_amount_entries: Vec<StakeAmount>,
) {
    let era_number = pre_snapshot.active_protocol_state.era;
    let period_number = pre_snapshot.active_protocol_state.period_number();
    let unstake_amount_entries_clone = unstake_amount_entries.clone();
    let unstake_amount = unstake_amount_entries_clone
        .iter()
        .max_by(|a, b| a.total().cmp(&b.total()))
        .expect("At least one value exists, otherwise we wouldn't be here.");

    assert_eq!(
        post_contract_stake.total_staked_amount(period_number),
        pre_contract_stake.total_staked_amount(period_number) - unstake_amount.total(),
        "Staked amount must decreased by the 'unstake_amount'"
    );
    assert_eq!(
        post_contract_stake.staked_amount(period_number, Subperiod::Voting),
        pre_contract_stake
            .staked_amount(period_number, Subperiod::Voting)
            .saturating_sub(unstake_amount.for_type(Subperiod::Voting)),
        "Voting staked amount must decreased by the 'unstake_amount'"
    );
    assert_eq!(
        post_contract_stake.staked_amount(period_number, Subperiod::BuildAndEarn),
        pre_contract_stake
            .staked_amount(period_number, Subperiod::BuildAndEarn)
            .saturating_sub(unstake_amount.for_type(Subperiod::BuildAndEarn)),
        "BuildAndEarn staked amount must decreased by the 'unstake_amount'"
    );

    // A generic check, comparing what was received in the unstaked StakeAmount entries and the impact it had on the contract stake.
    for entry in unstake_amount_entries {
        let (era, amount) = (entry.era, entry.total());
        assert_eq!(
            post_contract_stake
                .get(era, period_number)
                .unwrap_or_default() // it's possible that full unstake cleared the entry
                .total(),
            pre_contract_stake
                .get(era, period_number)
                .expect("Must exist")
                .total()
                - amount
        );
    }

    // More precise check, independent of the generic check above.
    // If next era entry exists, it must be reduced by the unstake amount, nothing less.
    if let Some(entry) = pre_contract_stake.get(era_number + 1, period_number) {
        assert_eq!(
            post_contract_stake
                .get(era_number + 1, period_number)
                .unwrap_or_default()
                .total(),
            entry.total().saturating_sub(unstake_amount.total())
        );
    }
}

fn assert_contract_stake_after_stake(
    pre_contract_stake: &ContractStakeAmount,
    post_contract_stake: &ContractStakeAmount,
    pre_snapshot: &MemorySnapshot,
    stake_amount: StakeAmount,
) {
    let era_number = pre_snapshot.active_protocol_state.era + 1;
    let period_number = pre_snapshot.active_protocol_state.period_number();

    assert_eq!(
        post_contract_stake.total_staked_amount(period_number),
        pre_contract_stake.total_staked_amount(period_number) + stake_amount.total(),
        "Staked amount must increase by the 'stake_amount'"
    );
    assert_eq!(
        post_contract_stake.staked_amount(period_number, Subperiod::Voting),
        pre_contract_stake.staked_amount(period_number, Subperiod::Voting) + stake_amount.voting,
        "Voting staked amount must increase by the 'stake_amount'"
    );
    assert_eq!(
        post_contract_stake.staked_amount(period_number, Subperiod::BuildAndEarn),
        pre_contract_stake.staked_amount(period_number, Subperiod::BuildAndEarn)
            + stake_amount.build_and_earn,
        "B&E staked amount must increase by the 'stake_amount'"
    );

    assert_eq!(
        post_contract_stake.latest_stake_period(),
        Some(period_number)
    );
    assert_eq!(post_contract_stake.latest_stake_era(), Some(era_number));
}

/// Helpers to compose Ledger verification

fn assert_ledger_contract_stake_count(
    pre_ledger: &AccountLedgerFor<Test>,
    post_ledger: &AccountLedgerFor<Test>,
    is_expected_to_decrease: bool,
    is_expected_to_increase: bool,
) {
    if is_expected_to_decrease {
        assert_eq!(
            pre_ledger.contract_stake_count.saturating_sub(1),
            post_ledger.contract_stake_count,
            "Number of contract stakes must be decreased."
        );
    } else if is_expected_to_increase {
        assert_eq!(
            pre_ledger.contract_stake_count.saturating_add(1),
            post_ledger.contract_stake_count,
            "Number of contract stakes must be increased."
        );
    } else {
        assert_eq!(
            pre_ledger.contract_stake_count, post_ledger.contract_stake_count,
            "Number of contract stakes must remain the same."
        );
    }
}

/// Helpers to compose EraInfo verification

fn assert_era_info_current(
    pre_era_info: &EraInfo,
    post_era_info: &EraInfo,
    stake_amount_entries: impl IntoIterator<Item = StakeAmount>,
) {
    let entries: Vec<StakeAmount> = stake_amount_entries.into_iter().collect();
    for entry in entries {
        let (era, amount) = (entry.era, entry.total());

        if era == pre_era_info.current_stake_amount.era {
            if pre_era_info.total_staked_amount() < amount {
                assert!(post_era_info.total_staked_amount().is_zero());
            } else {
                assert_eq!(
                    post_era_info.total_staked_amount(),
                    pre_era_info.total_staked_amount() - amount,
                    "Total staked amount for the current era must decrease by 'amount'."
                );
                assert_eq!(
                    post_era_info.staked_amount(Subperiod::Voting),
                    pre_era_info.staked_amount(Subperiod::Voting) - entry.voting,
                    "Total Voting staked amount for the current era must decrease by 'entry.voting'."
                );
                assert_eq!(
                    post_era_info.staked_amount(Subperiod::BuildAndEarn),
                    pre_era_info.staked_amount(Subperiod::BuildAndEarn) - entry.build_and_earn,
                    "Total BuildAndEarn staked amount for the current era must decrease by 'entry.voting'."
                );
            }
        }
    }
}
