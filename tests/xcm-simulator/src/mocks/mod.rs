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

pub(crate) mod msg_queue;
pub(crate) mod parachain;
pub(crate) mod relay_chain;

use frame_support::traits::{IsType, OnFinalize, OnInitialize};
use sp_runtime::traits::{Bounded, StaticLookup};
use sp_runtime::{BuildStorage, DispatchResult};
use xcm::latest::prelude::*;
use xcm_executor::traits::ConvertLocation;
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain, TestExt};

pub const ALICE: sp_runtime::AccountId32 = sp_runtime::AccountId32::new([0xFAu8; 32]);
pub const BOB: sp_runtime::AccountId32 = sp_runtime::AccountId32::new([0xFBu8; 32]);
pub const INITIAL_BALANCE: u128 = 1_000_000_000_000_000_000_000_000;
pub const ONE: u128 = 1_000_000_000_000_000_000;

decl_test_parachain! {
    pub struct ParaA {
        Runtime = parachain::Runtime,
        XcmpMessageHandler = parachain::MsgQueue,
        DmpMessageHandler = parachain::MsgQueue,
        new_ext = para_ext(1),
    }
}

decl_test_parachain! {
    pub struct ParaB {
        Runtime = parachain::Runtime,
        XcmpMessageHandler = parachain::MsgQueue,
        DmpMessageHandler = parachain::MsgQueue,
        new_ext = para_ext(2),
    }
}

decl_test_parachain! {
    pub struct ParaC {
        Runtime = parachain::Runtime,
        XcmpMessageHandler = parachain::MsgQueue,
        DmpMessageHandler = parachain::MsgQueue,
        new_ext = para_ext(1000),
    }
}

decl_test_relay_chain! {
    pub struct Relay {
        Runtime = relay_chain::Runtime,
        RuntimeCall = relay_chain::RuntimeCall,
        RuntimeEvent = relay_chain::RuntimeEvent,
        XcmConfig = relay_chain::XcmConfig,
        MessageQueue = relay_chain::MessageQueue,
        System = relay_chain::System,
        new_ext = relay_ext(),
    }
}

decl_test_network! {
    pub struct MockNet {
        relay_chain = Relay,
        parachains = vec![
            (1, ParaA),
            (2, ParaB),
            (1000, ParaC),
        ],
    }
}

pub type RelayChainPalletXcm = pallet_xcm::Pallet<relay_chain::Runtime>;

pub type ParachainPalletXcm = pallet_xcm::Pallet<parachain::Runtime>;
pub type ParachainAssets = pallet_assets::Pallet<parachain::Runtime>;
pub type ParachainBalances = pallet_balances::Pallet<parachain::Runtime>;
pub type ParachainXtokens = orml_xtokens::Pallet<parachain::Runtime>;

pub fn parent_account_id() -> parachain::AccountId {
    let location = (Parent,);
    parachain::LocationToAccountId::convert_location(&location.into()).unwrap()
}

/// Derive parachain sovereign account on relay chain, from parachain Id
pub fn child_para_account_id(para: u32) -> relay_chain::AccountId {
    let location = (Parachain(para),);
    relay_chain::LocationToAccountId::convert_location(&location.into()).unwrap()
}

/// Derive parachain sovereign account on a sibling parachain, from parachain Id
pub fn sibling_para_account_id(para: u32) -> parachain::AccountId {
    let location = (Parent, Parachain(para));
    parachain::LocationToAccountId::convert_location(&location.into()).unwrap()
}

/// Derive parachain's account's account on a sibling parachain
pub fn sibling_para_account_account_id(
    para: u32,
    who: sp_runtime::AccountId32,
) -> parachain::AccountId {
    let location = (
        Parent,
        Parachain(para),
        AccountId32 {
            // we have kusama as relay in mock
            network: Some(Kusama),
            id: who.into(),
        },
    );
    parachain::LocationToAccountId::convert_location(&location.into()).unwrap()
}

/// Prepare parachain test externality
pub fn para_ext(para_id: u32) -> sp_io::TestExternalities {
    use parachain::{MsgQueue, Runtime, System};

    let mut t = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Runtime> {
        balances: vec![
            (ALICE, INITIAL_BALANCE),
            (sibling_para_account_account_id(1, ALICE), INITIAL_BALANCE),
            (sibling_para_account_account_id(2, ALICE), INITIAL_BALANCE),
            (sibling_para_account_id(1), INITIAL_BALANCE),
            (sibling_para_account_id(2), INITIAL_BALANCE),
        ],
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
        MsgQueue::set_para_id(para_id.into());

        parachain::DappStaking::on_initialize(1);
    });
    ext
}

/// Prepare relay chain test externality
pub fn relay_ext() -> sp_io::TestExternalities {
    use relay_chain::{Runtime, System};

    let mut t = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Runtime> {
        balances: vec![
            (ALICE, INITIAL_BALANCE),
            (child_para_account_id(1), INITIAL_BALANCE),
            (child_para_account_id(2), INITIAL_BALANCE),
        ],
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Advance parachain blocks until `block_number`.
/// No effect if parachain is already at that number or exceeds it.
pub fn advance_parachain_block_to(block_number: u64) {
    while parachain::System::block_number() < block_number {
        // On Finalize
        let current_block_number = parachain::System::block_number();
        parachain::PolkadotXcm::on_finalize(current_block_number);
        parachain::Balances::on_finalize(current_block_number);
        parachain::DappStaking::on_finalize(current_block_number);
        parachain::System::on_finalize(current_block_number);

        // Forward 1 block
        let current_block_number = current_block_number + 1;
        parachain::System::set_block_number(current_block_number);
        parachain::System::reset_events();

        // On Initialize
        parachain::System::on_initialize(current_block_number);
        {
            parachain::DappStaking::on_initialize(current_block_number);
        }
        parachain::Balances::on_initialize(current_block_number);
        parachain::PolkadotXcm::on_initialize(current_block_number);
    }
}

/// Register and configure the asset for use in XCM
/// It first create the asset in `pallet_assets` and then register the asset multilocation
/// mapping in `pallet_xc_asset_config`, and lastly set the asset per second for calculating
/// XCM execution cost (only applicable if `is_sufficent` is true)
pub fn register_and_setup_xcm_asset<Runtime, AssetId>(
    origin: Runtime::RuntimeOrigin,
    // AssetId for the new asset
    asset_id: AssetId,
    // Asset multilocation
    asset_location: impl Into<Location> + Clone,
    // Asset controller
    asset_controller: <Runtime::Lookup as StaticLookup>::Source,
    // make asset payable, default true
    is_sufficent: Option<bool>,
    // minimum balance for account to exist (ED), default, 0
    min_balance: Option<Runtime::Balance>,
    // Asset unit per second for calculating execution cost for XCM, default 1_000_000_000_000
    units_per_second: Option<u128>,
) -> DispatchResult
where
    Runtime: pallet_xc_asset_config::Config + pallet_assets::Config,
    AssetId: IsType<<Runtime as pallet_xc_asset_config::Config>::AssetId>
        + IsType<<Runtime as pallet_assets::Config>::AssetId>
        + Clone,
{
    // Register the asset
    pallet_assets::Pallet::<Runtime>::force_create(
        origin.clone(),
        <Runtime as pallet_assets::Config>::AssetIdParameter::from(asset_id.clone().into()),
        asset_controller,
        is_sufficent.unwrap_or(true),
        min_balance.unwrap_or(Bounded::min_value()),
    )?;

    // Save the asset and multilocation mapping
    pallet_xc_asset_config::Pallet::<Runtime>::register_asset_location(
        origin.clone(),
        Box::new(asset_location.clone().into().into_versioned()),
        asset_id.into(),
    )?;

    // set the units per second for XCM cost
    pallet_xc_asset_config::Pallet::<Runtime>::set_asset_units_per_second(
        origin,
        Box::new(asset_location.into().into_versioned()),
        units_per_second.unwrap_or(1_000_000_000_000),
    )
}
