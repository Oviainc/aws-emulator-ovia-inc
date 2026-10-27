// SPDX-License-Identifier: CC0-1.0

//! What the tagging WRITES serve across the statuses a resource passes through.
//!
//! RULE: a tag map is metadata ON the resource record, so a write that changes it
//! (`TagResource`, `UntagResource`) answers to the resource EXISTING and to nothing else:
//! it is admitted whatever status change the clock still owes the data store the ARN
//! names — one that has not finished being created, one whose deletion is in flight, one
//! that has settled — and is refused only when the emulator holds no such resource, with
//! HTTP 404 `ResourceNotFoundException` stating the absent KIND (a retired image set
//! among them, because a tombstone takes part in no further work). None of the three
//! tagging operations declares a conflict error at all
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//! lists `AccessDeniedException`, `InternalServerException`, `ResourceNotFoundException`,
//! `ThrottlingException` and `ValidationException` for each), so no status of the resource
//! can turn one of these calls into a 409.
//!
//! The rows span what the rule generalises over: which status the data store is in
//! (`CREATING`, `ACTIVE`, `DELETING`) crossed with which resource the ARN names (the data
//! store itself, a live image set inside it, a retired one), and each row is driven on
//! BOTH write routes (`POST` TagResource, `DELETE` UntagResource) plus the reporting route
//! that proves what landed. The fourth status, the tombstone of a finished deletion, is
//! the data store the emulator no longer holds, and the tagging routes are driven against
//! it by `crate::datastore_deleted_window`.
//!
//! The second suite drives the rule through the REAL clock and no state reach-in at all:
//! the provisioning sequence a client runs (create, read the ARN back, tag, list, untag)
//! inside the activation span, and then again once the store has settled.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::datastore_status_guard`): real requests through the production front door, with
//! the state struct touched only to SEED an image set (which only a DICOM import mints)
//! and to HOLD a store in the status it has already reached, never to assert.

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, DATASTORE_ACTIVATION_DELAY, IMAGE_SET_DELETION_DELAY,
    IMAGE_SET_NOT_FOUND_MESSAGE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::datastore_status_guard::hold_transition;
use crate::image_set_copy::{get_image_set, seed_image_set};
use crate::image_set_deletion::delete_image_set;
use crate::resource_tagging::{list_tags, tag, tag_object, untag};

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
    /// HTTP 404 `ResourceNotFoundException`, carrying this exact sentence.
    NotFound(&'static str),
}

/// One row of the write table.
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
/// — `DeleteDatastore` refuses a store that still holds one — and a retired image set
/// inside a `CREATING` store has none either, because retiring one takes a `DeleteImageSet`
/// that store does not serve. Both retired-image-set rows pin the same refusal, under a
/// store that has settled and under one whose deletion is in flight, so the refusal is
/// the IMAGE SET's and not the store's status.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a store still being created, named by its own ARN",
            phase: Phase::Creating,
            target: Target::Datastore,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "an image set a store still being created holds",
            phase: Phase::Creating,
            target: Target::LiveImageSet,
            expectation: Expectation::Admitted,
        },
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
            case: "a store whose deletion is in flight, named by its own ARN",
            phase: Phase::Deleting,
            target: Target::Datastore,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "a retired image set of a store whose deletion is in flight",
            phase: Phase::Deleting,
            target: Target::RetiredImageSet,
            expectation: Expectation::NotFound(IMAGE_SET_NOT_FOUND_MESSAGE),
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
/// set itself: an image set is minted by a DICOM import alone, so there is no request a
/// case could carry the baseline in on.
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
///
/// A 409 is never an expectation here, so the assertions below also pin that no tagging
/// route answers with one: a conflict would fail the status assertion of every row.
fn assert_answer(
    case: &str,
    route: &str,
    expectation: Expectation,
    answer: &crate::datastore_lifecycle::Answer,
) {
    match expectation {
        Expectation::Admitted => {
            assert_eq!(
                answer.status, 200,
                "case {case:?} via {route}: expected HTTP 200, got {answer:?}"
            );
            assert!(
                answer.error_code.is_empty(),
                "case {case:?} via {route}: expected no error code on a success, got {answer:?}"
            );
            // An empty body: both write operations model an output with no members.
            assert!(
                matches!(&answer.body, Value::Null) || answer.body == json!({}),
                "case {case:?} via {route}: expected an empty body, got {answer:?}"
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

/// RULE (the whole table): a tagging write answers to the resource EXISTING and not to
/// the status it has reached — every resource record the emulator still holds takes the
/// write, whatever transition the clock owes its data store, and only a resource the
/// emulator holds none of refuses it, with the modelled 404; a write that was refused
/// persists nothing, whichever write route carried it.
#[tokio::test]
async fn a_tagging_write_answers_to_the_resource_existing_and_not_to_its_status() {
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

/// One row of the provisioning table: the tags a client writes straight after creating
/// the data store, and the key it then removes.
#[derive(Debug)]
struct ProvisionCase {
    case: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    remove: &'static str,
}

/// The tags, as the JSON object `TagResource` carries — built with the sibling suite's
/// own map builder, so there is one such builder in this crate.
fn tags_of(pairs: &[(&str, &str)]) -> Value {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    tag_object(&owned)
}

/// RULE (above), through the REAL clock and with no reach into the state struct: the
/// provisioning sequence a client runs — create the data store, read its ARN back, tag it,
/// list the tags, remove one — is served INSIDE the activation span the create left owed,
/// and the tags it wrote are still the resource's own once the store has settled.
///
/// Rows span how many tags of which forms one write carries (`TagMap`: a `TagKey` is
/// 1..128 characters, a `TagValue` 0..256 —
/// `smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`) and which of them is
/// removed, so no row depends on one particular pair.
#[tokio::test]
async fn tagging_is_served_while_the_data_store_is_still_being_created() {
    let cases = [
        ProvisionCase {
            case: "one tag, the only key removed",
            tags: vec![("env", "sim2real")],
            remove: "env",
        },
        ProvisionCase {
            case: "two tags, the first removed",
            tags: vec![("env", "sim2real"), ("owner", "behavioral")],
            remove: "env",
        },
        ProvisionCase {
            case: "two tags, the second removed",
            tags: vec![("env", "sim2real"), ("owner", "behavioral")],
            remove: "owner",
        },
        ProvisionCase {
            case: "three tags, one with an empty value, a key never written removed",
            tags: vec![("env", ""), ("owner", "behavioral"), ("cost.centre", "rad:1")],
            remove: "nobody-set-this",
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();

        let created = emulator
            .create_datastore(json!({
                "datastoreName": "tag.provision.store",
                "clientToken": format!("provision-{index}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
        );
        assert_eq!(
            created.string(&["datastoreStatus"]),
            "CREATING",
            "case {case:?}: expected the create to leave the store CREATING, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);

        // The ARN the client tags by, read back exactly as the sequence under test does.
        let read = emulator.get_datastore(&datastore_id).await;
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreStatus"]),
            "CREATING",
            "case {case:?}: expected the store to still be CREATING when it is tagged, got {read:?}"
        );
        let arn = read.string(&["datastoreProperties", "datastoreArn"]);

        let written = tag(&emulator, &arn, tags_of(&row.tags)).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected HTTP 200 tagging a store that is still CREATING, got {written:?}"
        );
        assert!(
            written.error_code.is_empty(),
            "case {case:?}: expected no error code tagging a CREATING store, got {written:?}"
        );

        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &tags_of(&row.tags),
            "case {case:?}: expected exactly the tags the write carried, got {listed:?}"
        );

        let removed = untag(&emulator, &arn, &[row.remove]).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected HTTP 200 untagging a store that is still CREATING, got {removed:?}"
        );
        let remaining: Vec<(&str, &str)> = row
            .tags
            .iter()
            .copied()
            .filter(|(key, _)| *key != row.remove)
            .collect();
        let after = list_tags(&emulator, &arn).await;
        assert_eq!(
            after.field(&["tags"]),
            &tags_of(&remaining),
            "case {case:?}: expected the removal to take exactly the named key, got {after:?}"
        );

        // The transition the create promised still lands, and the tags written while it
        // was in flight are the settled store's own.
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
        let settled = emulator.get_datastore(&datastore_id).await;
        assert_eq!(
            settled.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case {case:?}: expected the creation to complete after the tagging, got {settled:?}"
        );
        let settled_tags = list_tags(&emulator, &arn).await;
        assert_eq!(
            settled_tags.field(&["tags"]),
            &tags_of(&remaining),
            "case {case:?}: expected the settled store to keep the tags, got {settled_tags:?}"
        );
    }
}
