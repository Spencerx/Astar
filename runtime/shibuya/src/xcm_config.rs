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

use super::{
    AccountId, AllPalletsWithSystem, AssetId, Assets, Balance, Balances, DealWithFees,
    MessageQueue, ParachainInfo, ParachainSystem, PolkadotXcm, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeOrigin, ShibuyaAssetLocationIdConverter, TreasuryAccountId, XcAssetConfig,
    XcmWeightToFee, XcmpQueue,
};
use crate::weights;
use frame_support::{
    parameter_types,
    traits::{ConstU32, Contains, Everything, Nothing},
    weights::Weight,
};
use frame_system::EnsureRoot;
use sp_runtime::traits::{Convert, MaybeEquivalence};

// Polkadot imports
use cumulus_primitives_core::{AggregateMessageOrigin, ParaId};
use frame_support::traits::TransformOrigin;
use parachains_common::message_queue::ParaIdToSibling;
use polkadot_runtime_common::xcm_sender::NoPriceForMessageDelivery;
use xcm::{latest::prelude::*, v5::ROCOCO_GENESIS_HASH};
use xcm_builder::{
    AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowUnpaidExecutionFrom,
    ConvertedConcreteId, DescribeAllTerminal, DescribeFamily, EnsureXcmOrigin,
    FrameTransactionalProcessor, FungibleAdapter, FungiblesAdapter, HashedDescription, IsConcrete,
    NoChecking, ParentAsSuperuser, ParentIsPreset, RelayChainAsNative, SiblingParachainAsNative,
    SiblingParachainConvertsVia, SignedAccountId32AsNative, SignedToAccountId32,
    SovereignSignedViaLocation, TakeWeightCredit, UsingComponents, WeightInfoBounds,
    WithComputedOrigin,
};
use xcm_executor::{traits::JustTry, XcmExecutor};

// ORML imports
use orml_xcm_support::DisabledParachainFee;

// Astar imports
use astar_primitives::xcm::{
    AbsoluteAndRelativeReserveProvider, AccountIdToMultiLocation, AllowTopLevelPaidExecutionFrom,
    FixedRateOfForeignAsset, ReserveAssetFilter, XcmFungibleFeeHandler, MAX_ASSETS,
};

parameter_types! {
    pub RelayNetwork: Option<NetworkId> = Some(NetworkId::ByGenesis(ROCOCO_GENESIS_HASH));
    pub RelayChainOrigin: RuntimeOrigin = cumulus_pallet_xcm::Origin::Relay.into();
    pub UniversalLocation: InteriorLocation =
    [GlobalConsensus(RelayNetwork::get().unwrap()), Parachain(ParachainInfo::parachain_id().into())].into();
    pub const ShibuyaLocation: Location = Here.into_location();
    pub DummyCheckingAccount: AccountId = PolkadotXcm::check_account();
}

/// Type for specifying how a `Location` can be converted into an `AccountId`. This is used
/// when determining ownership of accounts for asset transacting and when attempting to use XCM
/// `Transact` in order to determine the dispatch Origin.
pub type LocationToAccountId = (
    // The parent (Relay-chain) origin converts to the default `AccountId`.
    ParentIsPreset<AccountId>,
    // Sibling parachain origins convert to AccountId via the `ParaId::into`.
    SiblingParachainConvertsVia<polkadot_parachain::primitives::Sibling, AccountId>,
    // Straight up local `AccountId32` origins just alias directly to `AccountId`.
    AccountId32Aliases<RelayNetwork, AccountId>,
    // Generates private `AccountId`s from `Location`s, in a stable & safe way.
    // Replaces the old `Account32Hash` approach.
    HashedDescription<AccountId, DescribeFamily<DescribeAllTerminal>>,
);

/// Means for transacting the native currency on this chain.
pub type CurrencyTransactor = FungibleAdapter<
    // Use this currency:
    Balances,
    // Use this currency when it is a fungible asset matching the given location or name:
    IsConcrete<ShibuyaLocation>,
    // Convert an XCM Location into a local account id:
    LocationToAccountId,
    // Our chain's account ID type (we can't get away without mentioning it explicitly):
    AccountId,
    // We don't track any teleports of `Balances`.
    (),
>;

/// Means for transacting assets besides the native currency on this chain.
pub type FungiblesTransactor = FungiblesAdapter<
    // Use this fungibles implementation:
    Assets,
    // Use this currency when it is a fungible asset matching the given location or name:
    ConvertedConcreteId<AssetId, Balance, ShibuyaAssetLocationIdConverter, JustTry>,
    // Convert an XCM Location into a local account id:
    LocationToAccountId,
    // Our chain's account ID type (we can't get away without mentioning it explicitly):
    AccountId,
    // We don't support teleport so no need to check any assets.
    NoChecking,
    // We don't support teleport so this is just a dummy account.
    DummyCheckingAccount,
>;

/// Means for transacting assets on this chain.
pub type AssetTransactors = (CurrencyTransactor, FungiblesTransactor);

/// This is the type we use to convert an (incoming) XCM origin into a local `Origin` instance,
/// ready for dispatching a transaction with Xcm's `Transact`. There is an `OriginKind` which can
/// biases the kind of local `Origin` it will become.
pub type XcmOriginToTransactDispatchOrigin = (
    // Sovereign account converter; this attempts to derive an `AccountId` from the origin location
    // using `LocationToAccountId` and then turn that into the usual `Signed` origin. Useful for
    // foreign chains who want to have a local sovereign account on this chain which they control.
    SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
    // Native converter for Relay-chain (Parent) location; will convert to a `Relay` origin when
    // recognised.
    RelayChainAsNative<RelayChainOrigin, RuntimeOrigin>,
    // Native converter for sibling Parachains; will convert to a `SiblingPara` origin when
    // recognised.
    SiblingParachainAsNative<cumulus_pallet_xcm::Origin, RuntimeOrigin>,
    // Superuser converter for the Relay-chain (Parent) location. This will allow it to issue a
    // transaction from the Root origin.
    ParentAsSuperuser<RuntimeOrigin>,
    // Xcm origins can be represented natively under the Xcm pallet's Xcm origin.
    pallet_xcm::XcmPassthrough<RuntimeOrigin>,
    // Native signed account converter; this just converts an `AccountId32` origin into a normal
    // `Origin::Signed` origin of the same 32-byte value.
    SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
);

parameter_types! {
    // One XCM operation is 1_000_000_000 weight - almost certainly a conservative estimate.
    // For the PoV size, we estimate 4 kB per instruction. This will be changed when we benchmark the instructions.
    pub UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 4 * 1024);
    pub const MaxInstructions: u32 = 100;
    pub const MaxAssetsIntoHolding: u32 = MAX_ASSETS as u32;
}

pub struct ParentOrParentsPlurality;
impl Contains<Location> for ParentOrParentsPlurality {
    fn contains(location: &Location) -> bool {
        matches!(location.unpack(), (1, []) | (1, [Plurality { .. }]))
    }
}

pub type XcmBarrier = (
    TakeWeightCredit,
    AllowTopLevelPaidExecutionFrom<Everything>,
    // This will first calculate the derived origin, before checking it against the barrier implementation
    WithComputedOrigin<AllowTopLevelPaidExecutionFrom<Everything>, UniversalLocation, ConstU32<8>>,
    // Parent and its plurality get free execution
    AllowUnpaidExecutionFrom<ParentOrParentsPlurality>,
    // Expected responses are OK.
    AllowKnownQueryResponses<PolkadotXcm>,
    // Subscriptions for version tracking are OK.
    AllowSubscriptionsFrom<Everything>,
);

// Used to handle XCM fee deposit into treasury account
pub type ShibuyaXcmFungibleFeeHandler = XcmFungibleFeeHandler<
    AccountId,
    ConvertedConcreteId<AssetId, Balance, ShibuyaAssetLocationIdConverter, JustTry>,
    Assets,
    TreasuryAccountId,
>;

pub type Weigher =
    WeightInfoBounds<weights::xcm::XcmWeight<Runtime, RuntimeCall>, RuntimeCall, MaxInstructions>;

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
    type RuntimeCall = RuntimeCall;
    type XcmSender = XcmRouter;
    type AssetTransactor = AssetTransactors;
    type OriginConverter = XcmOriginToTransactDispatchOrigin;
    type IsReserve = ReserveAssetFilter;
    type IsTeleporter = ();
    type UniversalLocation = UniversalLocation;
    type Barrier = XcmBarrier;
    type Weigher = Weigher;
    type Trader = (
        UsingComponents<XcmWeightToFee, ShibuyaLocation, AccountId, Balances, DealWithFees>,
        FixedRateOfForeignAsset<XcAssetConfig, ShibuyaXcmFungibleFeeHandler>,
    );
    type ResponseHandler = PolkadotXcm;
    type AssetTrap = PolkadotXcm;
    type AssetClaims = PolkadotXcm;
    type SubscriptionService = PolkadotXcm;

    type PalletInstancesInfo = AllPalletsWithSystem;
    type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
    type AssetLocker = ();
    type AssetExchanger = ();
    type FeeManager = ();
    type MessageExporter = ();
    type UniversalAliases = Nothing;
    type CallDispatcher = RuntimeCall;
    type SafeCallFilter = Everything;
    type Aliasers = Nothing;
    type TransactionalProcessor = FrameTransactionalProcessor;

    type HrmpNewChannelOpenRequestHandler = ();
    type HrmpChannelAcceptedHandler = ();
    type HrmpChannelClosingHandler = ();
    type XcmRecorder = PolkadotXcm;
}

/// Local origins on this chain are allowed to dispatch XCM sends/executions.
pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

/// The means for routing XCM messages which are not for local execution into the right message
/// queues.
pub type XcmRouter = (
    // Two routers - use UMP to communicate with the relay chain:
    cumulus_primitives_utility::ParentAsUmp<ParachainSystem, PolkadotXcm, ()>,
    // ..and XCMP to communicate with the sibling chains.
    XcmpQueue,
);

impl pallet_xcm::Config for Runtime {
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;

    type RuntimeEvent = RuntimeEvent;
    type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmRouter = XcmRouter;
    type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmExecuteFilter = Nothing;
    type XcmExecutor = XcmExecutor<XcmConfig>;
    type XcmTeleportFilter = Nothing;
    type XcmReserveTransferFilter = Everything;
    type Weigher = Weigher;
    type UniversalLocation = UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
    type Currency = Balances;
    type CurrencyMatcher = ();
    type TrustedLockers = ();
    type SovereignAccountOf = LocationToAccountId;
    type MaxLockers = ConstU32<0>;
    type WeightInfo = weights::pallet_xcm::SubstrateWeight<Runtime>;
    type MaxRemoteLockConsumers = ConstU32<0>;
    type RemoteLockConsumerIdentifier = ();
    type AdminOrigin = EnsureRoot<AccountId>;
}

impl cumulus_pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type XcmExecutor = XcmExecutor<XcmConfig>;
}

impl cumulus_pallet_xcmp_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ChannelInfo = ParachainSystem;
    type VersionWrapper = PolkadotXcm;
    type XcmpQueue = TransformOrigin<MessageQueue, AggregateMessageOrigin, ParaId, ParaIdToSibling>;
    type MaxInboundSuspended = ConstU32<1_000>;
    type MaxActiveOutboundChannels = ConstU32<128>;
    type MaxPageSize = ConstU32<{ 128 * 1024 }>;
    type ControllerOrigin = EnsureRoot<AccountId>;
    type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
    type PriceForSiblingDelivery = NoPriceForMessageDelivery<ParaId>;
    type WeightInfo = cumulus_pallet_xcmp_queue::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    /// The absolute location in perspective of the whole network.
    pub ShibuyaLocationAbsolute: Location = Location {
        parents: 1,
        interior: Parachain(ParachainInfo::parachain_id().into()).into()

    };
    /// Max asset types for one cross-chain transfer. `2` covers all current use cases.
    /// Can be updated with extra test cases in the future if needed.
    pub const MaxAssetsForTransfer: usize = 2;
}

/// Convert `AssetId` to optional `Location`. The impl is a wrapper
/// on `ShibuyaAssetLocationIdConverter`.
pub struct AssetIdConvert;
impl Convert<AssetId, Option<Location>> for AssetIdConvert {
    fn convert(asset_id: AssetId) -> Option<Location> {
        ShibuyaAssetLocationIdConverter::convert_back(&asset_id)
    }
}

impl orml_xtokens::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type CurrencyId = AssetId;
    type CurrencyIdConvert = AssetIdConvert;
    type AccountIdToLocation = AccountIdToMultiLocation;
    type SelfLocation = ShibuyaLocation;
    type XcmExecutor = XcmExecutor<XcmConfig>;
    type Weigher = Weigher;
    type BaseXcmWeight = UnitWeightCost;
    type UniversalLocation = UniversalLocation;
    type MaxAssetsForTransfer = MaxAssetsForTransfer;
    // Default impl. Refer to `orml-xtokens` docs for more details.
    type MinXcmFee = DisabledParachainFee;
    type LocationsFilter = Everything;
    type ReserveProvider = AbsoluteAndRelativeReserveProvider<ShibuyaLocationAbsolute>;
    type RateLimiter = ();
    type RateLimiterId = ();
}
