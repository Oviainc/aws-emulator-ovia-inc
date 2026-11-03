// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the tagging surface over EVERY resource form the
//! modelled `AmazonResourceName` admits: `TagResource` and
//! `ListTagsForResource` against a FHIR data store ARN
//! (`datastore/fhir/.{32}`) and a data transformation profile ARN
//! (`dataTransformationProfile/[a-f0-9]{32}`).
//!
//! The rule under test: `TagResource` merges the request's tags BY KEY onto the
//! resource its `ResourceARN` names, whichever of the modelled resource forms
//! that ARN has, and `ListTagsForResource` on the same ARN reports exactly the
//! stored set; an ARN whose resource the service does not hold is refused with
//! `ResourceNotFoundException` stating that resource's fixed sentence, and
//! nothing is stored.
//!
//! Every call below goes through [`crate::fhir_datastore_lifecycle::HealthLake`],
//! i.e. `MultiServiceRouter::dispatch` — the same entry point `core/router`
//! serves — so routing, deserialization, the modelled constraints and the error
//! envelope are all exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_not_found_message, DATASTORE_NOT_FOUND_MESSAGE, MAX_TAGS_PER_DATASTORE,
    MAX_TAGS_PER_PROFILE,
};

use crate::fhir_datastore_lifecycle::{error_code, string_at, HealthLake};

/// One taggable resource under test: its label, the ARN that addresses it, and
/// the cap its tag set is held to.
struct Resource {
    case: &'static str,
    arn: String,
    max_tags: usize,
}

/// The harness: the shared router plus the two creates whose records the
/// tagging surface writes.
struct Tagging {
    service: HealthLake,
}

impl Tagging {
    fn new() -> Self {
        Self {
            service: HealthLake::new(),
        }
    }

    /// Create a data store and return its `DatastoreArn`.
    async fn datastore(&self, tags: &Value) -> String {
        create_datastore(&self.service, "tagged store", Some(tags)).await
    }

    /// Create a data transformation profile and return its `ProfileId`.
    async fn profile(&self, tags: &Value) -> String {
        let tags = (!tags.is_null()).then_some(tags);
        create_profile(&self.service, "tagged profile", tags).await
    }

    /// `TagResource` with `tags`, which must succeed.
    async fn tag(&self, case: &str, arn: &str, tags: &Value) {
        let (status, payload) = self
            .service
            .call("TagResource", &json!({ "ResourceARN": arn, "Tags": tags }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource on {arn:?} must answer 200, got {status}: {payload}"
        );
    }

    /// `TagResource` with `tags`, which must be refused; returns
    /// `(status, error code)`.
    async fn tag_refused(&self, case: &str, arn: &str, tags: &Value) -> (u16, String, Value) {
        let (status, payload) = self
            .service
            .call("TagResource", &json!({ "ResourceARN": arn, "Tags": tags }))
            .await;
        assert!(
            status >= 400,
            "case {case:?}: TagResource on {arn:?} must be refused, got {status}: {payload}"
        );
        (status, error_code(&payload), payload)
    }

    /// The tags `ListTagsForResource` reports for `arn`, as a comparable map.
    async fn listed_tags(&self, case: &str, arn: &str) -> BTreeMap<String, String> {
        let payload = self
            .service
            .ok("ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        crate::list_tags_for_resource::reported_tags(case, &payload)
    }
}

/// The ARN of the data transformation profile `profile_id`, in the partition,
/// region and account this emulator mints its own ARNs in — read off an ARN the
/// service minted, so no test names an account id of its own.
///
/// `pub(crate)` so the sibling suites address a profile through the same
/// builder rather than each spelling the ARN form out.
pub(crate) fn profile_arn(minted_arn: &str, profile_id: &str) -> String {
    let fields: Vec<&str> = minted_arn.split(':').collect();
    assert_eq!(
        fields.len(),
        6,
        "a HealthLake ARN has six fields, got {minted_arn:?}"
    );
    format!(
        "{}:{}:{}:{}:{}:dataTransformationProfile/{profile_id}",
        fields[0], fields[1], fields[2], fields[3], fields[4]
    )
}

/// One `TagList` from `(key, value)` pairs.
///
/// `pub(crate)` so the sibling suites build the modelled `TagList` through one
/// builder rather than each spelling the `{Key, Value}` form out.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Create a data store named `name`, carrying `tags` (the modelled `TagList`,
/// the member omitted when `None`), and return its `DatastoreArn`.
///
/// `pub(crate)`: the one create the tagging suites write a data store through.
pub(crate) async fn create_datastore(
    service: &HealthLake,
    name: &str,
    tags: Option<&Value>,
) -> String {
    let mut request = json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name });
    if let Some(tags) = tags {
        request["Tags"] = tags.clone();
    }
    let (_, arn) = service.create(&request).await;
    arn
}

/// Create a data transformation profile named `name`, carrying `tags` (the
/// member omitted when `None`), and return its `ProfileId`.
///
/// Its `Tags` is the modelled `TagMap` — a map, not the `TagList` the tagging
/// operations take.
///
/// `pub(crate)`: the one create the tagging suites write a profile through.
pub(crate) async fn create_profile(
    service: &HealthLake,
    name: &str,
    tags: Option<&Value>,
) -> String {
    let mut request = json!({
        "ProfileName": name,
        "SourceFormat": "CCDA",
        "Source": { "ProfileMapping": { "ProfileMapping": { "a.map": "Patient.id <- id" } } },
    });
    if let Some(tags) = tags {
        request["Tags"] = tags.clone();
    }
    let (status, payload) = service
        .call("CreateDataTransformationProfile", &request)
        .await;
    assert!(
        (200..300).contains(&status),
        "CreateDataTransformationProfile must succeed, got {status}: {payload}"
    );
    string_at(&payload, "ProfileId")
}

/// `count` distinct tags, for the per-resource cap.
fn distinct_tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("key-{index:03}"), "Value": format!("{index}") }))
            .collect(),
    )
}

/// The tag batches one resource is tagged with, in order, each with the label it
/// reports itself under.
///
/// The rule generalises over the tags a request carries, so the rows differ
/// along exactly that: how many, whether a key is new or already stored, the
/// characters the key and value hold, and their lengths. The alphabet and the
/// lengths come from the modelled `Tag` members (`TagKey`: len 1..128, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`; `TagValue`: len 0..256, same pattern), each
/// taken at its published maximum.
fn merge_batches() -> Vec<(&'static str, Value)> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        ("an empty tag list changes nothing", tag_list(&[])),
        ("one new key", tag_list(&[("env", "test")])),
        (
            "the same key with another value overwrites it",
            tag_list(&[("env", "prod")]),
        ),
        (
            "several new keys in one call",
            tag_list(&[("team", "clinical"), ("cost.center", "42")]),
        ),
        (
            "keys and values across the modelled TagKey/TagValue alphabet, one value empty",
            tag_list(&[
                ("lab:unit/x=1+y-2@z", "ward 3"),
                ("клиника", "ünïcode"),
                ("empty.value", ""),
            ]),
        ),
        (
            "a key at the modelled 128-character maximum with a value at the 256-character maximum",
            tag_list(&[(longest_key.as_str(), longest_value.as_str())]),
        ),
        (
            "one call overwriting a stored key and adding another",
            tag_list(&[("env", "staging"), ("owner", "alice")]),
        ),
    ]
}

/// The tags in `batch`, as the map a merge of it leaves behind.
fn batch_as_map(batch: &Value) -> BTreeMap<String, String> {
    batch
        .as_array()
        .expect("a tag batch is a list")
        .iter()
        .map(|tag| (string_at(tag, "Key"), string_at(tag, "Value")))
        .collect()
}

#[tokio::test]
async fn tags_merge_by_key_onto_whichever_resource_the_resource_arn_names() {
    let harness = Tagging::new();
    // Both resources start from the SAME tag set, written by their own create,
    // so one table of batches applies to either resource form.
    let datastore_arn = harness.datastore(&tag_list(&[("owner", "platform")])).await;
    let profile_id = harness.profile(&json!({ "owner": "platform" })).await;
    let resources = vec![
        Resource {
            case: "a FHIR data store ARN",
            arn: datastore_arn.clone(),
            max_tags: MAX_TAGS_PER_DATASTORE,
        },
        Resource {
            case: "a data transformation profile ARN",
            arn: profile_arn(&datastore_arn, &profile_id),
            max_tags: MAX_TAGS_PER_PROFILE,
        },
    ];

    for resource in &resources {
        let mut expected: BTreeMap<String, String> =
            [("owner".to_string(), "platform".to_string())].into();
        let actual = harness.listed_tags(resource.case, &resource.arn).await;
        assert_eq!(
            actual, expected,
            "case {:?}: the create's own tags must be reported, got {actual:?}",
            resource.case
        );

        for (batch_case, batch) in merge_batches() {
            let case = format!("{} / {batch_case}", resource.case);
            harness.tag(&case, &resource.arn, &batch).await;
            expected.extend(batch_as_map(&batch));
            assert!(
                expected.len() <= resource.max_tags,
                "case {case:?}: the table must stay inside the {} tag cap, it reaches {}",
                resource.max_tags,
                expected.len()
            );
            let actual = harness.listed_tags(&case, &resource.arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: ListTagsForResource must report the merged set, got {actual:?}"
            );
        }
    }

    // Each resource holds its OWN tag set: a write through one ARN form is not
    // observable through the other.
    harness
        .tag(
            "tags stay on the resource they were written to",
            &resources[1].arn,
            &tag_list(&[("only.on", "the-profile")]),
        )
        .await;
    let datastore_tags = harness.listed_tags("the data store", &resources[0].arn).await;
    let profile_tags = harness.listed_tags("the profile", &resources[1].arn).await;
    assert_eq!(
        profile_tags.get("only.on").map(String::as_str),
        Some("the-profile"),
        "the profile must hold the tag written to it, got {profile_tags:?}"
    );
    assert!(
        !datastore_tags.contains_key("only.on"),
        "the data store must not hold a tag written to the profile, got {datastore_tags:?}"
    );
}

#[tokio::test]
async fn each_resource_tag_set_is_bounded_at_its_published_maximum() {
    // The bound: a profile's tags are also written by
    // `CreateDataTransformationProfile`'s `Tags` (`TagMap`, len 0..50) and a data
    // store's by `CreateFHIRDatastore`'s, so the accumulation `TagResource` builds
    // is held to the same maximum. Each row is driven at the maximum and one past
    // it; the modelled `TagList` bound (0..200) carries both batches in one call.
    for at_max in [false, true] {
        let harness = Tagging::new();
        let datastore_arn = harness.datastore(&tag_list(&[])).await;
        let profile_id = harness.profile(&Value::Null).await;
        let resources = vec![
            Resource {
                case: "a FHIR data store ARN",
                arn: datastore_arn.clone(),
                max_tags: MAX_TAGS_PER_DATASTORE,
            },
            Resource {
                case: "a data transformation profile ARN",
                arn: profile_arn(&datastore_arn, &profile_id),
                max_tags: MAX_TAGS_PER_PROFILE,
            },
        ];

        for resource in &resources {
            let count = if at_max {
                resource.max_tags
            } else {
                resource.max_tags + 1
            };
            let case = format!("{} / {count} tags in one call", resource.case);
            let batch = distinct_tags(count);
            if at_max {
                harness.tag(&case, &resource.arn, &batch).await;
                let actual = harness.listed_tags(&case, &resource.arn).await;
                assert_eq!(
                    actual.len(),
                    count,
                    "case {case:?}: a set at the maximum must be stored whole, got {} entries",
                    actual.len()
                );
            } else {
                let (status, code, payload) =
                    harness.tag_refused(&case, &resource.arn, &batch).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: past the maximum must answer 400, got {status}: {payload}"
                );
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: past the maximum must answer ValidationException, got {payload}"
                );
                let actual = harness.listed_tags(&case, &resource.arn).await;
                assert!(
                    actual.is_empty(),
                    "case {case:?}: a refused merge must store nothing, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_arn_whose_resource_is_not_held_is_refused_and_stores_nothing() {
    let harness = Tagging::new();
    let datastore_arn = harness.datastore(&tag_list(&[("owner", "platform")])).await;
    let profile_id = harness.profile(&json!({ "owner": "platform" })).await;
    let live_profile_arn = profile_arn(&datastore_arn, &profile_id);
    harness
        .tag(
            "seed the live profile",
            &live_profile_arn,
            &tag_list(&[("env", "test")]),
        )
        .await;

    // Every row satisfies the modelled `AmazonResourceName` pattern, so the
    // boundary validator passes it and the handler decides. Rows span the ways an
    // ARN can name a resource this service does not hold, in EITHER resource form,
    // each paired with the sentence of the resource it names.
    let unknown_profile = "0123456789abcdef0123456789abcdef";
    let fields: Vec<&str> = datastore_arn.split(':').collect();
    let (region, account, datastore_resource) = (fields[3], fields[4], fields[5]);
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a profile id nothing was minted under",
            profile_arn(&datastore_arn, unknown_profile),
            profile_not_found_message(unknown_profile),
        ),
        (
            "the live profile id in another region",
            format!(
                "arn:aws:healthlake:eu-west-1:{account}:dataTransformationProfile/{profile_id}"
            ),
            profile_not_found_message(&profile_id),
        ),
        (
            "the live profile id in another account",
            format!(
                "arn:aws:healthlake:{region}:210987654321:dataTransformationProfile/{profile_id}"
            ),
            profile_not_found_message(&profile_id),
        ),
        (
            "the live profile id in another partition",
            format!(
                "arn:aws-cn:healthlake:{region}:{account}:dataTransformationProfile/{profile_id}"
            ),
            profile_not_found_message(&profile_id),
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
            "the live data store in another partition",
            format!("arn:aws-us-gov:healthlake:{region}:{account}:{datastore_resource}"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, arn, expected_message) in &rows {
        for (operation, request) in [
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "leak", "Value": "no" }] }),
            ),
        ] {
            let (status, payload) = harness.service.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must answer 400, got {status}: {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: {operation} must answer ResourceNotFoundException, got {payload}"
            );
            let message = payload.get("Message").and_then(Value::as_str);
            assert_eq!(
                message,
                Some(expected_message.as_str()),
                "case {case:?}: {operation} must state {expected_message:?}, got {message:?}"
            );
            assert!(
                !payload.to_string().contains(arn.as_str()),
                "case {case:?}: {operation} must quote no ARN from the request, got {payload}"
            );
        }
    }

    // The live records are untouched by every refusal above.
    let profile_tags = harness
        .listed_tags("the live profile", &live_profile_arn)
        .await;
    assert_eq!(
        profile_tags,
        [
            ("env".to_string(), "test".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ]
        .into(),
        "the live profile must still report exactly its own tags, got {profile_tags:?}"
    );
    let datastore_tags = harness
        .listed_tags("the live data store", &datastore_arn)
        .await;
    assert_eq!(
        datastore_tags,
        [("owner".to_string(), "platform".to_string())].into(),
        "the live data store must still report exactly its own tags, got {datastore_tags:?}"
    );
}
