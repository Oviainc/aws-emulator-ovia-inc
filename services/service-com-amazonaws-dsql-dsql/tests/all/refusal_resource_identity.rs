// SPDX-License-Identifier: CC0-1.0

//! The identity pair every modelled refusal of this service reports, end to end
//! over the wire.
//!
//! RULE: a refusal that names a resource of this service reports ONE resource —
//! the type segment that resource is stored under together with that resource's
//! OWN identifier — so it never pairs the type of one resource with the identifier
//! of another. The model words the two members that way
//! (`smithy_query(query="shape",
//! shape="ResourceNotFoundException,ServiceQuotaExceededException,ConflictException")`:
//! the resource that "could not be found", the resource that "exceeds a quota",
//! the "Resource Id" and "Resource Type" of one conflict), and the resource a
//! refusal reports is the one the refused call addressed — the cluster a create of
//! a stream names, the stream a delete names — or, for a ceiling, the resource
//! that holds the collection at that ceiling.
//!
//! Every case drives the real request path through `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request, so the routes, the `@httpLabel` /
//! `@httpQuery` bindings and the handlers are exercised the way a client exercises
//! them. The wire harness (`Dsql`, `Answer`, the clock wait) is the one in
//! `tests/all/harness.rs` that every suite in this crate shares.
//!
//! The tables assert the rule over a CLASS of refusals rather than one recorded
//! body: the two refusals `CreateStream` owns (a reused `clientToken` and the
//! stream ceiling), across several regions and token forms; and the sibling
//! refusals of `DeleteStream`, `CreateStream` and `GetStream`, which span both
//! type segments a refusal of this service reports and three refusal classes
//! (a token conflict, a status conflict and a not-found). For every refusal about
//! a resource that exists, the identifier the body carries is read back through
//! the operation that reads a resource of the type the body claims — which is the
//! rule itself, rather than a literal copied out of the implementation.

use serde_json::json;
use service_com_amazonaws_dsql_dsql::state::{
    CLIENT_TOKEN_MISMATCH_MESSAGE, MAX_STREAMS_PER_CLUSTER, RESOURCE_TYPE_CLUSTER,
    RESOURCE_TYPE_STREAM,
};

use crate::harness::{absent_identifier, wait_for_the_clock, Answer, Dsql};
use crate::stream_lifecycle::kinesis_target;

/// The `serviceCode` every ceiling of this service names: the Service Quotas code
/// of Aurora DSQL, the same string as the service segment of its ARNs.
const SERVICE_CODE: &str = "dsql";
/// The `quotaCode` of the ceiling on the streams of one cluster.
const QUOTA_STREAMS_PER_CLUSTER: &str = "streams-per-cluster";

/// The identity pair an error body reports.
fn identity_of(answer: &Answer) -> (String, String) {
    (answer.str("resourceType"), answer.str("resourceId"))
}

/// Whether `resource_id` really is an identifier of a `resource_type` this store
/// holds, asked through the operation that READS a resource of that type: a
/// cluster by `GetCluster` and a stream of `cluster` by `GetStream`.
///
/// This is the rule's own test rather than a re-assertion of a literal: a pair
/// that names one resource resolves, and one that pairs a type with another
/// resource's identifier cannot.
async fn names_a_resource_of_its_own_type(
    dsql: &Dsql,
    cluster: &str,
    resource_type: &str,
    resource_id: &str,
) -> bool {
    match resource_type {
        RESOURCE_TYPE_CLUSTER => dsql.get(resource_id).await.status == 200,
        RESOURCE_TYPE_STREAM => dsql.get_stream(cluster, resource_id).await.status == 200,
        _ => false,
    }
}

/// Assert the pair `answer` carries is `(resource_type, resource_id)` and that it
/// names a resource of that very type.
async fn assert_identity(
    case: &str,
    dsql: &Dsql,
    cluster: &str,
    answer: &Answer,
    expected: (&str, &str),
) {
    assert_reported_identity(case, answer, expected);
    let (reported_type, reported_id) = identity_of(answer);
    assert!(
        names_a_resource_of_its_own_type(dsql, cluster, &reported_type, &reported_id).await,
        "case {case:?}: resourceId {reported_id:?} must be an identifier of a {reported_type:?} \
         this service holds, got {:?}",
        answer.body
    );
}

/// Assert the pair `answer` carries is `(resource_type, resource_id)`, without
/// reading the resource back: the identity a NOT-FOUND reports names a resource
/// that is absent by definition, so only the pair itself can be asserted.
fn assert_reported_identity(case: &str, answer: &Answer, expected: (&str, &str)) {
    let (reported_type, reported_id) = identity_of(answer);
    assert_eq!(
        (reported_type.as_str(), reported_id.as_str()),
        expected,
        "case {case:?}: the refusal must report the resource's own type and identifier, got {:?}",
        answer.body
    );
}

/// One reused-`clientToken` case: the region the requests are signed for and the
/// token both calls carry.
struct TokenReuseCase {
    region: &'static str,
    token: &'static str,
}

/// RULE: `CreateStream`'s reused-`clientToken` refusal reports the resource the
/// call ADDRESSED — the cluster the stream was to be created on — by that
/// cluster's own type segment and identifier; the stream the earlier call minted
/// is not what this refusal is about, and no stream of the refused call exists.
#[tokio::test]
async fn the_create_stream_token_conflict_names_the_cluster_it_addressed() {
    // Token forms spanning `ClientToken`'s own bounds (`smithy_query(query="shape",
    // shape="ClientToken")`: `@length(1, 128)`, `@pattern("^[!-~]+$")`).
    let cases = [
        TokenReuseCase {
            region: "us-east-1",
            token: "t",
        },
        TokenReuseCase {
            region: "eu-west-1",
            token: "tok-1",
        },
        TokenReuseCase {
            region: "ap-southeast-2",
            token: "~!$%&*+.",
        },
    ];

    for case in cases {
        let name = format!("token {:?} in {}", case.token, case.region);
        let dsql = Dsql::in_region(case.region);
        let cluster = dsql.create_settled().await;

        let first = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(case.region, "changes", "dsql-cdc"),
                    "clientToken": case.token,
                }),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first CreateStream must succeed, got {} {:?}",
            first.status, first.body
        );
        let minted = first.str("streamIdentifier");
        wait_for_the_clock().await;

        // The same token with DIFFERENT parameters is another request wearing a
        // used token.
        let reused = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(case.region, "other-changes", "dsql-cdc"),
                    "clientToken": case.token,
                }),
            )
            .await;
        assert_eq!(
            (reused.status, reused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {name:?}: the reused token must be refused with 409 ConflictException, got {} \
             {:?}",
            reused.status,
            reused.body
        );
        assert_eq!(
            reused.message(),
            CLIENT_TOKEN_MISMATCH_MESSAGE,
            "case {name:?}: the refusal must be worded as a token reuse, got {:?}",
            reused.message()
        );
        assert_identity(
            &name,
            &dsql,
            &cluster,
            &reused,
            (RESOURCE_TYPE_CLUSTER, cluster.as_str()),
        )
        .await;
        assert_ne!(
            reused.str("resourceId"),
            minted,
            "case {name:?}: the refusal is not about the stream the first call minted, got {:?}",
            reused.body
        );
    }
}

/// RULE: the refusal at a cluster's stream ceiling reports the resource whose
/// quota was exceeded — the CLUSTER whose stream table is full — by its own type
/// segment and identifier, together with the service and quota codes that name
/// the ceiling, and it creates nothing.
#[tokio::test]
async fn the_stream_quota_refusal_names_the_cluster_whose_table_is_full() {
    for region in ["us-east-1", "eu-west-1"] {
        let name = format!("a full cluster in {region}");
        let dsql = Dsql::in_region(region);
        let cluster = dsql.create_settled().await;

        for ordinal in 0..MAX_STREAMS_PER_CLUSTER {
            let created = dsql
                .create_stream(
                    &cluster,
                    json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(region, "changes", "dsql-cdc"),
                        "tags": {"ordinal": ordinal.to_string()},
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: CreateStream #{ordinal} must succeed, got {} {:?}",
                created.status, created.body
            );
        }

        let refused = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(region, "one-too-many", "dsql-cdc"),
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (402, Some("ServiceQuotaExceededException")),
            "case {name:?}: the create past the ceiling must be refused with 402 \
             ServiceQuotaExceededException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused
                .message()
                .contains(&MAX_STREAMS_PER_CLUSTER.to_string()),
            "case {name:?}: the refusal must name the ceiling it reached, got {:?}",
            refused.message()
        );
        assert_eq!(
            (refused.str("serviceCode"), refused.str("quotaCode")),
            (
                SERVICE_CODE.to_string(),
                QUOTA_STREAMS_PER_CLUSTER.to_string()
            ),
            "case {name:?}: the refusal must name the service and the quota, got {:?}",
            refused.body
        );
        assert_identity(
            &name,
            &dsql,
            &cluster,
            &refused,
            (RESOURCE_TYPE_CLUSTER, cluster.as_str()),
        )
        .await;

        let listed = dsql.list_streams(&cluster, "?max-results=100").await;
        let held = listed
            .get("streams")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, MAX_STREAMS_PER_CLUSTER,
            "case {name:?}: the refused create must have created nothing, got {held} streams"
        );
    }
}

/// Which refusal a row of the cross-operation table drives.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// `DeleteStream` carrying a `client-token` recorded against another delete:
    /// a conflict about the STREAM the refused call addressed.
    DeleteStreamTokenReuse,
    /// `CreateStream` on a cluster the clock still owes its `ACTIVE` status: a
    /// conflict about that CLUSTER's status.
    CreateStreamOnACreatingCluster,
    /// `DeleteStream` on a stream the clock still owes its `ACTIVE` status: a
    /// conflict about that STREAM's status.
    DeleteStreamOfACreatingStream,
    /// `GetStream` naming a stream the cluster does not hold: a not-found about
    /// the STREAM that is missing.
    GetStreamOfAnAbsentStream,
}

/// RULE (the same one, over the other refusals this service answers): each
/// reports one resource's own type segment beside that same resource's own
/// identifier — the stream a delete addressed, the cluster a create addressed,
/// the stream a read could not find — and never the type of one resource beside
/// the identifier of another.
#[tokio::test]
async fn every_refusal_names_a_resource_of_the_type_it_reports() {
    let region = "us-east-1";
    let rows = [
        Refusal::DeleteStreamTokenReuse,
        Refusal::CreateStreamOnACreatingCluster,
        Refusal::DeleteStreamOfACreatingStream,
        Refusal::GetStreamOfAnAbsentStream,
    ];

    for refusal in rows {
        let name = format!("{refusal:?}");
        let dsql = Dsql::in_region(region);
        let target = json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target(region, "changes", "dsql-cdc"),
        });

        match refusal {
            Refusal::DeleteStreamTokenReuse => {
                let cluster = dsql.create_settled().await;
                let mut streams = Vec::new();
                for ordinal in 0..2 {
                    let created = dsql
                        .create_stream(
                            &cluster,
                            json!({
                                "format": "JSON",
                                "ordering": "UNORDERED",
                                "targetDefinition": kinesis_target(region, "changes", "dsql-cdc"),
                                "tags": {"ordinal": ordinal.to_string()},
                            }),
                        )
                        .await;
                    assert_eq!(
                        created.status, 200,
                        "case {name:?}: CreateStream must succeed, got {} {:?}",
                        created.status, created.body
                    );
                    streams.push(created.str("streamIdentifier"));
                }
                wait_for_the_clock().await;

                let query = "?client-token=one-delete-only";
                let first = dsql.delete_stream(&cluster, &streams[0], query).await;
                assert_eq!(
                    first.status, 200,
                    "case {name:?}: the first DeleteStream must succeed, got {} {:?}",
                    first.status, first.body
                );

                // The same token carried by the delete of ANOTHER stream: the
                // refusal is about the stream THIS call addressed.
                let reused = dsql.delete_stream(&cluster, &streams[1], query).await;
                assert_eq!(
                    (reused.status, reused.error.as_deref()),
                    (409, Some("ConflictException")),
                    "case {name:?}: the reused token must be refused with 409 ConflictException, \
                     got {} {:?}",
                    reused.status,
                    reused.body
                );
                assert_identity(
                    &name,
                    &dsql,
                    &cluster,
                    &reused,
                    (RESOURCE_TYPE_STREAM, streams[1].as_str()),
                )
                .await;
            }
            Refusal::CreateStreamOnACreatingCluster => {
                // No wait: the cluster is still in the status the clock owes a
                // transition out of, so it takes no stream.
                let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
                assert_eq!(
                    created.status, 200,
                    "case {name:?}: CreateCluster must succeed, got {} {:?}",
                    created.status, created.body
                );
                let cluster = created.str("identifier");

                let refused = dsql.create_stream(&cluster, target).await;
                assert_eq!(
                    (refused.status, refused.error.as_deref()),
                    (409, Some("ConflictException")),
                    "case {name:?}: a create on a cluster that takes no mutation must be refused \
                     with 409 ConflictException, got {} {:?}",
                    refused.status,
                    refused.body
                );
                assert_identity(
                    &name,
                    &dsql,
                    &cluster,
                    &refused,
                    (RESOURCE_TYPE_CLUSTER, cluster.as_str()),
                )
                .await;
            }
            Refusal::DeleteStreamOfACreatingStream => {
                let cluster = dsql.create_settled().await;
                let created = dsql.create_stream(&cluster, target).await;
                assert_eq!(
                    created.status, 200,
                    "case {name:?}: CreateStream must succeed, got {} {:?}",
                    created.status, created.body
                );
                let stream = created.str("streamIdentifier");

                // No wait: the stream is still in the status the clock owes a
                // transition out of, so it takes no deletion.
                let refused = dsql.delete_stream(&cluster, &stream, "").await;
                assert_eq!(
                    (refused.status, refused.error.as_deref()),
                    (409, Some("ConflictException")),
                    "case {name:?}: a delete of a stream that takes no mutation must be refused \
                     with 409 ConflictException, got {} {:?}",
                    refused.status,
                    refused.body
                );
                assert_identity(
                    &name,
                    &dsql,
                    &cluster,
                    &refused,
                    (RESOURCE_TYPE_STREAM, stream.as_str()),
                )
                .await;
            }
            Refusal::GetStreamOfAnAbsentStream => {
                let cluster = dsql.create_settled().await;
                let absent = absent_identifier();

                let refused = dsql.get_stream(&cluster, &absent).await;
                assert_eq!(
                    (refused.status, refused.error.as_deref()),
                    (404, Some("ResourceNotFoundException")),
                    "case {name:?}: a read of a stream the cluster does not hold must answer 404 \
                     ResourceNotFoundException, got {} {:?}",
                    refused.status,
                    refused.body
                );
                // The missing resource is the STREAM the call named, so the pair
                // carries that stream's identifier and not the cluster's — the
                // resource is absent by definition, so there is nothing to read
                // back.
                assert_reported_identity(
                    &name,
                    &refused,
                    (RESOURCE_TYPE_STREAM, absent.as_str()),
                );
                assert_ne!(
                    refused.str("resourceId"),
                    cluster,
                    "case {name:?}: the refusal must not name the cluster the stream was looked \
                     for on, got {:?}",
                    refused.body
                );
            }
        }
    }
}
