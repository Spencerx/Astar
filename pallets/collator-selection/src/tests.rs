// This file is part of Astar.

// Copyright (C) Stake Technologies Pte.Ltd.
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

use crate as collator_selection;
use crate::{
    mock::*, CandidacyBond, CandidateInfo, Candidates, DesiredCandidates, Error, Invulnerables,
    LastAuthoredBlock, NonCandidates, SlashDestination,
};
use frame_support::{
    assert_noop, assert_ok,
    traits::{Currency, OnInitialize},
};
use pallet_balances::Error as BalancesError;
use sp_runtime::{traits::BadOrigin, BuildStorage};

#[test]
fn basic_setup_works() {
    new_test_ext().execute_with(|| {
        assert_eq!(DesiredCandidates::<Test>::get(), 2);
        assert_eq!(CandidacyBond::<Test>::get(), 10);

        assert!(Candidates::<Test>::get().is_empty());
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2]);
    });
}

#[test]
fn it_should_set_invulnerables() {
    new_test_ext().execute_with(|| {
        let new_set = vec![1, 2, 3, 4];
        assert_ok!(CollatorSelection::set_invulnerables(
            RuntimeOrigin::signed(RootAccount::get()),
            new_set.clone()
        ));
        assert_eq!(Invulnerables::<Test>::get(), new_set);

        // cannot set with non-root.
        assert_noop!(
            CollatorSelection::set_invulnerables(RuntimeOrigin::signed(1), new_set.clone()),
            BadOrigin
        );

        // cannot set invulnerables without associated validator keys
        let invulnerables = vec![7];
        assert_noop!(
            CollatorSelection::set_invulnerables(
                RuntimeOrigin::signed(RootAccount::get()),
                invulnerables.clone()
            ),
            Error::<Test>::ValidatorNotRegistered
        );
    });
}

#[test]
fn add_invulnerable_works() {
    new_test_ext().execute_with(|| {
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2]);
        assert_ok!(CollatorSelection::add_invulnerable(
            RuntimeOrigin::signed(RootAccount::get()),
            3
        ));
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2, 3]);

        // cannot add with non-root.
        assert_noop!(
            CollatorSelection::add_invulnerable(RuntimeOrigin::signed(1), 4),
            BadOrigin
        );

        // cannot add existing invulnerable
        assert_noop!(
            CollatorSelection::add_invulnerable(RuntimeOrigin::signed(RootAccount::get()), 1),
            Error::<Test>::AlreadyInvulnerable
        );

        // cannot add invulnerable without associated validator keys
        assert_noop!(
            CollatorSelection::add_invulnerable(RuntimeOrigin::signed(RootAccount::get()), 7),
            Error::<Test>::ValidatorNotRegistered
        );
    });
}

#[test]
fn remove_invulnerable_works() {
    new_test_ext().execute_with(|| {
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2]);
        assert_ok!(CollatorSelection::remove_invulnerable(
            RuntimeOrigin::signed(RootAccount::get()),
            1
        ));
        assert_eq!(Invulnerables::<Test>::get(), vec![2]);

        // cannot remove with non-root.
        assert_noop!(
            CollatorSelection::remove_invulnerable(RuntimeOrigin::signed(1), 2),
            BadOrigin
        );

        // cannot remove non-existent invulnerable
        assert_noop!(
            CollatorSelection::remove_invulnerable(RuntimeOrigin::signed(RootAccount::get()), 1),
            Error::<Test>::NotInvulnerable
        );
    });
}

#[test]
fn set_desired_candidates_works() {
    new_test_ext().execute_with(|| {
        // given
        assert_eq!(DesiredCandidates::<Test>::get(), 2);

        // can set
        assert_ok!(CollatorSelection::set_desired_candidates(
            RuntimeOrigin::signed(RootAccount::get()),
            7
        ));
        assert_eq!(DesiredCandidates::<Test>::get(), 7);

        // rejects bad origin
        assert_noop!(
            CollatorSelection::set_desired_candidates(RuntimeOrigin::signed(1), 8),
            BadOrigin
        );
    });
}

#[test]
fn set_candidacy_bond() {
    new_test_ext().execute_with(|| {
        // given
        assert_eq!(CandidacyBond::<Test>::get(), 10);

        // can set
        assert_ok!(CollatorSelection::set_candidacy_bond(
            RuntimeOrigin::signed(RootAccount::get()),
            7
        ));
        assert_eq!(CandidacyBond::<Test>::get(), 7);

        // rejects bad origin.
        assert_noop!(
            CollatorSelection::set_candidacy_bond(RuntimeOrigin::signed(1), 8),
            BadOrigin
        );
    });
}

#[test]
fn cannot_register_candidate_if_too_many() {
    new_test_ext().execute_with(|| {
        // reset desired candidates:
        <crate::DesiredCandidates<Test>>::put(0);

        // can't accept anyone anymore.
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(3)),
            Error::<Test>::TooManyCandidates,
        );

        // reset desired candidates:
        <crate::DesiredCandidates<Test>>::put(1);
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));

        // but no more
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(5)),
            Error::<Test>::TooManyCandidates,
        );
    })
}

#[test]
fn cannot_unregister_candidate_if_too_few() {
    new_test_ext().execute_with(|| {
        // reset desired candidates:
        <crate::DesiredCandidates<Test>>::put(1);
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));

        // can not remove too few
        assert_noop!(
            CollatorSelection::leave_intent(RuntimeOrigin::signed(4)),
            Error::<Test>::TooFewCandidates,
        );
    })
}

#[test]
fn cannot_register_as_candidate_if_invulnerable() {
    new_test_ext().execute_with(|| {
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2]);

        // can't 1 because it is invulnerable.
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(1)),
            Error::<Test>::AlreadyInvulnerable,
        );
    })
}

#[test]
fn cannot_register_as_candidate_if_keys_not_registered() {
    new_test_ext().execute_with(|| {
        // can't 7 because keys not registered.
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(7)),
            Error::<Test>::ValidatorNotRegistered
        );
    })
}

#[test]
fn cannot_register_dupe_candidate() {
    new_test_ext().execute_with(|| {
        // can add 3 as candidate
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        let addition = CandidateInfo {
            who: 3,
            deposit: 10,
        };
        assert_eq!(Candidates::<Test>::get(), vec![addition]);
        assert_eq!(LastAuthoredBlock::<Test>::get(3), 10);
        assert_eq!(Balances::free_balance(3), 90);

        // but no more
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(3)),
            Error::<Test>::AlreadyCandidate,
        );
    })
}

#[test]
fn cannot_register_as_candidate_if_poor() {
    new_test_ext().execute_with(|| {
        assert_eq!(Balances::free_balance(&3), 100);
        assert_eq!(Balances::free_balance(&33), 0);

        // works
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));

        // poor
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(33)),
            BalancesError::<Test>::InsufficientBalance,
        );
    });
}

#[test]
fn cannot_register_candidate_if_externally_blacklisted() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(BLACKLISTED_ACCOUNT)),
            Error::<Test>::NotAllowedCandidate,
        );
    })
}

#[test]
fn register_as_candidate_works() {
    new_test_ext().execute_with(|| {
        // given
        assert_eq!(DesiredCandidates::<Test>::get(), 2);
        assert_eq!(CandidacyBond::<Test>::get(), 10);
        assert_eq!(Candidates::<Test>::get(), Vec::new());
        assert_eq!(Invulnerables::<Test>::get(), vec![1, 2]);

        // take two endowed, non-invulnerables accounts.
        assert_eq!(Balances::free_balance(&3), 100);
        assert_eq!(Balances::free_balance(&4), 100);

        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));

        assert_eq!(Balances::free_balance(&3), 90);
        assert_eq!(Balances::free_balance(&4), 90);

        assert_eq!(Candidates::<Test>::get().len(), 2);
    });
}

#[test]
fn leave_intent() {
    new_test_ext().execute_with(|| {
        // register a candidate.
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_eq!(Balances::free_balance(3), 90);

        // register too so can leave above min candidates
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(5)
        ));
        assert_eq!(Balances::free_balance(5), 90);
        assert_eq!(Balances::reserved_balance(3), 10);

        // cannot leave if not candidate.
        assert_noop!(
            CollatorSelection::leave_intent(RuntimeOrigin::signed(4)),
            Error::<Test>::NotCandidate
        );

        // candidacy removed and unbonding started
        assert_ok!(CollatorSelection::leave_intent(RuntimeOrigin::signed(3)));
        assert_eq!(Balances::free_balance(3), 90);
        assert_eq!(Balances::reserved_balance(3), 10);
        assert_eq!(LastAuthoredBlock::<Test>::get(3), 10);
        // 10 unbonding from session 1
        assert_eq!(NonCandidates::<Test>::get(3), Some((1, 10)));
    });
}

#[test]
fn withdraw_unbond() {
    new_test_ext().execute_with(|| {
        // register a candidate.
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        // register too so can leave above min candidates
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(5)
        ));
        assert_ok!(CollatorSelection::leave_intent(RuntimeOrigin::signed(3)));

        initialize_to_block(9);

        // cannot register again during un-bonding
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(3)),
            Error::<Test>::BondStillLocked
        );
        assert_noop!(
            CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)),
            Error::<Test>::BondStillLocked
        );

        initialize_to_block(10);
        assert_ok!(CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)));
        assert_eq!(NonCandidates::<Test>::get(3), None);
        assert_eq!(Balances::free_balance(3), 100);
        assert_eq!(Balances::reserved_balance(3), 0);

        assert_noop!(
            CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)),
            Error::<Test>::NoCandidacyBond
        );
    });
}

#[test]
fn re_register_with_unbonding() {
    new_test_ext().execute_with(|| {
        // register a candidate.
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        // register too so can leave above min candidates
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(5)
        ));
        assert_eq!(Balances::free_balance(3), 90);
        assert_eq!(Balances::reserved_balance(3), 10);
        assert_ok!(CollatorSelection::leave_intent(RuntimeOrigin::signed(3)));
        assert_eq!(Balances::free_balance(3), 90);
        assert_eq!(Balances::reserved_balance(3), 10);

        // still on current session
        initialize_to_block(9);

        // cannot register again during un-bonding
        assert_noop!(
            CollatorSelection::register_as_candidate(RuntimeOrigin::signed(3)),
            Error::<Test>::BondStillLocked
        );
        initialize_to_block(10);
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        // previous bond is unreserved and reserved again
        assert_eq!(Balances::free_balance(3), 90);
        assert_eq!(Balances::reserved_balance(3), 10);
    })
}

#[test]
fn authorship_event_handler() {
    new_test_ext().execute_with(|| {
        // put 100 in the pot + 5 for ED
        Balances::make_free_balance_be(&CollatorSelection::account_id(), 105);

        // 4 is the default author.
        assert_eq!(Balances::free_balance(4), 100);
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));
        // triggers `note_author`
        Authorship::on_initialize(1);

        let collator = CandidateInfo {
            who: 4,
            deposit: 10,
        };

        assert_eq!(Candidates::<Test>::get(), vec![collator]);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 0);

        // half of the pot goes to the collator who's the author (4 in tests).
        assert_eq!(Balances::free_balance(4), 140);
        // half + ED stays.
        assert_eq!(Balances::free_balance(CollatorSelection::account_id()), 55);
    });
}

#[test]
fn fees_edgecases() {
    new_test_ext().execute_with(|| {
        // Nothing panics, no reward when no ED in balance
        Authorship::on_initialize(1);
        // put some money into the pot at ED
        Balances::make_free_balance_be(&CollatorSelection::account_id(), 5);
        // 4 is the default author.
        assert_eq!(Balances::free_balance(4), 100);
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));
        // triggers `note_author`
        Authorship::on_initialize(1);

        let collator = CandidateInfo {
            who: 4,
            deposit: 10,
        };

        assert_eq!(Candidates::<Test>::get(), vec![collator]);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 0);
        // Nothing received
        assert_eq!(Balances::free_balance(4), 90);
        // all fee stays
        assert_eq!(Balances::free_balance(CollatorSelection::account_id()), 5);
    });
}

#[test]
fn session_management_works() {
    new_test_ext().execute_with(|| {
        initialize_to_block(1);

        assert_eq!(SessionChangeBlock::get(), 0);
        assert_eq!(SessionCollators::get(), vec![1, 2]);

        initialize_to_block(4);

        assert_eq!(SessionChangeBlock::get(), 0);
        assert_eq!(SessionCollators::get(), vec![1, 2]);

        // add a new collator
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));

        // session won't see this.
        assert_eq!(SessionCollators::get(), vec![1, 2]);
        // but we have a new candidate.
        assert_eq!(Candidates::<Test>::get().len(), 1);

        initialize_to_block(10);
        assert_eq!(SessionChangeBlock::get(), 10);
        // pallet-session has 1 session delay; current validators are the same.
        assert_eq!(Session::validators(), vec![1, 2]);
        // queued ones are changed, and now we have 3.
        assert_eq!(Session::queued_keys().len(), 3);
        // session handlers (aura, et. al.) cannot see this yet.
        assert_eq!(SessionCollators::get(), vec![1, 2]);

        initialize_to_block(20);
        assert_eq!(SessionChangeBlock::get(), 20);
        // changed are now reflected to session handlers.
        assert_eq!(SessionCollators::get(), vec![1, 2, 3]);
    });
}

#[test]
fn kick_and_slash_mechanism() {
    new_test_ext().execute_with(|| {
        // Define slash destination account
        <crate::SlashDestination<Test>>::put(5);
        // add a new collator
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));
        initialize_to_block(10);
        assert_eq!(Candidates::<Test>::get().len(), 2);
        initialize_to_block(20);
        assert_eq!(SessionChangeBlock::get(), 20);
        // 4 authored this block, gets to stay. 3 was kicked
        assert_eq!(Candidates::<Test>::get().len(), 1);
        // 3 will be kicked after 1 session delay
        assert_eq!(SessionCollators::get(), vec![1, 2, 3, 4]);
        assert_eq!(NextSessionCollators::get(), vec![1, 2, 4]);
        let collator = CandidateInfo {
            who: 4,
            deposit: 10,
        };
        assert_eq!(Candidates::<Test>::get(), vec![collator]);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 20);
        initialize_to_block(30);
        // 3 gets kicked after 1 session delay
        assert_eq!(SessionCollators::get(), vec![1, 2, 4]);
        // kicked collator gets funds back except slashed 10% (of 10 bond)
        assert_eq!(Balances::free_balance(3), 99);
        assert_eq!(Balances::free_balance(5), 101);
    });
}

#[test]
fn slash_mechanism_for_unbonding_candidates_who_missed_block() {
    new_test_ext().execute_with(|| {
        // Define slash destination account
        <crate::SlashDestination<Test>>::put(5);
        // add a new collator
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));
        assert_eq!(LastAuthoredBlock::<Test>::get(3), 10);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 10);

        initialize_to_block(10);
        // gets included into next session, expected to build blocks
        assert_eq!(NextSessionCollators::get(), vec![1, 2, 3, 4]);
        // candidate left but still expected to produce blocks for current session
        assert_ok!(CollatorSelection::leave_intent(RuntimeOrigin::signed(3)));
        assert_eq!(Balances::free_balance(3), 90); // funds un-bonding
        initialize_to_block(19);
        // not there yet
        assert_noop!(
            CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)),
            Error::<Test>::BondStillLocked
        );
        // new session, candidate gets slashed
        initialize_to_block(20);
        assert_eq!(Candidates::<Test>::get().len(), 1);
        assert_eq!(SessionChangeBlock::get(), 20);
        assert_eq!(LastAuthoredBlock::<Test>::contains_key(3), false);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 20);

        // slashed, remaining bond was refunded
        assert_noop!(
            CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)),
            Error::<Test>::NoCandidacyBond
        );

        // slashed collator gets funds back except slashed 10% (of 10 bond)
        assert_eq!(Balances::free_balance(3), 99);
        assert_eq!(Balances::free_balance(5), 101);
    });
}

#[test]
fn should_not_slash_unbonding_candidates() {
    new_test_ext().execute_with(|| {
        // add a new collator
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(4)
        ));
        assert_eq!(LastAuthoredBlock::<Test>::get(3), 10);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 10);

        assert_ok!(CollatorSelection::leave_intent(RuntimeOrigin::signed(3)));
        // can withdraw on next session
        assert_eq!(NonCandidates::<Test>::get(3), Some((1, 10)));

        initialize_to_block(10);
        // not included next session and doesn't withdraw bond
        assert_eq!(NextSessionCollators::get(), vec![1, 2, 4]);
        assert_eq!(LastAuthoredBlock::<Test>::get(3), 10);
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 10);
        assert_eq!(NonCandidates::<Test>::get(3), Some((1, 10)));
        assert_eq!(Balances::free_balance(3), 90);

        initialize_to_block(20);
        assert_eq!(SessionChangeBlock::get(), 20);
        assert!(!LastAuthoredBlock::<Test>::contains_key(3));
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 20);

        assert_eq!(NonCandidates::<Test>::get(3), Some((1, 10)));
        assert_eq!(Balances::free_balance(3), 90);

        assert_ok!(CollatorSelection::withdraw_bond(RuntimeOrigin::signed(3)));
        assert_eq!(NonCandidates::<Test>::get(3), None);
        assert_eq!(Balances::free_balance(3), 100);
    });
}

#[test]
fn should_not_kick_mechanism_too_few() {
    new_test_ext().execute_with(|| {
        // add a new collator
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(3)
        ));
        assert_ok!(CollatorSelection::register_as_candidate(
            RuntimeOrigin::signed(5)
        ));
        initialize_to_block(10);
        assert_eq!(Candidates::<Test>::get().len(), 2);
        initialize_to_block(20);
        assert_eq!(SessionChangeBlock::get(), 20);
        // 4 authored this block, 3 gets to stay too few, 5 was kicked
        assert_eq!(Candidates::<Test>::get().len(), 1);
        // 5 will be kicked for next session
        assert_eq!(NextSessionCollators::get(), vec![1, 2, 3]);
        assert_eq!(
            Candidates::<Test>::get(),
            vec![CandidateInfo {
                who: 3,
                deposit: 10,
            }]
        );
        assert_eq!(LastAuthoredBlock::<Test>::get(4), 20);
        // kicked collator gets funds back (but slashed)
        assert_eq!(Balances::free_balance(5), 99);
        initialize_to_block(30);
        // next session doesn't include 5
        assert_eq!(SessionCollators::get(), vec![1, 2, 3]);
    });
}

#[test]
#[should_panic = "duplicate invulnerables in genesis."]
fn cannot_set_genesis_value_twice() {
    sp_tracing::try_init_simple();
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    let invulnerables = vec![1, 1];

    let collator_selection = collator_selection::GenesisConfig::<Test> {
        desired_candidates: 2,
        candidacy_bond: 10,
        invulnerables,
    };
    // collator selection must be initialized before session.
    collator_selection.assimilate_storage(&mut t).unwrap();
}

#[test]
fn set_slash_destination() {
    new_test_ext().execute_with(|| {
        assert_eq!(SlashDestination::<Test>::get(), None);

        // only UpdateOrigin can update
        assert_noop!(
            CollatorSelection::set_slash_destination(RuntimeOrigin::signed(1), Some(1)),
            sp_runtime::DispatchError::BadOrigin
        );

        // set destination
        assert_ok!(CollatorSelection::set_slash_destination(
            RuntimeOrigin::signed(RootAccount::get()),
            Some(1),
        ));
        assert_eq!(SlashDestination::<Test>::get(), Some(1));

        // remove destination
        assert_ok!(CollatorSelection::set_slash_destination(
            RuntimeOrigin::signed(RootAccount::get()),
            None,
        ));
        assert_eq!(SlashDestination::<Test>::get(), None);
    });
}
