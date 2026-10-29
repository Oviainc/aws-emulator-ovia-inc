// SPDX-License-Identifier: CC0-1.0

//! Which requests one `clientToken` may serve, end to end over the wire.
//!
//! RULE: a `clientToken` is bound to the VALUES of the request it first served,
//! so two requests that differ anywhere — including inside a member whose value
//! is composite (a map, a list, a structure, a union), wherever the difference
//! falls within it, and whether a nested member is carried at all — are
//! different requests: the second is refused 409 `ConflictException` and mints
//! nothing, while a request identical to the first is a retry answered with what
//! the first call produced.
//!
//! The table spans the composite members a create's token is bound to, and
//! within each the ways two values of it can differ while a rendering that
//! joined them with a separator character would read them as one value:
//!
//! * `tags` — `TagKey` and `TagValue` both admit `=`
//!   (`smithy_query(query="shape", shape="TagKey,TagValue")`:
//!   `^[a-zA-Z0-9_.:/=+\-@ ]*$`, key length 1..128, value length 0..256), so the
//!   rows move the `=` between the halves of an entry, across entries, and into
//!   an empty value;
//! * `multiRegionProperties` — `witnessRegion` (`Region`, which constrains no
//!   character) and `clusters` are both optional
//!   (`smithy_query(query="shape", shape="MultiRegionProperties")`), so the rows
//!   carry one of them absent against the same one carried empty;
//! * `targetDefinition` on the stream path — the union's `kinesis` members are
//!   ARNs, so a row moves the destination between them.
//!
//! Each class also carries a row that differs plainly (another value, another
//! entry count), so the table pins the whole rule rather than only the readings
//! a joined rendering confused.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::error_contract::{stream_request, CLIENT_TOKEN_MISMATCH};
use crate::harness::Dsql;

/// One row: two requests that differ in `member`, so one `clientToken` cannot
/// serve both.
struct Row {
    name: &'static str,
    /// The member the two requests differ in, under the name the create's
    /// reader reports it with.
    member: &'static str,
    /// The value `member` carries in the first request.
    first: Value,
    /// The value `member` carries in the second request.
    second: Value,
}

/// The `tags` rows, shared by the cluster and the stream path: both operations
/// bind their token to a tag map.
fn tag_rows() -> Vec<Row> {
    vec![
        Row {
            name: "the = ends the key, or starts the value",
            member: "tags",
            first: json!({"a": "b=c"}),
            second: json!({"a=b": "c"}),
        },
        Row {
            name: "a three-part entry split in two different places",
            member: "tags",
            first: json!({"k": "1=2=3"}),
            second: json!({"k=1=2": "3"}),
        },
        Row {
            name: "two entries whose key/value boundary moves",
            member: "tags",
            first: json!({"env": "test", "cost=center": "42"}),
            second: json!({"env": "test", "cost": "center=42"}),
        },
        Row {
            name: "a trailing = in the key against it alone in the value",
            member: "tags",
            first: json!({"x=": ""}),
            second: json!({"x": "="}),
        },
        Row {
            name: "the same key with another value",
            member: "tags",
            first: json!({"env": "test"}),
            second: json!({"env": "prod"}),
        },
        Row {
            name: "one entry against that entry plus another",
            member: "tags",
            first: json!({"env": "test"}),
            second: json!({"env": "test", "team": "data"}),
        },
    ]
}

/// The `multiRegionProperties` rows: a cluster's configuration is persisted and
/// reported as the request spelled it, so a nested member absent and that member
/// carried empty are different requests.
fn multi_region_rows() -> Vec<Row> {
    vec![
        Row {
            name: "no peer list against an empty peer list",
            member: "multiRegionProperties",
            first: json!({"witnessRegion": "us-west-2"}),
            second: json!({"witnessRegion": "us-west-2", "clusters": []}),
        },
        Row {
            name: "no witness Region against an empty witness Region",
            member: "multiRegionProperties",
            first: json!({"clusters": []}),
            second: json!({"witnessRegion": "", "clusters": []}),
        },
        Row {
            name: "another witness Region",
            member: "multiRegionProperties",
            first: json!({"witnessRegion": "us-west-2"}),
            second: json!({"witnessRegion": "eu-west-1"}),
        },
    ]
}

/// A `CreateCluster` body carrying `member` at `value`.
fn cluster_body(member: &str, value: &Value) -> Value {
    let mut body = json!({"deletionProtectionEnabled": false});
    body[member] = value.clone();
    body
}

/// Assert `answer` is the refusal a token reused for a different request gets.
fn assert_token_conflict(case: &str, answer: &crate::harness::Answer) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: a different request wearing the used token must answer 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ConflictException"),
        "case {case:?}: the refusal must be a ConflictException, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.message(),
        CLIENT_TOKEN_MISMATCH,
        "case {case:?}: got {:?}",
        answer.message()
    );
}

/// RULE: a `CreateCluster` `clientToken` is bound to the values of the request
/// it served, so a second create differing anywhere inside `tags` or
/// `multiRegionProperties` is 409 `ConflictException` and mints nothing, while
/// the identical request is answered with the first call's cluster.
#[tokio::test]
async fn a_create_cluster_token_is_bound_to_the_values_of_its_composite_members() {
    let dsql = Dsql::new();
    let rows: Vec<Row> = tag_rows().into_iter().chain(multi_region_rows()).collect();

    for (index, row) in rows.iter().enumerate() {
        let token = format!("bound-{index}");
        let mut first_body = cluster_body(row.member, &row.first);
        first_body["clientToken"] = json!(token.clone());
        let first = dsql.create(first_body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first call must answer 200, got {} {:?}",
            row.name, first.status, first.body
        );

        // The identical request under that token is a retry: the first cluster.
        let replay = dsql.create(first_body).await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: the identical request must answer 200, got {} {:?}",
            row.name, replay.status, replay.body
        );
        assert_eq!(
            replay.str("identifier"),
            first.str("identifier"),
            "case {:?}: the identical request must answer with the first cluster ({:?}), got {:?}",
            row.name,
            first.str("identifier"),
            replay.str("identifier")
        );

        // The row's other request asks for a different cluster, so the used
        // token refuses it.
        let mut second_body = cluster_body(row.member, &row.second);
        second_body["clientToken"] = json!(token.clone());
        let conflict = dsql.create(second_body.clone()).await;
        assert_token_conflict(row.name, &conflict);

        // Nothing of the refused request was minted, and the first cluster still
        // carries what the first request asked for.
        let first_read = dsql.get(&first.str("identifier")).await;
        assert_eq!(
            first_read.get(row.member),
            row.first,
            "case {:?}: the first cluster must still report the {} it was created with ({:?}), \
             got {:?}",
            row.name,
            row.member,
            row.first,
            first_read.get(row.member)
        );

        // The refusal was about the TOKEN, not about the request: under a token
        // of its own the same body mints a cluster of its own, whose own
        // configuration is the one it asked for — which is what makes the two
        // requests different requests.
        second_body["clientToken"] = json!(format!("{token}-of-its-own"));
        let second = dsql.create(second_body).await;
        assert_eq!(
            second.status, 200,
            "case {:?}: the second request must be servable under its own token, got {} {:?}",
            row.name, second.status, second.body
        );
        assert_ne!(
            second.str("identifier"),
            first.str("identifier"),
            "case {:?}: the second request must mint a cluster of its own, got {:?}",
            row.name,
            second.str("identifier")
        );
        let second_read = dsql.get(&second.str("identifier")).await;
        assert_eq!(
            second_read.get(row.member),
            row.second,
            "case {:?}: the second cluster must report the {} it was created with ({:?}), got {:?}",
            row.name,
            row.member,
            row.second,
            second_read.get(row.member)
        );
    }

    // Two clusters per row: the first request's and the second request's. Every
    // retry and every refusal minted nothing.
    let listed = dsql
        .list("?max-results=100")
        .await
        .get("clusters")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        listed,
        2 * rows.len(),
        "the retries and refusals must mint nothing: expected {} cluster(s), got {listed}",
        2 * rows.len()
    );
}

/// A `targetDefinition` naming a Kinesis stream by name, as a row that differs
/// only in the destination needs.
fn target(stream_name: &str, role: &str) -> Value {
    json!({
        "kinesis": {
            "streamArn": format!(
                "arn:aws:kinesis:us-east-1:{}:stream/{stream_name}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "roleArn": format!("arn:aws:iam::{}:role/{role}", engine::EMULATOR_ACCOUNT_ID),
        }
    })
}

/// The `targetDefinition` rows: the union's two ARN members, so a row moves the
/// difference from one to the other.
fn target_rows() -> Vec<Row> {
    vec![
        Row {
            name: "another Kinesis stream",
            member: "targetDefinition",
            first: target("changes", "dsql-cdc"),
            second: target("changes-later", "dsql-cdc"),
        },
        Row {
            name: "another role for the same stream",
            member: "targetDefinition",
            first: target("changes", "dsql-cdc"),
            second: target("changes", "service-role/dsql-cdc"),
        },
    ]
}

/// RULE: the same binding holds on the stream path — a `CreateStream`
/// `clientToken` is bound to the values of its own request, so a second create
/// differing inside `tags` or `targetDefinition` is 409 `ConflictException` and
/// mints no stream, while the identical request answers the first stream.
#[tokio::test]
async fn a_create_stream_token_is_bound_to_the_values_of_its_composite_members() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let rows: Vec<Row> = tag_rows().into_iter().chain(target_rows()).collect();

    for (index, row) in rows.iter().enumerate() {
        let token = format!("stream-bound-{index}");
        let mut first_body = stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc");
        first_body[row.member] = row.first.clone();
        first_body["clientToken"] = json!(token.clone());
        let first = dsql.create_stream(&cluster, first_body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first call must answer 200, got {} {:?}",
            row.name, first.status, first.body
        );

        let replay = dsql.create_stream(&cluster, first_body).await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: the identical request must answer 200, got {} {:?}",
            row.name, replay.status, replay.body
        );
        assert_eq!(
            replay.str("streamIdentifier"),
            first.str("streamIdentifier"),
            "case {:?}: the identical request must answer with the first stream ({:?}), got {:?}",
            row.name,
            first.str("streamIdentifier"),
            replay.str("streamIdentifier")
        );

        let mut second_body = stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc");
        second_body[row.member] = row.second.clone();
        second_body["clientToken"] = json!(token.clone());
        let conflict = dsql.create_stream(&cluster, second_body.clone()).await;
        assert_token_conflict(row.name, &conflict);

        let first_read = dsql
            .get_stream(&cluster, &first.str("streamIdentifier"))
            .await;
        assert_eq!(
            first_read.get(row.member),
            row.first,
            "case {:?}: the first stream must still report the {} it was created with ({:?}), \
             got {:?}",
            row.name,
            row.member,
            row.first,
            first_read.get(row.member)
        );

        second_body["clientToken"] = json!(format!("{token}-of-its-own"));
        let second = dsql.create_stream(&cluster, second_body).await;
        assert_eq!(
            second.status, 200,
            "case {:?}: the second request must be servable under its own token, got {} {:?}",
            row.name, second.status, second.body
        );
        assert_ne!(
            second.str("streamIdentifier"),
            first.str("streamIdentifier"),
            "case {:?}: the second request must mint a stream of its own, got {:?}",
            row.name,
            second.str("streamIdentifier")
        );
        let second_read = dsql
            .get_stream(&cluster, &second.str("streamIdentifier"))
            .await;
        assert_eq!(
            second_read.get(row.member),
            row.second,
            "case {:?}: the second stream must report the {} it was created with ({:?}), got {:?}",
            row.name,
            row.member,
            row.second,
            second_read.get(row.member)
        );
    }

    let listed = dsql
        .list_streams(&cluster, "?max-results=100")
        .await
        .get("streams")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        listed,
        2 * rows.len(),
        "the retries and refusals must mint nothing: expected {} stream(s), got {listed}",
        2 * rows.len()
    );
}
