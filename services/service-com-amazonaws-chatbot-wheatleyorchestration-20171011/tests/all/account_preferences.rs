// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT PREFERENCES rule, over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: the calling account's AWS Chatbot preferences
//! are one piece of account-scoped persisted state; `UpdateAccountPreferences` writes
//! ONLY the preferences its request carries and leaves every omitted one as it was, and
//! `GetAccountPreferences` reports that stored state — unchanged by being read, and the
//! same whichever region the request is signed for — starting from both preferences off
//! for an account that has never written them.
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

/// The reference model of the stored preferences: what the rule says the account holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Preferences {
    user_authorization_required: bool,
    training_data_collection_enabled: bool,
}

impl Preferences {
    /// What an account that never wrote its preferences holds: both opt-ins off.
    const FRESH: Self = Self {
        user_authorization_required: false,
        training_data_collection_enabled: false,
    };

    /// Apply a partial write: a member the request omits keeps its stored value.
    fn applied(self, write: &Write) -> Self {
        Self {
            user_authorization_required: write
                .user_authorization_required
                .unwrap_or(self.user_authorization_required),
            training_data_collection_enabled: write
                .training_data_collection_enabled
                .unwrap_or(self.training_data_collection_enabled),
        }
    }

    /// The modelled `AccountPreferences` these preferences must render as.
    fn expected(self) -> Value {
        json!({
            "UserAuthorizationRequired": self.user_authorization_required,
            "TrainingDataCollectionEnabled": self.training_data_collection_enabled,
        })
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
        let mut request = serde_json::Map::new();
        if let Some(required) = self.user_authorization_required {
            request.insert("UserAuthorizationRequired".to_string(), json!(required));
        }
        if let Some(enabled) = self.training_data_collection_enabled {
            request.insert(
                "TrainingDataCollectionEnabled".to_string(),
                json!(enabled),
            );
        }
        Value::Object(request)
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
            Preferences::FRESH.expected(),
            "case {case:?}: in {region} expected {}, got {read}",
            Preferences::FRESH.expected()
        );
    }
}

#[tokio::test]
async fn a_write_reports_and_persists_exactly_the_preferences_it_names() {
    // Each row starts from its own fresh store, so what the write leaves behind is not
    // confused with what an earlier write in the same store left.
    for (case, write) in every_write() {
        let emulator = Emulator::new();
        let expected = Preferences::FRESH.applied(&write).expected();

        let written = update(&emulator, &write.body(), &case).await;
        assert_eq!(
            written, expected,
            "case {case:?}: the write must answer {expected}, got {written}"
        );

        let read = get(&emulator, &case).await;
        assert_eq!(
            read, expected,
            "case {case:?}: the read must report the written preferences {expected}, got {read}"
        );
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
        let expected = model.expected();

        let written = update(&emulator, &write.body(), &case).await;
        assert_eq!(
            written, expected,
            "case {case:?}: the write must answer {expected}, got {written}"
        );

        let read = get(&emulator, &case).await;
        assert_eq!(
            read, expected,
            "case {case:?}: an omitted preference must keep its stored value, expected \
             {expected}, got {read}"
        );
    }
}

#[tokio::test]
async fn reading_the_preferences_never_changes_what_a_later_caller_reads() {
    let emulator = Emulator::new();

    for (case, write) in every_write() {
        let expected = update(&emulator, &write.body(), &case).await;

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
        let expected = Preferences::FRESH.applied(&write).expected();

        let answer =
            preferences_from(&emulator, OTHER_REGION, UPDATE_PATH, &write.body(), &case).await;
        assert_eq!(
            answer, expected,
            "case {case:?}: a write in {OTHER_REGION} must answer {expected}, got {answer}"
        );

        for region in [REGION, OTHER_REGION] {
            let read = get_in_region(&emulator, region, &case).await;
            assert_eq!(
                read, expected,
                "case {case:?}: {region} must read the account-wide preferences {expected}, \
                 got {read}"
            );
        }
    }
}
