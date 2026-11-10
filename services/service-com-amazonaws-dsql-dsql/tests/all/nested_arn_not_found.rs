// SPDX-License-Identifier: CC0-1.0

//! How an ARN that addresses a resource nested under another one is refused when the
//! resource is not there, end to end over the wire.
//!
//! RULE under test: the two ways a resource is addressed are refused by two different
//! surfaces. The operations bound to `/tags/{resourceArn}` (`ListTagsForResource`,
//! `TagResource`, `UntagResource`) are fronted by the ARN-addressed tagging surface,
//! which states one refusal for every resource it resolves nothing for — HTTP 404,
//! wire code `NotFoundException`, the fixed sentence "Resource was not found" and no
//! other member — so WHICH level of a nested path (`cluster/{cluster}/stream/{stream}`,
//! the one nested resource of this service) is missing does not change the answer, and
//! nothing from the request is echoed back. The operations addressed by an IDENTIFIER
//! answer the modelled `ResourceNotFoundException` instead, naming the resource the
//! lookup stopped at by that resource's OWN type segment and its OWN identifier.
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

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{
    absent_identifier, expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql,
};
use crate::resource_tags::untag;

/// The region every request in this suite is signed for.
const HOME: &str = "us-east-1";

/// A region whose resources this endpoint holds none of.
const AWAY: &str = "eu-west-1";

/// The wire code and the sentence the ARN-addressed tagging surface refuses an ARN it
/// resolves nothing for with. Recorded behaviour: that surface answers in its own
/// terms rather than with the `ResourceNotFoundException` the model lists on those
/// operations, and its sentence interpolates nothing.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
/// See [`TAGGING_NOT_FOUND_CODE`].
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The members that refusal's body may carry: the sentence, in either casing the
/// restJson1 envelope allows, plus the envelope's own discriminator. A member outside
/// this set would mean some other shape was serialized.
const TAGGING_NOT_FOUND_BODY_MEMBERS: &[&str] = &["__type", "message", "Message"];

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
/// spelled — 26 characters of the base32 alphabet, which `StreamArn`'s pattern
/// admits (source: `smithy_query(query="shape", shape="StreamArn")` — the stream
/// segment is `[a-z0-9]{26}`) — so no assertion depends on one particular value.
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

/// The members a body carries, so a body can be compared against a modelled set.
fn body_members(answer: &Answer) -> BTreeSet<String> {
    answer
        .body
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
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

/// One refused ARN, and which level of its path is the resource that is missing.
struct NotFoundCase {
    name: String,
    arn: String,
}

/// Assert one answer IS the tagging surface's own not-found: its status, its code,
/// its fixed sentence, and no member besides the sentence — and nothing the request
/// carried echoed back into it.
fn assert_tagging_surface_not_found(case: &str, arn: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an ARN naming no resource must answer 404, got {} {:?}",
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
        "case {case:?}: expected the fixed sentence {TAGGING_NOT_FOUND_MESSAGE:?}, got {:?}",
        answer.message()
    );
    assert!(
        !answer.message().contains(arn),
        "case {case:?}: the sentence must echo nothing from the request ({arn:?}), got {:?}",
        answer.message()
    );
    let unexpected: Vec<String> = body_members(answer)
        .into_iter()
        .filter(|member| !TAGGING_NOT_FOUND_BODY_MEMBERS.contains(&member.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "case {case:?}: the body must carry only {TAGGING_NOT_FOUND_BODY_MEMBERS:?}, got the \
         extra {unexpected:?} in {:?}",
        answer.body
    );
}

/// RULE: every operation bound to `/tags/{resourceArn}` refuses an ARN it resolves
/// nothing for with the tagging surface's own 404 — the same status, code, sentence
/// and body whichever level of a nested path is missing, and whichever of the three
/// operations answered.
#[tokio::test]
async fn a_nested_arn_is_refused_by_the_tagging_surface_whatever_is_missing() {
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
        },
        NotFoundCase {
            name: "another never-created stream of the same live cluster".to_string(),
            arn: expected_stream_arn(HOME, &held.cluster, &missing_b),
        },
        NotFoundCase {
            name: "a stream of a cluster this service never minted".to_string(),
            arn: expected_stream_arn(HOME, &absent_cluster, &missing_a),
        },
        NotFoundCase {
            name: "a stream of a cluster whose deletion the clock completed".to_string(),
            arn: expected_stream_arn(HOME, &retired, &missing_a),
        },
        NotFoundCase {
            name: "a stream path spelled for a region this endpoint serves none of".to_string(),
            arn: expected_stream_arn(AWAY, &held.cluster, &held.stream),
        },
        // The non-nested forms the same rule covers.
        NotFoundCase {
            name: "a plain cluster ARN this service never minted".to_string(),
            arn: expected_arn(HOME, &absent_cluster),
        },
        NotFoundCase {
            name: "a top-level ARN of a resource type this service holds none of".to_string(),
            arn: format!(
                "arn:aws:dsql:{HOME}:{}:stream/{missing_a}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        },
        NotFoundCase {
            name: "a nested path this service mints no resource for".to_string(),
            arn: format!(
                "arn:aws:dsql:{HOME}:{}:cluster/{}/snapshot/{missing_a}",
                engine::EMULATOR_ACCOUNT_ID,
                held.cluster
            ),
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
            assert_tagging_surface_not_found(
                &format!("{operation} with {}", case.name),
                &case.arn,
                answer,
            );
        }

        // One logical failure, so the three operations answer it identically.
        let (first_operation, first) = &answers[0];
        for (operation, answer) in &answers[1..] {
            assert_eq!(
                (answer.status, answer.error.as_deref(), answer.message()),
                (first.status, first.error.as_deref(), first.message()),
                "case {:?}: {operation} must answer exactly as {first_operation}, got {} {:?} \
                 against {} {:?}",
                case.name,
                answer.status,
                answer.body,
                first.status,
                first.body
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

/// RULE, the other half: the IDENTIFIER-addressed reader of the very same resources
/// answers the modelled `ResourceNotFoundException`, naming the resource the lookup
/// stopped at — the stream when the cluster is held and the stream under it is not,
/// the cluster when the cluster itself is not held — while the tagging surface states
/// its own refusal for that same absent resource, naming no resource at all.
#[tokio::test]
async fn the_identifier_reader_names_the_resource_the_tagging_surface_does_not() {
    let dsql = Dsql::new();
    let held = held_resources(&dsql).await;
    let absent_cluster = absent_cluster_identifier();
    let [missing_a, missing_b] = absent_stream_identifiers();

    // (case, cluster, stream, the identity the identifier-addressed reader reports):
    // a missing stream of a held cluster — twice, so the row does not depend on one
    // identifier — and a stream of a cluster that is missing too, where the cluster is
    // what the lookup stops at.
    let cases: &[(&str, &String, &String, (&str, &String))] = &[
        (
            "a never-created stream of a live cluster",
            &held.cluster,
            &missing_a,
            ("stream", &missing_a),
        ),
        (
            "another never-created stream of the same cluster",
            &held.cluster,
            &missing_b,
            ("stream", &missing_b),
        ),
        (
            "a stream of a cluster that was never created either",
            &absent_cluster,
            &missing_a,
            ("cluster", &absent_cluster),
        ),
    ];

    for (case, cluster, stream, (resource_type, resource_id)) in cases {
        let by_identifier = dsql.get_stream(cluster, stream).await;
        assert_eq!(
            by_identifier.status, 404,
            "case {case:?}: GetStream must answer 404, got {} {:?}",
            by_identifier.status, by_identifier.body
        );
        assert_eq!(
            by_identifier.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: GetStream's 404 must be the modelled ResourceNotFoundException, \
             got {:?}",
            by_identifier.error
        );
        assert_eq!(
            reported_identity(&by_identifier),
            ((*resource_type).to_string(), (*resource_id).clone()),
            "case {case:?}: GetStream must name ({resource_type:?}, {resource_id:?}), got {:?}",
            reported_identity(&by_identifier)
        );

        let arn = expected_stream_arn(HOME, cluster, stream);
        let by_arn = dsql.list_tags_for_resource(&arn).await;
        assert_tagging_surface_not_found(
            &format!("ListTagsForResource with {case}"),
            &arn,
            &by_arn,
        );
        assert_ne!(
            by_arn.error, by_identifier.error,
            "case {case:?}: the two surfaces state different codes, got {:?} both times",
            by_arn.error
        );
    }
}
