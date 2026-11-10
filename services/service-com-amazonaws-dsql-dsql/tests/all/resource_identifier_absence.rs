// SPDX-License-Identifier: CC0-1.0

//! What an identifier-addressed operation answers for an identifier the account
//! does not hold, and what it answers for a value that is no identifier at all.
//!
//! RULE under test: an identifier of this service is 26 characters of the base32
//! alphabet every cluster and stream identifier is rendered with, and every value
//! inside the range this service mints from is resolved AGAINST THE STORE on every
//! route — an identifier the account does not hold is the modelled
//! `ResourceNotFoundException`, HTTP 404 with the wire code
//! `ResourceNotFoundException` and all three of its `@required` members filled:
//! `message` naming the ARN the call addressed, `resourceId` the identifier the
//! request carried, and `resourceType` the type of the resource the lookup stopped
//! at. Nothing about which value the store holds may be refused, because a value
//! of that form is well formed whether or not this service ever issued that
//! particular one. The complements are pinned beside this suite, over this suite's
//! route table: a value outside `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$`
//! (`smithy_query(query="shape", shape="ClusterId,StreamId")`) is refused at the
//! boundary (`tests/all/malformed_resource_identifier.rs`), one inside that pattern
//! using a letter the alphabet leaves out is refused as input on every route
//! (`tests/all/identifier_format_refusal.rs`), and one written in the alphabet but
//! ABOVE the minted range is answered per operation — refused by the three that
//! state that range as a precondition, an absent resource on the rest
//! (`tests/all/identifier_value_range.rs`).
//!
//! The identifier table spans what the form generalises over — which symbols the
//! value uses and where, across the range this service's own minting scheme draws
//! from, including the renderings it would never have produced — and the route
//! table spans every operation that carries such an identifier in its URI, so the
//! rule is asserted on the member each one binds rather than on one reader, and on
//! the nested stream route as well as the cluster ones.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{absent_identifier, expected_arn, expected_stream_arn, Dsql};

/// The length `ClusterId` and `StreamId` fix (`smithy_query(query="shape",
/// shape="ClusterId,StreamId")`: `^[a-z0-9]{26}$`).
pub const IDENTIFIER_LEN: usize = 26;

/// The symbols an identifier of this service is rendered with, lowercased: the
/// digits and the letters with `i`, `l`, `o` and `u` left out. Source: the
/// rendering `state::new_resource_identifier` writes an identifier with (its
/// `CROCKFORD_SYMBOLS`).
pub const IDENTIFIER_SYMBOLS: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// The largest leading symbol an identifier this service MINTS can carry:
/// [`IDENTIFIER_LEN`] symbols carry 130 bits while a minted identifier renders
/// 128, so the leading one spells only the value's top 3 bits and the eight
/// symbols up to this are the range the mint draws from. It is not an input
/// requirement — `tests/all/identifier_value_range.rs` pins what a value above it
/// answers.
pub const LEADING_SYMBOL_MAX: char = '7';

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
    pub fn resource_type(self) -> &'static str {
        match self {
            Class::Cluster => TYPE_CLUSTER,
            Class::Stream => TYPE_STREAM,
        }
    }

    /// The ARN a refusal of `identifier` names, in `region` and under `cluster`
    /// for a stream.
    pub fn arn(self, region: &str, cluster: &str, identifier: &str) -> String {
        match self {
            Class::Cluster => expected_arn(region, identifier),
            Class::Stream => expected_stream_arn(region, cluster, identifier),
        }
    }
}

/// What a route answers for an identifier written in the identifier alphabet
/// whose symbols spell a value ABOVE the range this service mints from.
///
/// The model does not say: `ClusterId` and `StreamId` carry one `@pattern` each,
/// and every one of these operations models both `ValidationException` and
/// `ResourceNotFoundException` (`smithy_query(query="digest")`). It is the wire
/// behaviour of each operation that says, and the two answers are pinned per route
/// in `tests/all/identifier_value_range.rs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AboveTheMintedRange {
    /// The operation states the minted range as a precondition of its own: such a
    /// value is refused as input, 400 `ValidationException`, before any lookup.
    Refused,
    /// The operation states no range precondition: such a value is an identifier
    /// it resolves against the store, so an account that does not hold it answers
    /// 404 `ResourceNotFoundException`.
    Absent,
}

/// One route that carries a resource identifier: how to issue it with the
/// identifier under test, which member that identifier arrives in, which
/// resource it addresses, and what it answers for a value above the minted range.
pub struct Route {
    pub operation: &'static str,
    pub member: &'static str,
    pub class: Class,
    pub above_the_minted_range: AboveTheMintedRange,
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
            above_the_minted_range: AboveTheMintedRange::Refused,
            request: |id, _| ("GET", format!("/cluster/{id}"), None),
        },
        Route {
            operation: "DeleteCluster",
            member: "identifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Refused,
            request: |id, _| ("DELETE", format!("/cluster/{id}"), None),
        },
        Route {
            operation: "UpdateCluster",
            member: "identifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Absent,
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
            above_the_minted_range: AboveTheMintedRange::Refused,
            request: |id, _| ("GET", format!("/cluster/{id}/policy"), None),
        },
        Route {
            operation: "PutClusterPolicy",
            member: "identifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Absent,
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
            above_the_minted_range: AboveTheMintedRange::Absent,
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
            above_the_minted_range: AboveTheMintedRange::Absent,
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
            above_the_minted_range: AboveTheMintedRange::Absent,
            request: |id, _| ("POST", format!("/stream/{id}"), Some(stream_request())),
        },
        Route {
            operation: "ListStreams",
            member: "clusterIdentifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Absent,
            request: |id, _| ("GET", format!("/stream/{id}"), None),
        },
        Route {
            operation: "GetStream on the cluster label",
            member: "clusterIdentifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Absent,
            request: |id, _| ("GET", format!("/stream/{id}/{}", absent_identifier()), None),
        },
        Route {
            operation: "GetStream on the stream label",
            member: "streamIdentifier",
            class: Class::Stream,
            above_the_minted_range: AboveTheMintedRange::Absent,
            request: |id, live| ("GET", format!("/stream/{live}/{id}"), None),
        },
        Route {
            operation: "DeleteStream on the cluster label",
            member: "clusterIdentifier",
            class: Class::Cluster,
            above_the_minted_range: AboveTheMintedRange::Absent,
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
            above_the_minted_range: AboveTheMintedRange::Absent,
            request: |id, live| ("DELETE", format!("/stream/{live}/{id}"), None),
        },
    ]
}

/// One identifier under test and how the case is named.
pub struct IdCase {
    pub name: String,
    pub value: String,
}

/// Identifiers of this service's own MINTED form that no call ever created: every
/// row is 26 symbols of the identifier alphabet whose leading symbol keeps the
/// value inside the range the mint draws from, and the rows differ in which
/// symbols they use and where — from the smallest value the mint renders to the
/// largest, including the renderings this service's own minting scheme would never
/// have produced. Values ABOVE that range are identifiers too, pinned in
/// `tests/all/identifier_value_range.rs`.
fn well_formed_absent_identifiers() -> Vec<IdCase> {
    let mut cases = vec![
        IdCase {
            name: "the smallest value the encoding renders".to_string(),
            value: "0".repeat(IDENTIFIER_LEN),
        },
        IdCase {
            name: "the largest value the encoding renders".to_string(),
            value: format!("{LEADING_SYMBOL_MAX}{}", "z".repeat(IDENTIFIER_LEN - 1)),
        },
        IdCase {
            name: "digits after the leading symbol".to_string(),
            value: format!("1{}", "9".repeat(IDENTIFIER_LEN - 1)),
        },
        IdCase {
            name: "letters after the leading symbol".to_string(),
            value: "2aaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        },
        IdCase {
            name: "letters and digits mixed".to_string(),
            value: "3a1b2c3d4e5f6g7h8j9k0m1n2p".to_string(),
        },
    ];

    // The leading symbol, across the range the mint draws from: a value this
    // service would not have minted is an identifier all the same.
    for leading in ['4', '5', '6'] {
        cases.push(IdCase {
            name: format!("a leading {leading:?}"),
            value: format!("{leading}{}", "z".repeat(IDENTIFIER_LEN - 1)),
        });
    }

    // Each symbol of the alphabet, at the front of the value's body, in the
    // middle and at the end: the form is about the alphabet rather than about
    // which of its symbols a value happens to use.
    for symbol in IDENTIFIER_SYMBOLS.chars() {
        for (position, where_it_is) in [
            (1, "first"),
            (IDENTIFIER_LEN / 2, "middle"),
            (IDENTIFIER_LEN - 1, "last"),
        ] {
            let mut value: Vec<char> = "0".repeat(IDENTIFIER_LEN).chars().collect();
            value[position] = symbol;
            cases.push(IdCase {
                name: format!("{symbol:?} as the {where_it_is} symbol of the body"),
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
                .all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol)),
            "case {:?}: every row must be drawn from the identifier alphabet, got {:?}",
            case.name,
            case.value
        );
        assert!(
            case.value
                .chars()
                .next()
                .is_some_and(|leading| leading <= LEADING_SYMBOL_MAX),
            "case {:?}: every row's leading symbol must keep the value inside the \
             minted range, got {:?}",
            case.name,
            case.value
        );
    }
    cases
}

/// Assert that `route` reports `case` as an ABSENT resource: the modelled 404
/// with every `@required` member of `ResourceNotFoundException` taken from the
/// request and the ARN the call addressed.
///
/// The one place that body is spelled, so every suite that asserts an absent
/// resource over this route table — this one for the minted range,
/// `tests/all/identifier_value_range.rs` above it — asserts the same thing.
pub async fn reported_absent(dsql: &Dsql, route: &Route, live: &str, case: &IdCase) {
    let (method, path, body) = (route.request)(&case.value, live);
    let answer = dsql.call(method, &path, body).await;
    let name = format!("{} on {}", case.name, route.operation);

    assert_eq!(
        answer.status, 404,
        "case {name}: an identifier of the service's form that names nothing must \
         answer 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case {name}: the wire code must be ResourceNotFoundException, got {:?}",
        answer.error
    );
    let arn = route.class.arn(dsql.region(), live, &case.value);
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

/// RULE: every value of the identifier form is resolved against the store, so an
/// identifier the account does not hold answers 404 `ResourceNotFoundException`
/// with the ARN-naming message, the identifier the request carried as
/// `resourceId`, and the addressed resource's own type as `resourceType` — on
/// every route that carries such an identifier.
#[tokio::test]
async fn a_well_formed_identifier_the_account_does_not_hold_is_not_found() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the identifier under test in the stream
    // label is answered for the stream and not for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in well_formed_absent_identifiers() {
        for route in &routes {
            reported_absent(&dsql, route, &live, &case).await;
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

/// RULE, the other way round: an identifier this service MINTS is of that form
/// and resolves to the resource it names, so neither this suite nor either
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
                .all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol)),
            "case {cluster:?}: a minted identifier is drawn from the identifier alphabet"
        );
        assert!(
            cluster
                .chars()
                .next()
                .is_some_and(|leading| leading <= LEADING_SYMBOL_MAX),
            "case {cluster:?}: a minted identifier renders a 128-bit value, so its leading \
             symbol is at most {LEADING_SYMBOL_MAX:?}"
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
                    .all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol))
                && stream
                    .chars()
                    .next()
                    .is_some_and(|leading| leading <= LEADING_SYMBOL_MAX),
            "case {stream:?}: a minted stream identifier must be of the identifier form"
        );
        let read_stream = dsql.get_stream(cluster, &stream).await;
        assert_eq!(
            read_stream.status, 200,
            "case {stream:?}: a minted stream identifier must be read back, got {} {:?}",
            read_stream.status, read_stream.body
        );
    }
}
