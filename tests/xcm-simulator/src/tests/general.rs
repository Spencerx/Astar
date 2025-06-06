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

use crate::mocks::{msg_queue::mock_msg_queue, parachain, relay_chain, *};

use frame_support::{assert_ok, weights::Weight};
use parity_scale_codec::Encode;
use xcm::prelude::*;
use xcm_simulator::TestExt;

use astar_primitives::dapp_staking::SmartContractHandle;

#[test]
fn basic_dmp() {
    MockNet::reset();

    let remark = parachain::RuntimeCall::System(
        frame_system::Call::<parachain::Runtime>::remark_with_event {
            remark: vec![1, 2, 3],
        },
    );

    // A remote `Transact` is sent to the parachain A.
    // No need to pay for the execution time since parachain is configured to allow unpaid execution from parents.
    Relay::execute_with(|| {
        assert_ok!(RelayChainPalletXcm::send_xcm(
            Here,
            Parachain(1),
            Xcm(vec![Transact {
                origin_kind: OriginKind::SovereignAccount,
                fallback_max_weight: Some(Weight::from_parts(1_000_000_000, 1024 * 1024)),
                call: remark.encode().into(),
            }]),
        ));
    });

    // Execute remote transact and verify that `Remarked` event is emitted.
    ParaA::execute_with(|| {
        use parachain::{RuntimeEvent, System};
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::System(frame_system::Event::Remarked { .. })
        )));
    });
}

#[test]
fn basic_ump() {
    MockNet::reset();

    let remark = relay_chain::RuntimeCall::System(
        frame_system::Call::<relay_chain::Runtime>::remark_with_event {
            remark: vec![1, 2, 3],
        },
    );

    // A remote `Transact` is sent to the relaychain.
    // No need to pay for the execution time since relay chain is configured to allow unpaid execution from everything.
    ParaA::execute_with(|| {
        assert_ok!(ParachainPalletXcm::send_xcm(
            Here,
            Parent,
            Xcm(vec![Transact {
                origin_kind: OriginKind::SovereignAccount,
                fallback_max_weight: Some(Weight::from_parts(1_000_000_000, 1024 * 1024)),
                call: remark.encode().into(),
            }]),
        ));
    });

    Relay::execute_with(|| {
        use relay_chain::{RuntimeEvent, System};
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::System(frame_system::Event::Remarked { .. })
        )));
    });
}

#[test]
fn basic_xcmp() {
    MockNet::reset();

    let remark = parachain::RuntimeCall::System(
        frame_system::Call::<parachain::Runtime>::remark_with_event {
            remark: vec![1, 2, 3],
        },
    );
    ParaA::execute_with(|| {
        assert_ok!(ParachainPalletXcm::send_xcm(
            Here,
            (Parent, Parachain(2)),
            Xcm(vec![
                WithdrawAsset((Here, 100_000_000_000_u128).into()),
                BuyExecution {
                    fees: (Here, 100_000_000_000_u128).into(),
                    weight_limit: Unlimited
                },
                Transact {
                    origin_kind: OriginKind::SovereignAccount,
                    fallback_max_weight: Some(Weight::from_parts(1_000_000_000, 1024 * 1024)),
                    call: remark.encode().into(),
                }
            ]),
        ));
    });

    ParaB::execute_with(|| {
        use parachain::{RuntimeEvent, System};
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::System(frame_system::Event::Remarked { .. })
        )));
    });
}

#[test]
fn error_when_not_paying_enough() {
    MockNet::reset();

    let source_location: Location = (Parent,).into();
    let source_id: parachain::AssetId = 123;

    let dest: Location = Junction::AccountId32 {
        network: None,
        id: ALICE.into(),
    }
    .into();
    // This time we are gonna put a rather high number of units per second
    // Lets put (25 * 1e12) as units per second, later it will be divided by 1e12
    // to calculate cost
    ParaA::execute_with(|| {
        assert_ok!(register_and_setup_xcm_asset::<parachain::Runtime, _>(
            parachain::RuntimeOrigin::root(),
            source_id,
            source_location,
            parent_account_id(),
            Some(true),
            Some(1),
            Some(2_500_000_000_000u128)
        ));
    });

    // We are sending 99 tokens from relay.
    // we know the buy_execution will spend 4 * 25 = 100
    Relay::execute_with(|| {
        assert_ok!(RelayChainPalletXcm::limited_reserve_transfer_assets(
            relay_chain::RuntimeOrigin::signed(ALICE),
            Box::new(Parachain(1).into()),
            Box::new(VersionedLocation::V5(dest).clone().into()),
            Box::new((Here, 99).into()),
            0,
            WeightLimit::Unlimited,
        ));
    });

    ParaA::execute_with(|| {
        use parachain::{RuntimeEvent, System};

        // check for xcm too expensive error
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::MsgQueue(mock_msg_queue::Event::ExecutedDownward(
                _,
                Outcome::Incomplete {
                    error: XcmError::TooExpensive,
                    ..
                }
            ))
        )));

        // amount not received as it is not paying enough
        assert_eq!(ParachainAssets::balance(source_id, &ALICE.into()), 0);
    });
}

#[test]
fn remote_dapps_staking_staker_claim() {
    MockNet::reset();

    // The idea of this test case is to remotely claim dApps staking staker rewards.
    // Remote claim will be sent from parachain A to parachain B.

    let smart_contract =
        parachain::MockSmartContract::wasm(parachain::AccountId::from([13 as u8; 32]));
    let stake_amount = 100_000_000;

    // 1st step
    // Register contract & stake on it. Advance a few blocks until at least era 4 since we need 3 claimable rewards.
    // Enable parachain A sovereign account to claim on Alice's behalf.
    ParaB::execute_with(|| {
        assert_ok!(parachain::DappStaking::register(
            parachain::RuntimeOrigin::root(),
            ALICE,
            smart_contract.clone(),
        ));
        assert_ok!(parachain::DappStaking::lock(
            parachain::RuntimeOrigin::signed(ALICE),
            stake_amount,
        ));
        assert_ok!(parachain::DappStaking::stake(
            parachain::RuntimeOrigin::signed(ALICE),
            smart_contract.clone(),
            stake_amount,
        ));

        // advance enough blocks so we at least get to era 5 - this gives us era 2, 3 and 4 for claiming
        while pallet_dapp_staking::ActiveProtocolState::<parachain::Runtime>::get().era() < 5 {
            advance_parachain_block_to(parachain::System::block_number() + 1);
        }
        // Ensure it's not first block so event storage is clear
        advance_parachain_block_to(parachain::System::block_number() + 1);

        // Register para A sovereign account as proxy with dApps staking privileges
        assert_ok!(parachain::Proxy::add_proxy(
            parachain::RuntimeOrigin::signed(ALICE),
            sibling_para_account_id(1),
            parachain::ProxyType::StakerRewardClaim,
            0
        ));
    });

    let claim_staker_call = parachain::RuntimeCall::DappStaking(pallet_dapp_staking::Call::<
        parachain::Runtime,
    >::claim_staker_rewards {});

    // 2nd step
    // Dispatch remote `claim_staker` call from Para A to Para B
    ParaA::execute_with(|| {
        let proxy_call =
            parachain::RuntimeCall::Proxy(pallet_proxy::Call::<parachain::Runtime>::proxy {
                real: ALICE,
                force_proxy_type: None,
                call: Box::new(claim_staker_call.clone()),
            });

        // Send the remote transact operation
        assert_ok!(ParachainPalletXcm::send_xcm(
            Here,
            Location::new(1, Parachain(2)),
            Xcm(vec![
                WithdrawAsset((Here, 100_000_000_000_u128).into()),
                BuyExecution {
                    fees: (Here, 100_000_000_000_u128).into(),
                    weight_limit: Unlimited
                },
                Transact {
                    origin_kind: OriginKind::SovereignAccount,
                    fallback_max_weight: Some(Weight::from_parts(1_000_000_000, 1024 * 1024)),
                    call: proxy_call.encode().into(),
                },
            ]),
        ));
    });

    // 3rd step
    // Receive claim & verify it was successful
    ParaB::execute_with(|| {
        // We expect exactly one `Reward` event
        let dapp_staking_events = parachain::System::events()
            .into_iter()
            .map(|r| r.event)
            .filter_map(|e| {
                <parachain::Runtime as pallet_dapp_staking::Config>::RuntimeEvent::from(e)
                    .try_into()
                    .ok()
            })
            .collect::<Vec<pallet_dapp_staking::Event<parachain::Runtime>>>();

        assert_eq!(dapp_staking_events.len(), 1);
        assert_matches::assert_matches!(
            dapp_staking_events[0].clone(),
                pallet_dapp_staking::Event::Reward { account, .. }
            if account == ALICE
        );

        // Cleanup events
        parachain::System::reset_events();
    });

    // 4th step
    // Dispatch two remote `claim_staker` calls from Para A to Para B, but as a batch
    ParaA::execute_with(|| {
        let batch_call =
            parachain::RuntimeCall::Utility(pallet_utility::Call::<parachain::Runtime>::batch {
                calls: vec![claim_staker_call.clone(), claim_staker_call.clone()],
            });

        let proxy_call =
            parachain::RuntimeCall::Proxy(pallet_proxy::Call::<parachain::Runtime>::proxy {
                real: ALICE,
                force_proxy_type: None,
                call: Box::new(batch_call),
            });

        // Send the remote transact operation
        assert_ok!(ParachainPalletXcm::send_xcm(
            Here,
            Location::new(1, Parachain(2)),
            Xcm(vec![
                WithdrawAsset((Here, 100_000_000_000_u128).into()),
                BuyExecution {
                    fees: (Here, 100_000_000_000_u128).into(),
                    weight_limit: Unlimited
                },
                Transact {
                    origin_kind: OriginKind::SovereignAccount,
                    fallback_max_weight: Some(Weight::from_parts(1_000_000_000, 1024 * 1024)),
                    call: proxy_call.encode().into(),
                }
            ]),
        ));
    });

    // 5th step
    // Receive two claims & verify they were successful
    ParaB::execute_with(|| {
        // We expect exactly two `Reward` events
        assert_eq!(
            parachain::System::events()
                .iter()
                .filter(|r| matches!(
                    r.event,
                    parachain::RuntimeEvent::DappStaking(pallet_dapp_staking::Event::Reward { .. })
                ))
                .count(),
            2
        );
    });
}
