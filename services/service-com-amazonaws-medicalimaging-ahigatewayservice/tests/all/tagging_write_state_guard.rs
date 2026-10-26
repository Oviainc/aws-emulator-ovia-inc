// SPDX-License-Identifier: CC0-1.0

//! What the tagging WRITES serve for a resource that can no longer take part in work.
//!
//! RULE: a write that changes a resource's tag map (`TagResource`, `UntagResource`) is
//! admitted only against a resource that can still take part in work — the data store the
//! ARN names has to have settled into `ACTIVE`, and an image set the ARN names has to be
//! one whose retirement has not landed. A data store with a transition the clock still
//! owes it (`CREATING` on the way to `ACTIVE`, `DELETING` on the way to gone) is refused
//! with HTTP 409 `ConflictException` and this service's own not-active sentence, before
//! any image set inside it is looked up; a retired image set is refused with HTTP 404
//! `ResourceNotFoundException` stating the absent KIND. A refused write persists nothing,
//! so the tag map the reporting route (`ListTagsForResource`) reports is the one that was
//! there before the call — a tombstone still reports what it held, it just takes nothing
//! more.
//!
//! The rows span what the rule generalises over: which status the data store is in
//! (`CREATING`, `ACTIVE`, `DELETING`) crossed with which resource the ARN names (the data
//! store itself, a live image set inside it, a retired one), and each row is driven on
//! BOTH write routes (`POST` TagResource, `DELETE` UntagResource) plus the reporting route
//! that proves nothing changed.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::datastore_status_guard`): real requests through the production front door, with
//! the state struct touched only to SEED an image set (which only a DICOM import mints)
//! and to HOLD a store in the status it has already reached, never to assert.

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, DATASTORE_ACTIVATION_DELAY, DATASTORE_NOT_ACTIVE_MESSAGE,
    IMAGE_SET_DELETION_DELAY, IMAGE_SET_NOT_FOUND_MESSAGE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::datastore_status_guard::hold_transition;
use crate::image_set_copy::{get_image_set, seed_image_set};
use crate::image_set_deletion::delete_image_set;
use crate::resource_tagging::{list_tags, tag, untag};

/// The tag every case installs BEFORE the call under test, so a refused write has
/// something to leave alone and an admitted `UntagResource` has a key to remove.
const BASELINE_KEY: &str = "seeded";
const BASELINE_VALUE: &str = "before";

/// The tag the `TagResource` case tries to write, distinct from the baseline so a map
/// that took it can be told from one that did not.
const PROBE_KEY: &str = "probe";
const PROBE_VALUE: &str = "written";

/// The status a data store is in when the call under test arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `CREATING`: the clock owes the move to `ACTIVE`.
    Creating,
    /// `ACTIVE`: settled, nothing owed.
    Active,
    /// `DELETING`: the clock owes the deletion.
    Deleting,
}

impl Phase {
    /// The `DatastoreStatus` wire value a store in this phase reports
    /// (`smithy_query(query="shape",shape="DatastoreStatus")`).
    fn wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// Which resource the `resourceArn` under test names — the two the ARN form can spell
/// (`smithy_query(query="shape",shape="Arn")`: a data store, optionally with an
/// `/imageset/<id>` suffix), with the image set in either of its reachable states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Datastore,
    LiveImageSet,
    RetiredImageSet,
}

/// What the rule says a write at the resource is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// Admitted: HTTP 200, and the write lands.
    Admitted,
    /// HTTP 409 `ConflictException`, carrying this exact sentence.
    Conflict(&'static str),
    /// HTTP 404 `ResourceNotFoundException`, carrying this exact sentence.
    NotFound(&'static str),
}

/// One row of the write-guard table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    phase: Phase,
    target: Target,
    expectation: Expectation,
}

/// Every row: which status the store is in, crossed with which resource the ARN names.
///
/// A live image set inside a `DELETING` store has no row because no real path reaches it
/// — `DeleteDatastore` refuses a store that still holds one — so the `DELETING` store's
/// image-set row carries the retired image set, which also pins the PRECEDENCE: the
/// store's status answers before the image set is looked up at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a settled store, named by its own ARN",
            phase: Phase::Active,
            target: Target::Datastore,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "an image set a settled store holds",
            phase: Phase::Active,
            target: Target::LiveImageSet,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "an image set whose retirement has landed",
            phase: Phase::Active,
            target: Target::RetiredImageSet,
            expectation: Expectation::NotFound(IMAGE_SET_NOT_FOUND_MESSAGE),
        },
        Row {
            case: "a store still being created, named by its own ARN",
            phase: Phase::Creating,
            target: Target::Datastore,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "an image set a store still being created holds",
            phase: Phase::Creating,
            target: Target::LiveImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "a store whose deletion is in flight, named by its own ARN",
            phase: Phase::Deleting,
            target: Target::Datastore,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "a retired image set of a store whose deletion is in flight",
            phase: Phase::Deleting,
            target: Target::RetiredImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
    ]
}

/// A `ClientToken` for a case: the case name with every character the member's pattern
/// forbids folded to `-` (`ClientToken` is `^[A-Za-z0-9._-]+$`, len 1..64 —
/// `smithy_query(query="shape",shape="ClientToken")`).
fn case_token(case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("guard-{slug}").chars().take(64).collect()
}

/// The baseline tag map, as JSON — what every case starts from.
fn baseline() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(
        BASELINE_KEY.to_string(),
        Value::String(BASELINE_VALUE.to_string()),
    );
    map
}

/// Install the baseline tag on an image set the data store already holds.
///
/// Written through the crate's own state, exactly as the sibling suites seed the image
/// set itself: the store the case is about may be one no tagging write would be admitted
/// against, which is the whole point of the case.
fn seed_image_set_tag(emulator: &Emulator, datastore_id: &str, image_set_id: &str) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store")
        .image_sets
        .get_mut(image_set_id)
        .expect("the seeded image set");
    image_set
        .tags
        .insert(BASELINE_KEY.to_string(), BASELINE_VALUE.to_string());
}

/// Bring the row's resource into existence with the baseline tag on it, and answer the
/// ARN the case addresses it by — the ARN the resource itself reports.
async fn prepared_arn(emulator: &Emulator, row: &Row) -> String {
    let case = row.case;
    let created = emulator
        .create_datastore(json!({
            "datastoreName": "tag.guard.store",
            "clientToken": case_token(case),
            "tags": Value::Object(baseline()),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
    );
    let datastore_id = created.string(&["datastoreId"]);

    match row.phase {
        // Held before anything else happens, so the store cannot settle mid-case.
        Phase::Creating => hold_transition(emulator, case, &datastore_id),
        Phase::Active | Phase::Deleting => {
            tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
            let read = emulator.get_datastore(&datastore_id).await;
            assert_eq!(
                read.string(&["datastoreProperties", "datastoreStatus"]),
                "ACTIVE",
                "case {case:?}: expected the store to settle before the case sets it up, got {read:?}"
            );
        }
    }

    let image_set_id = match row.target {
        Target::Datastore => None,
        Target::LiveImageSet | Target::RetiredImageSet => {
            let image_set_id = seed_image_set(emulator, &datastore_id, 1);
            seed_image_set_tag(emulator, &datastore_id, &image_set_id);
            if row.target == Target::RetiredImageSet {
                let deleted = delete_image_set(emulator, &datastore_id, &image_set_id).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: expected HTTP 200 from DeleteImageSet, got {deleted:?}"
                );
                tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;
                let read = get_image_set(emulator, &datastore_id, &image_set_id, None).await;
                assert_eq!(
                    read.string(&["imageSetState"]),
                    "DELETED",
                    "case {case:?}: expected the retirement to have landed, got {read:?}"
                );
            }
            Some(image_set_id)
        }
    };

    if row.phase == Phase::Deleting {
        let deleted = emulator.delete_datastore(&datastore_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 deleting a settled store, got {deleted:?}"
        );
        hold_transition(emulator, case, &datastore_id);
    }

    let read = emulator.get_datastore(&datastore_id).await;
    assert_eq!(
        read.string(&["datastoreProperties", "datastoreStatus"]),
        row.phase.wire(),
        "case {case:?}: expected the store to report {} before the call under test, got {read:?}",
        row.phase.wire()
    );
    let datastore_arn = read.string(&["datastoreProperties", "datastoreArn"]);
    match image_set_id {
        None => datastore_arn,
        Some(image_set_id) => image_set_arn(&datastore_arn, &image_set_id),
    }
}

/// Assert `answer` is what the row expects, naming the case and the answer.
fn assert_answer(
    case: &str,
    route: &str,
    expectation: Expectation,
    answer: &crate::datastore_lifecycle::Answer,
) {
    match expectation {
        Expectation::Admitted => assert_eq!(
            answer.status, 200,
            "case {case:?} via {route}: expected HTTP 200, got {answer:?}"
        ),
        Expectation::Conflict(message) => {
            assert_eq!(
                answer.status, 409,
                "case {case:?} via {route}: expected HTTP 409, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ConflictException",
                "case {case:?} via {route}: expected the modelled conflict code, got {answer:?}"
            );
            assert_eq!(
                answer.string(&["message"]),
                message,
                "case {case:?} via {route}: expected the service's not-active sentence, got {answer:?}"
            );
        }
        Expectation::NotFound(message) => {
            assert_eq!(
                answer.status, 404,
                "case {case:?} via {route}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?} via {route}: expected the modelled not-found code, got {answer:?}"
            );
            assert_eq!(
                answer.string(&["message"]),
                message,
                "case {case:?} via {route}: expected the absent resource KIND, got {answer:?}"
            );
        }
    }
}

/// RULE (the whole table): a tagging write reaches only a resource that can still take
/// part in work — a data store that has settled into `ACTIVE`, and an image set that has
/// not been retired — and a write it cannot reach persists nothing, whichever write route
/// carried it.
#[tokio::test]
async fn a_tagging_write_reaches_only_a_resource_that_can_still_take_part_in_work() {
    for row in rows() {
        // One emulator per row: each case is about a data store in a status of its own.
        let emulator = Emulator::new();
        let case = row.case;
        let arn = prepared_arn(&emulator, &row).await;
        let mut expected = baseline();

        // The tag map before either write, so what the case compares against is what the
        // reporting route itself reports.
        let before = list_tags(&emulator, &arn).await;
        assert_eq!(
            before.field(&["tags"]),
            &Value::Object(expected.clone()),
            "case {case:?}: expected the baseline tag map before the writes, got {before:?}"
        );

        // TagResource: a key the resource does not carry.
        let tagged = tag(&emulator, &arn, json!({ PROBE_KEY: PROBE_VALUE })).await;
        assert_answer(case, "TagResource", row.expectation, &tagged);
        if row.expectation == Expectation::Admitted {
            expected.insert(
                PROBE_KEY.to_string(),
                Value::String(PROBE_VALUE.to_string()),
            );
        }
        let after_tag = list_tags(&emulator, &arn).await;
        assert_eq!(
            after_tag.field(&["tags"]),
            &Value::Object(expected.clone()),
            "case {case:?}: expected the tag map the admitted writes left, got {after_tag:?}"
        );

        // UntagResource: the key the resource came in carrying.
        let untagged = untag(&emulator, &arn, &[BASELINE_KEY]).await;
        assert_answer(case, "UntagResource", row.expectation, &untagged);
        if row.expectation == Expectation::Admitted {
            expected.remove(BASELINE_KEY);
        }
        let after_untag = list_tags(&emulator, &arn).await;
        assert_eq!(
            after_untag.field(&["tags"]),
            &Value::Object(expected.clone()),
            "case {case:?}: expected the tag map the admitted writes left, got {after_untag:?}"
        );
    }
}
