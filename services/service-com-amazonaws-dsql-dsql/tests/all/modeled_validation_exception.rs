// SPDX-License-Identifier: CC0-1.0

//! The BODY of every refusal this service's handlers answer with
//! `ValidationException`.
//!
//! RULE under test: a request one of this service's handlers refuses as invalid
//! input is answered with the SERVICE'S OWN modelled `ValidationException` — HTTP
//! 400, wire code `ValidationException`, and a body carrying its modelled members:
//! a non-empty `message` and the `@required` `reason` drawn from
//! `ValidationExceptionReason` — never the synthetic `smithy.framework`
//! `ValidationException`, which serializes `message` alone. Where the refusal is
//! about one input member, `fieldList` names that member too.
//!
//! The table spans the whole class of handler refusals rather than one recorded
//! request: five operations (`ListClusters`, `ListStreams`, `TagResource`,
//! `ListTagsForResource`, `UntagResource`, `DeleteCluster`, `PutClusterPolicy`),
//! and for each the forms its invalid value takes — a cursor that is not hex and
//! one that is hex of something that is not an identifier, an ARN of another
//! service and a `dsql` ARN whose resource field is unusable, a protected cluster,
//! a policy document that is not a policy.
//!
//! Every case here reaches a HANDLER: a value the model itself constrains (an ARN
//! failing `Arn`'s `^arn:.+$`, a `max-results` outside its `@range`) is refused by
//! the boundary validator BEFORE any handler runs, and that refusal is the
//! framework's own — a different code path, asserted elsewhere
//! (`tests/all/cluster_lifecycle.rs`, `tests/all/tag_resource.rs`).

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::harness::{percent_encode_label, wait_for_the_clock, Answer, Dsql};

/// The `ValidationExceptionReason` a member-value refusal carries. Source: the
/// service's own model, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")` — one of `unknownOperation`, `cannotParse`,
/// `fieldValidationFailed`, `deletionProtectionEnabled`, `other`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// One refused request: how the case is named, what came back, and — when the
/// refusal is about a single input member — the field `fieldList` must name: the name
/// the caller spelled the value with, which for a member bound to a query parameter
/// is that parameter's name.
struct Refusal {
    case: String,
    answer: Answer,
    member: Option<&'static str>,
}

/// Cursor values this service never issued, spanning both ways a `nextToken` can
/// fail to be one: not hex at all, and hex of something that is not a
/// `^[a-z0-9]{26}$` identifier (wrong case, too short, too long, not UTF-8).
fn unissued_tokens() -> Vec<String> {
    let mut tokens: Vec<String> = ["not-a-cursor", "zzzz", "ff00ffzz", "0"]
        .into_iter()
        .map(str::to_string)
        .collect();
    tokens.extend([
        ::hex::encode("hello"),
        ::hex::encode("AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode([0xff, 0xfe]),
    ]);
    tokens
}

/// ARNs that satisfy `Arn`'s modelled `^arn:.+$` — so they reach the handler — and
/// name no Aurora DSQL resource at all, as `state::parse_resource_arn` reads one.
fn arns_naming_no_dsql_resource() -> Vec<(&'static str, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        (
            "another service's resource",
            format!("arn:aws:kinesis:us-east-1:{account}:stream/orders"),
        ),
        ("an S3 ARN", "arn:aws:s3:::my-bucket/key".to_string()),
        (
            "a dsql ARN with an empty resource field",
            format!("arn:aws:dsql:us-east-1:{account}:"),
        ),
        (
            "a dsql ARN whose resource field has no type",
            format!("arn:aws:dsql:us-east-1:{account}:cluster"),
        ),
        (
            "a dsql ARN whose resource field has no id",
            format!("arn:aws:dsql:us-east-1:{account}:cluster/"),
        ),
        ("too few ARN fields", "arn:aws:dsql:us-east-1".to_string()),
    ]
}

/// `DELETE /tags/{resourceArn}?tagKeys=env` — `UntagResource` is told from the other
/// operations bound to that path by its `@required` `@httpQuery` `tagKeys`.
async fn untag(dsql: &Dsql, arn: &str) -> Answer {
    dsql.call(
        "DELETE",
        &format!("/tags/{}?tagKeys=env", percent_encode_label(arn)),
        None,
    )
    .await
}

/// Every handler refusal of this service, as one table.
async fn handler_refusals(dsql: &Dsql) -> Vec<Refusal> {
    let mut refusals = Vec::new();

    // A real cluster, so the only thing wrong with the paging and tagging calls
    // below is the value under test.
    let cluster = dsql.create_settled().await;

    for token in unissued_tokens() {
        refusals.push(Refusal {
            case: format!("ListClusters next-token={token:?}"),
            answer: dsql.list(&format!("?max-results=10&next-token={token}")).await,
            member: Some("next-token"),
        });
        refusals.push(Refusal {
            case: format!("ListStreams next-token={token:?}"),
            answer: dsql
                .list_streams(&cluster, &format!("?max-results=10&next-token={token}"))
                .await,
            member: Some("next-token"),
        });
    }

    for (name, arn) in arns_naming_no_dsql_resource() {
        refusals.push(Refusal {
            case: format!("TagResource resourceArn={name}"),
            answer: dsql.tag_resource(&arn, json!({"tags": {"env": "dev"}})).await,
            member: Some("resourceArn"),
        });
        refusals.push(Refusal {
            case: format!("ListTagsForResource resourceArn={name}"),
            answer: dsql.list_tags_for_resource(&arn).await,
            member: Some("resourceArn"),
        });
        refusals.push(Refusal {
            case: format!("UntagResource resourceArn={name}"),
            answer: untag(dsql, &arn).await,
            member: Some("resourceArn"),
        });
    }

    // A refusal about the STATE of the addressed resource rather than about one
    // member's value: it carries no `fieldList`, but the same `reason` contract.
    let protected = dsql.create(json!({"deletionProtectionEnabled": true})).await;
    let protected = protected.str("identifier");
    wait_for_the_clock().await;
    refusals.push(Refusal {
        case: "DeleteCluster against a deletion-protected cluster".to_string(),
        answer: dsql.delete(&protected).await,
        member: None,
    });

    for document in ["not json at all", "[]", "\"a string\"", "7"] {
        refusals.push(Refusal {
            case: format!("PutClusterPolicy policy={document:?}"),
            answer: dsql
                .put_policy(&cluster, json!({"policy": document}))
                .await,
            member: None,
        });
    }

    refusals
}

/// RULE: every handler refusal is the modelled `ValidationException` — 400, wire
/// code `ValidationException`, a non-empty `message` and the `@required` `reason`
/// from the modelled enum — and a member-value refusal also names the member in
/// `fieldList`.
#[tokio::test]
async fn every_handler_refusal_carries_the_modeled_validation_exception_body() {
    let dsql = Dsql::new();
    let refusals = handler_refusals(&dsql).await;
    assert!(
        refusals.len() > 10,
        "the table must span the service's refusals, got {} cases",
        refusals.len()
    );

    for Refusal {
        case,
        answer,
        member,
    } in refusals
    {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an invalid-input refusal is HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: the wire code must be ValidationException, got {:?}",
            answer.error
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: the modelled @required message must be filled, got {:?}",
            answer.body
        );

        let reason = answer.str("reason");
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason.as_str()),
            "case {case:?}: the modelled @required reason must be one of \
             {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {:?}",
            answer.body
        );

        let Some(member) = member else {
            continue;
        };
        assert_eq!(
            reason, REASON_FIELD_VALIDATION_FAILED,
            "case {case:?}: a member-value refusal reports {REASON_FIELD_VALIDATION_FAILED}, \
             got {reason:?}"
        );
        let field_list = answer.get("fieldList");
        let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
        assert!(
            fields.iter().any(|field| {
                field.get("name").and_then(Value::as_str) == Some(member)
                    && field
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|message| !message.is_empty())
            }),
            "case {case:?}: fieldList must name {member} with a message saying why, \
             got {field_list:?}"
        );
    }
}

/// RULE: only a cursor this service did not issue is refused — a `nextToken` a
/// previous page returned is still served, for both paginated readers. Guards the
/// refusal above against being answered for a legitimate cursor.
#[tokio::test]
async fn a_token_this_service_issued_is_still_served() {
    let dsql = Dsql::new();
    let clusters = dsql.create_settled_many(3).await;
    assert_eq!(clusters.len(), 3, "the fixture creates three clusters");

    let first = dsql.list("?max-results=1").await;
    assert_eq!(
        first.status, 200,
        "case the first page: got {} {:?}",
        first.status, first.body
    );
    let token = first.str("nextToken");
    assert!(
        !token.is_empty(),
        "case the first page: a page smaller than the store must emit a nextToken, got {:?}",
        first.body
    );

    let second = dsql
        .list(&format!("?max-results=1&next-token={token}"))
        .await;
    assert_eq!(
        second.status, 200,
        "case the issued token: ListClusters must serve it, got {} {:?}",
        second.status, second.body
    );
    assert_ne!(
        second.get("clusters"),
        first.get("clusters"),
        "case the issued token: the second page must differ from the first, got {:?}",
        second.body
    );
}
