// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT PREFERENCES rule, over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: the calling account's AWS Chatbot preferences
//! are one piece of account-scoped persisted state; `UpdateAccountPreferences` writes ONLY
//! the preferences its request carries, leaves every omitted one as it was and reports the
//! ones it SET — a preference the request did not name is left out of the answer rather
//! than reported as off, so a client cannot mistake "nothing configured this" for
//! "configured off" — while `GetAccountPreferences` reports the account's WHOLE preference
//! set out of that state, unchanged by being read and the same whichever region the
//! request is signed for, an opt-in nothing has turned on reading as off.
//!
//! The preferences are exercised as a CLASS of inputs rather than one recorded request:
//! each table below spans the two members (`smithy_query`
//! operation=UpdateAccountPreferences: `UserAuthorizationRequired` and
//! `TrainingDataCollectionEnabled`, both optional `BooleanAccountPreference`) over every
//! combination of present-true, present-false and absent, and the expected answer comes
//! from a reference model of the rule, never from a recorded response.
//!
//! The request plumbing is the shared `tests/all/wire.rs` harness.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

const GET_PATH: &str = "/get-account-preferences";
const UPDATE_PATH: &str = "/update-account-preferences";

/// The `AccountPreferences` member of a 200 answer from `path`.
async fn preferences_from(
    emulator: &Emulator,
    region: &str,
    path: &str,
    request: &Value,
    case: &str,
) -> Value {
    let answer = emulator.call_in_region(region, path, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {path} {request} in {region}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("AccountPreferences")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {path} in {region}: no AccountPreferences member in {}",
                answer.body
            )
        })
}

/// `GetAccountPreferences` in `region`.
async fn get_in_region(emulator: &Emulator, region: &str, case: &str) -> Value {
    preferences_from(emulator, region, GET_PATH, &json!({}), case).await
}

/// `GetAccountPreferences` in [`REGION`].
async fn get(emulator: &Emulator, case: &str) -> Value {
    get_in_region(emulator, REGION, case).await
}

/// `UpdateAccountPreferences` in [`REGION`], carrying exactly the members `request`
/// names.
async fn update(emulator: &Emulator, request: &Value, case: &str) -> Value {
    preferences_from(emulator, REGION, UPDATE_PATH, request, case).await
}

/// The modelled `AccountPreferences` carrying exactly the members `members` names.
fn preferences(members: [(&str, Option<bool>); 2]) -> Value {
    let mut rendered = serde_json::Map::new();
    for (member, value) in members {
        if let Some(value) = value {
            rendered.insert(member.to_string(), json!(value));
        }
    }
    Value::Object(rendered)
}

/// The reference model of the stored preferences: what the rule says the account holds.
/// `None` is a preference nothing has written yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Preferences {
    user_authorization_required: Option<bool>,
    training_data_collection_enabled: Option<bool>,
}

impl Preferences {
    /// What an account that never wrote its preferences holds: neither of them set.
    const FRESH: Self = Self {
        user_authorization_required: None,
        training_data_collection_enabled: None,
    };

    /// Apply a partial write: a member the request omits keeps its stored value.
    fn applied(self, write: &Write) -> Self {
        Self {
            user_authorization_required: write
                .user_authorization_required
                .or(self.user_authorization_required),
            training_data_collection_enabled: write
                .training_data_collection_enabled
                .or(self.training_data_collection_enabled),
        }
    }

    /// What a READ must answer for these preferences: every modelled preference, an
    /// opt-in nothing has turned on reported off.
    fn read_expected(self) -> Value {
        preferences([
            (
                "UserAuthorizationRequired",
                Some(self.user_authorization_required.unwrap_or(false)),
            ),
            (
                "TrainingDataCollectionEnabled",
                Some(self.training_data_collection_enabled.unwrap_or(false)),
            ),
        ])
    }

    /// What the WRITE `write` must answer once these preferences are stored: the stored
    /// value of each preference the request named, and nothing for one it did not.
    fn write_expected(self, write: &Write) -> Value {
        preferences([
            (
                "UserAuthorizationRequired",
                write
                    .user_authorization_required
                    .and(self.user_authorization_required),
            ),
            (
                "TrainingDataCollectionEnabled",
                write
                    .training_data_collection_enabled
                    .and(self.training_data_collection_enabled),
            ),
        ])
    }
}

/// One `UpdateAccountPreferences` request: each member either carried, with a value, or
/// absent.
#[derive(Debug, Clone, Copy)]
struct Write {
    user_authorization_required: Option<bool>,
    training_data_collection_enabled: Option<bool>,
}

impl Write {
    fn new(
        user_authorization_required: Option<bool>,
        training_data_collection_enabled: Option<bool>,
    ) -> Self {
        Self {
            user_authorization_required,
            training_data_collection_enabled,
        }
    }

    /// The request body carrying exactly the members this write names.
    fn body(&self) -> Value {
        preferences([
            (
                "UserAuthorizationRequired",
                self.user_authorization_required,
            ),
            (
                "TrainingDataCollectionEnabled",
                self.training_data_collection_enabled,
            ),
        ])
    }
}

/// Every combination of the two members over present-true, present-false and absent:
/// the whole input class the partial-write rule generalises over (3 x 3 = 9 requests).
fn every_write() -> Vec<(String, Write)> {
    let choices = [(Some(true), "true"), (Some(false), "false"), (None, "absent")];
    let mut writes = Vec::new();
    for (required, required_label) in choices {
        for (enabled, enabled_label) in choices {
            writes.push((
                format!("UserAuthorizationRequired={required_label}, TrainingDataCollectionEnabled={enabled_label}"),
                Write::new(required, enabled),
            ));
        }
    }
    writes
}

#[tokio::test]
async fn an_account_that_never_wrote_its_preferences_reads_both_of_them_off() {
    let emulator = Emulator::new();
    let case = "a fresh account";

    for region in [REGION, OTHER_REGION] {
        let read = get_in_region(&emulator, region, case).await;
        assert_eq!(
            read,
            Preferences::FRESH.read_expected(),
            "case {case:?}: in {region} expected {}, got {read}",
            Preferences::FRESH.read_expected()
        );
    }
}

#[tokio::test]
async fn a_write_answers_the_preferences_it_named_and_leaves_the_others_out() {
    // The modelled members are optional booleans with no default (`smithy_query`
    // shape=AccountPreferences), so a write's answer carries a member it set and nothing
    // at all for one it did not name — whatever the store already held for that member.
    // The second round writes onto what the first one left, so an "absent" member is one
    // the store HAS a value for and the answer still leaves out.
    for (case, write) in every_write() {
        let emulator = Emulator::new();
        let mut model = Preferences::FRESH;

        for round in 0..2 {
            let case = format!("round {round}: {case} onto {model:?}");
            model = model.applied(&write);
            let expected = model.write_expected(&write);

            let written = update(&emulator, &write.body(), &case).await;
            assert_eq!(
                written, expected,
                "case {case:?}: the write must answer {expected}, got {written}"
            );

            // Everything the write stored is what the read reports, the whole set at once.
            let read = get(&emulator, &case).await;
            assert_eq!(
                read,
                model.read_expected(),
                "case {case:?}: the read must report {}, got {read}",
                model.read_expected()
            );

            // Seed the store for the second round, so the row's "absent" member is one
            // the account already holds a value for.
            if round == 0 {
                let seed = Write::new(Some(true), Some(true));
                update(&emulator, &seed.body(), &case).await;
                model = model.applied(&seed);
            }
        }
    }
}

#[tokio::test]
async fn a_write_leaves_every_preference_it_does_not_name_as_it_was() {
    let emulator = Emulator::new();
    let mut model = Preferences::FRESH;

    // One store walked through every request in the class, twice over, so each write
    // lands on a different stored starting point — including the empty request, which
    // names no preference at all and must therefore change nothing.
    let writes = every_write();
    for (round, (case, write)) in writes.iter().cycle().take(writes.len() * 2).enumerate() {
        let case = format!("round {round}: {case} onto {model:?}");
        model = model.applied(write);

        let written = update(&emulator, &write.body(), &case).await;
        assert_eq!(
            written,
            model.write_expected(write),
            "case {case:?}: the write must answer {}, got {written}",
            model.write_expected(write)
        );

        let read = get(&emulator, &case).await;
        assert_eq!(
            read,
            model.read_expected(),
            "case {case:?}: an omitted preference must keep its stored value, expected {}, \
             got {read}",
            model.read_expected()
        );
    }
}

#[tokio::test]
async fn reading_the_preferences_never_changes_what_a_later_caller_reads() {
    let emulator = Emulator::new();
    let mut model = Preferences::FRESH;

    for (case, write) in every_write() {
        model = model.applied(&write);
        update(&emulator, &write.body(), &case).await;
        let expected = model.read_expected();

        // The read is reporting-only: repeating it, in either region, answers the same
        // state every time.
        for attempt in 0..3 {
            for region in [REGION, OTHER_REGION] {
                let read = get_in_region(&emulator, region, &case).await;
                assert_eq!(
                    read, expected,
                    "case {case:?}: read {attempt} in {region} must answer {expected}, got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_write_in_one_region_is_what_every_region_reads() {
    // The preferences are account-scoped, so a write signed for one region is the state
    // every other region reports — and the regions do not hold separate copies.
    for (case, write) in every_write() {
        let emulator = Emulator::new();
        let model = Preferences::FRESH.applied(&write);

        let answer =
            preferences_from(&emulator, OTHER_REGION, UPDATE_PATH, &write.body(), &case).await;
        assert_eq!(
            answer,
            model.write_expected(&write),
            "case {case:?}: a write in {OTHER_REGION} must answer {}, got {answer}",
            model.write_expected(&write)
        );

        for region in [REGION, OTHER_REGION] {
            let read = get_in_region(&emulator, region, &case).await;
            assert_eq!(
                read,
                model.read_expected(),
                "case {case:?}: {region} must read the account-wide preferences {}, got {read}",
                model.read_expected()
            );
        }
    }
}
