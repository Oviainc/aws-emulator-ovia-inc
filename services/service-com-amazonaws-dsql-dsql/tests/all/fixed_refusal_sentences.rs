// SPDX-License-Identifier: CC0-1.0

//! The refusals of this service whose TEXT belongs to the error class rather than to
//! the request, end to end over the wire.
//!
//! RULE under test: where Aurora DSQL states a refusal with fixed text, the emulator
//! states that same text — the whole sentence, with nothing from the refused request
//! interpolated into it — and the machine-readable members beside it carry the names
//! the caller used on the wire. Three classes, each asserted over a table that spans
//! what the class generalises over rather than over one recorded request:
//!
//! * an unusable pagination token: HTTP 400 `ValidationException` whose `message` and
//!   whose single `fieldList` entry both carry "Invalid pagination token. Request a
//!   new token and try again.", the entry naming the cursor by the `@httpQuery`
//!   parameter it travelled in (`smithy_query(query="operation",
//!   operation="ListClusters,ListStreams")`: `nextToken`, `httpQuery: "next-token"`);
//! * a `policy` the service cannot read as a policy document: HTTP 400
//!   `ValidationException` with "Aurora DSQL can't process the request because the
//!   policy that you specified isn't valid. Provide the policy in a valid format and
//!   try again." — the wording belongs to the class, so both operations that WRITE a
//!   document (`CreateCluster`'s inline `policy`, `PutClusterPolicy`'s `policy`,
//!   `smithy_query(query="operation", operation="CreateCluster,PutClusterPolicy")`)
//!   state it;
//! * a `resourceArn` the ARN-addressed tagging surface resolves no resource for: HTTP
//!   404 with the wire code `NotFoundException` and "Resource was not found", carrying
//!   no member besides the sentence — that surface's own refusal, where the operations
//!   addressed by an IDENTIFIER answer the modelled `ResourceNotFoundException` naming
//!   the ARN they could not resolve.
//!
//! The client is the shared `Dsql` harness in `harness.rs`; `UntagResource`'s query
//! encoding is the one the `resource_tags` suite spells.

use serde_json::json;

use crate::harness::{
    absent_identifier, expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql,
};
use crate::resource_tags::untag;

/// The sentence an unusable pagination token is refused with, in full.
const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token. Request a new token and try again.";

/// The field name that refusal's `fieldList` entry carries: the query parameter the
/// cursor travelled in, which is the name the caller spelled it with.
const NEXT_TOKEN_FIELD: &str = "next-token";

/// The sentence a `policy` that is not a policy document is refused with, in full.
const INVALID_POLICY_DOCUMENT: &str = "Aurora DSQL can't process the request because the policy \
     that you specified isn't valid. Provide the policy in a valid format and try again.";

/// The wire code and the sentence the tagging surface's not-found carries.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
/// See [`TAGGING_NOT_FOUND_CODE`].
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// Values no page of either reader ever issued, spanning every form the failure takes:
/// not hex at all, hex of something that is not the `^[a-z0-9]{26}$` identifier a
/// cursor carries, and the bare hex of such an identifier — well formed, and still
/// bound to no walk of this service.
fn unissued_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("a value that is not hex", "not-a-cursor".to_string()),
        ("hex digits out of range", "zzzz".to_string()),
        ("an odd number of hex digits", "0".to_string()),
        ("hex of a short word", ::hex::encode("hello")),
        (
            "hex of an upper-case identifier",
            ::hex::encode("AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ),
        ("hex of bytes that are not UTF-8", ::hex::encode([0xff, 0xfe])),
        (
            "the bare hex of a well-formed identifier",
            ::hex::encode(absent_identifier()),
        ),
    ]
}

/// RULE: both paginated readers refuse a `nextToken` no page of theirs issued with the
/// one fixed sentence, carried in `message` and in exactly one `fieldList` entry that
/// names the `next-token` query parameter — and nothing of the rejected value reaches
/// the body.
#[tokio::test]
async fn an_unissued_pagination_token_is_refused_with_the_fixed_sentence() {
    let dsql = Dsql::new();
    // Two clusters, so the control below has a page to leave something behind.
    let clusters = dsql.create_settled_many(2).await;
    let cluster = clusters[0].clone();

    let mut cases = 0usize;
    for (form, token) in unissued_tokens() {
        // Crossed with whether the request also pinned `max-results`: the cursor is
        // read on its own terms either way.
        for max_results in ["", "&max-results=10"] {
            let query = format!("?next-token={token}{max_results}");
            for (operation, answer) in [
                ("ListClusters", dsql.list(&query).await),
                ("ListStreams", dsql.list_streams(&cluster, &query).await),
            ] {
                let case = format!("{operation}, {form}, query {query:?}");
                cases += 1;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: an unissued cursor is HTTP 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: the wire code must be ValidationException, got {:?}",
                    answer.error
                );
                assert_eq!(
                    answer.message(),
                    INVALID_PAGINATION_TOKEN,
                    "case {case:?}: expected {INVALID_PAGINATION_TOKEN:?}, got {:?}",
                    answer.message()
                );
                assert_eq!(
                    answer.get("fieldList"),
                    json!([{"name": NEXT_TOKEN_FIELD, "message": INVALID_PAGINATION_TOKEN}]),
                    "case {case:?}: expected exactly one entry naming {NEXT_TOKEN_FIELD}, got {:?}",
                    answer.get("fieldList")
                );
                assert!(
                    !answer.body.to_string().contains(&token),
                    "case {case:?}: the body must echo nothing of the rejected token ({token:?}), \
                     got {:?}",
                    answer.body
                );
            }
        }
    }
    assert!(
        cases >= 20,
        "the table must span both readers, every cursor form and both query forms, got {cases} \
         cases"
    );

    // Control: a cursor a page DID issue is still served, so the sentence above is
    // about the token and not about paging at all.
    let paged = dsql.list("?max-results=1").await;
    let issued = paged.str("nextToken");
    assert!(
        !issued.is_empty(),
        "case the first page: a page smaller than the store must emit a nextToken, got {:?}",
        paged.body
    );
}

/// Documents that are not a policy document, spanning the forms the failure takes: not
/// JSON at all, JSON that is not an object, and JSON that does not parse.
fn documents_that_are_not_policies() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not JSON at all", "not a policy"),
        ("a JSON array", "[]"),
        ("a JSON string", "\"a string\""),
        ("a JSON number", "7"),
        ("a JSON boolean", "true"),
        ("JSON null", "null"),
        ("a truncated object", "{\"Statement\":["),
        ("two documents concatenated", "{}{}"),
    ]
}

/// RULE: both operations that write a cluster's resource-based policy refuse a `policy`
/// they cannot read as a policy document with the one fixed sentence that error class
/// carries, echoing neither the document nor anything else from the request.
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_with_the_fixed_sentence() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    for (form, document) in documents_that_are_not_policies() {
        for (operation, answer) in [
            (
                "CreateCluster",
                dsql.create(json!({"deletionProtectionEnabled": false, "policy": document}))
                    .await,
            ),
            (
                "PutClusterPolicy",
                dsql.put_policy(&cluster, json!({"policy": document})).await,
            ),
        ] {
            let case = format!("{operation} with {form}");
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a document that is not a policy is HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: the wire code must be ValidationException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.message(),
                INVALID_POLICY_DOCUMENT,
                "case {case:?}: expected {INVALID_POLICY_DOCUMENT:?}, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(document),
                "case {case:?}: the sentence must echo nothing of the document ({document:?}), \
                 got {:?}",
                answer.message()
            );
        }
    }

    // Control: a document that IS a policy is attached and reads back, so the sentence
    // above is about the document and not about the write path.
    let attached = dsql
        .put_policy(&cluster, json!({"policy": "{\"Statement\":[]}"}))
        .await;
    assert_eq!(
        attached.status, 200,
        "case a valid document: PutClusterPolicy must answer 200, got {} {:?}",
        attached.status, attached.body
    );
    let read_back = dsql.get_policy(&cluster).await;
    assert_eq!(
        read_back.str("policy"),
        "{\"Statement\":[]}",
        "case a valid document: GetClusterPolicy must report it verbatim, got {:?}",
        read_back.body
    );
}

/// RULE: every operation on `/tags/{resourceArn}` refuses an ARN it resolves no
/// resource for with the tagging surface's own 404 — the fixed sentence and no member
/// besides it — whatever kind of resource the ARN named and however it came to be
/// absent.
#[tokio::test]
async fn the_tagging_surface_refuses_an_unresolved_arn_with_its_own_sentence() {
    let region = "us-west-2";
    let dsql = Dsql::in_region(region);
    let account = engine::EMULATOR_ACCOUNT_ID;

    // A cluster and a stream that DO exist, so every refusal below is about the ARN
    // rather than about an empty store.
    let live = dsql.create_settled().await;
    let retired = dsql.create_settled().await;
    let deleted = dsql.delete(&retired).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteCluster must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    wait_for_the_clock().await;

    let absent = absent_identifier();
    let arns: Vec<(String, String)> = vec![
        (
            "a cluster this service never minted".to_string(),
            expected_arn(region, &absent),
        ),
        (
            "a cluster whose deletion the clock completed".to_string(),
            expected_arn(region, &retired),
        ),
        (
            "a stream of a live cluster, never created".to_string(),
            expected_stream_arn(region, &live, &absent),
        ),
        (
            "the live cluster's ARN spelled for another region".to_string(),
            expected_arn("eu-central-1", &live),
        ),
        (
            "a top-level ARN of a resource type this store holds none of".to_string(),
            format!("arn:aws:dsql:{region}:{account}:stream/{absent}"),
        ),
    ];

    for (name, arn) in &arns {
        for (operation, answer) in [
            ("ListTagsForResource", dsql.list_tags_for_resource(arn).await),
            (
                "TagResource",
                dsql.tag_resource(arn, json!({"tags": {"env": "dev"}})).await,
            ),
            (
                "UntagResource",
                untag(&dsql, arn, &["env".to_string()]).await,
            ),
        ] {
            let case = format!("{operation} with {name}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an unresolved ARN is HTTP 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some(TAGGING_NOT_FOUND_CODE),
                "case {case:?}: the wire code must be {TAGGING_NOT_FOUND_CODE}, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.message(),
                TAGGING_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {TAGGING_NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
            assert_eq!(
                reported_identity(&answer),
                (String::new(), String::new()),
                "case {case:?}: the tagging surface reports no resource identity, got {:?}",
                answer.body
            );
        }
    }

    // The other surface, unchanged: an operation addressed by an IDENTIFIER answers the
    // modelled `ResourceNotFoundException` naming the ARN it could not resolve, so the
    // sentence above belongs to the tagging path alone.
    let by_identifier = dsql.get(&absent).await;
    assert_eq!(
        (by_identifier.status, by_identifier.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case GetCluster with an absent cluster: got {} {:?}",
        by_identifier.status,
        by_identifier.body
    );
    assert_eq!(
        by_identifier.message(),
        format!(
            "The resource with ARN {} doesn't exist. Verify the ARN and try again.",
            expected_arn(region, &absent)
        ),
        "case GetCluster with an absent cluster: the modelled refusal names the ARN, got {:?}",
        by_identifier.message()
    );
    assert_eq!(
        reported_identity(&by_identifier),
        ("cluster".to_string(), absent.clone()),
        "case GetCluster with an absent cluster: the modelled refusal names the resource, got {:?}",
        by_identifier.body
    );

    // And the live cluster's own ARN is still served, so nothing above is about the
    // route: its tags are readable and the refused writes left them alone.
    let served = dsql.list_tags_for_resource(&expected_arn(region, &live)).await;
    assert_eq!(
        (served.status, served.get("tags")),
        (200, json!({})),
        "case the live cluster's own ARN: got {} {:?}",
        served.status,
        served.body
    );
}

/// The resource identity an answer reports: the two `ResourceNotFoundException`
/// members that name a resource, empty where the body carries neither.
fn reported_identity(answer: &Answer) -> (String, String) {
    (answer.str("resourceType"), answer.str("resourceId"))
}
