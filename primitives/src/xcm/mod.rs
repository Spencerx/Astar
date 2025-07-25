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

//! # XCM Primitives
//!
//! ## Overview
//!
//! Collection of common XCM primitives used by runtimes.
//!
//! - `AssetLocationIdConverter` - conversion between local asset Id and cross-chain asset multilocation
//! - `FixedRateOfForeignAsset` - weight trader for execution payment in foreign asset
//! - `ReserveAssetFilter` - used to check whether asset/origin are a valid reserve location
//! - `XcmFungibleFeeHandler` - used to handle XCM fee execution fees
//!
//! Please refer to implementation below for more info.
//!

use crate::AccountId;

use frame_support::{
    ensure,
    traits::{tokens::fungibles, Contains, ContainsPair, Get, ProcessMessageError},
    weights::constants::WEIGHT_REF_TIME_PER_SECOND,
};
use sp_runtime::traits::{Bounded, Convert, MaybeEquivalence, Zero};
use sp_std::marker::PhantomData;

// Polkadot imports
use xcm::latest::{prelude::*, Weight};
use xcm_builder::{CreateMatcher, MatchXcm, TakeRevenue};
use xcm_executor::traits::{MatchesFungibles, Properties, ShouldExecute, WeightTrader};

// ORML imports
use orml_traits::location::{RelativeReserveProvider, Reserve};

use pallet_xc_asset_config::{ExecutionPaymentRate, XcAssetLocation};

#[cfg(test)]
mod tests;

pub const XCM_SIZE_LIMIT: u32 = 2u32.pow(16);
pub const MAX_ASSETS: u32 = 64;
pub const ASSET_HUB_PARA_ID: u32 = 1000;

/// Used to convert between cross-chain asset multilocation and local asset Id.
///
/// This implementation relies on `XcAssetConfig` pallet to handle mapping.
/// In case asset location hasn't been mapped, it means the asset isn't supported (yet).
pub struct AssetLocationIdConverter<AssetId, AssetMapper>(PhantomData<(AssetId, AssetMapper)>);
impl<AssetId, AssetMapper> MaybeEquivalence<Location, AssetId>
    for AssetLocationIdConverter<AssetId, AssetMapper>
where
    AssetId: Clone + Eq + Bounded,
    AssetMapper: XcAssetLocation<AssetId>,
{
    fn convert(location: &Location) -> Option<AssetId> {
        AssetMapper::get_asset_id(location.clone())
    }

    fn convert_back(id: &AssetId) -> Option<Location> {
        AssetMapper::get_xc_asset_location(id.clone())
    }
}

/// Used as weight trader for foreign assets.
///
/// In case foreigin asset is supported as payment asset, XCM execution time
/// on-chain can be paid by the foreign asset, using the configured rate.
pub struct FixedRateOfForeignAsset<T: ExecutionPaymentRate, R: TakeRevenue> {
    /// Total used weight
    weight: Weight,
    /// Total consumed assets
    consumed: u128,
    /// Asset Id (as Location) and units per second for payment
    asset_location_and_units_per_second: Option<(Location, u128)>,
    _pd: PhantomData<(T, R)>,
}

impl<T: ExecutionPaymentRate, R: TakeRevenue> WeightTrader for FixedRateOfForeignAsset<T, R> {
    fn new() -> Self {
        Self {
            weight: Weight::zero(),
            consumed: 0,
            asset_location_and_units_per_second: None,
            _pd: PhantomData,
        }
    }

    fn buy_weight(
        &mut self,
        weight: Weight,
        payment: xcm_executor::AssetsInHolding,
        _: &XcmContext,
    ) -> Result<xcm_executor::AssetsInHolding, XcmError> {
        log::trace!(
            target: "xcm::weight",
            "FixedRateOfForeignAsset::buy_weight weight: {:?}, payment: {:?}",
            weight, payment,
        );

        // Atm in pallet, we only support one asset so this should work
        let payment_asset = payment
            .fungible_assets_iter()
            .next()
            .ok_or(XcmError::TooExpensive)?;

        match payment_asset {
            Asset {
                id: AssetId(asset_location),
                fun: Fungibility::Fungible(_),
            } => {
                if let Some(units_per_second) = T::get_units_per_second(asset_location.clone()) {
                    let amount = units_per_second.saturating_mul(weight.ref_time() as u128) // TODO: change this to u64?
                        / (WEIGHT_REF_TIME_PER_SECOND as u128);
                    if amount == 0 {
                        return Ok(payment);
                    }

                    let unused = payment
                        .checked_sub((asset_location.clone(), amount).into())
                        .map_err(|_| XcmError::TooExpensive)?;

                    self.weight = self.weight.saturating_add(weight);

                    // If there are multiple calls to `BuyExecution` but with different assets, we need to be able to handle that.
                    // Current primitive implementation will just keep total track of consumed asset for the FIRST consumed asset.
                    // Others will just be ignored when refund is concerned.
                    if let Some((old_asset_location, _)) =
                        self.asset_location_and_units_per_second.clone()
                    {
                        if old_asset_location == asset_location {
                            self.consumed = self.consumed.saturating_add(amount);
                        }
                    } else {
                        self.consumed = self.consumed.saturating_add(amount);
                        self.asset_location_and_units_per_second =
                            Some((asset_location, units_per_second));
                    }

                    Ok(unused)
                } else {
                    Err(XcmError::TooExpensive)
                }
            }
            _ => Err(XcmError::TooExpensive),
        }
    }

    fn refund_weight(&mut self, weight: Weight, _: &XcmContext) -> Option<Asset> {
        log::trace!(target: "xcm::weight", "FixedRateOfForeignAsset::refund_weight weight: {:?}", weight);

        if let Some((asset_location, units_per_second)) =
            self.asset_location_and_units_per_second.clone()
        {
            let weight = weight.min(self.weight);
            let amount = units_per_second.saturating_mul(weight.ref_time() as u128)
                / (WEIGHT_REF_TIME_PER_SECOND as u128);

            self.weight = self.weight.saturating_sub(weight);
            self.consumed = self.consumed.saturating_sub(amount);

            if amount > 0 {
                Some((asset_location, amount).into())
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl<T: ExecutionPaymentRate, R: TakeRevenue> Drop for FixedRateOfForeignAsset<T, R> {
    fn drop(&mut self) {
        if let Some((asset_location, _)) = self.asset_location_and_units_per_second.clone() {
            if self.consumed > 0 {
                R::take_revenue((asset_location, self.consumed).into());
            }
        }
    }
}

/// Used to determine whether the cross-chain asset is coming from a trusted reserve or not
///
/// Basically, we trust any cross-chain asset from any location to act as a reserve since
/// in order to support the xc-asset, we need to first register it in the `XcAssetConfig` pallet.
///
pub struct ReserveAssetFilter;
impl ContainsPair<Asset, Location> for ReserveAssetFilter {
    fn contains(asset: &Asset, origin: &Location) -> bool {
        // We assume that relay chain and sibling parachain assets are trusted reserves for their assets
        let AssetId(location) = &asset.id;
        let reserve_location = match (location.parents, location.first_interior()) {
            // sibling parachain
            (1, Some(Parachain(id))) => Some(Location::new(1, [Parachain(*id)])),
            // relay chain
            (1, _) => Some(Location::parent()),
            _ => None,
        };

        if let Some(ref reserve) = reserve_location {
            origin == reserve
        } else {
            false
        }
    }
}

/// Allow DOT from Asset Hub.
pub struct DotFromAssetHub;
impl ContainsPair<Asset, Location> for DotFromAssetHub {
    fn contains(asset: &Asset, origin: &Location) -> bool {
        Location::new(1, Parachain(ASSET_HUB_PARA_ID)) == *origin
            && matches!(
                asset,
                Asset {
                    id: AssetId(Location {
                        parents: 1,
                        interior: Here
                    }),
                    fun: Fungible(_),
                },
            )
    }
}

/// All locations we trust as reserves for particular assets.
pub type Reserves = (
    // Trusted reserves and DOT from relay
    ReserveAssetFilter,
    // DOT from Asset Hub
    DotFromAssetHub,
);

/// Used to deposit XCM fees into a destination account.
///
/// Only handles fungible assets for now.
/// If for any reason taking of the fee fails, it will be burned and and error trace will be printed.
///
pub struct XcmFungibleFeeHandler<AccountId, Matcher, Assets, FeeDestination>(
    sp_std::marker::PhantomData<(AccountId, Matcher, Assets, FeeDestination)>,
);
impl<
        AccountId: Eq,
        Assets: fungibles::Mutate<AccountId>,
        Matcher: MatchesFungibles<Assets::AssetId, Assets::Balance>,
        FeeDestination: Get<AccountId>,
    > TakeRevenue for XcmFungibleFeeHandler<AccountId, Matcher, Assets, FeeDestination>
{
    fn take_revenue(revenue: Asset) {
        match Matcher::matches_fungibles(&revenue) {
            Ok((asset_id, amount)) => {
                if amount > Zero::zero() {
                    if let Err(error) =
                        Assets::mint_into(asset_id.clone(), &FeeDestination::get(), amount)
                    {
                        log::error!(
                            target: "xcm::weight",
                            "XcmFeeHandler::take_revenue failed when minting asset: {:?}", error,
                        );
                    } else {
                        log::trace!(
                            target: "xcm::weight",
                            "XcmFeeHandler::take_revenue took {:?} of asset Id {:?}",
                            amount, asset_id,
                        );
                    }
                }
            }
            Err(_) => {
                log::error!(
                    target: "xcm::weight",
                    "XcmFeeHandler:take_revenue failed to match fungible asset, it has been burned."
                );
            }
        }
    }
}

/// Convert `AccountId` to `Location`.
pub struct AccountIdToMultiLocation;
impl Convert<AccountId, Location> for AccountIdToMultiLocation {
    fn convert(account: AccountId) -> Location {
        AccountId32 {
            network: None,
            id: account.into(),
        }
        .into()
    }
}

/// `Asset` reserve location provider. It's based on `RelativeReserveProvider` and in
/// addition will convert self absolute location to relative location.
pub struct AbsoluteAndRelativeReserveProvider<AbsoluteLocation>(PhantomData<AbsoluteLocation>);
impl<AbsoluteLocation: Get<Location>> Reserve
    for AbsoluteAndRelativeReserveProvider<AbsoluteLocation>
{
    fn reserve(asset: &Asset) -> Option<Location> {
        RelativeReserveProvider::reserve(asset).map(|reserve_location| {
            if reserve_location == AbsoluteLocation::get() {
                Location::here()
            } else {
                reserve_location
            }
        })
    }
}

// Copying the barrier here due to this issue - https://github.com/paritytech/polkadot-sdk/issues/1638
// The fix was introduced in v1.3.0 via this PR - https://github.com/paritytech/polkadot-sdk/pull/1733
// Below is the exact same copy from the fix PR.

const MAX_ASSETS_FOR_BUY_EXECUTION: usize = 2;

/// Allows execution from `origin` if it is contained in `T` (i.e. `T::Contains(origin)`) taking
/// payments into account.
///
/// Only allows for `TeleportAsset`, `WithdrawAsset`, `ClaimAsset` and `ReserveAssetDeposit` XCMs
/// because they are the only ones that place assets in the Holding Register to pay for execution.
pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<Location>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
    fn should_execute<RuntimeCall>(
        origin: &Location,
        instructions: &mut [Instruction<RuntimeCall>],
        max_weight: Weight,
        _properties: &mut Properties,
    ) -> Result<(), ProcessMessageError> {
        log::trace!(
            target: "xcm::barriers",
            "AllowTopLevelPaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
            origin, instructions, max_weight, _properties,
        );

        ensure!(T::contains(origin), ProcessMessageError::Unsupported);
        // We will read up to 5 instructions. This allows up to 3 `ClearOrigin` instructions. We
        // allow for more than one since anything beyond the first is a no-op and it's conceivable
        // that composition of operations might result in more than one being appended.
        let end = instructions.len().min(5);
        instructions[..end]
            .matcher()
            .match_next_inst(|inst| match inst {
                ReceiveTeleportedAsset(..) | ReserveAssetDeposited(..) => Ok(()),
                WithdrawAsset(ref assets) if assets.len() <= MAX_ASSETS_FOR_BUY_EXECUTION => Ok(()),
                ClaimAsset { ref assets, .. } if assets.len() <= MAX_ASSETS_FOR_BUY_EXECUTION => {
                    Ok(())
                }
                _ => Err(ProcessMessageError::BadFormat),
            })?
            .skip_inst_while(|inst| matches!(inst, ClearOrigin))?
            .match_next_inst(|inst| match inst {
                BuyExecution {
                    weight_limit: Limited(ref mut weight),
                    ..
                } if weight.all_gte(max_weight) => {
                    *weight = max_weight;
                    Ok(())
                }
                BuyExecution {
                    ref mut weight_limit,
                    ..
                } if weight_limit == &Unlimited => {
                    *weight_limit = Limited(max_weight);
                    Ok(())
                }
                _ => Err(ProcessMessageError::Overweight(max_weight)),
            })?;
        Ok(())
    }
}
