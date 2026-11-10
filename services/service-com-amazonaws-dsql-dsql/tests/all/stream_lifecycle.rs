// SPDX-License-Identifier: CC0-1.0

//! The Aurora DSQL change data capture (CDC) stream lifecycle, end to end over
//! the wire.
//!
//! Every case drives the real request path through `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request, so the `/stream/{clusterIdentifier}`
//! routes, the `@httpLabel` bindings, the boundary constraint checks and the
//! handlers are exercised the way a client exercises them. The wire harness
//! (`Dsql`, `Answer`, the clock wait) is the one in `tests/all/harness.rs` that
//! every suite in this crate shares.
//!
//! The tables assert RULES over a CLASS of inputs rather than one recorded
//! request: a stream minted and persisted for several regions, tag sets, Kinesis
//! targets and every published `StreamFormat` / `StreamOrdering`; the idempotency
//! of a repeated `clientToken` over several tokens and its refusal when the same
//! token carries different parameters; the pass-role rule for the target's
//! `roleArn`; the refusals for the whole class of unknown and malformed
//! identifiers; and the cluster-scoping of a stream.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::MAX_STREAMS_PER_CLUSTER;

use crate::harness::{absent_identifier, is_cluster_id, wait_for_the_clock, Answer, Dsql};

/// The values `StreamFormat` accepts on the wire. Source: the service's own
/// model, `smithy_query(query="shape", shape="StreamFormat")`.
const STREAM_FORMAT_VALUES: &[&str] = &["JSON"];

/// The values `StreamOrdering` accepts on the wire. Source:
/// `smithy_query(query="shape", shape="StreamOrdering")`.
const STREAM_ORDERING_VALUES: &[&str] = &["UNORDERED"];

/// The values `StreamStatus` accepts on the wire. Source:
/// `smithy_query(query="shape", shape="StreamStatus")`.
const STREAM_STATUS_VALUES: &[&str] = &[
    "CREATING",
    "ACTIVE",
    "DELETING",
    "DELETED",
    "FAILED",
    "IMPAIRED",
];

/// `CreateStream`: `POST /stream/{clusterIdentifier}`.
async fn create_stream(dsql: &Dsql, cluster: &str, body: Value) -> Answer {
    dsql.create_stream(cluster, body).await
}

/// `GetStream`: `GET /stream/{clusterIdentifier}/{streamIdentifier}`.
async fn get_stream(dsql: &Dsql, cluster: &str, stream: &str) -> Answer {
    dsql.get_stream(cluster, stream).await
}

/// A Kinesis stream ARN, as `KinesisStreamArn`'s pattern requires
/// (`^arn:aws[a-zA-Z-]*:kinesis:[a-z0-9-]*:[0-9]{12}:stream/[a-zA-Z0-9+=,.@_/\-]+$`).
fn kinesis_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:kinesis:{region}:{}:stream/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// An IAM role ARN, as `RoleArn`'s pattern requires
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_-]+)+$`).
fn role_arn(path_and_name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{path_and_name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN `StreamArn`'s pattern requires for `stream` of `cluster` in `region`.
fn expected_stream_arn(region: &str, cluster: &str, stream: &str) -> String {
    format!(
        "arn:aws:dsql:{region}:{}:cluster/{cluster}/stream/{stream}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `targetDefinition` naming a Kinesis destination. Shared with the suites that
/// need a well-formed target rather than a target to scrutinise.
pub fn kinesis_target(region: &str, stream_name: &str, role: &str) -> Value {
    json!({"kinesis": {"streamArn": kinesis_arn(region, stream_name), "roleArn": role_arn(role)}})
}

/// One `CreateStream` request and what the persisted stream must then report.
struct CreateCase {
    name: &'static str,
    region: &'static str,
    format: &'static str,
    ordering: &'static str,
    target: Value,
    /// The `tags` the request carries, or `Value::Null` to omit the member.
    tags: Value,
    /// The `tags` a read must report back: an omitted or empty map is reported
    /// as no map at all.
    expected_tags: Value,
}

fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "no optional members at all",
            region: "us-east-1",
            format: "JSON",
            ordering: "UNORDERED",
            target: kinesis_target("us-east-1", "changes", "dsql-cdc"),
            tags: Value::Null,
            expected_tags: Value::Null,
        },
        CreateCase {
            name: "one tag, a role with a path prefix",
            region: "eu-west-1",
            format: "JSON",
            ordering: "UNORDERED",
            target: kinesis_target("eu-west-1", "cdc.stream-1", "service-role/dsql-cdc"),
            tags: json!({"env": "test"}),
            expected_tags: json!({"env": "test"}),
        },
        CreateCase {
            name: "several tags, a Kinesis stream name with every allowed form",
            region: "ap-southeast-2",
            format: "JSON",
            ordering: "UNORDERED",
            target: kinesis_target("ap-southeast-2", "a.b-c_d+e=f,g@h/i", "team-a/dsql-cdc"),
            tags: json!({"env": "prod", "owner": "team-a", "cost-centre": "42"}),
            expected_tags: json!({"env": "prod", "owner": "team-a", "cost-centre": "42"}),
        },
        CreateCase {
            name: "an empty tag value, which TagValue's length 0..256 allows",
            region: "us-west-2",
            format: "JSON",
            ordering: "UNORDERED",
            target: kinesis_target("us-west-2", "changes", "dsql-cdc"),
            tags: json!({"solo": ""}),
            expected_tags: json!({"solo": ""}),
        },
        CreateCase {
            name: "an empty tag map, which is reported as no tags",
            region: "us-east-2",
            format: "JSON",
            ordering: "UNORDERED",
            target: kinesis_target("us-east-2", "changes", "dsql-cdc"),
            tags: json!({}),
            expected_tags: Value::Null,
        },
    ]
}

impl CreateCase {
    /// The request body this case sends.
    fn request(&self) -> Value {
        let mut body = json!({
            "format": self.format,
            "ordering": self.ordering,
            "targetDefinition": self.target.clone(),
        });
        if !self.tags.is_null() {
            body["tags"] = self.tags.clone();
        }
        body
    }
}

/// RULE: `CreateStream` mints and PERSISTS a stream on the named cluster, and
/// `GetStream` reports back exactly what was persisted — for every combination of
/// optional members and in every region.
#[tokio::test]
async fn create_persists_a_stream_that_get_reports_back() {
    for case in create_cases() {
        let dsql = Dsql::in_region(case.region);
        let cluster = dsql.create_settled().await;

        let created = create_stream(&dsql, &cluster, case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateStream must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );

        let stream = created.str("streamIdentifier");
        assert!(
            is_cluster_id(&stream),
            "case {:?}: streamIdentifier must match ^[a-z0-9]{{26}}$, got {stream:?}",
            case.name
        );
        assert_ne!(
            stream, cluster,
            "case {:?}: the stream must have an identifier of its own, got {stream:?}",
            case.name
        );
        assert_eq!(
            created.str("clusterIdentifier"),
            cluster,
            "case {:?}: CreateStream must report the cluster it created the stream on",
            case.name
        );
        assert_eq!(
            created.str("arn"),
            expected_stream_arn(case.region, &cluster, &stream),
            "case {:?}: arn must nest the minted stream under its cluster in the \
             signed region, got {:?}",
            case.name,
            created.str("arn")
        );
        assert_eq!(
            created.str("status"),
            "CREATING",
            "case {:?}: a freshly minted stream reports CREATING, got {:?}",
            case.name,
            created.str("status")
        );
        assert_eq!(
            created.str("format"),
            case.format,
            "case {:?}: format must be the requested one",
            case.name
        );
        assert_eq!(
            created.str("ordering"),
            case.ordering,
            "case {:?}: ordering must be the requested one",
            case.name
        );
        let creation_time = created.get("creationTime");
        assert!(
            creation_time.as_f64().is_some_and(|t| t > 0.0),
            "case {:?}: creationTime must be a timestamp, got {creation_time:?}",
            case.name
        );

        // The round trip: everything above came out of the store, so a read
        // through GetStream reports the same values.
        let fetched = get_stream(&dsql, &cluster, &stream).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: GetStream on the created stream must answer 200, got {} {:?}",
            case.name, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.str("streamIdentifier"),
            stream,
            "case {:?}: GetStream must report the stored streamIdentifier",
            case.name
        );
        assert_eq!(
            fetched.str("clusterIdentifier"),
            cluster,
            "case {:?}: GetStream must report the stored clusterIdentifier",
            case.name
        );
        assert_eq!(
            fetched.str("arn"),
            created.str("arn"),
            "case {:?}: GetStream must report the stored arn, got {:?}",
            case.name,
            fetched.str("arn")
        );
        assert_eq!(
            fetched.get("creationTime"),
            creation_time,
            "case {:?}: creationTime must not change between reads",
            case.name
        );
        assert_eq!(
            fetched.get("targetDefinition"),
            case.target,
            "case {:?}: GetStream must report the stored targetDefinition, got {:?}",
            case.name,
            fetched.get("targetDefinition")
        );
        assert_eq!(
            fetched.get("tags"),
            case.expected_tags,
            "case {:?}: GetStream must report the tags supplied at create time, got {:?}",
            case.name,
            fetched.get("tags")
        );
        assert_eq!(
            fetched.get("statusReason"),
            Value::Null,
            "case {:?}: a stream that has not failed reports no statusReason, got {:?}",
            case.name,
            fetched.get("statusReason")
        );
        assert!(
            STREAM_STATUS_VALUES.contains(&fetched.str("status").as_str()),
            "case {:?}: GetStream must report a modeled StreamStatus, got {:?}",
            case.name,
            fetched.str("status")
        );

        // A read changes nothing a later caller sees.
        let again = get_stream(&dsql, &cluster, &stream).await;
        assert_eq!(
            again.body,
            fetched.body,
            "case {:?}: two reads of the same stream must report the same state",
            case.name
        );
    }
}

/// RULE: the IAM role in the target has to be one this account can pass — a role
/// in another account cannot have its trust relationship with the service
/// established, so the call is refused as an AUTHORIZATION failure (403
/// `AccessDeniedException`), not as a malformed request, and nothing is persisted.
#[tokio::test]
async fn a_role_this_account_cannot_pass_is_refused_as_access_denied() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    // Well-formed `RoleArn`s (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role(/...)+$`)
    // whose account field is NOT this emulator's.
    let foreign_accounts = ["123456789012", "999999999999", "000000000001"];
    for account in foreign_accounts {
        let answer = create_stream(
            &dsql,
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": {"kinesis": {
                    "streamArn": kinesis_arn("us-east-1", "changes"),
                    "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
                }},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 403,
            "case account={account}: a cross-account roleArn must be refused with 403, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("AccessDeniedException"),
            "case account={account}: the refusal must be an AccessDeniedException, got {:?}",
            answer.error
        );
    }

    // The same request with a role of this account is accepted, so the refusals
    // above are about the role and nothing else.
    let accepted = create_stream(
        &dsql,
        &cluster,
        json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a role in this account must be passable, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// RULE: every value `StreamFormat` and `StreamOrdering` publish is accepted and
/// reported back, by `CreateStream` and by the read that follows it.
#[tokio::test]
async fn every_published_format_and_ordering_round_trips() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    for format in STREAM_FORMAT_VALUES {
        for ordering in STREAM_ORDERING_VALUES {
            let case = format!("format={format} ordering={ordering}");
            let created = create_stream(
                &dsql,
                &cluster,
                json!({
                    "format": format,
                    "ordering": ordering,
                    "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case}: a published enum value must be accepted, got {} {:?}",
                created.status, created.body
            );

            let fetched =
                get_stream(&dsql, &cluster, &created.str("streamIdentifier")).await;
            assert_eq!(
                fetched.str("format"),
                *format,
                "case {case}: GetStream must report the stored format, got {:?}",
                fetched.str("format")
            );
            assert_eq!(
                fetched.str("ordering"),
                *ordering,
                "case {case}: GetStream must report the stored ordering, got {:?}",
                fetched.str("ordering")
            );
        }
    }
}

/// RULE: a repeat of a `CreateStream` carrying the same `clientToken` returns the
/// stream the first call minted and has no additional effect; a different token
/// mints a different stream.
#[tokio::test]
async fn a_repeated_client_token_returns_the_first_stream() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let target = kinesis_target("us-east-1", "changes", "dsql-cdc");

    // `ClientToken`: length 1..128, pattern ^[!-~]+$. Source:
    // `smithy_query(query="operation", operation="CreateStream")`.
    let tokens = [
        ("one character", "a".to_string()),
        ("a word", "token-2".to_string()),
        ("punctuation only", "!~#$%&*".to_string()),
        ("the maximum length", "t".repeat(128)),
    ];

    let mut minted = Vec::new();
    for (name, token) in &tokens {
        let request = json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": target.clone(),
            "clientToken": token,
        });

        let first = create_stream(&dsql, &cluster, request.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first CreateStream must answer 200, got {} {:?}",
            first.status, first.body
        );
        let repeat = create_stream(&dsql, &cluster, request).await;
        assert_eq!(
            repeat.status, 200,
            "case {name:?}: the repeat must answer 200, got {} {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.str("streamIdentifier"),
            first.str("streamIdentifier"),
            "case {name:?}: the repeat must return the stream the first call minted, got {:?}",
            repeat.str("streamIdentifier")
        );
        assert_eq!(
            repeat.get("creationTime"),
            first.get("creationTime"),
            "case {name:?}: the repeat must not re-stamp creationTime"
        );
        minted.push(first.str("streamIdentifier"));
    }

    let distinct: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        tokens.len(),
        "each distinct clientToken must mint its own stream, got {minted:?}"
    );
}

/// RULE: a `clientToken` names ONE request, so reusing it for a DIFFERENT request
/// is a conflict (409 `ConflictException`) rather than a retry — whichever member
/// of the request differs.
#[tokio::test]
async fn a_client_token_reused_with_different_parameters_is_a_conflict() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let token = "token-for-one-request";
    let first = create_stream(
        &dsql,
        &cluster,
        json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
            "tags": {"env": "test"},
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first CreateStream must answer 200, got {} {:?}",
        first.status, first.body
    );

    // One member differs per row: the Kinesis stream, the role, the tags.
    let mismatches = [
        (
            "a different Kinesis stream",
            kinesis_target("us-east-1", "other-changes", "dsql-cdc"),
            json!({"env": "test"}),
        ),
        (
            "a different role",
            kinesis_target("us-east-1", "changes", "other/dsql-cdc"),
            json!({"env": "test"}),
        ),
        (
            "different tags",
            kinesis_target("us-east-1", "changes", "dsql-cdc"),
            json!({"env": "prod"}),
        ),
    ];

    for (name, target, tags) in mismatches {
        let answer = create_stream(
            &dsql,
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": target,
                "tags": tags,
                "clientToken": token,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {name:?}: a token reused with different parameters must answer 409, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ConflictException"),
            "case {name:?}: the refusal must be a ConflictException, got {:?}",
            answer.error
        );
    }

    // The first stream is untouched, and the original request still replays.
    let replay = create_stream(
        &dsql,
        &cluster,
        json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
            "tags": {"env": "test"},
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        replay.str("streamIdentifier"),
        first.str("streamIdentifier"),
        "the original request must still replay to the stream it minted, got {:?}",
        replay.str("streamIdentifier")
    );
}

/// RULE: a stream operation naming a cluster this service does not hold answers
/// 404 `ResourceNotFoundException` rather than creating or reporting anything.
#[tokio::test]
async fn an_unknown_cluster_is_not_found() {
    let dsql = Dsql::new();
    let target = kinesis_target("us-east-1", "changes", "dsql-cdc");
    // Well-formed identifiers — 26 characters of the base32 alphabet — for
    // clusters that were never created.
    let absent = [
        absent_identifier(),
        "00000000000000000000000000".to_string(),
        "7zzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
    ];

    for cluster in absent {
        let created = create_stream(
            &dsql,
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": target.clone(),
            }),
        )
        .await;
        for (operation, answer) in [
            ("CreateStream", created),
            (
                "GetStream",
                get_stream(&dsql, &cluster, &absent_identifier()).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {cluster:?} on {operation}: an unknown cluster must answer 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {cluster:?} on {operation}: the 404 must be a ResourceNotFoundException, \
                 got {:?}",
                answer.error
            );
        }
    }
}

/// RULE: `GetStream` for a stream the named cluster does not hold answers 404
/// `ResourceNotFoundException` — including a stream that belongs to a DIFFERENT
/// cluster, since a stream is scoped to the cluster it captures.
#[tokio::test]
async fn an_unknown_stream_of_a_known_cluster_is_not_found() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let other_cluster = dsql.create_settled().await;
    let created = create_stream(
        &dsql,
        &cluster,
        json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
        }),
    )
    .await;
    let stream = created.str("streamIdentifier");

    let cases = [
        ("never created", cluster.clone(), absent_identifier()),
        (
            "all zeroes",
            cluster.clone(),
            "00000000000000000000000000".to_string(),
        ),
        (
            "a stream of another cluster",
            other_cluster.clone(),
            stream.clone(),
        ),
        (
            "the cluster's own id used as a stream id",
            cluster.clone(),
            cluster.clone(),
        ),
    ];

    for (name, on_cluster, stream_identifier) in cases {
        let answer = get_stream(&dsql, &on_cluster, &stream_identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: GetStream on a stream the cluster does not hold must answer 404, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: the 404 must be a ResourceNotFoundException, got {:?}",
            answer.error
        );
    }

    // The stream is still readable where it actually lives.
    assert_eq!(
        get_stream(&dsql, &cluster, &stream).await.status,
        200,
        "the stream must still be readable on its own cluster"
    );
}

/// RULE: a `streamIdentifier` outside `StreamId`'s `^[a-z0-9]{26}$` is refused as
/// invalid input (400 `ValidationException`) rather than looked up.
#[tokio::test]
async fn a_malformed_stream_identifier_is_refused_as_invalid() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    let cases = [
        ("too short", "abc"),
        ("one char short", "aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("one char long", "aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase", "AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ("hyphenated", "aaaaaaaaaaaa-aaaaaaaaaaaaa"),
        ("underscored", "aaaaaaaaaaaa_aaaaaaaaaaaaa"),
    ];

    for (name, stream_identifier) in cases {
        let answer = get_stream(&dsql, &cluster, stream_identifier).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: a malformed streamIdentifier must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {name:?}: the refusal must be a ValidationException, got {:?}",
            answer.error
        );
    }
}

/// RULE: a cluster the clock still owes a transition does not accept a mutation,
/// and creating a stream on it is one — so it is refused with 409
/// `ConflictException` until the cluster is `ACTIVE`.
#[tokio::test]
async fn a_cluster_still_creating_refuses_a_new_stream() {
    let dsql = Dsql::new();
    // A cluster straight out of CreateCluster is CREATING, not yet ACTIVE.
    let created_cluster = dsql
        .call("POST", "/cluster", Some(json!({})))
        .await
        .str("identifier");
    let request = json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
    });

    let refused = create_stream(&dsql, &created_cluster, request.clone()).await;
    assert_eq!(
        refused.status, 409,
        "a CREATING cluster must refuse a new stream with 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ConflictException"),
        "the refusal must be a ConflictException, got {:?}",
        refused.error
    );

    wait_for_the_clock().await;
    let accepted = create_stream(&dsql, &created_cluster, request).await;
    assert_eq!(
        accepted.status, 200,
        "once the cluster is ACTIVE the same CreateStream must succeed, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// RULE: a stream AWS returns in a non-terminal status is moved on by the CLOCK:
/// `CREATING` straight after the call, `ACTIVE` once the transition comes due,
/// with no caller needed in between.
#[tokio::test]
async fn the_clock_moves_a_new_stream_from_creating_to_active() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let created = create_stream(
        &dsql,
        &cluster,
        json!({
            "format": "JSON",
            "ordering": "UNORDERED",
            "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
        }),
    )
    .await;
    let stream = created.str("streamIdentifier");

    let immediately = get_stream(&dsql, &cluster, &stream).await;
    assert_eq!(
        immediately.str("status"),
        "CREATING",
        "a caller reading straight after CreateStream must still see CREATING, got {:?}",
        immediately.str("status")
    );

    wait_for_the_clock().await;
    let settled = get_stream(&dsql, &cluster, &stream).await;
    assert_eq!(
        settled.str("status"),
        "ACTIVE",
        "once the transition is due the stream must report ACTIVE, got {:?}",
        settled.str("status")
    );
    assert_eq!(
        settled.get("creationTime"),
        created.get("creationTime"),
        "the transition must not re-stamp creationTime"
    );
}

/// RULE: the store is bounded — a cluster accepts streams up to its ceiling and
/// then refuses with 402 `ServiceQuotaExceededException`.
#[tokio::test]
async fn a_cluster_fills_its_stream_ceiling_and_then_refuses() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let request = json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
    });

    for n in 0..MAX_STREAMS_PER_CLUSTER {
        let answer = create_stream(&dsql, &cluster, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case stream #{n}: a create below the ceiling must answer 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let refused = create_stream(&dsql, &cluster, request).await;
    assert_eq!(
        refused.status, 402,
        "a create past the ceiling of {MAX_STREAMS_PER_CLUSTER} must answer 402, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ServiceQuotaExceededException"),
        "the refusal must be a ServiceQuotaExceededException, got {:?}",
        refused.error
    );
}
