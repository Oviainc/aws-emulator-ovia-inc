// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of the body every paginated reader of this service answers an
//! unissued `nextToken` with.
//!
//! RULE under test: a `nextToken` this service never issued is refused with the
//! service's OWN modelled `ValidationException` — HTTP 400, wire code
//! `ValidationException`, and a body whose members are that shape's members and
//! no others: a non-empty `message`, the `@required` `reason` drawn from
//! `ValidationExceptionReason`, and the `fieldList` naming the member. It is
//! never the synthetic `smithy.framework#ValidationException`, which serializes
//! `message` ALONE and so carries no `reason` for a client to branch on.
//!
//! Source for the member set and the enum: the service's own model,
//! `smithy_query(query="shape",
//! shape="ValidationException,ValidationExceptionReason,ValidationExceptionField")`
//! — `ValidationException` members `message` (`@required`), `reason`
//! (`@required`, `ValidationExceptionReason`) and `fieldList`.
//!
//! The table spans the class rather than one recorded request: BOTH paginated
//! readers (`ListClusters` over cluster order, `ListStreams` over a cluster's
//! stream order) crossed with every form a value can fail to be a cursor, and
//! crossed with whether the request also carried `max-results`. The presence
//! assertions (`reason` is filled, `fieldList` names the member) overlap
//! `tests/all/modeled_validation_exception.rs`; what this suite adds is the
//! ABSENCE assertion — no member outside the modelled shape, which is the only
//! thing that tells the two `ValidationException` shapes apart on the wire — and
//! the complement below, that a token the service DID issue is still served by
//! `ListStreams`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::harness::{absent_identifier, Answer, Dsql};

/// The `ValidationExceptionReason` a refusal about one member's value carries.
/// Source: `smithy_query(query="shape", shape="ValidationExceptionReason")` —
/// one of `unknownOperation`, `cannotParse`, `fieldValidationFailed`,
/// `deletionProtectionEnabled`, `other`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The field a cursor refusal names: the `@httpQuery` parameter the `nextToken`
/// member is bound to (`smithy_query(query="operation",
/// operation="ListClusters,ListStreams")`: `httpQuery: "next-token"`), which is the
/// name the caller spelled the value with.
const NEXT_TOKEN_FIELD: &str = "next-token";

/// The members an error body may carry: the modelled `ValidationException`'s
/// three, plus the restJson1 envelope's own `__type` discriminator and the
/// `Message` spelling the envelope is allowed to use for `message`. Anything
/// else means the serialized shape is not the modelled one.
const MODELED_BODY_MEMBERS: &[&str] = &["__type", "message", "Message", "reason", "fieldList"];

/// Values this service never issued as a cursor, spanning every form the failure
/// takes: not hex at all (so it cannot decode), hex of something that is not the
/// `^[a-z0-9]{26}$` identifier a cursor carries (wrong case, one character short,
/// one character long, not UTF-8 at all, the empty string), and — the form that is
/// well formed and still was never issued — the bare hex of such an identifier,
/// which carries no integrity check binding it to a walk of this service and so
/// names no page of either reader.
fn unissued_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("not hex", "not-a-cursor".to_string()),
        ("hex digits only out of range", "zzzz".to_string()),
        ("hex with a non-hex tail", "ff00ffzz".to_string()),
        ("an odd number of hex digits", "0".to_string()),
        ("hex of a short word", ::hex::encode("hello")),
        (
            "hex of an upper-case identifier",
            ::hex::encode("AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ),
        (
            "hex of a 25-character identifier",
            ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            "hex of a 27-character identifier",
            ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            "hex of bytes that are not UTF-8",
            ::hex::encode([0xff, 0xfe]),
        ),
        ("hex of the empty string", ::hex::encode("")),
        (
            "the bare hex of a well-formed identifier, all digits",
            ::hex::encode("00000000000000000000000000"),
        ),
        (
            "the bare hex of a well-formed identifier, all letters",
            ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            "the bare hex of a well-formed identifier, mixed",
            ::hex::encode(absent_identifier()),
        ),
    ]
}

/// One refused request and how to name it in a failure.
struct Refusal {
    case: String,
    answer: Answer,
}

/// Every paginated reader crossed with every unissued-cursor form, and with
/// whether the request also pinned `max-results`.
async fn cursor_refusals(dsql: &Dsql, cluster: &str) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    for (form, token) in unissued_tokens() {
        for max_results in ["", "&max-results=10"] {
            let query = format!("?next-token={token}{max_results}");
            refusals.push(Refusal {
                case: format!("ListStreams, {form}, query {query:?}"),
                answer: dsql.list_streams(cluster, &query).await,
            });
            refusals.push(Refusal {
                case: format!("ListClusters, {form}, query {query:?}"),
                answer: dsql.list(&query).await,
            });
        }
    }
    refusals
}

/// Assert one refusal against the rule: 400, the `ValidationException` wire code,
/// and exactly the modelled shape's members with `reason` and `fieldList` filled.
///
/// Shared with `tests/all/pagination_cursor_binding.rs`, which asserts the same
/// body shape for the cursors that are well formed but belong to another walk.
pub fn assert_modeled_validation_exception(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an unissued cursor is refused with HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ValidationException"),
        "case {case:?}: the wire error code must be ValidationException, got {:?}",
        answer.error
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the modelled @required message must be filled, got {:?}",
        answer.body
    );

    // The discriminator between the two shapes: the synthetic
    // `smithy.framework#ValidationException` serializes `message` and nothing
    // else, so a body without `reason` — or one carrying a member the modelled
    // shape does not declare — is not the modelled shape.
    let reason = answer.str("reason");
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason.as_str()),
        "case {case:?}: the modelled @required reason must be one of \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {:?}",
        answer.body
    );
    assert_eq!(
        reason, REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a refusal about one member's value reports \
         {REASON_FIELD_VALIDATION_FAILED}, got {reason:?} in {:?}",
        answer.body
    );

    let members: Vec<String> = answer
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert!(
        !members.is_empty(),
        "case {case:?}: the refusal must carry a modelled body, got {:?}",
        answer.body
    );
    let unmodeled: Vec<&String> = members
        .iter()
        .filter(|member| !MODELED_BODY_MEMBERS.contains(&member.as_str()))
        .collect();
    assert!(
        unmodeled.is_empty(),
        "case {case:?}: the body may carry only the modelled ValidationException \
         members {MODELED_BODY_MEMBERS:?}, got the extra {unmodeled:?} in {:?}",
        answer.body
    );

    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields.iter().any(|field| {
            field.get("name").and_then(Value::as_str) == Some(NEXT_TOKEN_FIELD)
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {NEXT_TOKEN_FIELD} with a message \
         saying why, got {field_list:?}"
    );
}

/// `count` streams on `cluster`, returning the identifiers `CreateStream` minted.
///
/// Shared with `tests/all/pagination_cursor_binding.rs`, which populates the same
/// stream walk.
pub async fn create_streams(dsql: &Dsql, cluster: &str, count: usize) -> BTreeSet<String> {
    let mut minted = BTreeSet::new();
    for index in 0..count {
        let created = dsql
            .create_stream(
                cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": {"kinesis": {
                        "streamArn": format!(
                            "arn:aws:kinesis:{}:{}:stream/changes-{index}",
                            dsql.region(),
                            engine::EMULATOR_ACCOUNT_ID
                        ),
                        "roleArn": format!(
                            "arn:aws:iam::{}:role/dsql-cdc",
                            engine::EMULATOR_ACCOUNT_ID
                        ),
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "stream {index} must be created, got {} {:?}",
            created.status, created.body
        );
        minted.insert(created.str("streamIdentifier"));
    }
    minted
}

/// RULE: every paginated reader refuses an unissued `nextToken` with the
/// service's own modelled `ValidationException` body — `message`, the
/// `@required` `reason` and the `fieldList` naming the member, and no member
/// outside that shape.
#[tokio::test]
async fn an_unissued_cursor_is_refused_with_the_modeled_validation_exception_body() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    create_streams(&dsql, &cluster, 2).await;

    let refusals = cursor_refusals(&dsql, &cluster).await;
    assert!(
        refusals.len() >= 40,
        "the table must span both readers, every cursor form and both query \
         forms, got {} cases",
        refusals.len()
    );
    for Refusal { case, answer } in refusals {
        assert_modeled_validation_exception(&case, &answer);
    }
}

/// RULE: the cursor is checked on its own terms, so the refusal does not depend
/// on the addressed cluster existing — `ListStreams` answers the same modelled
/// `ValidationException` for an unissued cursor whether the cluster is one this
/// service holds or one it never created.
#[tokio::test]
async fn the_cursor_refusal_does_not_depend_on_the_addressed_cluster() {
    let dsql = Dsql::new();
    let held = dsql.create_settled().await;
    let absent = absent_identifier();

    let addressed = [
        ("a held cluster", held.as_str()),
        ("an absent cluster", absent.as_str()),
    ];
    for (name, cluster) in addressed {
        let answer = dsql.list_streams(cluster, "?next-token=not-a-cursor").await;
        assert_modeled_validation_exception(&format!("ListStreams against {name}"), &answer);
    }
}

/// RULE (the complement that keeps the refusal honest): a `nextToken` this
/// service DID issue is still served — `ListStreams` pages through a cluster's
/// streams on its own tokens, returning each exactly once.
#[tokio::test]
async fn a_cursor_list_streams_issued_is_still_served() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let minted = create_streams(&dsql, &cluster, 5).await;

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 1..=minted.len() + 1 {
        let query = match &token {
            Some(cursor) => format!("?max-results=2&next-token={cursor}"),
            None => "?max-results=2".to_string(),
        };
        let page = dsql.list_streams(&cluster, &query).await;
        assert_eq!(
            page.status, 200,
            "case page {page_number} on an issued cursor: ListStreams must serve \
             it, got {} {:?}",
            page.status, page.body
        );
        let items = page.get("streams");
        for item in items.as_array().map(Vec::as_slice).unwrap_or_default() {
            seen.push(
                item.get("streamIdentifier")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        token = page.get("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }

    assert!(
        token.is_none(),
        "case paging on issued cursors: the last page must omit nextToken, got {token:?}"
    );
    let unique: BTreeSet<String> = seen.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "case paging on issued cursors: no stream may repeat, got {seen:?}"
    );
    assert_eq!(
        unique, minted,
        "case paging on issued cursors: every created stream must be returned \
         exactly once, got {unique:?}"
    );
}
