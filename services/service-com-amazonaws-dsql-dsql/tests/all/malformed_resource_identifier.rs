// SPDX-License-Identifier: CC0-1.0

//! The FORM of a resource identifier, decided before anything is looked up by it.
//!
//! RULE: a cluster or stream identifier this service accepts is the 26-character
//! base32 rendering of a 128-bit value — exactly what it mints
//! (`state::new_resource_identifier`, whose alphabet and length this suite's
//! catalogue is read from) — so a value outside that form names no resource and
//! is refused as INPUT, 400 `ValidationException`, before any state is read;
//! only a value inside the form reaches the lookup, where an identifier naming
//! nothing is the 404 `ResourceNotFoundException`. `ClusterId`'s and `StreamId`'s
//! `^[a-z0-9]{26}$` is wider than the form, so the refusal is the service's own
//! and not the model-constraint one.
//!
//! The identifier table spans the ways a 26-character `[a-z0-9]` string can fail
//! to be such a rendering — a leading symbol carrying more than the three bits
//! 128 bits leave of the leading group, and each letter the alphabet leaves out,
//! at the front, in the middle and at the end — against rows that ARE renderings
//! (the all-zero one, the largest leading symbol, and an identifier the service
//! itself minted). The operation table spans every route that carries such an
//! identifier, so the rule is asserted on the member each one binds rather than
//! on one reader.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{absent_identifier, Dsql};

/// The symbols an identifier is written in, read from the renderer this service
/// mints identifiers with (`state::new_resource_identifier` renders 128 bits in
/// the 32-symbol lowercase base32 alphabet `0123456789abcdefghjkmnpqrstvwxyz`),
/// and the length that renderer produces.
const IDENTIFIER_SYMBOLS: &str = "0123456789abcdefghjkmnpqrstvwxyz";
/// See [`IDENTIFIER_SYMBOLS`]: `ClusterId`/`StreamId` are 26 symbols long
/// (`smithy_query(query="shape", shape="ClusterId,StreamId")`).
const IDENTIFIER_LEN: usize = 26;

/// The letters `[a-z0-9]` admits that the alphabet above leaves out — the ones a
/// 26-character lowercase-alphanumeric string can carry while being no base32
/// rendering at all.
fn letters_outside_the_alphabet() -> Vec<char> {
    ('a'..='z')
        .filter(|letter| !IDENTIFIER_SYMBOLS.contains(*letter))
        .collect()
}

/// One identifier under test, and whether it is a rendering of a 128-bit value.
struct IdCase {
    name: String,
    value: String,
    well_formed: bool,
}

/// The identifier table: every row is 26 characters and inside
/// `^[a-z0-9]{26}$`, so none of them is refused by the modelled pattern.
fn identifier_cases() -> Vec<IdCase> {
    let mut cases = vec![
        IdCase {
            name: "the all-zero rendering".to_string(),
            value: "0".repeat(IDENTIFIER_LEN),
            well_formed: true,
        },
        IdCase {
            name: "the largest leading symbol 128 bits leave".to_string(),
            value: format!("7{}", "z".repeat(IDENTIFIER_LEN - 1)),
            well_formed: true,
        },
    ];

    // A leading symbol above seven carries more than the three bits the leading
    // group of a 128-bit value has left, so the string renders no 128-bit value.
    for leading in ['8', '9', 'a', 'z'] {
        cases.push(IdCase {
            name: format!("a leading {leading:?}, past the three bits the leading group leaves"),
            value: format!("{leading}{}", "z".repeat(IDENTIFIER_LEN - 1)),
            well_formed: false,
        });
    }

    // Each letter outside the alphabet, placed at the front, in the middle and at
    // the end of an otherwise well-formed identifier.
    for letter in letters_outside_the_alphabet() {
        for (position, where_it_is) in [
            (0, "first"),
            (IDENTIFIER_LEN / 2, "middle"),
            (IDENTIFIER_LEN - 1, "last"),
        ] {
            let mut value: Vec<char> = "0".repeat(IDENTIFIER_LEN).chars().collect();
            value[position] = letter;
            cases.push(IdCase {
                name: format!("{letter:?}, outside the alphabet, as the {where_it_is} symbol"),
                value: value.into_iter().collect(),
                well_formed: false,
            });
        }
    }

    cases
}

/// Which sentence a refusal of this member carries: Aurora DSQL words a cluster
/// identifier around the input it rejected and a stream identifier around the
/// format the member has to be in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Cluster,
    Stream,
}

impl Class {
    /// The message the refusal must carry, for `member` and `value`.
    fn message(self, member: &str, value: &str) -> String {
        match self {
            Class::Cluster => format!(
                "Invalid input '{member}: {value}'. The cluster identifier format is invalid. \
                 Provide a valid cluster identifier and try again."
            ),
            Class::Stream => format!(
                "Aurora DSQL can't process the request because the {member} that you specified \
                 isn't valid. Provide the {member} in the format 'a valid base32-encoded \
                 26-character string' and try again."
            ),
        }
    }
}

/// One route that carries a resource identifier: how to issue it with the
/// identifier under test, and which member that identifier arrives in.
struct Route {
    operation: &'static str,
    member: &'static str,
    class: Class,
    /// The request, given the identifier under test and a live cluster to
    /// address the rows that put the identifier in the OTHER label.
    request: fn(&str, &str) -> (&'static str, String, Option<Value>),
}

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream`
/// requires (`smithy_query(query="shape", shape="KinesisTargetDefinition")` — a
/// `streamArn` and a `roleArn`).
fn kinesis_target() -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:us-east-1:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// A benign resource policy document, so a `PutClusterPolicy` row is refused for
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

/// Every route an identifier arrives on. The URIs are the modelled ones
/// (`smithy_query(query="digest")`).
fn routes() -> Vec<Route> {
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
            request: |id, _| (
                "GET",
                format!("/clusters/{id}/vpc-endpoint-service-name"),
                None,
            ),
        },
        Route {
            operation: "CreateStream",
            member: "clusterIdentifier",
            class: Class::Cluster,
            request: |id, _| {
                (
                    "POST",
                    format!("/stream/{id}"),
                    Some(json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(),
                    })),
                )
            },
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
            request: |id, _| {
                (
                    "GET",
                    format!("/stream/{id}/{}", absent_identifier()),
                    None,
                )
            },
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

/// RULE: the identifier's form is decided before the lookup — a value that is no
/// 26-character base32 rendering is 400 `ValidationException` with the sentence
/// its member's class carries, and a value that IS one but names nothing is 404
/// `ResourceNotFoundException`, on every route that carries an identifier.
#[tokio::test]
async fn an_identifier_outside_the_rendered_form_is_refused_before_the_lookup() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the identifier under test in the
    // stream label is refused for that label rather than for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in identifier_cases() {
        for route in &routes {
            let (method, path, body) = (route.request)(&case.value, &live);
            let answer = dsql.call(method, &path, body).await;
            let name = format!("{} on {}", case.name, route.operation);

            if case.well_formed {
                assert_eq!(
                    answer.status, 404,
                    "case {name}: a rendering that names nothing must answer 404, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {name}: the 404 must be a ResourceNotFoundException, got {:?}",
                    answer.error
                );
                continue;
            }

            assert_eq!(
                answer.status, 400,
                "case {name}: a value outside the rendered form must answer 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {name}: the refusal must be a ValidationException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.message(),
                route.class.message(route.member, &case.value),
                "case {name}: expected the {:?} sentence for {}, got {:?}",
                route.class,
                route.member,
                answer.message()
            );
            assert!(
                !answer.str("reason").is_empty(),
                "case {name}: ValidationException models reason as @required, got {:?}",
                answer.body
            );
        }
    }

    // Nothing any refusal addressed was created, removed or changed: the one live
    // cluster is still the only one, still readable, and still without a policy.
    let read = dsql.get(&live).await;
    assert_eq!(
        read.status, 200,
        "the live cluster must be untouched by every refusal, got {} {:?}",
        read.status, read.body
    );
    let listed = dsql.list("").await;
    let clusters = listed.get("clusters");
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
}

/// RULE, the other way round: every identifier this service MINTS is inside the
/// form, so the check refuses nothing the service itself hands out — asserted by
/// reading each minted cluster and each minted stream back through its reader.
#[tokio::test]
async fn every_minted_identifier_is_accepted_by_the_form_check() {
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
            cluster.chars().all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol)),
            "case {cluster:?}: a minted identifier uses the alphabet {IDENTIFIER_SYMBOLS:?}"
        );
        let read = dsql.get(cluster).await;
        assert_eq!(
            read.status, 200,
            "case {cluster:?}: a minted identifier must be read back, got {} {:?}",
            read.status, read.body
        );

        let created = dsql
            .create_stream(
                cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {cluster:?}: CreateStream must be served, got {} {:?}",
            created.status, created.body
        );
        let stream = created.str("streamIdentifier");
        assert!(
            stream.len() == IDENTIFIER_LEN
                && stream.chars().all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol)),
            "case {stream:?}: a minted stream identifier must be inside the form"
        );
        let read_stream = dsql.get_stream(cluster, &stream).await;
        assert_eq!(
            read_stream.status, 200,
            "case {stream:?}: a minted stream identifier must be read back, got {} {:?}",
            read_stream.status, read_stream.body
        );
    }
}
