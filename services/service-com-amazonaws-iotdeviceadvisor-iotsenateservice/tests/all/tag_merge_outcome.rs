// SPDX-License-Identifier: CC0-1.0

//! What a `TagResource` merge WRITES, and the record it leaves, over the wire.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket (the shared harness in [`super::suite_lifecycle`]) and reads the
//! result back through `ListTagsForResource`, so what is asserted is the service's wire
//! behaviour and not a store internal.
//!
//! The rule under test: a non-refused mutating call's audit outcome states what the call
//! actually CHANGED, so a merge reports how many of the keys it named it wrote — created or
//! gave a new value — and one that wrote nothing is recorded as the no-op it was, with the
//! stored tags left exactly as they were.
//!
//! The table spans what that rule generalises over: both taggable resource kinds (suite
//! definition, suite run) crossed with merges differing in how many keys they name (none,
//! one, several, the modelled maximum), in how many of those keys the resource already
//! carries with the value asked for (none, some, all), in key and value length and script,
//! and in whether the resource was tagged at all beforehand. The refusals a merge can
//! answer — a key past the per-resource limit, an unknown resource, a value that is not one
//! of this service's ARNs — are rows of `tests/all/audit_trail.rs`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::audit_trail::{
    assert_record, audit_capture, audit_serial, audited_call, tags_uri, REGION,
};
use super::suite_lifecycle::{thing_arn, Emulator, ROLE_ARN};

/// The status a successful tag mutation answers with: 204 No Content, as AWS answers an
/// empty-output tag mutation — written out here rather than read from the service, so a
/// regression to the protocol default fails these rows.
const TAG_MUTATION_STATUS: u16 = 204;

/// `TagMap` is `len 0..50` (source: `smithy_query(query="shape",shape="TagMap")`), so this
/// is how many tags one resource holds at once. The key past it is refused, which
/// `tests/all/audit_trail.rs` pins.
const MAX_TAGS: usize = 50;

/// `TagMap`'s key target is `String128` (`len 1..128`) and its value target `String256`
/// (`len 1..256`) — the bounds the longest-key/longest-value rows below sit on, with the
/// one-character rows on the lower ones (source:
/// `smithy_query(query="shape",shape="TagMap,String128,String256")`).
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// The two resource kinds an IoT Device Advisor `resourceArn` can name: every tagging
/// operation documents the same thing about it — "This can be SuiteDefinition ARN or
/// SuiteRun ARN".
#[derive(Debug, Clone, Copy)]
enum Kind {
    SuiteDefinition,
    SuiteRun,
}

const KINDS: [Kind; 2] = [Kind::SuiteDefinition, Kind::SuiteRun];

/// A legal suite definition configuration.
fn configuration() -> Value {
    json!({
        "suiteDefinitionName": "tag-merge-subject",
        "rootGroup": "root-group",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": [{ "thingArn": thing_arn(1) }],
    })
}

/// A tag map carrying exactly `key` -> `value`, for a key a literal cannot hold.
fn one_tag(key: &str, value: &str) -> Value {
    Value::Object([(key.to_string(), json!(value))].into_iter().collect())
}

/// A tag map of `count` distinct keys.
fn tag_map(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// The tag object `value` holds, as a map.
fn object(value: &Value) -> Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("a tag map is a JSON object, got {value}"))
        .clone()
}

/// `initial` with every key of `merge` set to `merge`'s value — what the resource must
/// carry after the merge.
fn overlaid(initial: &Value, merge: &Value) -> Value {
    let mut map = object(initial);
    for (key, value) in object(merge) {
        map.insert(key, value);
    }
    Value::Object(map)
}

/// `tags` as the store holds a row, for comparing a row against the tags a call asked for.
fn as_row(tags: &Value) -> BTreeMap<String, String> {
    object(tags)
        .into_iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("a tag value is a string, got {value}"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// Create a resource of `kind` carrying exactly `tags` and answer its ARN.
///
/// The tags go on the resource under test only: a run's definition is created untagged, so
/// a row about a run cannot pass by reading its definition's tags. These setup calls
/// present the harness's default access key id, so the records they write are not among
/// the ones the rows read back.
async fn tagged_resource(emulator: &Emulator, case: &str, kind: Kind, tags: &Value) -> String {
    let mut body = json!({ "suiteDefinitionConfiguration": configuration() });
    if matches!(kind, Kind::SuiteDefinition) && !object(tags).is_empty() {
        body["tags"] = tags.clone();
    }
    let created = emulator
        .call("POST", "/suiteDefinitions", REGION, Some(body))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
        created.status, created.body
    );

    match kind {
        Kind::SuiteDefinition => created.string("suiteDefinitionArn"),
        Kind::SuiteRun => {
            let definition_id = created.string("suiteDefinitionId");
            let mut body = json!({
                "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
            });
            if !object(tags).is_empty() {
                body["tags"] = tags.clone();
            }
            let started = emulator
                .call(
                    "POST",
                    &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                    REGION,
                    Some(body),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
                started.status, started.body
            );
            started.string("suiteRunArn")
        }
    }
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn reported_tags(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let listed = emulator.call("GET", &tags_uri(arn), REGION, None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} {}",
        listed.status, listed.body
    );
    listed.body["tags"].clone()
}

/// The tag row the STORE holds for `arn`; `None` when it holds none at all.
fn stored_row(emulator: &Emulator, arn: &str) -> Option<BTreeMap<String, String>> {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let store = state.lock();
    store.tags.get(arn).cloned()
}

/// One row: `(case, the tags the resource is created with, the tags the merge names, how
/// many keys the merge must WRITE)`.
///
/// The written count is written out per row rather than computed, so a row states the rule
/// independently of the code under test.
fn merge_rows() -> Vec<(String, Value, Value, usize)> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let full = tag_map(MAX_TAGS);

    vec![
        (
            // `TagMap` is `len 0..50`, so a merge naming no key at all is a legal request.
            "no key at all, onto an untagged resource".to_string(),
            json!({}),
            json!({}),
            0,
        ),
        (
            "no key at all, onto a resource carrying tags".to_string(),
            json!({ "env": "prod", "team": "iot" }),
            json!({}),
            0,
        ),
        (
            "one new key onto an untagged resource".to_string(),
            json!({}),
            json!({ "owner": "qa" }),
            1,
        ),
        (
            "three new keys beside one the resource carries".to_string(),
            json!({ "env": "prod" }),
            json!({ "team": "iot", "owner": "qa", "tier": "gold" }),
            3,
        ),
        (
            "one key re-set to the value it already carries".to_string(),
            json!({ "env": "prod" }),
            json!({ "env": "prod" }),
            0,
        ),
        (
            "every named key already carries the value asked for".to_string(),
            json!({ "env": "prod", "team": "iot", "owner": "qa" }),
            json!({ "env": "prod", "team": "iot" }),
            0,
        ),
        (
            "one key given a new value".to_string(),
            json!({ "env": "prod", "team": "iot" }),
            json!({ "env": "staging" }),
            1,
        ),
        (
            "one key unchanged, one overwritten, one new".to_string(),
            json!({ "env": "prod", "team": "iot" }),
            json!({ "env": "prod", "team": "lab", "owner": "qa" }),
            2,
        ),
        (
            // `String128` and `String256` at their lower bound.
            "shortest legal key and value".to_string(),
            json!({}),
            json!({ "k": "v" }),
            1,
        ),
        (
            // Their upper bounds.
            "longest legal key and value, newly set".to_string(),
            json!({}),
            one_tag(&long_key, &long_value),
            1,
        ),
        (
            "longest legal key re-set to the value it carries".to_string(),
            one_tag(&long_key, &long_value),
            one_tag(&long_key, &long_value),
            0,
        ),
        (
            "a non-ASCII key given a new value".to_string(),
            json!({ "проект": "устройство", "plain": "value" }),
            json!({ "проект": "сервер" }),
            1,
        ),
        (
            "a key carrying URI-significant characters, unchanged".to_string(),
            json!({ "a&b=c d": "v" }),
            json!({ "a&b=c d": "v" }),
            0,
        ),
        (
            format!("the modelled maximum of {MAX_TAGS} keys, all new"),
            json!({}),
            full.clone(),
            MAX_TAGS,
        ),
        (
            format!("the modelled maximum of {MAX_TAGS} keys, re-sent unchanged"),
            full.clone(),
            full,
            0,
        ),
    ]
}

/// The rule: the record a merge leaves states how many of the keys it named it wrote, and a
/// merge that wrote nothing leaves the stored tags exactly as they were.
///
/// The capture the record is read out of is one process-wide buffer, so this suite holds
/// the audit serial lock for its duration.
#[tokio::test]
async fn a_tag_merge_records_the_keys_it_wrote_and_a_no_op_changes_nothing() {
    let _serial = audit_serial().await;
    let captured = audit_capture();
    let emulator = Emulator::new();

    for kind in KINDS {
        for (row, initial, merge, written) in merge_rows() {
            let case = format!("{kind:?}: {row}");
            let arn = tagged_resource(&emulator, &case, kind, &initial).await;
            let before = stored_row(&emulator, &arn);
            let named = object(&merge).len();

            let (answer, record) = audited_call(
                &emulator,
                captured,
                &case,
                "POST",
                &tags_uri(&arn),
                Some(json!({ "tags": merge.clone() })),
            )
            .await;
            assert_eq!(
                answer.status, TAG_MUTATION_STATUS,
                "case {case:?}: expected HTTP {TAG_MUTATION_STATUS} from TagResource, got {} {} \
                 ({:?})",
                answer.status, answer.body, answer.error_code
            );
            assert!(
                answer.error_code.is_none(),
                "case {case:?}: a successful TagResource carries no error code, got {:?}",
                answer.error_code
            );
            assert_record(
                &case,
                &record,
                "TagResource",
                &arn,
                &format!("tagged {written} of {named} key(s)"),
            );

            let expected = overlaid(&initial, &merge);
            let reported = reported_tags(&emulator, &case, &arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the merged tags {expected}, got {reported}"
            );

            let after = stored_row(&emulator, &arn);
            // A resource with no tags keeps NO row at all, so "merged down to nothing" is
            // indistinguishable from "never tagged".
            let expected_row = if object(&expected).is_empty() {
                None
            } else {
                Some(as_row(&expected))
            };
            assert_eq!(
                after, expected_row,
                "case {case:?}: expected the store to hold {expected_row:?}, got {after:?}"
            );
            if written == 0 {
                assert_eq!(
                    after, before,
                    "case {case:?}: a merge that wrote nothing must leave the stored tags as \
                     they were ({before:?}), got {after:?}"
                );
            }
        }
    }
}

/// Repeating a merge: the call that wrote the keys records them, and every repetition after
/// it records the no-op — the answer and the tags being the same each time.
#[tokio::test]
async fn repeating_a_merge_records_the_write_once_and_the_no_op_after() {
    let _serial = audit_serial().await;
    let captured = audit_capture();
    let emulator = Emulator::new();

    for kind in KINDS {
        for (row, initial, merge, written) in [
            (
                "one new key",
                json!({}),
                json!({ "owner": "qa" }),
                1usize,
            ),
            (
                "two keys, one of them already carried",
                json!({ "env": "prod" }),
                json!({ "env": "prod", "team": "iot" }),
                1,
            ),
            (
                "a key given a new value",
                json!({ "env": "prod" }),
                json!({ "env": "staging" }),
                1,
            ),
        ] {
            let case = format!("{kind:?}: {row}");
            let arn = tagged_resource(&emulator, &case, kind, &initial).await;
            let named = object(&merge).len();
            let expected = overlaid(&initial, &merge);

            for attempt in 1..=3 {
                let (answer, record) = audited_call(
                    &emulator,
                    captured,
                    &case,
                    "POST",
                    &tags_uri(&arn),
                    Some(json!({ "tags": merge.clone() })),
                )
                .await;
                assert_eq!(
                    answer.status, TAG_MUTATION_STATUS,
                    "case {case:?} attempt {attempt}: expected HTTP {TAG_MUTATION_STATUS}, got \
                     {} {}",
                    answer.status, answer.body
                );
                // Only the first call has anything to write; the resource already carries
                // what the later ones ask for.
                let expected_written = if attempt == 1 { written } else { 0 };
                assert_record(
                    &format!("{case} attempt {attempt}"),
                    &record,
                    "TagResource",
                    &arn,
                    &format!("tagged {expected_written} of {named} key(s)"),
                );

                let reported = reported_tags(&emulator, &case, &arn).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?} attempt {attempt}: expected the merged tags {expected}, got \
                     {reported}"
                );
            }
        }
    }
}
