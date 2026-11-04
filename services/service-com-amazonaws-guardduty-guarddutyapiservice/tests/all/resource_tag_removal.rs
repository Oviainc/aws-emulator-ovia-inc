// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` against every GuardDuty resource a tag ARN can name, end to end
//! over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. "Removes tags from a resource": every key the request names stops being
//!    carried by the resource its `ResourceArn` names, every key it does not name
//!    stays, and the resource's OWN read is what reports the result. Removal is by
//!    key and idempotent — a key the resource never carried, a key named twice in
//!    one request, and a request replayed, all change nothing beyond the first
//!    removal.
//! 2. The ARN names the resource by its resource PATH, which is that resource's own
//!    REST URI: the detector, and each sub-resource segment
//!    `state::TAGGED_SUB_RESOURCE_SEGMENTS` names.
//! 3. Nothing else moves: the tags of every OTHER resource are untouched.
//! 4. An ARN this endpoint resolves no resource for — another account's, another
//!    partition's, another Region's, an id nothing minted, a resource type GuardDuty
//!    does not tag — is refused with HTTP 400 under the `InvalidInputException`
//!    class, under the ONE fixed sentence the whole tagging surface answers, which
//!    never echoes the ARN, and persists nothing.
//!
//! The fixture that creates one resource of every class a tag ARN can name, and the
//! ARN/read-URI helpers around it, are `pub(crate)` so the `resource_tag_addition`
//! suite drives `TagResource` against the SAME set of resources: the two operations
//! must agree about which ARNs resolve and where each resource's tags live.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, new_router, Router, ABSENT_DETECTOR_ID};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::TAGGED_SUB_RESOURCE_SEGMENTS;

/// The Region every request in this suite is signed for, unless a case names
/// another.
const REGION: &str = "us-east-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// This service's segment in an ARN it owns (`smithy_query(query="service_info")`
/// gives `guardduty` as both the `arnNamespace` and the `endpointPrefix`).
const ARN_SERVICE: &str = "guardduty";

/// A well-formed minted id (32 hexadecimal characters) that no create ever minted.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// A KMS key ARN for the publishing destination a tag ARN names. Nothing is
/// published, so the value is only ever stored and reported back.
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The tags every resource in this suite is created with. They differ along what a
/// `TagKey` admits — `len 1..128` with the punctuation
/// `smithy_query(shape="TagMap")` allows — and along the VALUE, including the empty
/// string a `TagValue` (`len 0..256`) may be.
const BASE_TAGS: [(&str, &str); 4] = [
    ("Env", "prod"),
    ("Team/Owner", "security"),
    ("cost-center", "4711"),
    ("Reviewed", ""),
];

/// A key no resource here is created with.
const ABSENT_KEY: &str = "Absent";

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

pub(crate) fn tag_map(pairs: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The tags `BASE_TAGS` is left with once `removed` is taken out of it — the rule
/// itself, so each row only has to name the keys it removes.
fn remaining_after(removed: &[&str]) -> Value {
    let kept: Vec<(&str, &str)> = BASE_TAGS
        .iter()
        .copied()
        .filter(|(key, _)| !removed.contains(key))
        .collect();
    tag_map(&kept)
}

/// The ARN of the resource at `path`, built exactly as this emulator builds every
/// ARN it reports (`engine::emulator_arn`), so a test ARN cannot drift from the one
/// the service resolves.
pub(crate) fn arn_of(region: &str, path: &str) -> String {
    engine::emulator_arn(ARN_SERVICE, region, path)
}

/// A `ResourceArn` as the wire carries it. `/tags/{ResourceArn}` binds a
/// NON-GREEDY URI label (`smithy_query(operation="UntagResource")`), so the ARN
/// occupies one path segment and its `/` separators travel percent-encoded.
pub(crate) fn uri_label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

/// A tag key as a query value: `+` would decode to a space and `&`/`=` would split
/// the pair, so those travel percent-encoded.
fn query_value(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '+' => "%2B".to_string(),
            '=' => "%3D".to_string(),
            '&' => "%26".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One resource a tagging ARN can name.
///
/// It carries only its resource PATH, because the ARN's resource path IS the
/// resource's own REST URI: `arn:…:detector/{id}/ipset/{id}` for the resource read
/// at `/detector/{id}/ipset/{id}`.
pub(crate) struct Tagged {
    pub(crate) case: &'static str,
    pub(crate) path: String,
}

impl Tagged {
    pub(crate) fn arn(&self, region: &str) -> String {
        arn_of(region, &self.path)
    }

    pub(crate) fn read_path(&self) -> String {
        format!("/{}", self.path)
    }
}

/// Create a resource and return the id its create minted under `id_member`.
async fn minted(
    router: &Router,
    region: &str,
    path: &str,
    body: Value,
    id_member: &str,
) -> String {
    let (status, got) = call_in(router, region, "POST", path, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {path:?}: expected the create to answer 200, got {status} {got:?}"
    );
    got[id_member]
        .as_str()
        .unwrap_or_else(|| panic!("case {path:?}: expected a {id_member}, got {got:?}"))
        .to_string()
}

/// Create one resource of every class a tag ARN can name, each carrying `tags`, and
/// return the detector's id beside them.
pub(crate) async fn tagged_resources(
    router: &Router,
    region: &str,
    tags: &Value,
) -> (String, Vec<Tagged>) {
    let detector = minted(
        router,
        region,
        "/detector",
        json!({"enable": true, "tags": tags}),
        "detectorId",
    )
    .await;
    let base = format!("/detector/{detector}");

    // The entity sets and lists share the create shape; only the collection they
    // land in differs.
    let list_body = |name: &str| {
        json!({
            "name": name,
            "format": "TXT",
            "location": "s3://guardduty-lists/list.txt",
            "activate": true,
            "tags": tags,
        })
    };

    let filter = minted(
        router,
        region,
        &format!("{base}/filter"),
        json!({
            "name": "tagged-filter",
            "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
            "tags": tags,
        }),
        "name",
    )
    .await;
    let ip_set = minted(
        router,
        region,
        &format!("{base}/ipset"),
        list_body("tagged-ip-set"),
        "ipSetId",
    )
    .await;
    let threat_intel_set = minted(
        router,
        region,
        &format!("{base}/threatintelset"),
        list_body("tagged-threat-intel-set"),
        "threatIntelSetId",
    )
    .await;
    let trusted_entity_set = minted(
        router,
        region,
        &format!("{base}/trustedentityset"),
        list_body("tagged-trusted-set"),
        "trustedEntitySetId",
    )
    .await;
    let threat_entity_set = minted(
        router,
        region,
        &format!("{base}/threatentityset"),
        list_body("tagged-threat-set"),
        "threatEntitySetId",
    )
    .await;
    let destination = minted(
        router,
        region,
        &format!("{base}/publishingDestination"),
        json!({
            "destinationType": "S3",
            "destinationProperties": {
                "destinationArn": "arn:aws:s3:::guardduty-findings",
                "kmsKeyArn": KEY_ARN,
            },
            "tags": tags,
        }),
        "destinationId",
    )
    .await;

    let resources = vec![
        Tagged {
            case: "the detector itself",
            path: format!("detector/{detector}"),
        },
        Tagged {
            case: "a finding filter",
            path: format!("detector/{detector}/filter/{filter}"),
        },
        Tagged {
            case: "an IPSet",
            path: format!("detector/{detector}/ipset/{ip_set}"),
        },
        Tagged {
            case: "a ThreatIntelSet",
            path: format!("detector/{detector}/threatintelset/{threat_intel_set}"),
        },
        Tagged {
            case: "a trusted entity set",
            path: format!("detector/{detector}/trustedentityset/{trusted_entity_set}"),
        },
        Tagged {
            case: "a threat entity set",
            path: format!("detector/{detector}/threatentityset/{threat_entity_set}"),
        },
        Tagged {
            case: "a publishing destination",
            path: format!("detector/{detector}/publishingDestination/{destination}"),
        },
    ];
    (detector, resources)
}

/// Remove `keys` from the resource `arn` names, through `region`'s endpoint.
pub(crate) async fn untag_in(router: &Router, region: &str, arn: &str, keys: &[&str]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect::<Vec<String>>()
        .join("&");
    call_in(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{query}", uri_label(arn)),
        None,
    )
    .await
}

/// The tags the resource's own read reports.
///
/// Every read of this service reports the `tags` member, empty when the resource
/// carries none, so this asserts the member is there rather than normalising its
/// absence away.
pub(crate) async fn tags_of(
    router: &Router,
    region: &str,
    resource: &Tagged,
    case: &str,
) -> Value {
    let (status, body) = call_in(router, region, "GET", &resource.read_path(), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the read to answer 200, got {status} {body:?}"
    );
    let tags = body.get("tags").cloned().unwrap_or(Value::Null);
    assert!(
        tags.is_object(),
        "case {case:?}: every read reports a tag map, empty when nothing tagged the \
         resource, got {tags:?}"
    );
    tags
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: the named keys go, the rest stay, for every resource class.
// ---------------------------------------------------------------------------

/// The keys one `UntagResource` call names. The rows differ along everything the
/// rule generalises over: the COUNT removed (one, several, all), WHICH keys (each
/// character class and the empty-valued one), the ORDER they are named in, whether
/// a key is carried at all, whether one is named twice, and the LENGTH of the list
/// at the published maximum of `TagKeyList` (`len 1..200` —
/// `smithy_query(shape="TagKeyList")`).
fn removal_rows() -> Vec<(String, Vec<String>)> {
    let mut rows: Vec<(String, Vec<String>)> = vec![
        ("one of the four keys".to_string(), vec!["Env".to_string()]),
        (
            "two keys at once".to_string(),
            vec!["Env".to_string(), "cost-center".to_string()],
        ),
        (
            "the same two keys named in the other order".to_string(),
            vec!["cost-center".to_string(), "Env".to_string()],
        ),
        (
            "the key whose value is the empty string".to_string(),
            vec!["Reviewed".to_string()],
        ),
        (
            "a key carrying the punctuation a TagKey admits".to_string(),
            vec!["Team/Owner".to_string()],
        ),
        (
            "every key the resource carries".to_string(),
            BASE_TAGS
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect(),
        ),
        (
            "a key the resource does not carry".to_string(),
            vec![ABSENT_KEY.to_string()],
        ),
        (
            "the same key named twice in one request".to_string(),
            vec!["Env".to_string(), "Env".to_string()],
        ),
    ];

    // The published maximum of the list: 200 keys, one of which the resource
    // carries.
    let mut longest: Vec<String> = (0..199).map(|index| format!("filler-{index}")).collect();
    longest.push("Env".to_string());
    rows.push((
        format!(
            "a list at the published maximum of {} keys",
            longest.len()
        ),
        longest,
    ));
    rows
}

#[tokio::test]
async fn removing_keys_leaves_exactly_the_tags_the_request_did_not_name() {
    for (row, keys) in removal_rows() {
        // Removal is destructive, so each row gets a store of its own.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        let named: Vec<&str> = keys.iter().map(String::as_str).collect();
        let expected = remaining_after(&named);

        for resource in &resources {
            let case = format!("{row} on {}", resource.case);
            let (status, body) = untag_in(&router, REGION, &resource.arn(REGION), &named).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204, got {status} {body:?}"
            );
            assert!(
                body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
                "case {case:?}: expected no response members, got {body:?}"
            );

            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the remaining tags {expected:?}, got {actual:?}"
            );
        }
    }
}

/// Every sub-resource segment the service resolves is named by one of the ARNs
/// above, and each of those ARN paths is the resource's own read URI — so a
/// segment nothing here creates would fail this, not pass silently.
#[tokio::test]
async fn every_tagged_sub_resource_segment_is_exercised_by_an_arn() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        let matching = resources
            .iter()
            .filter(|resource| resource.path.contains(&format!("/{segment}/")))
            .count();
        assert_eq!(
            matching, 1,
            "case {segment:?}: exactly one resource must be named through this segment, got {matching}"
        );
    }

    // The ARN resource path doubles as the read URI, so every resource created
    // above reads back on it and reports the tags the create stored.
    let expected = base_tags();
    for resource in &resources {
        let actual = tags_of(&router, REGION, resource, resource.case).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected the created tags {expected:?}, got {actual:?}",
            resource.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 1 + 3: idempotent, and no other resource moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untagging_one_resource_is_idempotent_and_touches_no_other() {
    // How many classes a tag ARN names: the detector, plus one per sub-resource
    // segment the service resolves.
    let class_count = TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1;

    for target in 0..class_count {
        // One store per target, so what the assertions see is this call's work.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        assert_eq!(
            resources.len(),
            class_count,
            "every class a tag ARN names must be created here, got {}",
            resources.len()
        );
        let case = format!("untagging {}", resources[target].case);
        let arn = resources[target].arn(REGION);

        // The same request twice: the second removes nothing and is accepted.
        for attempt in 1..=2 {
            let (status, body) = untag_in(&router, REGION, &arn, &["Env"]).await;
            assert_eq!(
                status, 204,
                "case {case:?} attempt {attempt}: expected 204, got {status} {body:?}"
            );
            let actual = tags_of(&router, REGION, &resources[target], &case).await;
            let expected = remaining_after(&["Env"]);
            assert_eq!(
                actual, expected,
                "case {case:?} attempt {attempt}: expected {expected:?}, got {actual:?}"
            );
        }

        // Every other resource still carries everything the create gave it.
        let untouched = base_tags();
        for (index, resource) in resources.iter().enumerate() {
            if index == target {
                continue;
            }
            let other = format!("{case}, reading {}", resource.case);
            let actual = tags_of(&router, REGION, resource, &other).await;
            assert_eq!(
                actual, untouched,
                "case {other:?}: an untag elsewhere must change nothing here, expected {untouched:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an ARN this endpoint resolves nothing for is a classified HTTP 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_resource_arn_is_refused_without_echoing_it() {
    let router = new_router();
    let (detector, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    // Each row: the case, the Region whose endpoint is called, and the ARN.
    let rows: Vec<(String, &str, String)> = vec![
        (
            "a detectorId no create minted".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{ABSENT_DETECTOR_ID}")),
        ),
        (
            "an ipSetId the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/ipset/{ABSENT_ID}")),
        ),
        (
            "a filterName the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/filter/absent-filter")),
        ),
        (
            "a destinationId the detector does not hold".to_string(),
            REGION,
            arn_of(
                REGION,
                &format!("detector/{detector}/publishingDestination/{ABSENT_ID}"),
            ),
        ),
        (
            "another account's detector".to_string(),
            REGION,
            format!("arn:aws:guardduty:{REGION}:111122223333:detector/{detector}"),
        ),
        (
            "another partition's detector".to_string(),
            REGION,
            format!("arn:aws-cn:guardduty:{REGION}:000000000000:detector/{detector}"),
        ),
        (
            "an ARN claiming another Region".to_string(),
            REGION,
            arn_of("eu-west-1", &format!("detector/{detector}")),
        ),
        (
            "this Region's ARN at another Region's endpoint".to_string(),
            "eu-west-1",
            arn_of(REGION, &format!("detector/{detector}")),
        ),
        (
            "a sub-resource type GuardDuty does not tag".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/widget/{ABSENT_ID}")),
        ),
    ];

    for (case, region, arn) in &rows {
        let (status, body) = untag_in(&router, region, arn, &["Env"]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, INVALID_RESOURCE_ARN_MESSAGE,
            "case {case:?}: expected {INVALID_RESOURCE_ARN_MESSAGE:?}, got {message:?}"
        );
        for echoed in [arn.as_str(), detector.as_str()] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
    }

    // Every refusal persisted nothing: each resource still carries all four tags.
    let untouched = base_tags();
    for resource in &resources {
        let actual = tags_of(&router, REGION, resource, resource.case).await;
        assert_eq!(
            actual, untouched,
            "case {:?}: a refused untag must change nothing, expected {untouched:?}, got {actual:?}",
            resource.case
        );
    }
}

/// A request the model itself refuses never reaches the handler, and is answered
/// with HTTP 400 rather than a success: `ResourceArn` is a `GuardDutyArn`, whose
/// pattern admits only this service's detector resource path, and `TagKeys` is
/// `len 1..200` (`smithy_query(operation="UntagResource")`).
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = new_router();
    let (detector, _) = tagged_resources(&router, REGION, &base_tags()).await;

    let one_past_the_maximum: Vec<String> =
        (0..201).map(|index| format!("filler-{index}")).collect();
    let named: Vec<&str> = one_past_the_maximum
        .iter()
        .map(String::as_str)
        .collect();

    let rows: Vec<(String, String, Vec<&str>)> = vec![
        (
            "a Malware Protection plan ARN, which the pattern does not admit".to_string(),
            arn_of(REGION, &format!("malware-protection-plan/{ABSENT_ID}")),
            vec!["Env"],
        ),
        (
            "another service's ARN".to_string(),
            "arn:aws:s3:::guardduty-findings".to_string(),
            vec!["Env"],
        ),
        (
            "a detector path shorter than the pattern admits".to_string(),
            arn_of(REGION, "detector/short"),
            vec!["Env"],
        ),
        (
            format!(
                "a tagKeys list one past the maximum, with {} keys",
                named.len()
            ),
            arn_of(REGION, &format!("detector/{detector}")),
            named,
        ),
    ];

    for (case, arn, keys) in rows {
        let (status, body) = untag_in(&router, REGION, &arn, &keys).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
    }
}
