// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTagsForResource`.
//!
//! The rule asserted here, one row per input class rather than one recorded request:
//! `ListTagsForResource` reports the tags PERSISTED on the canary or group whose whole
//! ARN the request names — every key and value as stored, and no `Tags` member at all
//! when the resource carries none (`TagMap` is modelled `length 1..50`, so an empty map
//! is not a legal value) — and refuses any other well-formed `ResourceArn` with the
//! modelled `NotFoundException`, 404, carrying one fixed sentence that names neither the
//! resource kind nor the ARN from the request.
//!
//! `ResourceArn`'s pattern
//! (`^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:(canary|group):[0-9a-z_\-]+$`, from
//! `smithy_query(query="shape",shape="ResourceArn")`) is what makes the ARN say which
//! collection it resolves against, so both kinds are exercised, and the partition, the
//! Region, the account and the resource kind are all part of a resource's identity: a row
//! that changes any one of them resolves to nothing rather than to the local resource
//! that shares the rest.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite already defines.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::canary_lifecycle::{create_body, Answer, Synthetics};

/// `TagMap` is modelled `length 1..50` — "You can associate as many as 50 tags with a
/// canary or group" (`smithy_query(query="shape",shape="TagMap")`).
const MAX_TAGS: usize = 50;

/// `TagKey` is modelled `length 1..128`
/// (`smithy_query(query="shape",shape="TagKey")`).
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue` is modelled `length 0..256`
/// (`smithy_query(query="shape",shape="TagValue")`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// The fixed sentence a tag lookup that misses answers with: one sentence whatever the
/// ARN asked about, carrying nothing from the request.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// Which collection a row's resource lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Canary,
    Group,
}

/// One resource to tag at creation and then read the tags of.
struct Row {
    case: &'static str,
    kind: Kind,
    /// The canary name, or the group name — the identity the create is addressed by.
    name: String,
    /// The `Tags` map sent to the create, or `None` when the member is omitted.
    tags: Option<Value>,
}

/// `MAX_TAGS` tags, the most a resource may carry.
///
/// `TagKey` is `length 1..128, pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` (the `+-=` range spans
/// `+ , - . / 0-9 : ; < =`, so digits are in the alphabet) and `TagValue` is
/// `length 0..256`, so these keys are legal and the values stay inside the bound.
fn max_tags() -> Value {
    let mut map = Map::new();
    for index in 0..MAX_TAGS {
        map.insert(format!("key{index}"), json!(format!("value{index}")));
    }
    Value::Object(map)
}

/// A tag at both modelled extremes: the longest legal key and the longest legal value.
fn extreme_tags() -> Value {
    let mut map = Map::new();
    map.insert(
        "k".repeat(MAX_TAG_KEY_LEN),
        json!("v".repeat(MAX_TAG_VALUE_LEN)),
    );
    Value::Object(map)
}

/// The input classes the rule generalises over: resource kind, tag count (none, one, a
/// few, the modelled maximum), key form (plain, punctuated, maximum length) and value
/// form (plain, empty, maximum length).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "canary-untagged",
            kind: Kind::Canary,
            name: "untagged-probe".to_string(),
            tags: None,
        },
        Row {
            case: "canary-one-tag",
            kind: Kind::Canary,
            name: "single-tagged-probe".to_string(),
            tags: Some(json!({ "env": "prod" })),
        },
        Row {
            case: "canary-three-tags-punctuated-keys",
            kind: Kind::Canary,
            name: "probe_3".to_string(),
            tags: Some(json!({
                "owner/service": "synthetics",
                "cost.center": "4711",
                "tier:level": "gold",
            })),
        },
        Row {
            // `TagValue` is `length 0..256`: an empty value is legal and must come back.
            case: "canary-empty-tag-value",
            kind: Kind::Canary,
            name: "empty-value-probe".to_string(),
            tags: Some(json!({ "a.b_c:d": "" })),
        },
        Row {
            case: "canary-max-length-key-and-value",
            kind: Kind::Canary,
            name: "extreme-probe".to_string(),
            tags: Some(extreme_tags()),
        },
        Row {
            case: "canary-max-tags",
            kind: Kind::Canary,
            name: "fifty-tagged-probe".to_string(),
            tags: Some(max_tags()),
        },
        Row {
            case: "group-untagged",
            kind: Kind::Group,
            name: "untagged-group".to_string(),
            tags: None,
        },
        Row {
            case: "group-one-tag",
            kind: Kind::Group,
            name: "single-tagged-group".to_string(),
            tags: Some(json!({ "team": "observability" })),
        },
        Row {
            // "The name for the group. It can include any Unicode characters."
            // (`smithy_query(query="doc",shape="CreateGroupRequest",member="Name")`) — the
            // ARN carries the minted ID, so a name a URI cannot hold is still addressable.
            case: "group-unicode-name-two-tags",
            kind: Kind::Group,
            name: "grupo-café-群組".to_string(),
            tags: Some(json!({ "team": "observabilité", "env": "staging" })),
        },
        Row {
            case: "group-max-tags",
            kind: Kind::Group,
            name: "fifty-tagged-group".to_string(),
            tags: Some(max_tags()),
        },
    ]
}

/// Assert a 200, naming the case on failure.
fn expect_ok(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
}

/// Create `row`'s resource and answer its ARN.
///
/// A group's ARN is reported by the create (`Group$Arn`); the modelled `Canary` structure
/// carries no ARN member, so a canary's is the `ResourceArn` form its own documentation
/// states — `arn:aws:synthetics:Region:account-id:canary:canary-name`
/// (`smithy_query(query="doc",shape="ListTagsForResourceRequest",member="ResourceArn")`).
async fn seed(aws: &Synthetics, row: &Row, region: &str, account: &str) -> String {
    match row.kind {
        Kind::Canary => {
            let mut body = create_body(&row.name);
            if let Some(tags) = row.tags.clone() {
                body["Tags"] = tags;
            }
            let created = aws.create(body).await;
            expect_ok(row.case, &created);
            format!("arn:aws:synthetics:{region}:{account}:canary:{}", row.name)
        }
        Kind::Group => {
            let mut body = Map::new();
            body.insert("Name".to_string(), json!(row.name));
            if let Some(tags) = row.tags.clone() {
                body.insert("Tags".to_string(), tags);
            }
            let created = aws.call("POST", "/group", Some(Value::Object(body))).await;
            expect_ok(row.case, &created);
            created.string_at("Group.Arn")
        }
    }
}

/// `GET /tags/{ResourceArn}` — the `ListTagsForResource` route
/// (`smithy_query(query="operation",operation="ListTagsForResource")`).
async fn list_tags(aws: &Synthetics, resource_arn: &str) -> Answer {
    aws.call("GET", &format!("/tags/{resource_arn}"), None).await
}

/// The resource's own `Tags` member, read back through the implemented reader for its
/// kind, so what `ListTagsForResource` reports is checked against the stored record
/// rather than only against itself.
async fn tags_on_the_resource(aws: &Synthetics, row: &Row, arn: &str) -> Value {
    let read = match row.kind {
        Kind::Canary => aws.get(&row.name).await,
        Kind::Group => aws.call("GET", &format!("/group/{arn}"), None).await,
    };
    expect_ok(row.case, &read);
    match row.kind {
        Kind::Canary => read.at("Canary.Tags").clone(),
        Kind::Group => read.at("Group.Tags").clone(),
    }
}

/// Every input class: the tags reported are the tags persisted, an untagged resource
/// reports no map at all, and the read is repeatable — the same ARN answers the same
/// thing twice, because listing tags changes nothing.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_each_resource_was_created_with() {
    let region = "eu-west-2";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    for row in rows() {
        let case = row.case;
        let arn = seed(&aws, &row, region, account).await;

        let listed = list_tags(&aws, &arn).await;
        expect_ok(case, &listed);
        let reported = listed.at("Tags").clone();

        match row.tags.as_ref() {
            Some(expected) => {
                assert_eq!(
                    &reported, expected,
                    "case {case}: expected the tags the resource was created with, got \
                     {reported:?}"
                );
                let expected_count = expected.as_object().map_or(0, Map::len);
                let reported_count = reported.as_object().map_or(0, Map::len);
                assert_eq!(
                    reported_count, expected_count,
                    "case {case}: expected {expected_count} tags, got {reported_count} \
                     ({reported:?})"
                );
            }
            // `TagMap` is modelled `length 1..50`, so there is no empty map to report.
            None => assert!(
                reported.is_null(),
                "case {case}: expected no Tags on an untagged resource, got {reported:?}"
            ),
        }

        // The same tags hang off the resource itself: one stored field, two readers.
        let on_resource = tags_on_the_resource(&aws, &row, &arn).await;
        assert_eq!(
            reported, on_resource,
            "case {case}: expected ListTagsForResource to agree with the resource's own \
             Tags member, got {reported:?} and {on_resource:?}"
        );

        // A read must not change what a later caller sees.
        let again = list_tags(&aws, &arn).await;
        expect_ok(case, &again);
        assert_eq!(
            again.at("Tags"),
            &reported,
            "case {case}: expected a repeated read to report the same tags, got {:?} then \
             {:?}",
            reported,
            again.at("Tags")
        );
    }
}

/// Well-formed `ResourceArn`s that name no resource this store holds, spanning every
/// field of an ARN that is part of a resource's identity.
///
/// `existing_canary` and `existing_group` are the ARNs of resources that DO exist, so
/// each row is one field away from a hit.
fn missing_arns(
    region: &str,
    account: &str,
    existing_canary: &str,
    existing_group: &str,
) -> Vec<(String, String)> {
    let group_id = existing_group
        .rsplit(':')
        .next()
        .expect("a group ARN ends in its id")
        .to_string();
    let canary_name = existing_canary
        .rsplit(':')
        .next()
        .expect("a canary ARN ends in the canary name")
        .to_string();
    vec![
        (
            "unknown-canary-name".to_string(),
            format!("arn:aws:synthetics:{region}:{account}:canary:no-such-canary"),
        ),
        (
            "unknown-group-id".to_string(),
            format!("arn:aws:synthetics:{region}:{account}:group:0123456789ab"),
        ),
        (
            // One character long, and 255 (the modelled `CanaryName` maximum): the
            // shortest and longest resource segments a caller can ask about.
            "single-character-canary-name".to_string(),
            format!("arn:aws:synthetics:{region}:{account}:canary:x"),
        ),
        (
            "max-length-canary-name".to_string(),
            format!(
                "arn:aws:synthetics:{region}:{account}:canary:{}",
                "c".repeat(255)
            ),
        ),
        (
            // Same name, another Region: a resource lives in exactly one.
            "existing-canary-wrong-region".to_string(),
            format!("arn:aws:synthetics:us-west-1:{account}:canary:{canary_name}"),
        ),
        (
            "existing-group-wrong-region".to_string(),
            format!("arn:aws:synthetics:ap-southeast-2:{account}:group:{group_id}"),
        ),
        (
            // Same name, another account.
            "existing-canary-wrong-account".to_string(),
            format!("arn:aws:synthetics:{region}:210987654321:canary:{canary_name}"),
        ),
        (
            // Same name, another partition (the pattern admits `aws[a-zA-Z-]*`).
            "existing-canary-wrong-partition".to_string(),
            format!("arn:aws-cn:synthetics:{region}:{account}:canary:{canary_name}"),
        ),
        (
            // The right resource segment under the WRONG kind: a group's id read as a
            // canary name, and a canary's name read as a group id. Tags do not leak
            // across the two collections.
            "existing-group-id-as-a-canary".to_string(),
            format!("arn:aws:synthetics:{region}:{account}:canary:{group_id}"),
        ),
        (
            "existing-canary-name-as-a-group".to_string(),
            format!("arn:aws:synthetics:{region}:{account}:group:{canary_name}"),
        ),
    ]
}

/// Every unresolvable ARN is refused with the modelled 404 `NotFoundException`, carrying
/// one fixed sentence that names nothing from the request — and the resources that DO
/// exist are untouched by all of it.
#[tokio::test]
async fn an_unresolvable_resource_arn_is_refused_with_one_fixed_request_free_message() {
    let region = "eu-central-1";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    // Resources that exist, so the near-miss rows are answered by a store that holds a
    // similar ARN rather than by an empty one.
    let canary_row = Row {
        case: "seed-canary",
        kind: Kind::Canary,
        name: "live-probe".to_string(),
        tags: Some(json!({ "env": "prod", "owner/service": "synthetics" })),
    };
    let group_row = Row {
        case: "seed-group",
        kind: Kind::Group,
        name: "live-group".to_string(),
        tags: Some(json!({ "team": "observability" })),
    };
    let canary_arn = seed(&aws, &canary_row, region, account).await;
    let group_arn = seed(&aws, &group_row, region, account).await;

    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (case, arn) in missing_arns(region, account, &canary_arn, &group_arn) {
        let answer = list_tags(&aws, &arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error_code
        );
        let message = answer
            .at("Message")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a Message on the error, got {:?}",
                    answer.body
                )
            })
            .to_string();
        assert!(
            !message.to_lowercase().contains(&arn.to_lowercase()),
            "case {case}: expected a Message that does not name the requested ARN {arn:?}, \
             got {message:?}"
        );
        assert_eq!(
            message, RESOURCE_NOT_FOUND,
            "case {case}: expected the fixed missing-resource sentence, got {message:?}"
        );
        messages.insert(message);
    }
    // The invariant the rule really is: ONE message over every ARN and both kinds.
    assert_eq!(
        messages,
        BTreeSet::from([RESOURCE_NOT_FOUND.to_string()]),
        "case request-independence: expected one fixed message across every ARN, got \
         {messages:?}"
    );

    // Nothing above resolved to, or disturbed, the resources that do exist.
    for (row, arn) in [(&canary_row, &canary_arn), (&group_row, &group_arn)] {
        let listed = list_tags(&aws, arn).await;
        expect_ok(row.case, &listed);
        assert_eq!(
            listed.at("Tags"),
            row.tags.as_ref().expect("both seeds are tagged"),
            "case {}: expected the seeded tags to survive the misses, got {:?}",
            row.case,
            listed.at("Tags")
        );
    }
}
