// SPDX-License-Identifier: CC0-1.0

//! Amazon DocumentDB's subnet-group and cluster-parameter-group paths, through the WHOLE
//! assembled router.
//!
//! The shared `rds` signing name is Amazon RDS's — three models publish one wire identity
//! (`AmazonRDSv19`, `2014-10-31`, `awsQuery`, endpointPrefix and sigv4 name `rds`) and the
//! key resolves to the service whose own identity it is, which
//! `all/rds_docdb_neptune_signing_name_routing.rs` pins. Each peer is reachable under the
//! key that names IT, and this suite pins what Amazon DocumentDB ANSWERS there: before the
//! change behind it, every one of these requests was the router's `500 AmbiguousService` as
//! `application/x-amz-json-1.1` — a code in no AWS model, in a body an awsQuery client
//! cannot parse — because all three models claimed the key.
//!
//! The rules the rows assert, each over the whole router rather than one mounted service:
//!
//! * a request that names DocumentDB reaches exactly ONE service (never
//!   `AmbiguousService`, never `UnknownService`) and is answered in that family's own
//!   `text/xml` envelope;
//! * a subnet group created there reads back there, and a name state does not hold is the
//!   modelled `404 DBSubnetGroupNotFoundFault` — on the describe and on the delete;
//! * a cluster parameter group the model publishes reads back, and a name it does not hold
//!   is the modelled `404 DBParameterGroupNotFound`;
//! * an `Action` the model does not declare is the query protocol's own `400 InvalidAction`
//!   as `text/xml`, never a JSON router refusal.
//!
//! The crate's own suite (`services/service-com-amazonaws-docdb-amazonrdsv19/tests/all/`)
//! states these rules over tables of inputs; this one proves they survive assembly with
//! every other service mounted. Assembling the router costs stack (see `boot_serve.rs`), so
//! it is built — and the requests driven — on an explicitly sized thread, and the harness
//! for this family comes from the sibling suite rather than a second copy.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::rds_docdb_neptune_signing_name_routing::{authorization, API_VERSION, BOOT_STACK_SIZE};

/// The form-urlencoded body of an awsQuery call: the `Action`, the `Version`, and the
/// already-encoded members.
fn body(action: &str, params: &[(&str, &str)]) -> bytes::Bytes {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    body.push(char::from(byte));
                }
                other => body.push_str(&format!("%{other:02X}")),
            }
        }
    }
    bytes::Bytes::from(body)
}

/// The `<Code>` of an awsQuery `<ErrorResponse>`, or of the router's JSON refusal.
fn error_code(rendered: &str) -> Option<String> {
    if let Some((_, rest)) = rendered.split_once("<Code>") {
        return rest.split_once("</Code>").map(|(code, _)| code.to_string());
    }
    serde_json::from_str::<serde_json::Value>(rendered)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

/// What a row expects of the answer, beyond "it reached exactly one service".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The service answered successfully.
    Ok,
    /// The service answered this wire error code, with a 4xx.
    Code(&'static str),
}

/// One row: the case, the awsQuery `Action`, its members, and what must come back.
type Row = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    Expect,
);

#[test]
fn documentdb_serves_its_subnet_and_parameter_group_paths_through_the_router() {
    std::thread::Builder::new()
        .name("docdb-group-paths".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The rows walk one lifecycle and then the not-found answers either side of it,
            // so each later row depends on the state an earlier one left: the create, the
            // read-back, the cluster that references the group, the refusals, the delete,
            // and the read that must no longer find it.
            let rows: [Row; 11] = [
                (
                    "create a subnet group",
                    "CreateDBSubnetGroup",
                    &[
                        ("DBSubnetGroupName", "router-subnets"),
                        ("DBSubnetGroupDescription", "created through the router"),
                        ("SubnetIds.SubnetIdentifier.1", "subnet-0aaaaaaaaaaaaaaa1"),
                        ("SubnetIds.SubnetIdentifier.2", "subnet-0bbbbbbbbbbbbbbb2"),
                    ],
                    Expect::Ok,
                ),
                (
                    "read the group back",
                    "DescribeDBSubnetGroups",
                    &[("DBSubnetGroupName", "router-subnets")],
                    Expect::Ok,
                ),
                (
                    // The issue's own step: a subnet group that does not exist is the
                    // modelled 404, not a 500 refusal.
                    "describe a subnet group that does not exist",
                    "DescribeDBSubnetGroups",
                    &[("DBSubnetGroupName", "no-such-group")],
                    Expect::Code("DBSubnetGroupNotFoundFault"),
                ),
                (
                    "delete a subnet group that does not exist",
                    "DeleteDBSubnetGroup",
                    &[("DBSubnetGroupName", "no-such-group")],
                    Expect::Code("DBSubnetGroupNotFoundFault"),
                ),
                (
                    // A cluster create that names the stored group: the reference resolves.
                    "create a cluster in the group",
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", "router-cluster"),
                        ("Engine", "docdb"),
                        ("MasterUsername", "admin"),
                        ("MasterUserPassword", "secret99"),
                        ("DBSubnetGroupName", "router-subnets"),
                    ],
                    Expect::Ok,
                ),
                (
                    // …and one that names a group nothing holds is the modelled 404.
                    "create a cluster in a group that does not exist",
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", "refused-cluster"),
                        ("Engine", "docdb"),
                        ("MasterUsername", "admin"),
                        ("MasterUserPassword", "secret99"),
                        ("DBSubnetGroupName", "no-such-group"),
                    ],
                    Expect::Code("DBSubnetGroupNotFoundFault"),
                ),
                (
                    // An engine version the engine does not publish: awsQuery's documented
                    // common code, not a success and not a 5xx.
                    "create a cluster on a version the engine does not publish",
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", "refused-version"),
                        ("Engine", "docdb"),
                        ("MasterUsername", "admin"),
                        ("MasterUserPassword", "secret99"),
                        ("EngineVersion", "99.9.9"),
                    ],
                    Expect::Code("InvalidParameterCombination"),
                ),
                (
                    // The group is in use by the cluster created above.
                    "delete the group while a cluster is in it",
                    "DeleteDBSubnetGroup",
                    &[("DBSubnetGroupName", "router-subnets")],
                    Expect::Code("InvalidDBSubnetGroupStateFault"),
                ),
                (
                    "a cluster parameter group the model publishes",
                    "DescribeDBClusterParameterGroups",
                    &[("DBClusterParameterGroupName", "default.docdb5.0")],
                    Expect::Ok,
                ),
                (
                    // The answer a waiter polls a deleted parameter group for.
                    "a cluster parameter group that does not exist",
                    "DescribeDBClusterParameterGroups",
                    &[("DBClusterParameterGroupName", "no-such-group")],
                    // The model's `awsQueryError` code for `DBParameterGroupNotFoundFault`.
                    Expect::Code("DBParameterGroupNotFound"),
                ),
                (
                    // No operation of this model is named, so the query protocol's own
                    // routing error answers — as text/xml, which is the second half of the
                    // issue: the refusal must be parseable by an awsQuery client.
                    "an Action this model does not declare",
                    "DescribeNothingAtAll",
                    &[],
                    Expect::Code("InvalidAction"),
                ),
            ];

            for (case, action, params, expect) in rows {
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    // The key that names Amazon DocumentDB itself; the shared `rds` name is
                    // Amazon RDS's (see the module docs).
                    .header("authorization", authorization("docdb"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(body(action, params))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let code = error_code(&rendered);

                // No request may be a routing failure: `AmbiguousService` (two or more
                // claimants) or `UnknownService` (none).
                assert!(
                    code.as_deref() != Some("AmbiguousService")
                        && code.as_deref() != Some("UnknownService"),
                    "case {case:?} ({action}): expected exactly one claimant, got {status} \
                     {content_type} with {rendered}"
                );
                // Every answer is this family's own envelope, never the router's JSON.
                assert!(
                    content_type.starts_with("text/xml"),
                    "case {case:?} ({action}): an awsQuery service answers text/xml, got \
                     {content_type} with {rendered}"
                );

                match expect {
                    Expect::Ok => assert_eq!(
                        status, 200,
                        "case {case:?} ({action}): expected 200, got {status} with {rendered}"
                    ),
                    Expect::Code(expected) => assert_eq!(
                        (status < 500, code.as_deref()),
                        (true, Some(expected)),
                        "case {case:?} ({action}): expected the 4xx {expected}, got {status} \
                         with {rendered}"
                    ),
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("DocumentDB's group paths must be served through the assembled router");
}
