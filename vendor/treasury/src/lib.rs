// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! > Made with *Substrate*, for *Polkadot*.
//!
//! [![github]](https://github.com/paritytech/substrate/frame/fast-unstake) -
//! [![polkadot]](https://polkadot.network)
//!
//! [polkadot]: https://img.shields.io/badge/polkadot-E6007A?style=for-the-badge&logo=polkadot&logoColor=white
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//!
//! # Treasury Pallet
//!
//! The Treasury pallet provides a "pot" of funds that can be managed by stakeholders in the system
//! and a structure for making spending proposals from this pot.
//!
//! ## Overview
//!
//! The Treasury Pallet itself provides the pot to store funds, and a means for stakeholders to
//! propose and claim expenditures (aka spends). The chain will need to provide a method to approve
//! spends (e.g. public referendum) and a method for collecting funds (e.g. inflation, fees).
//!
//! By way of example, stakeholders could vote to fund the Treasury with a portion of the block
//! reward and use the funds to pay developers.
//!
//! ### Terminology
//!
//! - **Proposal:** A suggestion to allocate funds from the pot to a beneficiary.
//! - **Beneficiary:** An account who will receive the funds from a proposal iff the proposal is
//!   approved.
//! - **Pot:** Unspent funds accumulated by the treasury pallet.
//!
//! ## Pallet API
//!
//! See the [`pallet`] module for more information about the interfaces this pallet exposes,
//! including its configuration trait, dispatchables, storage items, events and errors.
//!

#![cfg_attr(not(feature = "std"), no_std)]

mod benchmarking;
#[cfg(test)]
mod tests;
pub mod weights;
use core::marker::PhantomData;

extern crate alloc;

use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use sp_runtime::{
    traits::{AccountIdConversion, Saturating, StaticLookup, Zero},
    Permill, RuntimeDebug,
};

use frame_support::{
    dispatch::DispatchResult,
    ensure, print,
    traits::{
        Currency, ExistenceRequirement::KeepAlive, Get, Imbalance, OnUnbalanced,
        ReservableCurrency, WithdrawReasons,
    },
    weights::Weight,
    PalletId,
};

pub use pallet::*;
pub use weights::WeightInfo;

pub type BalanceOf<T, I = ()> =
    <<T as Config<I>>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
pub type PositiveImbalanceOf<T, I = ()> = <<T as Config<I>>::Currency as Currency<
    <T as frame_system::Config>::AccountId,
>>::PositiveImbalance;
pub type NegativeImbalanceOf<T, I = ()> = <<T as Config<I>>::Currency as Currency<
    <T as frame_system::Config>::AccountId,
>>::NegativeImbalance;
type AccountIdLookupOf<T> = <<T as frame_system::Config>::Lookup as StaticLookup>::Source;

/// A trait to allow the Treasury Pallet to spend it's funds for other purposes.
/// There is an expectation that the implementer of this trait will correctly manage
/// the mutable variables passed to it:
/// * `budget_remaining`: How much available funds that can be spent by the treasury. As funds are
///   spent, you must correctly deduct from this value.
/// * `imbalance`: Any imbalances that you create should be subsumed in here to maximize efficiency
///   of updating the total issuance. (i.e. `deposit_creating`)
/// * `total_weight`: Track any weight that your `spend_fund` implementation uses by updating this
///   value.
/// * `missed_any`: If there were items that you want to spend on, but there were not enough funds,
///   mark this value as `true`. This will prevent the treasury from burning the excess funds.
#[impl_trait_for_tuples::impl_for_tuples(30)]
pub trait SpendFunds<T: Config<I>, I: 'static = ()> {
    fn spend_funds(
        budget_remaining: &mut BalanceOf<T, I>,
        imbalance: &mut PositiveImbalanceOf<T, I>,
        total_weight: &mut Weight,
        missed_any: &mut bool,
    );
}

/// An index of a proposal. Just a `u32`.
pub type ProposalIndex = u32;

/// A spending proposal.
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
#[derive(Encode, Decode, Clone, PartialEq, Eq, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct Proposal<AccountId, Balance> {
    /// The account proposing it.
    proposer: AccountId,
    /// The (total) amount that should be paid if the proposal is accepted.
    value: Balance,
    /// The account to whom the payment should be made if the proposal is accepted.
    beneficiary: AccountId,
    /// The amount held on deposit (reserved) for making this proposal.
    bond: Balance,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

    #[pallet::config]
    pub trait Config<I: 'static = ()>: frame_system::Config {
        /// The staking balance.
        type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;

        /// Origin from which approvals must come.
        type ApproveOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Origin from which rejections must come.
        type RejectOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// The overarching event type.
        type RuntimeEvent: From<Event<Self, I>>
            + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Handler for the unbalanced decrease when slashing for a rejected proposal or bounty.
        type OnSlash: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

        /// Fraction of a proposal's value that should be bonded in order to place the proposal.
        /// An accepted proposal gets these back. A rejected proposal does not.
        #[pallet::constant]
        type ProposalBond: Get<Permill>;

        /// Minimum amount of funds that should be placed in a deposit for making a proposal.
        #[pallet::constant]
        type ProposalBondMinimum: Get<BalanceOf<Self, I>>;

        /// Maximum amount of funds that should be placed in a deposit for making a proposal.
        #[pallet::constant]
        type ProposalBondMaximum: Get<Option<BalanceOf<Self, I>>>;

        /// Period between successive spends.
        #[pallet::constant]
        type SpendPeriod: Get<BlockNumberFor<Self>>;

        /// Percentage of spare funds (if any) that are burnt per spend period.
        #[pallet::constant]
        type Burn: Get<Permill>;

        /// The treasury's pallet id, used for deriving its sovereign account ID.
        #[pallet::constant]
        type PalletId: Get<PalletId>;

        /// Handler for the unbalanced decrease when treasury funds are burned.
        type BurnDestination: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;

        /// Runtime hooks to external pallet using treasury to compute spend funds.
        type SpendFunds: SpendFunds<Self, I>;

        /// The maximum number of approvals that can wait in the spending queue.
        ///
        /// NOTE: This parameter is also used within the Bounties Pallet extension if enabled.
        #[pallet::constant]
        type MaxApprovals: Get<u32>;
    }

    /// Number of proposals that have been made.
    #[pallet::storage]
    #[pallet::getter(fn proposal_count)]
    pub(crate) type ProposalCount<T, I = ()> = StorageValue<_, ProposalIndex, ValueQuery>;

    /// Proposals that have been made.
    #[pallet::storage]
    #[pallet::getter(fn proposals)]
    pub type Proposals<T: Config<I>, I: 'static = ()> = StorageMap<
        _,
        Twox64Concat,
        ProposalIndex,
        Proposal<T::AccountId, BalanceOf<T, I>>,
        OptionQuery,
    >;

    /// The amount which has been reported as inactive to Currency.
    #[pallet::storage]
    pub type Deactivated<T: Config<I>, I: 'static = ()> =
        StorageValue<_, BalanceOf<T, I>, ValueQuery>;

    /// Proposal indices that have been approved but not yet awarded.
    #[pallet::storage]
    #[pallet::getter(fn approvals)]
    pub type Approvals<T: Config<I>, I: 'static = ()> =
        StorageValue<_, BoundedVec<ProposalIndex, T::MaxApprovals>, ValueQuery>;

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config<I>, I: 'static = ()> {
        #[serde(skip)]
        _config: core::marker::PhantomData<(T, I)>,
    }

    #[pallet::genesis_build]
    impl<T: Config<I>, I: 'static> BuildGenesisConfig for GenesisConfig<T, I> {
        fn build(&self) {
            // Create Treasury account
            let account_id = <Pallet<T, I>>::account_id();
            let min = T::Currency::minimum_balance();
            if T::Currency::free_balance(&account_id) < min {
                let _ = T::Currency::make_free_balance_be(&account_id, min);
            }
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    #[repr(u8)]
    pub enum Event<T: Config<I>, I: 'static = ()> {
        /// New proposal.
        Proposed { proposal_index: ProposalIndex } = 0,
        /// We have ended a spend period and will now allocate funds.
        Spending { budget_remaining: BalanceOf<T, I> } = 1,
        /// Some funds have been allocated.
        Awarded {
            proposal_index: ProposalIndex,
            award: BalanceOf<T, I>,
            account: T::AccountId,
        } = 2,
        /// A proposal was rejected; funds were slashed.
        Rejected {
            proposal_index: ProposalIndex,
            slashed: BalanceOf<T, I>,
        } = 3,
        /// Some of our funds have been burnt.
        Burnt { burnt_funds: BalanceOf<T, I> } = 4,
        /// Spending has finished; this is the amount that rolls over until next spend.
        Rollover { rollover_balance: BalanceOf<T, I> } = 5,
        /// Some funds have been deposited.
        Deposit { value: BalanceOf<T, I> } = 6,
        /// A new spend proposal has been approved.
        /// The inactive funds of the pallet have been updated.
        UpdatedInactive {
            reactivated: BalanceOf<T, I>,
            deactivated: BalanceOf<T, I>,
        } = 8,
    }

    /// Error for the treasury pallet.
    #[pallet::error]
    pub enum Error<T, I = ()> {
        /// Proposer's balance is too low.
        InsufficientProposersBalance,
        /// No proposal, bounty or spend at that index.
        InvalidIndex,
        /// Too many approvals in the queue.
        TooManyApprovals,
        /// The spend origin is valid but the amount it is allowed to spend is lower than the
        /// amount to be spent.
        InsufficientPermission,
        /// Proposal has not been approved.
        ProposalNotApproved,
    }

    #[pallet::hooks]
    impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
        /// ## Complexity
        /// - `O(A)` where `A` is the number of approvals
        fn on_initialize(n: frame_system::pallet_prelude::BlockNumberFor<T>) -> Weight {
            let pot = Self::pot();
            let deactivated = Deactivated::<T, I>::get();
            if pot != deactivated {
                T::Currency::reactivate(deactivated);
                T::Currency::deactivate(pot);
                Deactivated::<T, I>::put(&pot);
                Self::deposit_event(Event::<T, I>::UpdatedInactive {
                    reactivated: deactivated,
                    deactivated: pot,
                });
            }

            // Check to see if we should spend some funds!
            if (n % T::SpendPeriod::get()).is_zero() {
                Self::spend_funds()
            } else {
                Weight::zero()
            }
        }

        #[cfg(feature = "try-runtime")]
        fn try_state(
            _: frame_system::pallet_prelude::BlockNumberFor<T>,
        ) -> Result<(), sp_runtime::TryRuntimeError> {
            Self::do_try_state()?;
            Ok(())
        }
    }

    #[pallet::call]
    impl<T: Config<I>, I: 'static> Pallet<T, I> {
        /// Put forward a suggestion for spending.
        ///
        /// ## Dispatch Origin
        ///
        /// Must be signed.
        ///
        /// ## Details
        /// A deposit proportional to the value is reserved and slashed if the proposal is rejected.
        /// It is returned once the proposal is awarded.
        ///
        /// ### Complexity
        /// - O(1)
        ///
        /// ## Events
        ///
        /// Emits [`Event::Proposed`] if successful.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::propose_spend())]
        #[allow(deprecated)]
        #[deprecated(
            note = "`propose_spend` will be removed in February 2024. Use `spend` instead."
        )]
        pub fn propose_spend(
            origin: OriginFor<T>,
            #[pallet::compact] value: BalanceOf<T, I>,
            beneficiary: AccountIdLookupOf<T>,
        ) -> DispatchResult {
            let proposer = ensure_signed(origin)?;
            let beneficiary = T::Lookup::lookup(beneficiary)?;

            let bond = Self::calculate_bond(value);
            T::Currency::reserve(&proposer, bond)
                .map_err(|_| Error::<T, I>::InsufficientProposersBalance)?;

            let c = Self::proposal_count();
            <ProposalCount<T, I>>::put(c + 1);
            <Proposals<T, I>>::insert(
                c,
                Proposal {
                    proposer,
                    value,
                    beneficiary,
                    bond,
                },
            );

            Self::deposit_event(Event::Proposed { proposal_index: c });
            Ok(())
        }

        /// Reject a proposed spend.
        ///
        /// ## Dispatch Origin
        ///
        /// Must be [`Config::RejectOrigin`].
        ///
        /// ## Details
        /// The original deposit will be slashed.
        ///
        /// ### Complexity
        /// - O(1)
        ///
        /// ## Events
        ///
        /// Emits [`Event::Rejected`] if successful.
        #[pallet::call_index(1)]
        #[pallet::weight((T::WeightInfo::reject_proposal(), DispatchClass::Operational))]
        #[allow(deprecated)]
        #[deprecated(
            note = "`reject_proposal` will be removed in February 2024. Use `spend` instead."
        )]
        pub fn reject_proposal(
            origin: OriginFor<T>,
            #[pallet::compact] proposal_id: ProposalIndex,
        ) -> DispatchResult {
            T::RejectOrigin::ensure_origin(origin)?;

            let proposal =
                <Proposals<T, I>>::take(&proposal_id).ok_or(Error::<T, I>::InvalidIndex)?;
            let value = proposal.bond;
            let imbalance = T::Currency::slash_reserved(&proposal.proposer, value).0;
            T::OnSlash::on_unbalanced(imbalance);

            Self::deposit_event(Event::<T, I>::Rejected {
                proposal_index: proposal_id,
                slashed: value,
            });
            Ok(())
        }

        /// Approve a proposal.
        ///
        /// ## Dispatch Origin
        ///
        /// Must be [`Config::ApproveOrigin`].
        ///
        /// ## Details
        ///
        /// At a later time, the proposal will be allocated to the beneficiary and the original
        /// deposit will be returned.
        ///
        /// ### Complexity
        ///  - O(1).
        ///
        /// ## Events
        ///
        /// No events are emitted from this dispatch.
        #[pallet::call_index(2)]
        #[pallet::weight((T::WeightInfo::approve_proposal(T::MaxApprovals::get()), DispatchClass::Operational))]
        #[allow(deprecated)]
        #[deprecated(
            note = "`approve_proposal` will be removed in February 2024. Use `spend` instead."
        )]
        pub fn approve_proposal(
            origin: OriginFor<T>,
            #[pallet::compact] proposal_id: ProposalIndex,
        ) -> DispatchResult {
            T::ApproveOrigin::ensure_origin(origin)?;

            ensure!(
                <Proposals<T, I>>::contains_key(proposal_id),
                Error::<T, I>::InvalidIndex
            );
            Approvals::<T, I>::try_append(proposal_id)
                .map_err(|_| Error::<T, I>::TooManyApprovals)?;
            Ok(())
        }
    }
}

impl<T: Config<I>, I: 'static> Pallet<T, I> {
    // Add public immutables and private mutables.

    /// The account ID of the treasury pot.
    ///
    /// This actually does computation. If you need to keep using it, then make sure you cache the
    /// value and only call this once.
    pub fn account_id() -> T::AccountId {
        T::PalletId::get().into_account_truncating()
    }

    /// The needed bond for a proposal whose spend is `value`.
    fn calculate_bond(value: BalanceOf<T, I>) -> BalanceOf<T, I> {
        let mut r = T::ProposalBondMinimum::get().max(T::ProposalBond::get() * value);
        if let Some(m) = T::ProposalBondMaximum::get() {
            r = r.min(m);
        }
        r
    }

    /// Spend some money! returns number of approvals before spend.
    pub fn spend_funds() -> Weight {
        let mut total_weight = Weight::zero();

        let mut budget_remaining = Self::pot();
        Self::deposit_event(Event::Spending { budget_remaining });
        let account_id = Self::account_id();

        let mut missed_any = false;
        let mut imbalance = <PositiveImbalanceOf<T, I>>::zero();
        let proposals_len = Approvals::<T, I>::mutate(|v| {
            let proposals_approvals_len = v.len() as u32;
            v.retain(|&index| {
                // Should always be true, but shouldn't panic if false or we're screwed.
                if let Some(p) = Self::proposals(index) {
                    if p.value <= budget_remaining {
                        budget_remaining -= p.value;
                        <Proposals<T, I>>::remove(index);

                        // return their deposit.
                        let err_amount = T::Currency::unreserve(&p.proposer, p.bond);
                        debug_assert!(err_amount.is_zero());

                        // provide the allocation.
                        imbalance.subsume(T::Currency::deposit_creating(&p.beneficiary, p.value));

                        Self::deposit_event(Event::Awarded {
                            proposal_index: index,
                            award: p.value,
                            account: p.beneficiary,
                        });
                        false
                    } else {
                        missed_any = true;
                        true
                    }
                } else {
                    false
                }
            });
            proposals_approvals_len
        });

        total_weight += T::WeightInfo::on_initialize_proposals(proposals_len);

        // Call Runtime hooks to external pallet using treasury to compute spend funds.
        T::SpendFunds::spend_funds(
            &mut budget_remaining,
            &mut imbalance,
            &mut total_weight,
            &mut missed_any,
        );

        if !missed_any {
            // burn some proportion of the remaining budget if we run a surplus.
            let burn = (T::Burn::get() * budget_remaining).min(budget_remaining);
            budget_remaining -= burn;

            let (debit, credit) = T::Currency::pair(burn);
            imbalance.subsume(debit);
            T::BurnDestination::on_unbalanced(credit);
            Self::deposit_event(Event::Burnt { burnt_funds: burn })
        }

        // Must never be an error, but better to be safe.
        // proof: budget_remaining is account free balance minus ED;
        // Thus we can't spend more than account free balance minus ED;
        // Thus account is kept alive; qed;
        if let Err(problem) =
            T::Currency::settle(&account_id, imbalance, WithdrawReasons::TRANSFER, KeepAlive)
        {
            print("Inconsistent state - couldn't settle imbalance for funds spent by treasury");
            // Nothing else to do here.
            drop(problem);
        }

        Self::deposit_event(Event::Rollover {
            rollover_balance: budget_remaining,
        });

        total_weight
    }

    /// Return the amount of money in the pot.
    // The existential deposit is not part of the pot so treasury account never gets deleted.
    pub fn pot() -> BalanceOf<T, I> {
        T::Currency::free_balance(&Self::account_id())
            // Must never be less than 0 but better be safe.
            .saturating_sub(T::Currency::minimum_balance())
    }

    /// Ensure the correctness of the state of this pallet.
    #[cfg(any(feature = "try-runtime", test))]
    fn do_try_state() -> Result<(), sp_runtime::TryRuntimeError> {
        Self::try_state_proposals()?;
        Ok(())
    }

    /// ### Invariants of proposal storage items
    ///
    /// 1. [`ProposalCount`] >= Number of elements in [`Proposals`].
    /// 2. Each entry in [`Proposals`] should be saved under a key strictly less than current
    /// [`ProposalCount`].
    /// 3. Each [`ProposalIndex`] contained in [`Approvals`] should exist in [`Proposals`].
    /// Note, that this automatically implies [`Approvals`].count() <= [`Proposals`].count().
    #[cfg(any(feature = "try-runtime", test))]
    fn try_state_proposals() -> Result<(), sp_runtime::TryRuntimeError> {
        let current_proposal_count = ProposalCount::<T, I>::get();
        ensure!(
            current_proposal_count as usize >= Proposals::<T, I>::iter().count(),
            "Actual number of proposals exceeds `ProposalCount`."
        );

        Proposals::<T, I>::iter_keys().try_for_each(|proposal_index| -> DispatchResult {
            ensure!(
				current_proposal_count as u32 > proposal_index,
				"`ProposalCount` should by strictly greater than any ProposalIndex used as a key for `Proposals`."
			);
            Ok(())
        })?;

        Approvals::<T, I>::get()
            .iter()
            .try_for_each(|proposal_index| -> DispatchResult {
                ensure!(
                    Proposals::<T, I>::contains_key(proposal_index),
                    "Proposal indices in `Approvals` must also be contained in `Proposals`."
                );
                Ok(())
            })?;

        Ok(())
    }
}

impl<T: Config<I>, I: 'static> OnUnbalanced<NegativeImbalanceOf<T, I>> for Pallet<T, I> {
    fn on_nonzero_unbalanced(amount: NegativeImbalanceOf<T, I>) {
        let numeric_amount = amount.peek();

        // Must resolve into existing but better to be safe.
        let _ = T::Currency::resolve_creating(&Self::account_id(), amount);

        Self::deposit_event(Event::Deposit {
            value: numeric_amount,
        });
    }
}

/// TypedGet implementation to get the AccountId of the Treasury.
pub struct TreasuryAccountId<R>(PhantomData<R>);
impl<R> sp_runtime::traits::TypedGet for TreasuryAccountId<R>
where
    R: crate::Config,
{
    type Type = <R as frame_system::Config>::AccountId;
    fn get() -> Self::Type {
        <crate::Pallet<R>>::account_id()
    }
}
