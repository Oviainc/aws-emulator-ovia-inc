// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` against the tag set a resource's OWN create call
//! stored, over EVERY resource form the modelled `AmazonResourceName` admits.
//!
//! The rule under test: `ListTagsForResource` answers with exactly the tag set
//! the service holds for the resource its `ResourceARN` names — a FHIR data
//! store (`datastore/fhir/.{32}`) or a data transformation profile
//! (`dataTransformationProfile/[a-f0-9]{32}`), the two forms the modelled
//! pattern admits — as a `TagList`, whichever call wrote those tags, and,
//! being a report, leaves that set unchanged. A live resource of either form is
//! therefore never answered `ResourceNotFoundException`.
//!
//! `tests/all/resource_tagging.rs` drives the same surface through
//! `TagResource`; this suite pins the READ against what each resource's create
//! stored, across the tag sets the two create inputs admit
//! (`CreateFHIRDatastore$Tags`, a `TagList` of len 0..200 held to the
//! per-resource cap, and `CreateDataTransformationProfile$Tags`, a `TagMap` of
//! len 0..50).
//!
//! Every call below goes through [`crate::fhir_datastore_lifecycle::HealthLake`],
//! i.e. `MultiServiceRouter::dispatch` — the same entry point `core/router`
//! serves — so routing, deserialization, the modelled constraints and the error
//! envelope are all exercised, not bypassed.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, string_at, HealthLake};
use crate::resource_tagging::{create_datastore, create_profile, profile_arn};

/// One resource under test: the label it reports itself under, the ARN that
/// addresses it, and the tag set its create stored.
struct Reported {
    case: String,
    arn: String,
    expected: BTreeMap<String, String>,
}

/// A tag set from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The create-time tag sets the table spans, each with the label it reports
/// itself under. `None` is the `Tags` member left out of the request
/// altogether, which is a different request from an empty one.
///
/// The rule generalises over the tags a resource holds, so the rows differ
/// along exactly that: whether the member was sent at all, how many tags,
/// which characters the key and value hold, their lengths, and the count at the
/// published maximum. The alphabet and the lengths come from the modelled `Tag`
/// members (`TagKey`: len 1..128, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`;
/// `TagValue`: len 0..256, same pattern), and the count from
/// `CreateDataTransformationProfile$Tags` (`TagMap`, len 0..50).
fn create_time_tag_sets() -> Vec<(&'static str, Option<BTreeMap<String, String>>)> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        ("no Tags member at all", None),
        ("an explicitly empty tag set", Some(BTreeMap::new())),
        ("one tag", Some(tags(&[("env", "test")]))),
        (
            "keys and values across the modelled TagKey/TagValue alphabet, one value empty",
            Some(tags(&[
                ("lab:unit/x=1+y-2@z", "ward 3"),
                ("клиника", "ünïcode"),
                ("empty.value", ""),
            ])),
        ),
        (
            "a key at the modelled 128-character maximum with a value at the 256-character maximum",
            Some(tags(&[(longest_key.as_str(), longest_value.as_str())])),
        ),
        (
            "as many tags as the modelled TagMap admits",
            Some(
                (0..50)
                    .map(|index| (format!("key-{index:03}"), format!("{index}")))
                    .collect(),
            ),
        ),
    ]
}

/// A create-time tag set rendered for each create's own modelled shape: the
/// `TagList` `CreateFHIRDatastore$Tags` takes and the `TagMap`
/// `CreateDataTransformationProfile$Tags` takes.
fn as_tag_list(tags: &BTreeMap<String, String>) -> Value {
    let pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    crate::resource_tagging::tag_list(&pairs)
}

fn as_tag_map(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The `Tags` of a `ListTagsForResource` response as a comparable map, holding
/// the payload to the modelled `TagList` shape: a list of `Tag` structures,
/// each with exactly the required `Key` and `Value` string members, and no two
/// entries sharing a key.
///
/// `pub(crate)` so the sibling tagging suite reads a reported tag set through
/// the same parse, with the same shape assertions, rather than its own.
pub(crate) fn reported_tags(case: &str, payload: &Value) -> BTreeMap<String, String> {
    let entries = payload["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a Tags list: {payload}"));
    for entry in entries {
        // A set, so this asserts WHICH members a `Tag` carries without
        // depending on the order the serializer emitted them in.
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: every Tags entry is an object: {payload}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BTreeSet::from(["Key", "Value"]),
            "case {case:?}: a Tag carries exactly Key and Value, got {members:?}"
        );
    }
    let reported: BTreeMap<String, String> = entries
        .iter()
        .map(|entry| (string_at(entry, "Key"), string_at(entry, "Value")))
        .collect();
    assert_eq!(
        reported.len(),
        entries.len(),
        "case {case:?}: no two Tags entries may share a key, got {payload}"
    );
    reported
}

/// `ListTagsForResource` on `arn`, which must answer 200 for a live resource.
async fn list_tags(service: &HealthLake, case: &str, arn: &str) -> Value {
    let (status, payload) = service
        .call("ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: a live resource must be reported, got {status} {}: {payload}",
        error_code(&payload)
    );
    payload
}

#[tokio::test]
async fn each_resource_form_reports_the_tag_set_its_create_stored() {
    let service = HealthLake::new();

    // One data store and one profile per row, so BOTH resource forms are driven
    // with the same tag set and neither form's answer can come from the other's
    // record.
    let mut resources: Vec<Reported> = Vec::new();
    for (index, (case, tags)) in create_time_tag_sets().into_iter().enumerate() {
        let datastore_arn = create_datastore(
            &service,
            &format!("store-{index}"),
            tags.as_ref().map(as_tag_list).as_ref(),
        )
        .await;
        let profile_id = create_profile(
            &service,
            &format!("profile-{index}"),
            tags.as_ref().map(as_tag_map).as_ref(),
        )
        .await;
        let expected = tags.unwrap_or_default();
        resources.push(Reported {
            case: format!("a FHIR data store ARN / {case}"),
            arn: datastore_arn.clone(),
            expected: expected.clone(),
        });
        resources.push(Reported {
            case: format!("a data transformation profile ARN / {case}"),
            // The profile ARN is built from a minted data store ARN, so no test
            // names a partition, region or account of its own.
            arn: profile_arn(&datastore_arn, &profile_id),
            expected,
        });
    }

    for resource in &resources {
        let payload = list_tags(&service, &resource.case, &resource.arn).await;
        let reported = reported_tags(&resource.case, &payload);
        assert_eq!(
            reported, resource.expected,
            "case {:?}: the create's own tag set must be reported, got {reported:?}",
            resource.case
        );
        // A report changes nothing a later caller sees: the very same call
        // answers identically, entry for entry.
        let again = list_tags(&service, &resource.case, &resource.arn).await;
        assert_eq!(
            again, payload,
            "case {:?}: a second report must answer identically, got {again}",
            resource.case
        );
    }

    // After every read above, each resource still reports exactly its own set —
    // so the ARN is resolved to the resource it names rather than to whichever
    // record was touched last.
    for resource in &resources {
        let payload = list_tags(&service, &resource.case, &resource.arn).await;
        let reported = reported_tags(&resource.case, &payload);
        assert_eq!(
            reported, resource.expected,
            "case {:?}: the tag set must survive every report, got {reported:?}",
            resource.case
        );
    }
}

#[tokio::test]
async fn a_profile_arn_reports_the_tags_of_the_profile_its_id_names() {
    let service = HealthLake::new();
    let datastore_arn = create_datastore(
        &service,
        "store",
        Some(&as_tag_list(&tags(&[("on", "datastore")]))),
    )
    .await;

    // Several profiles in one store, each created with its own tag set: the id
    // segment of the ARN is what decides which set is reported.
    let rows = vec![
        (
            "the first profile",
            tags(&[("on", "first"), ("env", "test")]),
        ),
        ("the second profile", tags(&[("on", "second")])),
        ("a profile with no tags", BTreeMap::new()),
    ];
    let mut arns: Vec<(&str, String, BTreeMap<String, String>)> = Vec::new();
    for (case, expected) in rows {
        let profile_id = create_profile(&service, case, Some(&as_tag_map(&expected))).await;
        arns.push((case, profile_arn(&datastore_arn, &profile_id), expected));
    }

    for (case, arn, expected) in &arns {
        let payload = list_tags(&service, case, arn).await;
        let reported = reported_tags(case, &payload);
        assert_eq!(
            &reported, expected,
            "case {case:?}: a profile ARN must report its own tag set, got {reported:?}"
        );
    }

    // The data store addressed by the ARN the profile ARNs were built from
    // keeps its own set: the two resource forms name different resources.
    let payload = list_tags(&service, "the data store", &datastore_arn).await;
    let reported = reported_tags("the data store", &payload);
    assert_eq!(
        reported,
        tags(&[("on", "datastore")]),
        "the data store must report its own tag set, got {reported:?}"
    );
}
