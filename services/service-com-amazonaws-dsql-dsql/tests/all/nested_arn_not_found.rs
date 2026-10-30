// SPDX-License-Identifier: CC0-1.0

//! Which resource a `ResourceNotFoundException` NAMES when the ARN it refused
//! addresses a resource nested under another one, end to end over the wire.
//!
//! RULE under test: a refusal that identifies a resource carries that resource's OWN
//! type segment and its OWN identifier, and the resource it identifies is the one the
//! lookup stopped at. For a stream of a cluster — the one nested resource of this
//! service, addressed as `cluster/{cluster}/stream/{stream}` — that means the stream's
//! type and its own identifier when the cluster is held and the stream under it is
//! not, and the cluster's type and identifier when the cluster itself is not held;
//! never a parent's type segment carrying the rest of the resource path for an
//! identifier.
//!
//! The rule is the service's, not one operation's, so every row is put to all three
//! operations bound to `/tags/{resourceArn}` (`ListTagsForResource`, `TagResource`,
//! `UntagResource`), and the identity each answers is compared with what `GetStream`
//! — the identifier-addressed reader of the very same resources — reports for the
//! same absent resource. Two paths describing one absent resource must describe it
//! identically.
//!
//! The table spans the class the rule generalises over: which level of the nested path
//! is missing (the stream, the cluster), the stream identifier (two never-created
//! ones, so no row depends on a particular value), how the cluster came to be absent
//! (never minted, retired by a completed deletion, in a scope this endpoint serves
//! none of), and the non-nested forms the same rule covers (a plain cluster ARN, a
//! top-level ARN of a resource type this service holds none of, and a nested path this
//! service mints no resource for).
//!
//! The client is the shared `Dsql` harness in `harness.rs`; `UntagResource`'s query
//! encoding is the one the `resource_tags` suite spells.

use serde_json::{json, Value};

use crate::harness::{
    absent_identifier, expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql,
};
use crate::resource_tags::untag;

/// The region every request in this suite is signed for.
const HOME: &str = "us-east-1";

/// A region whose resources this endpoint holds none of.
const AWAY: &str = "eu-west-1";

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream` requires.
/// Source: `smithy_query(query="shape", shape="KinesisTargetDefinition")` — a
/// `streamArn` and a `roleArn`.
fn kinesis_target(region: &str) -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:{region}:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// Two never-created stream identifiers, both spelled as a stream identifier is
/// spelled — the 26-character base32 rendering of a 128-bit value, which
/// `StreamArn`'s pattern admits (source: `smithy_query(query="shape",
/// shape="StreamArn")` — the stream segment is `[a-z0-9]{26}`) — so no assertion
/// depends on one particular value.
fn absent_stream_identifiers() -> [String; 2] {
    [
        absent_identifier(),
        "0123456789abcdefghjkmnpqrs".to_string(),
    ]
}

/// A never-created CLUSTER identifier, spelled the same way and distinct from
/// every stream identifier in this suite: a row about a missing cluster must be
/// able to tell the cluster's identity from the stream's.
fn absent_cluster_identifier() -> String {
    "4zzzzzzzzzzzzzzzzzzzzzzzzz".to_string()
}

/// The resource identity an answer reports: the two `@required` members of
/// `ResourceNotFoundException` that name the resource.
fn reported_identity(answer: &Answer) -> (String, String) {
    (answer.str("resourceType"), answer.str("resourceId"))
}

/// The cluster and the stream this suite refuses ARNs around.
struct Held {
    cluster: String,
    stream: String,
    stream_arn: String,
}

/// A settled cluster carrying one stream, so a row about a missing stream is about
/// that stream and not about a cluster that holds no streams at all.
async fn held_resources(dsql: &Dsql) -> Held {
    let cluster = dsql.create_settled().await;
    let created = dsql
        .create_stream(
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": kinesis_target(HOME),
                "tags": {"env": "prod"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateStream must answer 200, got {} {:?}",
        created.status, created.body
    );
    wait_for_the_clock().await;
    Held {
        cluster,
        stream: created.str("streamIdentifier"),
        stream_arn: created.str("arn"),
    }
}

/// One refused ARN and the identity every refusal of it must carry.
struct NotFoundCase {
    name: String,
    arn: String,
    resource_type: &'static str,
    resource_id: String,
}

/// RULE: the refusal of an ARN that addresses a nested resource names the resource the
/// lookup stopped at, by that resource's own type segment and its own identifier — and
/// the three operations bound to `/tags/{resourceArn}` all name the same one.
#[tokio::test]
async fn a_nested_arn_is_refused_naming_the_resource_that_is_missing() {
    let dsql = Dsql::new();
    let held = held_resources(&dsql).await;

    // A cluster whose deletion the clock has already completed: nothing under its ARN
    // resolves afterwards.
    let retired = dsql.create_settled().await;
    let deleted = dsql.delete(&retired).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteCluster must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    wait_for_the_clock().await;

    let absent_cluster = absent_cluster_identifier();
    let [missing_a, missing_b] = absent_stream_identifiers();
    assert_ne!(
        absent_cluster, missing_a,
        "the absent cluster and the absent stream must be told apart by their ids"
    );

    let mut cases = vec![
        NotFoundCase {
            name: "a stream never created, under a live cluster that holds another stream"
                .to_string(),
            arn: expected_stream_arn(HOME, &held.cluster, &missing_a),
            resource_type: "stream",
            resource_id: missing_a.clone(),
        },
        NotFoundCase {
            name: "another never-created stream of the same live cluster".to_string(),
            arn: expected_stream_arn(HOME, &held.cluster, &missing_b),
            resource_type: "stream",
            resource_id: missing_b.clone(),
        },
        NotFoundCase {
            name: "a stream of a cluster this service never minted".to_string(),
            arn: expected_stream_arn(HOME, &absent_cluster, &missing_a),
            resource_type: "cluster",
            resource_id: absent_cluster.clone(),
        },
        NotFoundCase {
            name: "a stream of a cluster whose deletion the clock completed".to_string(),
            arn: expected_stream_arn(HOME, &retired, &missing_a),
            resource_type: "cluster",
            resource_id: retired.clone(),
        },
        NotFoundCase {
            name: "a stream path spelled for a region this endpoint serves none of".to_string(),
            arn: expected_stream_arn(AWAY, &held.cluster, &held.stream),
            resource_type: "cluster",
            resource_id: held.cluster.clone(),
        },
        // The non-nested forms the same rule covers: there the type segment the ARN
        // was split at IS the addressed resource's own type.
        NotFoundCase {
            name: "a plain cluster ARN this service never minted".to_string(),
            arn: expected_arn(HOME, &absent_cluster),
            resource_type: "cluster",
            resource_id: absent_cluster.clone(),
        },
        NotFoundCase {
            name: "a top-level ARN of a resource type this service holds none of".to_string(),
            arn: format!(
                "arn:aws:dsql:{HOME}:{}:stream/{missing_a}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            resource_type: "stream",
            resource_id: missing_a.clone(),
        },
        // A path this service mints no resource for names no resource of it to report
        // instead, so it is reported exactly as the ARN spelled it.
        NotFoundCase {
            name: "a nested path this service mints no resource for".to_string(),
            arn: format!(
                "arn:aws:dsql:{HOME}:{}:cluster/{}/snapshot/{missing_a}",
                engine::EMULATOR_ACCOUNT_ID, held.cluster
            ),
            resource_type: "cluster",
            resource_id: format!("{}/snapshot/{missing_a}", held.cluster),
        },
    ];
    cases.sort_by(|left, right| left.name.cmp(&right.name));

    for case in &cases {
        let answers = [
            (
                "ListTagsForResource",
                dsql.list_tags_for_resource(&case.arn).await,
            ),
            (
                "TagResource",
                dsql.tag_resource(&case.arn, json!({"tags": {"env": "dev"}}))
                    .await,
            ),
            (
                "UntagResource",
                untag(&dsql, &case.arn, &["env".to_string()]).await,
            ),
        ];

        for (operation, answer) in &answers {
            assert_eq!(
                answer.status, 404,
                "case {:?}: {operation} must answer 404, got {} {:?}",
                case.name, answer.status, answer.body
            );
            assert!(
                answer
                    .error
                    .as_deref()
                    .is_some_and(|code| code.contains("ResourceNotFoundException")),
                "case {:?}: {operation}'s wire code must be ResourceNotFoundException, got {:?}",
                case.name,
                answer.error
            );
            assert_eq!(
                reported_identity(answer),
                (case.resource_type.to_string(), case.resource_id.clone()),
                "case {:?}: {operation} must name ({:?}, {:?}), got {:?}",
                case.name,
                case.resource_type,
                case.resource_id,
                reported_identity(answer)
            );
            assert!(
                answer.message().contains(&case.arn),
                "case {:?}: {operation}'s message must name the ARN the request carried ({:?}), \
                 got {:?}",
                case.name,
                case.arn,
                answer.message()
            );
        }
    }

    // Control: the stream that DOES exist is still served through the very same nested
    // ARN form, so every refusal above is about an absent resource and not about the
    // nesting — and no refused call wrote or removed a tag on it.
    let served = dsql.list_tags_for_resource(&held.stream_arn).await;
    assert_eq!(
        served.status, 200,
        "case {:?}: the held stream's own nested ARN must answer 200, got {} {:?}",
        "the held stream", served.status, served.body
    );
    assert_eq!(
        served.get("tags"),
        json!({"env": "prod"}),
        "case {:?}: the held stream must keep its own tags, got {:?}",
        "the held stream",
        served.get("tags")
    );
}

/// RULE, the other half: the identity a tagging refusal reports for an absent resource
/// is the identity the identifier-addressed reader of the same resource reports, so one
/// absent resource is described one way whichever path refused.
#[tokio::test]
async fn the_tagging_and_the_stream_reader_name_one_absent_resource_alike() {
    let dsql = Dsql::new();
    let held = held_resources(&dsql).await;
    let absent_cluster = absent_cluster_identifier();
    let [missing_a, missing_b] = absent_stream_identifiers();

    // (case, cluster, stream): a missing stream of a held cluster — twice, so the row
    // does not depend on one identifier — and a stream of a cluster that is missing
    // too, where the cluster is what the lookup stops at.
    let cases: &[(&str, &String, &String)] = &[
        (
            "a never-created stream of a live cluster",
            &held.cluster,
            &missing_a,
        ),
        (
            "another never-created stream of the same cluster",
            &held.cluster,
            &missing_b,
        ),
        (
            "a stream of a cluster that was never created either",
            &absent_cluster,
            &missing_a,
        ),
    ];

    for (case, cluster, stream) in cases {
        let by_identifier = dsql.get_stream(cluster, stream).await;
        assert_eq!(
            by_identifier.status, 404,
            "case {case:?}: GetStream must answer 404, got {} {:?}",
            by_identifier.status, by_identifier.body
        );

        let by_arn = dsql
            .list_tags_for_resource(&expected_stream_arn(HOME, cluster, stream))
            .await;
        assert_eq!(
            by_arn.status, 404,
            "case {case:?}: ListTagsForResource must answer 404, got {} {:?}",
            by_arn.status, by_arn.body
        );
        assert_eq!(
            reported_identity(&by_arn),
            reported_identity(&by_identifier),
            "case {case:?}: both paths must name one resource, got {:?} by ARN and {:?} by \
             identifier",
            reported_identity(&by_arn),
            reported_identity(&by_identifier)
        );
    }
}
