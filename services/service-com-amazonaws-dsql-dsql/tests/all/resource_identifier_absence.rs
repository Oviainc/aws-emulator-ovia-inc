// SPDX-License-Identifier: CC0-1.0

//! What an identifier-addressed operation answers for an identifier the account
//! does not hold, and what it answers for a value that is no identifier at all.
//!
//! RULE under test: the FORM of a cluster or stream identifier is exactly the
//! `^[a-z0-9]{26}$` that `ClusterId` and `StreamId` declare
//! (`smithy_query(query="shape", shape="ClusterId,StreamId")`), so every value
//! that pattern admits is resolved AGAINST THE STORE — an identifier the account
//! does not hold is the modelled `ResourceNotFoundException`, HTTP 404 with the
//! wire code `ResourceNotFoundException` and all three of its `@required` members
//! filled: `message` naming the ARN the call addressed, `resourceId` the
//! identifier the request carried, and `resourceType` the type of the resource
//! the lookup stopped at. Nothing narrower than the pattern may be refused,
//! because a 26-character lowercase-alphanumeric value is well formed whether or
//! not this service's own minting scheme would have produced it; a value OUTSIDE
//! the pattern is no identifier at all and is refused as input, which
//! `tests/all/malformed_resource_identifier.rs` pins over this suite's route
//! table.
//!
//! The identifier table spans what the pattern generalises over — which symbols
//! the value uses and where, including every form a 26-character
//! lowercase-alphanumeric string can take that no minting scheme of this service
//! produces — and the route table spans every operation that carries such an
//! identifier in its URI, so the rule is asserted on the member each one binds
//! rather than on one reader, and on the nested stream route as well as the
//! cluster ones.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{absent_identifier, expected_arn, expected_stream_arn, Dsql};

/// The length `ClusterId` and `StreamId` fix (`smithy_query(query="shape",
/// shape="ClusterId,StreamId")`: `^[a-z0-9]{26}$`).
pub const IDENTIFIER_LEN: usize = 26;

/// The resource types a refusal of this service names, as the service spells them
/// in an ARN's resource field (`state::RESOURCE_TYPE_CLUSTER` /
/// `RESOURCE_TYPE_STREAM`, the segments `ClusterArn` and `StreamArn` carry).
const TYPE_CLUSTER: &str = "cluster";
/// See [`TYPE_CLUSTER`].
const TYPE_STREAM: &str = "stream";

/// Which resource an identifier under test addresses, and so which ARN a refusal
/// of it names and which type it reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// The identifier is a cluster's, so the refusal names the cluster ARN.
    Cluster,
    /// The identifier is a stream's, under a cluster that IS held, so the refusal
    /// names the stream ARN nested under that cluster's.
    Stream,
}

impl Class {
    /// The `resourceType` a refusal of this class reports.
    fn resource_type(self) -> &'static str {
        match self {
            Class::Cluster => TYPE_CLUSTER,
            Class::Stream => TYPE_STREAM,
        }
    }

    /// The ARN a refusal of `identifier` names, in `region` and under `cluster`
    /// for a stream.
    fn arn(self, region: &str, cluster: &str, identifier: &str) -> String {
        match self {
            Class::Cluster => expected_arn(region, identifier),
            Class::Stream => expected_stream_arn(region, cluster, identifier),
        }
    }
}

/// One route that carries a resource identifier: how to issue it with the
/// identifier under test, which member that identifier arrives in, and which
/// resource it addresses.
pub struct Route {
    pub operation: &'static str,
    pub member: &'static str,
    pub class: Class,
    /// The request, given the identifier under test and a live cluster, so a row
    /// that puts the identifier under test in one label can fill the other with a
    /// cluster that is really held.
    pub request: fn(&str, &str) -> (&'static str, String, Option<Value>),
}

/// A `targetDefinition` naming a Kinesis destination of this emulator's own
/// account, as `CreateStream` requires (`smithy_query(query="shape",
/// shape="KinesisTargetDefinition")` — a `streamArn` and a `roleArn`), so a
/// `CreateStream` row is answered for its cluster identifier rather than for the
/// role it passes.
fn kinesis_target() -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:us-east-1:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// A benign resource policy document, so a `PutClusterPolicy` row is answered for
/// its identifier rather than for its document.
fn policy_document() -> String {
    serde_json::to_string(&json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
            "Action": "dsql:DbConnect",
            "Resource": "*",
        }],
    }))
    .expect("a policy document serializes")
}

/// A `CreateStream` body with every `@required` member filled.
pub fn stream_request() -> Value {
    json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target(),
    })
}

/// Every route an identifier arrives on. The URIs are the modelled ones
/// (`smithy_query(query="digest")`). Shared with the suite that pins the
/// complement of this rule, so both halves are asserted over one route table.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            operation: "GetCluster",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| ("GET", format!("/cluster/{id}"), None),
        },
        Route {
            operation: "DeleteCluster",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| ("DELETE", format!("/cluster/{id}"), None),
        },
        Route {
            operation: "UpdateCluster",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "POST",
                    format!("/cluster/{id}"),
                    Some(json!({"deletionProtectionEnabled": false})),
                )
            },
        },
        Route {
            operation: "GetClusterPolicy",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| ("GET", format!("/cluster/{id}/policy"), None),
        },
        Route {
            operation: "PutClusterPolicy",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "POST",
                    format!("/cluster/{id}/policy"),
                    Some(json!({"policy": policy_document()})),
                )
            },
        },
        Route {
            operation: "DeleteClusterPolicy",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "DELETE",
                    format!("/cluster/{id}/policy?expected-policy-version=1"),
                    None,
                )
            },
        },
        Route {
            operation: "GetVpcEndpointServiceName",
            member: "identifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "GET",
                    format!("/clusters/{id}/vpc-endpoint-service-name"),
                    None,
                )
            },
        },
        Route {
            operation: "CreateStream",
            member: "clusterIdentifier",
            class: Class::Cluster,
            request: |id, _| ("POST", format!("/stream/{id}"), Some(stream_request())),
        },
        Route {
            operation: "ListStreams",
            member: "clusterIdentifier",
            class: Class::Cluster,
            request: |id, _| ("GET", format!("/stream/{id}"), None),
        },
        Route {
            operation: "GetStream on the cluster label",
            member: "clusterIdentifier",
            class: Class::Cluster,
            request: |id, _| ("GET", format!("/stream/{id}/{}", absent_identifier()), None),
        },
        Route {
            operation: "GetStream on the stream label",
            member: "streamIdentifier",
            class: Class::Stream,
            request: |id, live| ("GET", format!("/stream/{live}/{id}"), None),
        },
        Route {
            operation: "DeleteStream on the cluster label",
            member: "clusterIdentifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "DELETE",
                    format!("/stream/{id}/{}", absent_identifier()),
                    None,
                )
            },
        },
        Route {
            operation: "DeleteStream on the stream label",
            member: "streamIdentifier",
            class: Class::Stream,
            request: |id, live| ("DELETE", format!("/stream/{live}/{id}"), None),
        },
    ]
}

/// One identifier under test and how the case is named.
pub struct IdCase {
    pub name: String,
    pub value: String,
}

/// Identifiers INSIDE `^[a-z0-9]{26}$` that no call ever created: every row is 26
/// characters drawn from `[a-z0-9]`, so the model admits all of them, and the
/// rows differ in which symbols they use and where — including the forms this
/// service's own minting scheme never produces, which the pattern admits all the
/// same.
fn well_formed_absent_identifiers() -> Vec<IdCase> {
    let mut cases = vec![
        IdCase {
            name: "every symbol the same letter".to_string(),
            value: "a".repeat(IDENTIFIER_LEN),
        },
        IdCase {
            name: "every symbol a zero".to_string(),
            value: "0".repeat(IDENTIFIER_LEN),
        },
        IdCase {
            name: "every symbol a nine".to_string(),
            value: "9".repeat(IDENTIFIER_LEN),
        },
        IdCase {
            name: "the last letter of the alphabet throughout".to_string(),
            value: "z".repeat(IDENTIFIER_LEN),
        },
        IdCase {
            name: "letters and digits mixed".to_string(),
            value: "a1b2c3d4e5f6g7h8i9j0klmnop".to_string(),
        },
    ];

    // The leading symbol, across the digit range: a value this service would not
    // mint is still a value the pattern admits.
    for leading in ['8', '9'] {
        cases.push(IdCase {
            name: format!("a leading {leading:?}"),
            value: format!("{leading}{}", "z".repeat(IDENTIFIER_LEN - 1)),
        });
    }

    // The four letters `[a-z0-9]` carries that this service's identifier renderer
    // never emits, each at the front, in the middle and at the end.
    for letter in ['i', 'l', 'o', 'u'] {
        for (position, where_it_is) in [
            (0, "first"),
            (IDENTIFIER_LEN / 2, "middle"),
            (IDENTIFIER_LEN - 1, "last"),
        ] {
            let mut value: Vec<char> = "0".repeat(IDENTIFIER_LEN).chars().collect();
            value[position] = letter;
            cases.push(IdCase {
                name: format!("{letter:?} as the {where_it_is} symbol"),
                value: value.into_iter().collect(),
            });
        }
    }

    for case in &cases {
        assert_eq!(
            case.value.len(),
            IDENTIFIER_LEN,
            "case {:?}: every row must be {IDENTIFIER_LEN} characters, got {:?}",
            case.name,
            case.value
        );
        assert!(
            case.value
                .chars()
                .all(|symbol| symbol.is_ascii_lowercase() || symbol.is_ascii_digit()),
            "case {:?}: every row must be inside [a-z0-9], got {:?}",
            case.name,
            case.value
        );
    }
    cases
}

/// RULE: every value `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$` admits is
/// resolved against the store, so an identifier the account does not hold answers
/// 404 `ResourceNotFoundException` with the ARN-naming message, the identifier the
/// request carried as `resourceId`, and the addressed resource's own type as
/// `resourceType` — on every route that carries such an identifier.
#[tokio::test]
async fn a_well_formed_identifier_the_account_does_not_hold_is_not_found() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the identifier under test in the stream
    // label is answered for the stream and not for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in well_formed_absent_identifiers() {
        for route in &routes {
            let (method, path, body) = (route.request)(&case.value, &live);
            let answer = dsql.call(method, &path, body).await;
            let name = format!("{} on {}", case.name, route.operation);

            assert_eq!(
                answer.status, 404,
                "case {name}: an identifier inside the modelled pattern that names \
                 nothing must answer 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {name}: the wire code must be ResourceNotFoundException, got {:?}",
                answer.error
            );
            let arn = route.class.arn(dsql.region(), &live, &case.value);
            assert_eq!(
                answer.message(),
                format!("The resource with ARN {arn} doesn't exist. Verify the ARN and try again."),
                "case {name}: expected the sentence naming {arn:?}, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.str("resourceId"),
                case.value,
                "case {name}: resourceId is @required and is the identifier the request \
                 carried, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str("resourceType"),
                route.class.resource_type(),
                "case {name}: resourceType is @required and is the addressed resource's \
                 own type, got {:?}",
                answer.body
            );
        }
    }

    // Nothing any refused call addressed was created, removed or changed: the one
    // live cluster is still the only one, still readable, still without a policy,
    // and still holding no stream.
    let read = dsql.get(&live).await;
    assert_eq!(
        read.status, 200,
        "the live cluster must be untouched by every refusal, got {} {:?}",
        read.status, read.body
    );
    let clusters = dsql.list("").await.get("clusters");
    assert_eq!(
        clusters.as_array().map(Vec::len),
        Some(1),
        "no refused request may have created a cluster, got {clusters:?}"
    );
    assert_eq!(
        dsql.get_policy(&live).await.status,
        404,
        "no refused request may have attached a policy"
    );
    let streams = dsql.list_streams(&live, "").await.get("streams");
    assert_eq!(
        streams.as_array().map(Vec::len),
        Some(0),
        "no refused request may have created a stream, got {streams:?}"
    );
}

/// RULE, the other way round: an identifier this service MINTS is inside the
/// pattern and resolves to the resource it names, so neither this suite nor the
/// complement refuses anything the service itself handed out — asserted by
/// reading each minted cluster and each minted stream back through its reader.
#[tokio::test]
async fn every_minted_identifier_resolves_to_its_resource() {
    let dsql = Dsql::new();
    // Several, because the entropy of a minted identifier differs per mint: the
    // rows differ in the value itself rather than in anything the test chooses.
    let clusters = dsql.create_settled_many(8).await;

    for cluster in &clusters {
        assert_eq!(
            cluster.len(),
            IDENTIFIER_LEN,
            "case {cluster:?}: a minted identifier is {IDENTIFIER_LEN} symbols, got {}",
            cluster.len()
        );
        assert!(
            cluster
                .chars()
                .all(|symbol| symbol.is_ascii_lowercase() || symbol.is_ascii_digit()),
            "case {cluster:?}: a minted identifier is inside [a-z0-9]"
        );
        let read = dsql.get(cluster).await;
        assert_eq!(
            read.status, 200,
            "case {cluster:?}: a minted identifier must be read back, got {} {:?}",
            read.status, read.body
        );

        let created = dsql.create_stream(cluster, stream_request()).await;
        assert_eq!(
            created.status, 200,
            "case {cluster:?}: CreateStream must be served, got {} {:?}",
            created.status, created.body
        );
        let stream = created.str("streamIdentifier");
        assert!(
            stream.len() == IDENTIFIER_LEN
                && stream
                    .chars()
                    .all(|symbol| symbol.is_ascii_lowercase() || symbol.is_ascii_digit()),
            "case {stream:?}: a minted stream identifier must be inside the pattern"
        );
        let read_stream = dsql.get_stream(cluster, &stream).await;
        assert_eq!(
            read_stream.status, 200,
            "case {stream:?}: a minted stream identifier must be read back, got {} {:?}",
            read_stream.status, read_stream.body
        );
    }
}
