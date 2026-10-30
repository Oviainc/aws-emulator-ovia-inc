// SPDX-License-Identifier: CC0-1.0

//! The identity every reused-`clientToken` refusal of this service reports, end to
//! end over the wire.
//!
//! RULE: a refusal that carries a resource identity reports ONE resource — that
//! resource's own type segment TOGETHER WITH that same resource's own identifier,
//! both members filled — and the resource it reports is the one the refused call
//! addressed, which for a call that would ADD to a collection is the resource that
//! holds that collection. The model words the two members as the "Resource Id" and
//! the "Resource Type" of one conflict (`smithy_query(query="shape",
//! shape="ConflictException")`, both optional), so a body that names a type with no
//! instance behind it describes no resource at all.
//!
//! The table spans the class the rule generalises over: EVERY operation of this
//! service that carries a `clientToken` — the two creates, whose refusal names the
//! holder of the collection they would add to (the account for a cluster, the
//! cluster for a stream), and the updates and deletes, whose refusal names the
//! cluster or the stream the call addressed — each driven in two regions and under
//! a token form that rotates per region, so all four forms spanning
//! `ClientToken`'s own bounds are exercised and no refusal is pinned to one of
//! them. For every row the reported pair is
//! READ BACK the way a client would — a cluster through `GetCluster`, a stream
//! through `GetStream`, the account through the account segment of an ARN this
//! service minted — so the assertion is the rule itself rather than a literal
//! copied out of a handler.
//!
//! Every call goes the whole way round through the shared wire harness in
//! `tests/all/harness.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    CLIENT_TOKEN_MISMATCH_MESSAGE, RESOURCE_TYPE_ACCOUNT, RESOURCE_TYPE_CLUSTER,
    RESOURCE_TYPE_STREAM,
};

use crate::cluster_policy::allow;
use crate::harness::{
    account_of, expected_arn, is_cluster_id, names_a_resource_of_its_own_type, wait_for_the_clock,
    Answer, Dsql,
};
use crate::stream_lifecycle::kinesis_target;

/// Which operation's reused-`clientToken` refusal a row of the table drives.
#[derive(Debug, Clone, Copy)]
enum TokenReuse {
    /// `CreateCluster`: the call would add to the clusters the ACCOUNT holds, and
    /// no cluster of the refused request was ever minted.
    CreateCluster,
    /// `UpdateCluster`: the call addressed one CLUSTER.
    UpdateCluster,
    /// `DeleteCluster`: the call addressed one CLUSTER — the one THIS delete
    /// named, not the one the token's first delete removed.
    DeleteCluster,
    /// `PutClusterPolicy`: the call addressed one CLUSTER's policy, which is
    /// addressed by that cluster's identifier.
    PutClusterPolicy,
    /// `DeleteClusterPolicy`: likewise one CLUSTER's policy.
    DeleteClusterPolicy,
    /// `CreateStream`: the call would add to the streams one CLUSTER holds, and no
    /// stream of the refused request exists.
    CreateStream,
    /// `DeleteStream`: the call addressed one STREAM — the one THIS delete named.
    DeleteStream,
}

/// What a row expects of the refusal: the addressed resource's own identity pair,
/// and the cluster the pair is resolved against.
struct Refused {
    answer: Answer,
    expected_type: String,
    expected_id: String,
    /// The cluster `GetStream` reads a reported stream on.
    cluster: String,
    /// An ARN this service minted for a resource it holds, which witnesses the
    /// account segment every ARN of this service carries.
    cluster_arn: String,
}

/// Token forms spanning `ClientToken`'s own bounds (`smithy_query(query="shape",
/// shape="ClientToken")`: `@length(1, 128)`, `@pattern("^[!-~]+$")`): the shortest
/// the length admits, an ordinary one, one built only from punctuation the pattern
/// admits, and the longest the length admits. Every form travels in a request body
/// for some rows and in a `client-token` query parameter for others, so none of
/// them carries a character a query string would reinterpret.
fn token_forms() -> [String; 4] {
    [
        "t".to_string(),
        "tok-1".to_string(),
        "~!.-_*".to_string(),
        "m".repeat(128),
    ]
}

/// One `CreateStream` request, distinct per `ordinal` so a fill creates distinct
/// streams.
fn stream_request(region: &str, ordinal: usize, token: Option<&str>) -> Value {
    let mut body = json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target(region, "changes", "dsql-cdc"),
        "tags": {"ordinal": ordinal.to_string()},
    });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

/// Assert `answer` was served, so the token really does name a request this
/// service recorded.
fn assert_served(case: &str, what: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: {what} must succeed, got {} {:?}",
        answer.status,
        answer.body
    );
}

/// Drive one operation's reused-`clientToken` refusal: a call the service serves
/// under `token`, then a DIFFERENT call of the same operation wearing it.
async fn drive(dsql: &Dsql, reuse: TokenReuse, token: &str, case: &str) -> Refused {
    let region = dsql.region().to_string();
    match reuse {
        TokenReuse::CreateCluster => {
            let first = dsql
                .create(json!({"deletionProtectionEnabled": false, "clientToken": token}))
                .await;
            assert_served(case, "the first CreateCluster", &first);
            let arn = first.str("arn");
            let answer = dsql
                .create(json!({"deletionProtectionEnabled": true, "clientToken": token}))
                .await;
            Refused {
                answer,
                // The holder of the account's cluster collection is the account,
                // identified by the id every ARN this service mints carries.
                expected_type: RESOURCE_TYPE_ACCOUNT.to_string(),
                expected_id: account_of(&arn),
                cluster: first.str("identifier"),
                cluster_arn: arn,
            }
        }
        TokenReuse::UpdateCluster => {
            let cluster = dsql.create_settled().await;
            let first = dsql
                .update(
                    &cluster,
                    json!({"deletionProtectionEnabled": true, "clientToken": token}),
                )
                .await;
            assert_served(case, "the first UpdateCluster", &first);
            let answer = dsql
                .update(
                    &cluster,
                    json!({"deletionProtectionEnabled": false, "clientToken": token}),
                )
                .await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
        TokenReuse::DeleteCluster => {
            let clusters = dsql.create_settled_many(2).await;
            let first = dsql.delete_with_token(&clusters[0], token).await;
            assert_served(case, "the first DeleteCluster", &first);
            // The same token carried by the delete of ANOTHER cluster.
            let answer = dsql.delete_with_token(&clusters[1], token).await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: clusters[1].clone(),
                cluster_arn: expected_arn(&region, &clusters[1]),
                cluster: clusters[1].clone(),
            }
        }
        TokenReuse::PutClusterPolicy => {
            let cluster = dsql.create_settled().await;
            let first = dsql
                .put_policy(
                    &cluster,
                    json!({"policy": allow("dsql:DbConnect"), "clientToken": token}),
                )
                .await;
            assert_served(case, "the first PutClusterPolicy", &first);
            // Another document under the same token is another request.
            let answer = dsql
                .put_policy(
                    &cluster,
                    json!({"policy": allow("dsql:DbConnectAdmin"), "clientToken": token}),
                )
                .await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
        TokenReuse::DeleteClusterPolicy => {
            let cluster = dsql.create_settled().await;
            let attached = dsql
                .put_policy(&cluster, json!({"policy": allow("dsql:DbConnect")}))
                .await;
            assert_served(case, "attaching a policy", &attached);
            let first = dsql
                .delete_policy(&cluster, &format!("?client-token={token}"))
                .await;
            assert_served(case, "the first DeleteClusterPolicy", &first);
            // A delete conditioned on a version the first one did not claim is
            // another request wearing that token.
            let answer = dsql
                .delete_policy(
                    &cluster,
                    &format!("?client-token={token}&expected-policy-version=1"),
                )
                .await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
        TokenReuse::CreateStream => {
            let cluster = dsql.create_settled().await;
            let first = dsql
                .create_stream(&cluster, stream_request(&region, 0, Some(token)))
                .await;
            assert_served(case, "the first CreateStream", &first);
            let answer = dsql
                .create_stream(&cluster, stream_request(&region, 1, Some(token)))
                .await;
            Refused {
                answer,
                // The holder of the cluster's stream collection is that cluster,
                // and no stream of the refused request exists.
                expected_type: RESOURCE_TYPE_CLUSTER.to_string(),
                expected_id: cluster.clone(),
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
        TokenReuse::DeleteStream => {
            let cluster = dsql.create_settled().await;
            let mut streams = Vec::new();
            for ordinal in 0..2 {
                let created = dsql
                    .create_stream(&cluster, stream_request(&region, ordinal, None))
                    .await;
                assert_served(case, "CreateStream", &created);
                streams.push(created.str("streamIdentifier"));
            }
            wait_for_the_clock().await;

            let query = format!("?client-token={token}");
            let first = dsql.delete_stream(&cluster, &streams[0], &query).await;
            assert_served(case, "the first DeleteStream", &first);
            // The same token carried by the delete of ANOTHER stream.
            let answer = dsql.delete_stream(&cluster, &streams[1], &query).await;
            Refused {
                answer,
                expected_type: RESOURCE_TYPE_STREAM.to_string(),
                expected_id: streams[1].clone(),
                cluster_arn: expected_arn(&region, &cluster),
                cluster,
            }
        }
    }
}

/// RULE (see the module docs): every reused-`clientToken` refusal reports the
/// resource the refused call addressed — the holder of the collection a create
/// would add to, the resource an update or a delete named — by that resource's own
/// type segment AND own identifier, both members filled.
#[tokio::test]
async fn every_token_conflict_names_one_whole_resource() {
    let rows = [
        TokenReuse::CreateCluster,
        TokenReuse::UpdateCluster,
        TokenReuse::DeleteCluster,
        TokenReuse::PutClusterPolicy,
        TokenReuse::DeleteClusterPolicy,
        TokenReuse::CreateStream,
        TokenReuse::DeleteStream,
    ];
    let tokens = token_forms();

    for (offset, region) in ["us-east-1", "eu-west-1"].into_iter().enumerate() {
        for (index, reuse) in rows.into_iter().enumerate() {
            // Each operation meets a different token form in each region, and
            // every form is driven against several operations, so no refusal is
            // pinned to one token form and no form goes unexercised.
            let token = &tokens[(index + offset * rows.len()) % tokens.len()];
            let case = format!("{reuse:?} in {region} under token {token:?}");
            let dsql = Dsql::in_region(region);
            let refused = drive(&dsql, reuse, token, &case).await;
            let answer = &refused.answer;

            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: the reused token must be refused with 409 ConflictException, got \
                 {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.message(),
                CLIENT_TOKEN_MISMATCH_MESSAGE,
                "case {case:?}: the refusal must be worded as a token reuse, got {:?}",
                answer.message()
            );

            // Both members of the pair, or the body describes no resource: a type
            // with no instance behind it is not an identity.
            for member in ["resourceType", "resourceId"] {
                let reported = answer.body.get(member).and_then(Value::as_str);
                assert!(
                    reported.is_some_and(|value| !value.is_empty()),
                    "case {case:?}: the refusal must fill {member}, got {:?}",
                    answer.body
                );
            }
            assert_eq!(
                (
                    answer.str("resourceType").as_str(),
                    answer.str("resourceId").as_str()
                ),
                (refused.expected_type.as_str(), refused.expected_id.as_str()),
                "case {case:?}: the refusal must report the addressed resource's own type and \
                 identifier, got {:?}",
                answer.body
            );
            assert!(
                names_a_resource_of_its_own_type(
                    &dsql,
                    &refused.cluster,
                    &refused.cluster_arn,
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

/// RULE (the same one, stated negatively for the create the bundle is about): a
/// `CreateCluster` refused for a reused `clientToken` minted no cluster, so the
/// identity it reports is the ACCOUNT that holds the clusters — an account id,
/// never a `ClusterId` (`smithy_query(query="shape", shape="ClusterId")`:
/// `^[a-z0-9]{26}$`) and never any cluster of the store — and the refused call
/// creates nothing.
#[tokio::test]
async fn the_create_cluster_token_conflict_names_no_cluster() {
    for (region, token) in [("us-east-1", "t"), ("ap-southeast-2", "~!.-_*")] {
        let case = format!("a reused token {token:?} in {region}");
        let dsql = Dsql::in_region(region);

        let first = dsql
            .create(json!({"deletionProtectionEnabled": false, "clientToken": token}))
            .await;
        assert_served(&case, "the first CreateCluster", &first);
        let minted = first.str("identifier");

        let refused = dsql
            .create(json!({"deletionProtectionEnabled": true, "clientToken": token}))
            .await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: the reused token must be refused with 409 ConflictException, got {} \
             {:?}",
            refused.status,
            refused.body
        );

        let reported = refused.str("resourceId");
        assert_eq!(
            refused.str("resourceType"),
            RESOURCE_TYPE_ACCOUNT,
            "case {case:?}: the holder of the account's clusters is the account, got {:?}",
            refused.body
        );
        assert_eq!(
            reported,
            account_of(&first.str("arn")),
            "case {case:?}: the account reported is the one every ARN this service mints carries, \
             got {reported:?}"
        );
        assert!(
            !is_cluster_id(&reported),
            "case {case:?}: an account id is no ClusterId, got {reported:?}"
        );
        assert_ne!(
            reported, minted,
            "case {case:?}: the refusal is not about the cluster the first call minted, got {:?}",
            refused.body
        );
        let read_back = dsql.get(&reported).await;
        assert_ne!(
            read_back.status, 200,
            "case {case:?}: the reported id is no cluster of this service, so GetCluster must not \
             resolve it, got {:?}",
            read_back.body
        );

        // The refused create added nothing: the store still holds exactly the one
        // cluster the served call minted.
        let listed = dsql.list("?max-results=100").await;
        let held = listed
            .get("clusters")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, 1,
            "case {case:?}: the refused create must have created nothing, got {held} clusters"
        );
    }
}
