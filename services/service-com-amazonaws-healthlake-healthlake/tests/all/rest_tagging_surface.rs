// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the data-transformation endpoint's REST tagging surface:
//! `GET /tags/{ResourceArn}`, `POST /tags/{ResourceArn}` and
//! `POST /untag/{ResourceArn}`.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] — the same entry point
//! `core/router` serves — as an ordinary signed REST request carrying NO
//! `X-Amz-Target`, which is how a client reaches a path-addressed surface, through the
//! shared harness in `fhir_rest_and_validate_source`. The control-plane calls that
//! create the resources being tagged go over the awsJson1_0 wire in the same test, so
//! the ARN a tagging request names is one the service itself minted.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. the three REST tagging operations read and write the SAME per-resource tag map
//!    the bound awsJson tagging operations do, for whichever resource form the
//!    `ResourceArn` names: a merge is reported by a later report over EITHER wire, a
//!    removal is by key and idempotent, and nothing leaks between resources;
//! 2. an `ResourceArn` naming a resource this service does not hold is refused with
//!    the modelled `ResourceNotFoundException` at the HTTP 404 its `@httpError` names,
//!    stating that resource's fixed sentence and quoting nothing from the request;
//! 3. a request outside a rule the model declares — a required member left out, a
//!    collection past its `@length`, a key or value past its own — is refused with the
//!    modelled `ValidationException` at HTTP 400 and writes nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_not_found_message, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::error_code;
use crate::fhir_rest_and_validate_source::HealthLake;

/// The media type a tagging request's JSON body travels as.
const JSON: &str = "application/json";

/// The largest tag map the surface accepts, from the modelled `TagMap`
/// `@length(min: 0, max: 50)` both mutating requests target.
const MAX_TAGS: usize = 50;

/// The modelled `DataTransformationTagKey` `@length(min: 1, max: 128)`.
const MAX_TAG_KEY_LEN: usize = 128;

/// The modelled `DataTransformationTagValue` `@length(min: 0, max: 256)`.
const MAX_TAG_VALUE_LEN: usize = 256;

/// One taggable resource under test: the label it reports itself under, its ARN, and
/// the sentence a request naming a resource of its kind that is not held states.
struct Resource {
    case: &'static str,
    arn: String,
    not_found: String,
}

/// `arn` as a client puts it in a path: an `@httpLabel` is one path segment, so the
/// ARN's `:` and `/` are percent-encoded.
fn encoded(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The URI of a tagging request addressing `arn` under the path segment `segment`.
fn tagging_uri(segment: &str, arn: &str) -> String {
    format!("/{segment}/{}", encoded(arn))
}

/// Create a data store over the awsJson1_0 wire and return its `DatastoreArn`.
async fn datastore_arn(service: &HealthLake, name: &str) -> String {
    let (status, created) = service
        .rpc(
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFHIRDatastore must answer 200, got {status}: {created}"
    );
    created["DatastoreArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reports a DatastoreArn, got {created}"))
        .to_string()
}

/// Create a data transformation profile over the awsJson1_0 wire and return its
/// `ProfileId`.
async fn profile_id(service: &HealthLake, name: &str) -> String {
    let (status, created) = service
        .rpc(
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": name,
                "SourceFormat": "CCDA",
                "Source": { "ProfileMapping": { "ProfileMapping": { "a.map": "Patient.id <- id" } } },
            }),
        )
        .await;
    assert!(
        (200..300).contains(&status),
        "CreateDataTransformationProfile must succeed, got {status}: {created}"
    );
    created["ProfileId"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reports a ProfileId, got {created}"))
        .to_string()
}

/// The two resource forms the tagging state holds, both minted by this service: a FHIR
/// data store and a data transformation profile, the resource the
/// data-transformation endpoint's tagging operations reference.
async fn resources(service: &HealthLake) -> Vec<Resource> {
    let datastore = datastore_arn(service, "rest tagged store").await;
    let profile = profile_id(service, "rest tagged profile").await;
    vec![
        Resource {
            case: "a data transformation profile ARN",
            arn: crate::resource_tagging::profile_arn(&datastore, &profile),
            not_found: profile_not_found_message(&profile),
        },
        Resource {
            case: "a FHIR data store ARN",
            arn: datastore,
            not_found: DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        },
    ]
}

/// `POST /tags/{ResourceArn}` with `tags`, asserted to have succeeded.
async fn merge(service: &HealthLake, case: &str, arn: &str, tags: &Value) {
    let (status, body) = service
        .rest(
            "POST",
            &tagging_uri("tags", arn),
            JSON,
            serde_json::to_vec(&json!({ "Tags": tags })).expect("the request body serializes"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: POST /tags must answer 200, got {status}: {body}"
    );
    // The modelled output is `Unit`: there is no member to carry, so the answer has no
    // body at all.
    assert_eq!(
        body,
        Value::Null,
        "case {case:?}: POST /tags answers the modelled Unit output, got {body}"
    );
}

/// `POST /untag/{ResourceArn}` with `keys`, asserted to have succeeded.
async fn remove(service: &HealthLake, case: &str, arn: &str, keys: &[&str]) {
    let (status, body) = service
        .rest(
            "POST",
            &tagging_uri("untag", arn),
            JSON,
            serde_json::to_vec(&json!({ "TagKeys": keys })).expect("the request body serializes"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: POST /untag must answer 200, got {status}: {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case {case:?}: POST /untag answers the modelled Unit output, got {body}"
    );
}

/// The tags `GET /tags/{ResourceArn}` reports, as a comparable map.
async fn reported(service: &HealthLake, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = service
        .rest("GET", &tagging_uri("tags", arn), JSON, Vec::new())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GET /tags must answer 200, got {status}: {body}"
    );
    let reported = body["Tags"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: the report carries the modelled Tags map, got {body}")
    });
    reported
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a reported tag value is a string, got {body}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The tags the awsJson1_0 `ListTagsForResource` reports for `arn`, as the same
/// comparable map — the other wire over the one tag map.
async fn listed(service: &HealthLake, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = service
        .rpc("ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {status}: {body}"
    );
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the report carries a TagList, got {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The tag maps one resource is merged with, in order, each with the label it reports
/// itself under.
///
/// The rule generalises over the tags a request carries, so the rows differ along
/// exactly that: how many, whether a key is new or already stored, the characters the
/// key and value hold, and their lengths. The lengths are the modelled
/// `DataTransformationTagKey` (1..128) and `DataTransformationTagValue` (0..256) maxima,
/// each taken at its published bound.
fn merge_batches() -> Vec<(&'static str, Value)> {
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        ("an empty map changes nothing", json!({})),
        ("one new key", json!({ "env": "test" })),
        (
            "the same key with another value overwrites it",
            json!({ "env": "prod" }),
        ),
        (
            "several new keys in one call",
            json!({ "team": "clinical", "cost.center": "42" }),
        ),
        (
            "keys and values outside ASCII, one value empty",
            json!({ "клиника": "ünïcode", "empty.value": "" }),
        ),
        (
            "a key at the modelled 128-character maximum with a value at the 256-character \
             maximum",
            json!({ longest_key.clone(): longest_value.clone() }),
        ),
        (
            "one call overwriting a stored key and adding another",
            json!({ "env": "staging", "owner": "alice" }),
        ),
        (
            "a map at the modelled 50-entry maximum",
            distinct_tags(MAX_TAGS),
        ),
    ]
}

/// `count` distinct tags, as the modelled `TagMap`.
fn distinct_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index:03}"), Value::String(index.to_string())))
            .collect(),
    )
}

/// The tags in `batch`, as the map a merge of it leaves behind.
fn batch_as_map(batch: &Value) -> BTreeMap<String, String> {
    batch
        .as_object()
        .expect("a tag batch is a map")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().expect("a tag value is a string").to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_rest_tagging_surface_reads_and_writes_the_one_tag_map_of_the_named_resource() {
    let service = HealthLake::new();
    let resources = resources(&service).await;

    for resource in &resources {
        // A freshly created resource carries no tags, and the REST report says so
        // rather than failing to route.
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        let actual = reported(&service, resource.case, &resource.arn).await;
        assert_eq!(
            actual, expected,
            "case {:?}: a resource created with no tags reports an empty map, got {actual:?}",
            resource.case
        );

        for (batch_case, batch) in merge_batches() {
            let case = format!("{} / {batch_case}", resource.case);
            // Each row is merged onto whatever the previous rows left, so the stored
            // set has to stay inside the resource's cap.
            let mut after = expected.clone();
            after.extend(batch_as_map(&batch));
            if after.len() > MAX_TAGS {
                // The stored set is what has to give way, and it is itself inside the
                // cap, so one removal names at most the maximum number of keys.
                let stored: Vec<String> = expected.keys().cloned().collect();
                remove(
                    &service,
                    &format!("{case} / make room"),
                    &resource.arn,
                    &stored.iter().map(String::as_str).collect::<Vec<&str>>(),
                )
                .await;
                expected.clear();
            }
            merge(&service, &case, &resource.arn, &batch).await;
            expected.extend(batch_as_map(&batch));

            let actual = reported(&service, &case, &resource.arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: GET /tags must report the merged set, got {actual:?}"
            );
            // The SAME map, over the bound awsJson wire: one tag map, two wires.
            let over_rpc = listed(&service, &case, &resource.arn).await;
            assert_eq!(
                over_rpc, expected,
                "case {case:?}: ListTagsForResource must report the same set, got {over_rpc:?}"
            );
        }

        // Removal is by key: only the keys named go, and a key the resource does not
        // carry leaves the set as it was — so repeating the same removal answers the
        // same way and reports the same set.
        let present: Vec<String> = expected.keys().take(2).cloned().collect();
        let absent = "never.stored";
        let keys: Vec<&str> = present
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(absent))
            .collect();
        for attempt in 1..=2 {
            let case = format!("{} / removal attempt {attempt}", resource.case);
            remove(&service, &case, &resource.arn, &keys).await;
            for key in &present {
                expected.remove(key);
            }
            let actual = reported(&service, &case, &resource.arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: POST /untag removes exactly the keys it names, got {actual:?}"
            );
        }

        // A tag written over the awsJson wire is reported by the REST surface too.
        let (status, body) = service
            .rpc(
                "TagResource",
                &json!({
                    "ResourceARN": &resource.arn,
                    "Tags": [{ "Key": "written.over.rpc", "Value": "yes" }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: TagResource must answer 200, got {status}: {body}",
            resource.case
        );
        expected.insert("written.over.rpc".to_string(), "yes".to_string());
        let actual = reported(&service, resource.case, &resource.arn).await;
        assert_eq!(
            actual, expected,
            "case {:?}: GET /tags must report what the awsJson wire wrote, got {actual:?}",
            resource.case
        );
    }

    // Each resource holds its OWN tag set: a write through one ARN is not observable
    // through the other.
    merge(
        &service,
        "tags stay on the resource they were written to",
        &resources[0].arn,
        &json!({ "only.on": "the-first" }),
    )
    .await;
    let second = reported(&service, resources[1].case, &resources[1].arn).await;
    assert!(
        !second.contains_key("only.on"),
        "a tag written to one resource must not appear on the other, got {second:?}"
    );

    // A client that sent the label's `/` unencoded addressed the same resource.
    let raw = service
        .rest(
            "GET",
            &format!("/tags/{}", resources[0].arn),
            JSON,
            Vec::new(),
        )
        .await;
    assert_eq!(
        raw.0, 200,
        "an unencoded label addresses the same resource, got {}: {}",
        raw.0, raw.1
    );
    assert_eq!(
        raw.1["Tags"]["only.on"].as_str(),
        Some("the-first"),
        "an unencoded label reports the same tags, got {}",
        raw.1
    );
}

#[tokio::test]
async fn an_arn_whose_resource_is_not_held_is_refused_with_the_modelled_not_found() {
    let service = HealthLake::new();
    let resources = resources(&service).await;
    let live = &resources[0].arn;
    merge(&service, "seed the live resource", live, &json!({ "env": "test" })).await;

    let fields: Vec<&str> = live.split(':').collect();
    let (region, account) = (fields[3], fields[4]);
    // Rows span the ways an ARN can name a resource this service does not hold, in
    // EITHER resource form, each paired with the sentence of the resource it
    // names. The REST `ResourceArn` member targets a plain `String`, so an ARN of
    // neither form reaches the handler too and is answered the same way.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a profile id nothing was minted under",
            format!(
                "arn:aws:healthlake:{region}:{account}:dataTransformationProfile/\
                 0123456789abcdef0123456789abcdef"
            ),
            profile_not_found_message("0123456789abcdef0123456789abcdef"),
        ),
        (
            "the live resource in another region",
            live.replacen(region, "eu-west-1", 1),
            resources[0].not_found.clone(),
        ),
        (
            "the live resource in another account",
            live.replacen(account, "210987654321", 1),
            resources[0].not_found.clone(),
        ),
        (
            "the live resource in another partition",
            live.replacen("arn:aws:", "arn:aws-cn:", 1),
            resources[0].not_found.clone(),
        ),
        (
            "a data store id nothing was minted under",
            format!(
                "arn:aws:healthlake:{region}:{account}:datastore/fhir/\
                 00000000000000000000000000000000"
            ),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "an ARN of neither resource form",
            format!("arn:aws:healthlake:{region}:{account}:somethingElse/abc"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a value that is not an ARN at all",
            "not-an-arn".to_string(),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, arn, expected_message) in &rows {
        for (label, method, segment, body) in [
            ("GET /tags", "GET", "tags", Value::Null),
            ("POST /tags", "POST", "tags", json!({ "Tags": { "leak": "no" } })),
            (
                "POST /untag",
                "POST",
                "untag",
                json!({ "TagKeys": ["leak"] }),
            ),
        ] {
            let payload = if body.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&body).expect("the request body serializes")
            };
            let (status, answered) = service
                .rest(method, &tagging_uri(segment, arn), JSON, payload)
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: {label} must answer the modelled 404, got {status}: {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: {label} must answer ResourceNotFoundException, got {answered}"
            );
            let message = answered.get("Message").and_then(Value::as_str);
            assert_eq!(
                message,
                Some(expected_message.as_str()),
                "case {case:?}: {label} must state {expected_message:?}, got {message:?}"
            );
            assert!(
                !answered.to_string().contains(arn.as_str()),
                "case {case:?}: {label} must quote no ARN from the request, got {answered}"
            );
        }
    }

    // Every refusal above left the live resource exactly as it was.
    let actual = reported(&service, "the live resource", live).await;
    assert_eq!(
        actual,
        [("env".to_string(), "test".to_string())].into(),
        "the live resource must still report exactly its own tags, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused_and_writes_nothing() {
    let service = HealthLake::new();
    let resources = resources(&service).await;
    let arn = &resources[0].arn;
    merge(&service, "seed the resource", arn, &json!({ "env": "test" })).await;
    let stored: BTreeMap<String, String> = [("env".to_string(), "test".to_string())].into();

    let over_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    // Rows span the classes of rule the model states on the two mutating request
    // shapes: a required member left out, a member of the wrong type, a collection
    // outside its `@length`, and a key or value outside its own `@length`. Each bound
    // is driven one past its published maximum (the maxima themselves are merged
    // successfully in the round-trip suite above).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        ("no Tags member at all", "POST", "tags", json!({})),
        (
            "a Tags member of the wrong type",
            "POST",
            "tags",
            json!({ "Tags": ["env"] }),
        ),
        (
            "one tag past the 50-entry TagMap maximum",
            "POST",
            "tags",
            json!({ "Tags": distinct_tags(MAX_TAGS + 1) }),
        ),
        (
            "a tag key one character past its 128-character maximum",
            "POST",
            "tags",
            json!({ "Tags": { over_long_key.clone(): "x" } }),
        ),
        (
            "a tag key of zero length, below its 1-character minimum",
            "POST",
            "tags",
            json!({ "Tags": { "": "x" } }),
        ),
        (
            "a tag value one character past its 256-character maximum",
            "POST",
            "tags",
            json!({ "Tags": { "env": over_long_value.clone() } }),
        ),
        (
            "a tag value of the wrong type",
            "POST",
            "tags",
            json!({ "Tags": { "env": 7 } }),
        ),
        ("no TagKeys member at all", "POST", "untag", json!({})),
        (
            "an empty TagKeys list, below its 1-element minimum",
            "POST",
            "untag",
            json!({ "TagKeys": [] }),
        ),
        (
            "one key past the 50-element TagKeyList maximum",
            "POST",
            "untag",
            Value::Object(
                [(
                    "TagKeys".to_string(),
                    Value::Array(
                        (0..=MAX_TAGS)
                            .map(|index| Value::String(format!("key-{index:03}")))
                            .collect(),
                    ),
                )]
                .into_iter()
                .collect(),
            ),
        ),
        (
            "a tag key one character past its maximum, in an untag",
            "POST",
            "untag",
            json!({ "TagKeys": [over_long_key.clone()] }),
        ),
        (
            "a tag key of the wrong type, in an untag",
            "POST",
            "untag",
            json!({ "TagKeys": [7] }),
        ),
        (
            "a body that is not a JSON object",
            "POST",
            "tags",
            Value::String("env=test".to_string()),
        ),
    ];

    for (case, method, segment, body) in &rows {
        let (status, answered) = service
            .rest(
                method,
                &tagging_uri(segment, arn),
                JSON,
                serde_json::to_vec(body).expect("the request body serializes"),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: must answer the modelled 400, got {status}: {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {answered}"
        );
        let message = answered
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal states its reason in the modelled Message, got {answered}"
        );
        // A refusal is all-or-nothing: nothing the refused request named was written.
        let actual = reported(&service, case, arn).await;
        assert_eq!(
            actual, stored,
            "case {case:?}: a refused request must change nothing, got {actual:?}"
        );
    }

    // A method and path pair the model declares no operation for is NOT served here:
    // it stays the framework's own answer for a request naming no operation.
    for (case, method, segment) in [
        ("a delete on the tags path", "DELETE", "tags"),
        ("a report on the untag path", "GET", "untag"),
    ] {
        let (status, answered) = service
            .rest(method, &tagging_uri(segment, arn), JSON, Vec::new())
            .await;
        assert!(
            status >= 400,
            "case {case:?}: an undeclared method must not be served, got {status}: {answered}"
        );
        assert!(
            answered.get("Tags").is_none(),
            "case {case:?}: an undeclared method must report no tags, got {answered}"
        );
    }
}
