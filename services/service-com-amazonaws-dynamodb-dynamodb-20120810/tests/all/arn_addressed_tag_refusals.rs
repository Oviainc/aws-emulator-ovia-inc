// SPDX-License-Identifier: CC0-1.0

//! How the ARN-ADDRESSED operations — the tagging surface (`TagResource`,
//! `UntagResource`, `ListTagsOfResource`) and the resource-policy surface
//! (`PutResourcePolicy`, `GetResourcePolicy`, `DeleteResourcePolicy`) — classify and
//! word a refusal, and what a request's tag list may hold.
//!
//! Two rules, each asserted over a SET of inputs rather than one example:
//!
//! 1. A `ResourceArn` is VALIDATED before any resource is looked up. It must be an ARN
//!    of this service naming a table and nothing deeper, and it must address a resource
//!    of the endpoint the request reached — the same partition, account and region the
//!    request is signed for. A value that is not (malformed, another service, a
//!    sub-resource, another region, another account) names no resource whose existence
//!    could be checked, so it is an input failure: HTTP 400 `ValidationException`,
//!    worded over the value the request carried. Only an in-scope ARN reaches the
//!    lookup, where a table that is not there is HTTP 400 `ResourceNotFoundException`
//!    naming the `ResourceArn` MEMBER and echoing what was sent — never a table name
//!    derived from it, which no member of such a request carries.
//! 2. A tag key identifies at most one tag on a resource ("Each DynamoDB table can only
//!    have up to one tag with the same key" — the `Tag$Key` doc), so a request whose
//!    tag list repeats a key is refused with HTTP 400 `ValidationException` naming the
//!    repeated key, wherever a tag list is accepted (`TagResource$Tags`,
//!    `CreateTable$Tags`). Keys are compared as sent: a tag key is case sensitive, so
//!    `Env` and `env` are two keys and not one repeated.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator, error_code, message, REGION};

/// The account every ARN this emulator mints carries, and so the only account an
/// ARN-addressed request can address: the canonical emulator account.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A second region, for the scope half of rule 1: the same ARN is in scope for the
/// endpoint it names and out of scope for any other.
const OTHER_REGION: &str = "eu-west-1";

/// A resource policy document `PutResourcePolicy` accepts, so a refusal of that
/// operation below is about the `ResourceArn` and never about the document.
fn policy(resource_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": resource_arn,
        }],
    })
    .to_string()
}

/// Every operation whose whole selector is a `ResourceArn` member, each with a body
/// that is otherwise valid (the members the model marks required on that input:
/// `TagResource$Tags`, `UntagResource$TagKeys`, `PutResourcePolicy$Policy`).
fn arn_addressed(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["env"]}),
        ),
        ("ListTagsOfResource", json!({"ResourceArn": resource_arn})),
        (
            "PutResourcePolicy",
            json!({"ResourceArn": resource_arn, "Policy": policy(resource_arn)}),
        ),
        ("GetResourcePolicy", json!({"ResourceArn": resource_arn})),
        ("DeleteResourcePolicy", json!({"ResourceArn": resource_arn})),
    ]
}

/// Create one table in `region` and return the ARN the service minted for it.
async fn create_table(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create {name} in {region}: expected HTTP 200, got {status} with {body}"
    );
    body["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The tag set `ListTagsOfResource` reports for `arn`, as `Key=Value` pairs in key
/// order, read once the tag writes before it have propagated.
///
/// A tag read is eventually consistent — "The application or removal of tags using
/// TagResource and UntagResource APIs is eventually consistent. ListTagsOfResource API
/// will only reflect the changes after a few seconds" (the `TagResource` doc) — so a
/// write becomes visible one `STATUS_TRANSITION` after the call that made it. These
/// refusal tests are about WHAT a call leaves stored, so they wait the span out first;
/// `tag_read_propagation` is the test that pins WHEN it appears.
async fn listed_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Vec<String> {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(50),
    )
    .await;
    let (status, body) = call_in(
        router,
        region,
        "ListTagsOfResource",
        json!({"ResourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case list {arn}: expected HTTP 200, got {status} with {body}"
    );
    let mut pairs: Vec<String> = body["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            format!(
                "{}={}",
                tag["Key"].as_str().unwrap_or("<missing>"),
                tag["Value"].as_str().unwrap_or("<missing>")
            )
        })
        .collect();
    pairs.sort();
    pairs
}

#[tokio::test]
async fn a_resource_arn_outside_this_endpoint_s_scope_is_an_input_refusal_naming_the_value() {
    let router = emulator();
    // One real table, so every refusal below is about the ARN and nothing else.
    let real = create_table(&router, REGION, "scoped-table").await;

    // (case, the submitted ResourceArn). The rows span the ways a value that IS an ARN
    // can fail to address a table of this endpoint: the ARN's shape (a missing or wrong
    // resource type, a deeper sub-resource, a table name outside the documented 3..=255
    // `[a-zA-Z0-9_.-]+` form) and its scope (another service, partition, account or
    // region). A value that is no ARN at all breaks a different rule and states that one;
    // `tests/all/required_member_refusals.rs` drives it.
    let cases: Vec<(&str, String)> = vec![
        ("an ARN with too few fields", format!("arn:aws:dynamodb:{REGION}:table/scoped-table")),
        (
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:scoped-table"),
        ),
        (
            "an ARN naming another resource type",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:backup/scoped-table"),
        ),
        (
            "an ARN with no resource type",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:scoped-table"),
        ),
        (
            "an index sub-resource ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/scoped-table/index/by-env"),
        ),
        (
            "a stream sub-resource ARN",
            format!(
                "arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/scoped-table/stream/\
                 2026-01-01T00:00:00.000"
            ),
        ),
        (
            "a trailing slash after the table name",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/scoped-table/"),
        ),
        (
            "a table name below the documented minimum length",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/ab"),
        ),
        (
            "a table name outside the documented pattern",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/not a name"),
        ),
        (
            "an ARN with no region",
            format!("arn:aws:dynamodb::{ACCOUNT}:table/scoped-table"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:dynamodb:{OTHER_REGION}:{ACCOUNT}:table/scoped-table"),
        ),
        (
            "an ARN of another account",
            format!("arn:aws:dynamodb:{REGION}:111111111111:table/scoped-table"),
        ),
        (
            "an ARN of another partition",
            format!("arn:aws-cn:dynamodb:{REGION}:{ACCOUNT}:table/scoped-table"),
        ),
    ];

    for (case, resource_arn) in &cases {
        for (operation, body) in arn_addressed(resource_arn) {
            let (status, answered) = call_in(&router, REGION, operation, body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case:?} on {operation}: expected 400 ValidationException for \
                 {resource_arn:?}, got {status} with {answered}"
            );
            assert_eq!(
                message(&answered),
                format!("Invalid TableArn: Invalid ResourceArn provided as input {resource_arn}"),
                "case {case:?} on {operation}: the refusal names the submitted value, got {}",
                message(&answered)
            );
        }
    }

    // The ARN of the real table is in scope for the endpoint it names, and out of scope
    // for any other: the scope is the REQUEST's, not something the store holds.
    let (status, body) = call_in(
        &router,
        REGION,
        "TagResource",
        json!({"ResourceArn": real, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {body}",
        "the table's own ARN in its own region"
    );
    for (operation, body) in arn_addressed(&real) {
        let (status, answered) = call_in(&router, OTHER_REGION, operation, body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {:?} on {operation}: expected 400 ValidationException, got {status} with \
             {answered}",
            "a table's ARN sent to another region's endpoint"
        );
    }

    // None of those refusals touched the table that does exist.
    let listed = listed_tags(&router, REGION, &real).await;
    assert_eq!(
        listed,
        vec!["env=prod".to_string()],
        "case {:?}: the stored set is untouched by every refusal, got {listed:?}",
        "the real table"
    );
}

#[tokio::test]
async fn an_in_scope_arn_naming_no_table_is_reported_by_the_surface_that_was_addressed() {
    let router = emulator();
    // One real table, so the store is not empty: these refusals are about the table the
    // ARN names and not about an empty region.
    create_table(&router, REGION, "present-table").await;

    // The rows differ along what the rule generalises over — the table name inside an
    // otherwise valid ARN, at the documented minimum length (3), with each character
    // class the name pattern allows, and at the documented maximum (255).
    let absent_names: Vec<String> = vec![
        "abc".to_string(),
        "absent-table".to_string(),
        "dot.ted.name".to_string(),
        "under_scored".to_string(),
        "MixedCase99".to_string(),
        "z".repeat(255),
    ];

    for name in &absent_names {
        let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}");
        for (operation, body) in arn_addressed(&resource_arn) {
            let (status, answered) = call_in(&router, REGION, operation, body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?} on {operation}: expected 400 ResourceNotFoundException, got \
                 {status} with {answered}"
            );
            let stated = message(&answered);
            // Each surface words its own refusal: the TAGGING surface names the
            // `ResourceArn` member it was addressed by and echoes what was sent, while
            // the resource-POLICY surface reports the resource as the Table it is, by the
            // bare name inside that ARN.
            if operation.ends_with("ResourcePolicy") {
                assert_eq!(
                    stated,
                    format!("Requested resource not found: Table: {name} not found"),
                    "case {name:?} on {operation}: the refusal names the missing table, got \
                     {stated:?}"
                );
            } else {
                assert_eq!(
                    stated,
                    format!(
                        "Requested resource not found: ResourceArn: {resource_arn} not found"
                    ),
                    "case {name:?} on {operation}: the refusal names the ResourceArn that was \
                     sent, got {stated:?}"
                );
                assert!(
                    !stated.contains("Table:"),
                    "case {name:?} on {operation}: a tagging refusal names no derived table \
                     name, got {stated:?}"
                );
            }
        }
    }

    // A table that WAS there and has been deleted is the same refusal: the ARN keeps
    // its form, the resource is gone.
    let doomed = create_table(&router, REGION, "doomed-table").await;
    // A table may only be deleted once it is out of `CREATING`, so wait out the
    // transition the clock owes it.
    std::thread::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(50),
    );
    let (status, body) = call_in(
        &router,
        REGION,
        "DeleteTable",
        json!({"TableName": "doomed-table"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {body}",
        "deleting the table whose ARN is then used"
    );
    std::thread::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(50),
    );
    let (status, answered) = call_in(
        &router,
        REGION,
        "ListTagsOfResource",
        json!({"ResourceArn": doomed}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ResourceNotFoundException"),
        "case {:?}: expected 400 ResourceNotFoundException, got {status} with {answered}",
        "a deleted table's ARN"
    );
    assert_eq!(
        message(&answered),
        format!("Requested resource not found: ResourceArn: {doomed} not found"),
        "case {:?}: the refusal names the submitted ARN, got {}",
        "a deleted table's ARN",
        message(&answered)
    );
}

/// One tag-list case: the list a request carries, and the key whose repeat makes it
/// invalid (`None` for a list AWS accepts).
struct TagListCase {
    name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    duplicate: Option<&'static str>,
}

#[tokio::test]
async fn a_tag_list_that_repeats_a_key_is_refused_wherever_a_tag_list_is_accepted() {
    let router = emulator();
    let arn = create_table(&router, REGION, "tag-list-table").await;

    // The rows span where the repeat falls, how many times a key repeats, whether the
    // repeated values agree, the key's case and its length, and the lists that are
    // NOT repeats (distinct keys, keys differing only in case, one tag, none at all).
    let cases: [TagListCase; 11] = [
        TagListCase {
            name: "no tags at all",
            tags: &[],
            duplicate: None,
        },
        TagListCase {
            name: "one tag",
            tags: &[("env", "prod")],
            duplicate: None,
        },
        TagListCase {
            name: "distinct keys",
            tags: &[("env", "prod"), ("owner", "platform"), ("tier", "gold")],
            duplicate: None,
        },
        TagListCase {
            name: "keys differing only in case are two keys",
            tags: &[("env", "prod"), ("Env", "staging"), ("ENV", "dev")],
            duplicate: None,
        },
        TagListCase {
            name: "a key repeated at the front of the list",
            tags: &[("env", "prod"), ("env", "staging"), ("owner", "platform")],
            duplicate: Some("env"),
        },
        TagListCase {
            name: "a key repeated at the end of the list",
            tags: &[("owner", "platform"), ("env", "prod"), ("env", "staging")],
            duplicate: Some("env"),
        },
        TagListCase {
            name: "a key repeated across the list",
            tags: &[("env", "prod"), ("owner", "platform"), ("env", "staging")],
            duplicate: Some("env"),
        },
        TagListCase {
            name: "a key repeated with the SAME value",
            tags: &[("env", "prod"), ("env", "prod")],
            duplicate: Some("env"),
        },
        TagListCase {
            name: "a key repeated three times",
            tags: &[("env", "a"), ("env", "b"), ("env", "c")],
            duplicate: Some("env"),
        },
        TagListCase {
            name: "two repeated keys: the first repeat reached is reported",
            tags: &[
                ("owner", "platform"),
                ("env", "prod"),
                ("owner", "sre"),
                ("env", "staging"),
            ],
            duplicate: Some("owner"),
        },
        TagListCase {
            name: "a repeated key at the modelled maximum key length",
            // `TagKeyString` is @length 1..128, so a 128-character key is a legal key
            // and repeating it is still a repeat.
            tags: &[("KKK", "a"), ("KKK", "b")],
            duplicate: Some("KKK"),
        },
    ];

    // The 128-character key the last row stands for, spelled once.
    let longest_key = "k".repeat(128);
    let spell = |key: &str| {
        if key == "KKK" {
            longest_key.clone()
        } else {
            key.to_string()
        }
    };

    for (index, case) in cases.iter().enumerate() {
        let tags = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({"Key": spell(key), "Value": value}))
                .collect(),
        );

        // TagResource: the write half of the tagging surface.
        let (status, body) = call_in(
            &router,
            REGION,
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tags}),
        )
        .await;
        match case.duplicate {
            Some(key) => {
                let key = spell(key);
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {:?} on TagResource: expected 400 ValidationException, got {status} \
                     with {body}",
                    case.name
                );
                assert_eq!(
                    message(&body),
                    format!("Duplicate Tag Keys provided as input: Duplicate Tag Key found {key}"),
                    "case {:?} on TagResource: the refusal names the repeated key, got {}",
                    case.name,
                    message(&body)
                );
                let listed = listed_tags(&router, REGION, &arn).await;
                assert!(
                    listed.is_empty(),
                    "case {:?} on TagResource: a refused write stores nothing, got {listed:?}",
                    case.name
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?} on TagResource: expected HTTP 200, got {status} with {body}",
                    case.name
                );
                let listed = listed_tags(&router, REGION, &arn).await;
                assert_eq!(
                    listed.len(),
                    case.tags.len(),
                    "case {:?} on TagResource: every tag of an accepted list is stored, got \
                     {listed:?}",
                    case.name
                );
                // Leave the resource empty for the next row.
                let keys: Vec<String> = case.tags.iter().map(|(key, _)| spell(key)).collect();
                let (status, body) = call_in(
                    &router,
                    REGION,
                    "UntagResource",
                    json!({"ResourceArn": arn, "TagKeys": keys}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected HTTP 200 clearing the set, got {status} with {body}",
                    case.name
                );
            }
        }

        // CreateTable takes the same `TagList`, so the same rule holds for it — and a
        // refused create stores no table.
        let table = format!("tag-list-create-{index}");
        let tags = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({"Key": spell(key), "Value": value}))
                .collect(),
        );
        let (status, body) = call_in(
            &router,
            REGION,
            "CreateTable",
            json!({
                "TableName": table,
                "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
                "Tags": tags,
            }),
        )
        .await;
        let (describe_status, described) = call_in(
            &router,
            REGION,
            "DescribeTable",
            json!({"TableName": table}),
        )
        .await;
        match case.duplicate {
            Some(key) => {
                let key = spell(key);
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {:?} on CreateTable: expected 400 ValidationException, got {status} \
                     with {body}",
                    case.name
                );
                assert_eq!(
                    message(&body),
                    format!("Duplicate Tag Keys provided as input: Duplicate Tag Key found {key}"),
                    "case {:?} on CreateTable: the refusal names the repeated key, got {}",
                    case.name,
                    message(&body)
                );
                assert_eq!(
                    (describe_status, error_code(&described).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {:?} on CreateTable: a refused create stores no table, got \
                     {describe_status} with {described}",
                    case.name
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?} on CreateTable: expected HTTP 200, got {status} with {body}",
                    case.name
                );
                assert_eq!(
                    describe_status, 200,
                    "case {:?} on CreateTable: the created table is readable, got \
                     {describe_status} with {described}",
                    case.name
                );
            }
        }
    }
}

#[tokio::test]
async fn a_tag_list_is_validated_before_the_resource_is_resolved() {
    let router = emulator();
    // A repeat is a fact of the REQUEST, so it is reported whatever the ARN names: the
    // caller is told to fix the list rather than to create the resource first.
    let absent = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/never-created");
    let (status, body) = call_in(
        &router,
        REGION,
        "TagResource",
        json!({
            "ResourceArn": absent,
            "Tags": [{"Key": "env", "Value": "a"}, {"Key": "env", "Value": "b"}],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {:?}: expected 400 ValidationException, got {status} with {body}",
        "a repeated key on an ARN naming no table"
    );
    assert_eq!(
        message(&body),
        "Duplicate Tag Keys provided as input: Duplicate Tag Key found env",
        "case {:?}: the refusal is about the list, got {}",
        "a repeated key on an ARN naming no table",
        message(&body)
    );
}
