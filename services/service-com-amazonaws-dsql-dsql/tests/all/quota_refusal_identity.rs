// SPDX-License-Identifier: CC0-1.0

//! The identity every CEILING of this service reports, end to end over the wire.
//!
//! RULE: a `ServiceQuotaExceededException` reports the resource whose quota was
//! exceeded — the one that HOLDS the collection standing at its ceiling — by that
//! resource's own type segment together with that same resource's own identifier,
//! never the type of one resource beside the identifier of another. The model
//! words the two members that way (`smithy_query(query="shape",
//! shape="ServiceQuotaExceededException")`: `resourceId` "The resource ID exceeds
//! a quota", `resourceType` "The resource type exceeds a quota", both
//! `@required`), so the holder of a per-account collection is reported as the
//! ACCOUNT and the holder of a per-cluster or per-resource collection as that
//! cluster or that resource.
//!
//! The table spans the class the rule generalises over: every ceiling this service
//! answers at, which is every KIND of holder it has (an account, a cluster, and a
//! stream of a cluster), over two regions and both tagged resource types. For each
//! row the reported pair is READ BACK the way a client would — a cluster through
//! `GetCluster`, a stream through `GetStream`, and the account through the account
//! segment of an ARN this service minted for a resource it holds — so the
//! assertion is the rule itself rather than a literal copied out of the handler.
//!
//! Every call goes the whole way round through the shared wire harness in
//! `tests/all/harness.rs`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_dsql_dsql::state::{
    MAX_CLUSTERS, MAX_STREAMS_PER_CLUSTER, RESOURCE_TYPE_ACCOUNT, RESOURCE_TYPE_CLUSTER,
    RESOURCE_TYPE_STREAM,
};

use crate::harness::{
    expected_arn, expected_stream_arn, is_cluster_id, wait_for_the_clock, Answer, Dsql,
    TAG_MAP_MAX,
};
use crate::stream_lifecycle::kinesis_target;

/// The `serviceCode` every ceiling of this service names: the Service Quotas code
/// of Aurora DSQL, the same string as the service segment of its ARNs.
const SERVICE_CODE: &str = "dsql";
/// The `quotaCode` of the ceiling on the clusters ONE ACCOUNT holds, as
/// `src/ops/create_cluster.rs` spells it.
const QUOTA_CLUSTERS_PER_ACCOUNT: &str = "clusters-per-account";
/// The `quotaCode` of the ceiling on the streams ONE CLUSTER holds, as
/// `src/ops/create_stream.rs` spells it.
const QUOTA_STREAMS_PER_CLUSTER: &str = "streams-per-cluster";
/// The `quotaCode` of the ceiling on the tags ONE RESOURCE holds, as
/// `src/ops/tag_resource.rs` spells it.
const QUOTA_TAGS_PER_RESOURCE: &str = "tags-per-resource";

/// Which ceiling a row of the table drives.
#[derive(Debug, Clone, Copy)]
enum Ceiling {
    /// The clusters of one account: the ACCOUNT holds them, and no cluster of the
    /// refused call was ever minted.
    ClustersPerAccount,
    /// The streams of one cluster: that CLUSTER holds them.
    StreamsPerCluster,
    /// The tags of one cluster: that CLUSTER holds them.
    TagsPerCluster,
    /// The tags of one stream: that STREAM holds them, not the cluster the stream
    /// is stored on.
    TagsPerStream,
}

/// What a row expects of the refusal: the holder's own identity pair, the quota
/// that was exceeded, and a cluster of the store the pair is resolved against.
struct Refused {
    answer: Answer,
    expected_type: String,
    expected_id: String,
    quota_code: &'static str,
    /// The cluster `GetStream` reads a reported stream on, and whose ARN witnesses
    /// the account segment this service mints with.
    cluster: String,
    cluster_arn: String,
}

/// `count` distinct tags, as a restJson1 `tags` member.
fn tag_object(prefix: &str, count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("{prefix}-{n}"), Value::String(format!("value-{n}"))))
            .collect::<Map<String, Value>>(),
    )
}

/// Drive one ceiling to its refusal.
async fn drive(dsql: &Dsql, ceiling: Ceiling, case: &str) -> Refused {
    let region = dsql.region().to_string();
    match ceiling {
        Ceiling::ClustersPerAccount => {
            let mut first = String::new();
            for ordinal in 0..MAX_CLUSTERS {
                let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: CreateCluster #{ordinal} must succeed, got {} {:?}",
                    created.status, created.body
                );
                if first.is_empty() {
                    first = created.str("identifier");
                }
            }
            let answer = dsql.create(json!({"deletionProtectionEnabled": false})).await;
            Refused {
                answer,
                // The holder of the account's cluster collection is the account,
                // identified by the id every ARN this service mints carries.
                expected_type: RESOURCE_TYPE_ACCOUNT.to_string(),
                expected_id: account_of(&expected_arn(&region, &first)),
                quota_code: QUOTA_CLUSTERS_PER_ACCOUNT,
                cluster_arn: expected_arn(&region, &first),
                cluster: first,
            }
        }
        Ceiling::StreamsPerCluster => {
            let cluster = dsql.create_settled().await;
            for ordinal in 0..MAX_STREAMS_PER_CLUSTER {
                let created = dsql
                    .create_stream(&cluster, stream_request(&region, ordinal))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: CreateStream #{ordinal} must succeed, got {} {:?}",
                    created.status, created.body
                );
            }
            let answer = dsql
                .create_stream(&cluster, stream_request(&region, MAX_STREAMS_PER_CLUSTER))
                .await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                quota_code: QUOTA_STREAMS_PER_CLUSTER,
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
        Ceiling::TagsPerCluster => {
            let cluster = dsql.create_settled().await;
            let arn = expected_arn(&region, &cluster);
            let filled = dsql
                .tag_resource(&arn, json!({"tags": tag_object("kept", TAG_MAP_MAX)}))
                .await;
            assert_eq!(
                filled.status, 200,
                "case {case:?}: filling the tag map to its ceiling must succeed, got {} {:?}",
                filled.status, filled.body
            );
            let answer = dsql
                .tag_resource(&arn, json!({"tags": {"one-too-many": "x"}}))
                .await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                quota_code: QUOTA_TAGS_PER_RESOURCE,
                cluster_arn: arn,
                cluster,
            }
        }
        Ceiling::TagsPerStream => {
            let cluster = dsql.create_settled().await;
            // No tags on the stream itself: the fill below is what brings its map
            // exactly to the ceiling.
            let created = dsql
                .create_stream(
                    &cluster,
                    json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(&region, "changes", "dsql-cdc"),
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateStream must succeed, got {} {:?}",
                created.status, created.body
            );
            let stream = created.str("streamIdentifier");
            wait_for_the_clock().await;

            let arn = expected_stream_arn(&region, &cluster, &stream);
            let filled = dsql
                .tag_resource(&arn, json!({"tags": tag_object("kept", TAG_MAP_MAX)}))
                .await;
            assert_eq!(
                filled.status, 200,
                "case {case:?}: filling the tag map to its ceiling must succeed, got {} {:?}",
                filled.status, filled.body
            );
            let answer = dsql
                .tag_resource(&arn, json!({"tags": {"one-too-many": "x"}}))
                .await;
            Refused {
                answer,
                // The tags are the STREAM's, so the refusal is about the stream and
                // carries its own identifier, not the cluster's.
                expected_type: RESOURCE_TYPE_STREAM.to_string(),
                expected_id: stream,
                quota_code: QUOTA_TAGS_PER_RESOURCE,
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
    }
}

/// One `CreateStream` request, distinct per `ordinal` so a fill creates distinct
/// streams.
fn stream_request(region: &str, ordinal: usize) -> Value {
    json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target(region, "changes", "dsql-cdc"),
        "tags": {"ordinal": ordinal.to_string()},
    })
}

/// The account segment of an ARN this service mints
/// (`arn:{partition}:dsql:{region}:{account}:{resource}`).
fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// Whether `resource_id` really is an identifier of a `resource_type` this
/// emulator holds, asked the way a client would ask: a cluster through
/// `GetCluster`, a stream of `cluster` through `GetStream`, and the account
/// through the account segment of an ARN this service minted.
async fn names_a_resource_of_its_own_type(
    dsql: &Dsql,
    refused: &Refused,
    resource_type: &str,
    resource_id: &str,
) -> bool {
    match resource_type {
        RESOURCE_TYPE_CLUSTER => dsql.get(resource_id).await.status == 200,
        RESOURCE_TYPE_STREAM => {
            dsql.get_stream(&refused.cluster, resource_id).await.status == 200
        }
        RESOURCE_TYPE_ACCOUNT => resource_id == account_of(&refused.cluster_arn),
        _ => false,
    }
}

/// RULE (see the module docs): every ceiling reports the holder of the collection
/// at that ceiling, by that holder's own type segment and own identifier, beside
/// the service and quota codes that name the ceiling.
#[tokio::test]
async fn every_ceiling_names_the_resource_that_holds_the_collection() {
    let rows = [
        Ceiling::ClustersPerAccount,
        Ceiling::StreamsPerCluster,
        Ceiling::TagsPerCluster,
        Ceiling::TagsPerStream,
    ];

    for region in ["us-east-1", "eu-west-1"] {
        for ceiling in rows {
            let case = format!("{ceiling:?} in {region}");
            let dsql = Dsql::in_region(region);
            let refused = drive(&dsql, ceiling, &case).await;
            let answer = &refused.answer;

            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (402, Some("ServiceQuotaExceededException")),
                "case {case:?}: the call past the ceiling must be refused with 402 \
                 ServiceQuotaExceededException, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                (answer.str("serviceCode"), answer.str("quotaCode")),
                (SERVICE_CODE.to_string(), refused.quota_code.to_string()),
                "case {case:?}: the refusal must name the service and the quota, got {:?}",
                answer.body
            );
            assert_eq!(
                (
                    answer.str("resourceType").as_str(),
                    answer.str("resourceId").as_str()
                ),
                (refused.expected_type.as_str(), refused.expected_id.as_str()),
                "case {case:?}: the refusal must report the holder's own type and identifier, \
                 got {:?}",
                answer.body
            );
            assert!(
                names_a_resource_of_its_own_type(
                    &dsql,
                    &refused,
                    &answer.str("resourceType"),
                    &answer.str("resourceId"),
                )
                .await,
                "case {case:?}: resourceId {:?} must be an identifier of a {:?} this service \
                 holds, got {:?}",
                answer.str("resourceId"),
                answer.str("resourceType"),
                answer.body
            );
        }
    }
}

/// RULE (the same one, stated negatively for the per-account ceiling the bundle is
/// about): the cluster ceiling reports the ACCOUNT, so the identifier it carries is
/// an account id — never a `ClusterId` (`smithy_query(query="shape",
/// shape="ClusterId")`: `^[a-z0-9]{26}$`), and never the identifier of any cluster
/// the store holds — and the refused call creates nothing.
#[tokio::test]
async fn the_cluster_ceiling_names_no_cluster() {
    for region in ["us-east-1", "ap-southeast-2"] {
        let case = format!("a full store in {region}");
        let dsql = Dsql::in_region(region);
        let held = dsql.create_settled_many(MAX_CLUSTERS).await;

        let refused = dsql.create(json!({"deletionProtectionEnabled": false})).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (402, Some("ServiceQuotaExceededException")),
            "case {case:?}: the create past the ceiling must be refused with 402 \
             ServiceQuotaExceededException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(&MAX_CLUSTERS.to_string()),
            "case {case:?}: the refusal must name the ceiling it reached, got {:?}",
            refused.message()
        );

        let reported = refused.str("resourceId");
        assert_eq!(
            refused.str("resourceType"),
            RESOURCE_TYPE_ACCOUNT,
            "case {case:?}: the holder of the account's clusters is the account, got {:?}",
            refused.body
        );
        assert!(
            !is_cluster_id(&reported),
            "case {case:?}: an account id is no ClusterId, got {reported:?}"
        );
        assert!(
            !held.contains(&reported),
            "case {case:?}: the refusal must name no cluster of the store, got {reported:?}"
        );
        let read_back = dsql.get(&reported).await;
        assert_ne!(
            read_back.status, 200,
            "case {case:?}: the reported id is no cluster of this service, so GetCluster must \
             not resolve it, got {:?}",
            read_back.body
        );

        // The refused create added nothing: the store still holds exactly the
        // clusters the accepted calls minted.
        let listed = dsql.list("?max-results=100").await;
        let count = listed
            .get("clusters")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, MAX_CLUSTERS,
            "case {case:?}: the refused create must have created nothing, got {count} clusters"
        );
    }
}
