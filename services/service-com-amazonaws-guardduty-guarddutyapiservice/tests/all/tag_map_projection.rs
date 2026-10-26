// SPDX-License-Identifier: CC0-1.0

//! The `Tags` projection every read of this service shares, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A read of a taggable GuardDuty resource ALWAYS reports its `tags` member, and
//!    it carries exactly the tags the resource holds: an EMPTY map for a resource
//!    nothing tagged, never an absent member. It is asserted for every reader that
//!    projects a `TagMap`, over a table of tag maps, so no one reader can answer the
//!    untagged case its own way.
//! 2. The `len 1..200` the model puts on `TagMap` (`smithy_query(shape="TagMap")`)
//!    bounds what a REQUEST may carry, not what a read reports: a create sending an
//!    empty map is still refused at the modelled boundary with HTTP 400, for every
//!    create that carries the member.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, new_router, published_rule_ids, Router};

/// A KMS key ARN for the publishing destination. Nothing is published, so the value
/// is only ever stored and reported back.
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// An IAM role ARN of the form `CreateMalwareProtectionPlan.Role` takes.
const ROLE: &str = "arn:aws:iam::000000000000:role/GuardDutyMalwareProtectionRole";

/// The `FindingCriteria` a filter create must carry.
fn criteria() -> Value {
    json!({"criterion": {"severity": {"greaterThanOrEqual": 4}}})
}

/// `pairs` as a request/response `TagMap`.
fn tag_map(pairs: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

/// A string of `len` characters a `TagKey` admits (its pattern is
/// `^(?!aws:)[a-zA-Z+-=._:/]+$`).
fn key_of(len: usize) -> String {
    "k".repeat(len)
}

/// A string of `len` characters, for a `TagValue` (which carries no pattern).
fn value_of(len: usize) -> String {
    "v".repeat(len)
}

/// The tag maps a resource is created with, and which every reader must report back.
///
/// The rows differ along what the rule generalises over: the COUNT of entries (none
/// at all, one, several), the key ALPHABET and LENGTH and the value LENGTH — the
/// key is `len 1..128` with the pattern above, the value `len 0..256`, both
/// published bounds included (`smithy_query(query="shape", shape="TagMap")`). `None`
/// is a resource created with no `tags` member at all.
fn tag_maps() -> Vec<(String, Option<Value>)> {
    vec![
        ("no tags member at all".to_string(), None),
        ("a single tag".to_string(), Some(json!({"Env": "prod"}))),
        (
            "four tags: punctuation keys and the empty value a TagValue may be".to_string(),
            Some(json!({
                "Env": "prod",
                "Team/Owner": "security",
                "cost-center": "4711",
                "Reviewed": "",
            })),
        ),
        (
            "a one-character key, the low end of the key's length".to_string(),
            Some(tag_map(&[(key_of(1), value_of(1))])),
        ),
        (
            "a key and a value at their published maxima, 128 and 256 characters".to_string(),
            Some(tag_map(&[(key_of(128), value_of(256))])),
        ),
    ]
}

/// `body` with the row's `tags` member, or `body` untouched for a row that names
/// none — the two inputs the rule has to answer the same way.
fn with_tags(mut body: Value, tags: Option<&Value>) -> Value {
    if let (Some(object), Some(tags)) = (body.as_object_mut(), tags) {
        object.insert("tags".to_string(), tags.clone());
    }
    body
}

/// Create a resource and return the id its create minted under `id_member`.
async fn minted(router: &Router, path: &str, body: Value, id_member: &str) -> String {
    let (status, got) = call(router, "POST", path, Some(body)).await;
    assert_eq!(
        status, 200,
        "create {path:?}: expected 200, got {status} {got:?}"
    );
    got[id_member]
        .as_str()
        .unwrap_or_else(|| panic!("create {path:?}: expected a {id_member}, got {got:?}"))
        .to_string()
}

/// One reader of a taggable resource: the operation it belongs to, and the URI its
/// own read is bound to.
struct Reader {
    operation: &'static str,
    path: String,
}

/// Create one resource of every class this service reports tags for, each carrying
/// `tags`, and return the read of each.
async fn readers(router: &Router, tags: Option<&Value>) -> Vec<Reader> {
    let detector = minted(
        router,
        "/detector",
        with_tags(json!({"enable": true}), tags),
        "detectorId",
    )
    .await;
    let base = format!("/detector/{detector}");

    // The lists and the entity sets share the create shape; only the collection they
    // land in differs.
    let list_body = |name: &str| {
        with_tags(
            json!({
                "name": name,
                "format": "TXT",
                "location": "s3://guardduty-lists/list.txt",
                "activate": false,
            }),
            tags,
        )
    };

    let filter = minted(
        router,
        &format!("{base}/filter"),
        with_tags(
            json!({"name": "tag-projection", "findingCriteria": criteria()}),
            tags,
        ),
        "name",
    )
    .await;
    let ip_set = minted(
        router,
        &format!("{base}/ipset"),
        list_body("tag-projection-ip-set"),
        "ipSetId",
    )
    .await;
    let threat_intel_set = minted(
        router,
        &format!("{base}/threatintelset"),
        list_body("tag-projection-threat-intel-set"),
        "threatIntelSetId",
    )
    .await;
    let trusted_entity_set = minted(
        router,
        &format!("{base}/trustedentityset"),
        list_body("tag-projection-trusted-set"),
        "trustedEntitySetId",
    )
    .await;
    let threat_entity_set = minted(
        router,
        &format!("{base}/threatentityset"),
        list_body("tag-projection-threat-set"),
        "threatEntitySetId",
    )
    .await;
    let destination = minted(
        router,
        &format!("{base}/publishingDestination"),
        with_tags(
            json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            tags,
        ),
        "destinationId",
    )
    .await;
    let plan = minted(
        router,
        "/malware-protection-plan",
        with_tags(
            json!({
                "role": ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": "guardduty-tag-projection"}},
            }),
            tags,
        ),
        "malwareProtectionPlanId",
    )
    .await;
    let (rule_id, association) = associated_rule(router, tags).await;

    vec![
        Reader {
            operation: "GetDetector",
            path: base.clone(),
        },
        Reader {
            operation: "GetFilter",
            path: format!("{base}/filter/{filter}"),
        },
        Reader {
            operation: "GetIPSet",
            path: format!("{base}/ipset/{ip_set}"),
        },
        Reader {
            operation: "GetThreatIntelSet",
            path: format!("{base}/threatintelset/{threat_intel_set}"),
        },
        Reader {
            operation: "GetTrustedEntitySet",
            path: format!("{base}/trustedentityset/{trusted_entity_set}"),
        },
        Reader {
            operation: "GetThreatEntitySet",
            path: format!("{base}/threatentityset/{threat_entity_set}"),
        },
        Reader {
            operation: "DescribePublishingDestination",
            path: format!("{base}/publishingDestination/{destination}"),
        },
        Reader {
            operation: "GetMalwareProtectionPlan",
            path: format!("/malware-protection-plan/{plan}"),
        },
        Reader {
            operation: "GetCustomDetectionRuleAssociation",
            path: format!("/custom-detection-rule/rule/{rule_id}/association/{association}"),
        },
    ]
}

/// Associate the account with the first published custom detection rule, carrying
/// `tags`, and return the rule and the minted association id. The id is nested under
/// `ruleAssociation`, so it is minted here rather than through [`minted`].
async fn associated_rule(router: &Router, tags: Option<&Value>) -> (String, String) {
    let rule_id = published_rule_ids(router)
        .await
        .into_iter()
        .next()
        .expect("the catalogue publishes at least one rule");
    let (status, got) = call(
        router,
        "POST",
        "/custom-detection-rule/association",
        Some(with_tags(
            json!({"ruleId": rule_id, "mode": "DRY_RUN"}),
            tags,
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-association: expected 200, got {status} {got:?}"
    );
    let association = got["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-association: expected an associationId, got {got:?}"))
        .to_string();
    (rule_id, association)
}

/// The `tags` member a read reports, as the body carries it — `Value::Null` when the
/// member is absent altogether, which is what the rule forbids.
async fn reported_tags(router: &Router, reader: &Reader, case: &str) -> Value {
    let (status, body) = call(router, "GET", &reader.path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?} via {}: expected 200, got {status} {body:?}",
        reader.operation
    );
    body.get("tags").cloned().unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Rule 1: every reader reports the map the resource carries, empty when it
// carries none.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_read_reports_the_tag_map_the_resource_carries() {
    for (case, tags) in tag_maps() {
        // A fresh store per row: each row creates one resource of every class, and
        // several of those are one per account and Region.
        let router = new_router();
        let expected = tags.clone().unwrap_or_else(|| json!({}));
        for reader in readers(&router, tags.as_ref()).await {
            let got = reported_tags(&router, &reader, &case).await;
            assert_eq!(
                got, expected,
                "case {case:?} via {}: expected the tag map {expected:?}, got {got:?}",
                reader.operation
            );
        }
    }
}

/// A read reports the same map however often it is called: the projection is a
/// report of what the resource holds, not something a read consumes.
#[tokio::test]
async fn reading_twice_reports_the_same_tag_map() {
    let router = new_router();
    let tags = json!({"Env": "prod", "Team/Owner": "security"});

    for reader in readers(&router, Some(&tags)).await {
        let first = reported_tags(&router, &reader, "first read").await;
        let second = reported_tags(&router, &reader, "second read").await;
        assert_eq!(
            first, tags,
            "case \"first read\" via {}: expected the created tag map {tags:?}, got {first:?}",
            reader.operation
        );
        assert_eq!(
            second, first,
            "case \"second read\" via {}: a read must report the same map, got {second:?} after {first:?}",
            reader.operation
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the modelled minimum still bounds the way IN.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_carrying_an_empty_tag_map_is_still_refused() {
    let router = new_router();
    let detector = minted(&router, "/detector", json!({"enable": true}), "detectorId").await;
    let base = format!("/detector/{detector}");
    let rule_id = published_rule_ids(&router)
        .await
        .into_iter()
        .next()
        .expect("the catalogue publishes at least one rule");
    let list_body = |name: &str| {
        json!({
            "name": name,
            "format": "TXT",
            "location": "s3://guardduty-lists/list.txt",
            "activate": false,
            "tags": {},
        })
    };

    let rows: Vec<(&str, String, Value)> = vec![
        (
            "CreateDetector",
            "/detector".to_string(),
            json!({"enable": true, "tags": {}}),
        ),
        (
            "CreateFilter",
            format!("{base}/filter"),
            json!({"name": "empty-tag-map", "findingCriteria": criteria(), "tags": {}}),
        ),
        (
            "CreateIPSet",
            format!("{base}/ipset"),
            list_body("empty-tag-map-ip-set"),
        ),
        (
            "CreateThreatIntelSet",
            format!("{base}/threatintelset"),
            list_body("empty-tag-map-threat-intel-set"),
        ),
        (
            "CreateTrustedEntitySet",
            format!("{base}/trustedentityset"),
            list_body("empty-tag-map-trusted-set"),
        ),
        (
            "CreateThreatEntitySet",
            format!("{base}/threatentityset"),
            list_body("empty-tag-map-threat-set"),
        ),
        (
            "CreatePublishingDestination",
            format!("{base}/publishingDestination"),
            json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings",
                    "kmsKeyArn": KEY_ARN,
                },
                "tags": {},
            }),
        ),
        (
            "CreateMalwareProtectionPlan",
            "/malware-protection-plan".to_string(),
            json!({
                "role": ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": "guardduty-empty-tag-map"}},
                "tags": {},
            }),
        ),
        (
            "CreateCustomDetectionRuleAssociation",
            "/custom-detection-rule/association".to_string(),
            json!({"ruleId": rule_id, "mode": "DRY_RUN", "tags": {}}),
        ),
    ];

    for (case, path, body) in rows {
        let (status, got) = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an empty tag map is below the published minimum of one entry, \
             so the create must be refused, got {status} {got:?}"
        );
    }
}
