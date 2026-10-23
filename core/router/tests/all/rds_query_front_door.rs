// SPDX-License-Identifier: CC0-1.0

//! The cluster-snapshot family on the RDS Query front door, end to end through the whole
//! router.
//!
//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are three emulated services generated
//! from the same `AmazonRDSv19` model, all three declaring `sigv4Name: rds` and
//! `endpointPrefix: rds`. Which service a request lands on is pinned by
//! `all/rds_docdb_neptune_signing_name_routing.rs`; this suite pins what the services
//! ANSWER once it has landed, for the operations that name a DB cluster snapshot:
//!
//! * the request is SERVED — never the router's own `AmbiguousService` / `UnknownService`
//!   refusal, which is a code no AWS model declares and which an awsQuery client cannot
//!   parse at all;
//! * a snapshot identifier that names nothing is the modelled
//!   `DBClusterSnapshotNotFoundFault`: HTTP 404, `text/xml`, `Error/Type` `Sender`,
//!   `Error/Code` `DBClusterSnapshotNotFoundFault`, a `<Message>` naming the identifier
//!   the request asked for, and the `<RequestId>` an awsQuery `<ErrorResponse>` always
//!   carries — in whatever form that identifier was written;
//! * what IS stored is reported: a cluster snapshot written through the front door is read
//!   back through it.
//!
//! The rows span both the signing name every AWS SDK client of this endpoint sends
//! (`rds` — a DocumentDB or Neptune client signs it too, because its model gives it no
//! other name) and the routing key that names the DocumentDB service itself (`docdb`), so
//! the rule is asserted on both views that implement the family rather than on one.

use std::sync::Arc;

use bytes::Bytes;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::query_wire::{form_encode, opt_element as element};
// The harness for this family comes from the sibling routing suite rather than a second
// copy: the stack the router assembly needs, the awsQuery API version every view of this
// front door declares, and the SigV4 `Authorization` header a signed request carries.
use crate::rds_docdb_neptune_signing_name_routing::{authorization, API_VERSION, BOOT_STACK_SIZE};

/// The refusals the router itself answers when claim resolution fails. Neither is an AWS
/// error code, so seeing one means no service served the request.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// One row of the serve table: the `Action`, the signing name the request carries, the
/// member that names the snapshot, the other members the model requires, and the form the
/// identifier that names nothing was written in.
type ServeRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    IdentifierForm,
);

/// One entry of [`MISSING_IDENTIFIERS`]: what to call the form, and an identifier written
/// that way which names no stored snapshot.
type IdentifierForm = (&'static str, &'static str);

/// The forms an identifier that names nothing arrives in. The rule is about an identifier
/// naming no snapshot, not about one spelling of one: a plain name, the same name in
/// another case (`DBClusterSnapshotIdentifier` "is stored as a lowercase string", so case
/// cannot turn a miss into a hit), and the ARN form `DBClusterSnapshotArn` reports, which
/// these operations accept in place of the bare identifier.
const MISSING_IDENTIFIERS: [IdentifierForm; 3] = [
    ("a plain name", "no-such-snapshot"),
    ("the same name upper-cased", "NO-SUCH-SNAPSHOT"),
    (
        "an ARN form",
        "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:no-such-snapshot",
    ),
];

/// One entry of [`SNAPSHOT_OPERATIONS`]: the `Action`, the member that names the snapshot
/// it works on, and the other members the model requires.
type SnapshotOperation = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Every operation that names a DB cluster snapshot, with the members its model requires
/// (`smithy_query(query="operation", operation="CopyDBClusterSnapshot,…")`).
const SNAPSHOT_OPERATIONS: [SnapshotOperation; 5] = [
    (
        "DescribeDBClusterSnapshots",
        "DBClusterSnapshotIdentifier",
        &[],
    ),
    ("DeleteDBClusterSnapshot", "DBClusterSnapshotIdentifier", &[]),
    (
        "CopyDBClusterSnapshot",
        "SourceDBClusterSnapshotIdentifier",
        &[("TargetDBClusterSnapshotIdentifier", "the-copy")],
    ),
    (
        "DescribeDBClusterSnapshotAttributes",
        "DBClusterSnapshotIdentifier",
        &[],
    ),
    (
        "ModifyDBClusterSnapshotAttribute",
        "DBClusterSnapshotIdentifier",
        &[("AttributeName", "restore")],
    ),
];

/// One awsQuery call through the whole router, signed for `signing_name`.
fn call(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String, String) {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", "localhost")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(response.body()).into_owned();
    (status, content_type, body)
}

#[test]
fn the_cluster_snapshot_family_is_served_on_the_rds_query_front_door() {
    std::thread::Builder::new()
        .name("rds-cluster-snapshots".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Every operation of the family, under BOTH the shared signing name an SDK
            // client sends and the routing key that names DocumentDB itself.
            let mut rows: Vec<ServeRow> = Vec::new();
            for signing_name in ["rds", "docdb"] {
                for (action, naming_member, extra) in SNAPSHOT_OPERATIONS {
                    for form in MISSING_IDENTIFIERS {
                        rows.push((action, signing_name, naming_member, extra, form));
                    }
                }
            }

            for (action, signing_name, naming_member, extra, (form, identifier)) in rows {
                let case = format!("{action} signed for {signing_name} with {form}");
                let mut params: Vec<(&str, &str)> = vec![(naming_member, identifier)];
                params.extend_from_slice(extra);
                let (status, content_type, body) =
                    call(&router, &runtime, signing_name, action, &params);

                for refusal in ROUTER_REFUSALS {
                    assert!(
                        !body.contains(refusal),
                        "case {case:?}: the request must be SERVED, not refused by the \
                         router, got {status} with {body}"
                    );
                }
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the modelled DBClusterSnapshotNotFoundFault's \
                     404, got {status} with {body}"
                );
                assert!(
                    content_type.starts_with("text/xml"),
                    "case {case:?}: an awsQuery error is text/xml, got {content_type:?} \
                     with {body}"
                );
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("DBClusterSnapshotNotFoundFault"),
                    "case {case:?}: got {body}"
                );
                assert_eq!(
                    element(&body, "Type").as_deref(),
                    Some("Sender"),
                    "case {case:?}: a client error is a Sender fault, got {body}"
                );
                // The message is built from the request, so it names what was looked for:
                // a client that logs it can tell WHICH snapshot was missing.
                let message = element(&body, "Message").unwrap_or_default();
                assert!(
                    message.contains(identifier),
                    "case {case:?}: the message must name the identifier the request \
                     asked for, got {body}"
                );
                // The envelope element an SDK reads back as the error's request id.
                let request_id = element(&body, "RequestId").unwrap_or_default();
                assert_eq!(
                    request_id.split('-').map(str::len).collect::<Vec<_>>(),
                    vec![8, 4, 4, 4, 12],
                    "case {case:?}: expected an 8-4-4-4-12 <RequestId> in the envelope, \
                     got {body}"
                );
            }
        })
        .expect("spawn the front-door thread")
        .join()
        .expect("the cluster-snapshot family must be served on the RDS Query front door");
}

#[test]
fn a_cluster_snapshot_written_through_the_front_door_is_read_back_through_it() {
    std::thread::Builder::new()
        .name("rds-cluster-snapshot-roundtrip".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The DocumentDB view takes a cluster snapshot as the final snapshot of a
            // cluster it deletes, which is the one path that writes one without a
            // snapshot operation — so the read below runs against state another operation
            // put there.
            let (status, _, body) = call(
                &router,
                &runtime,
                "docdb",
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", "front-door-cluster"),
                    ("Engine", "docdb"),
                    ("MasterUsername", "admin"),
                    ("MasterUserPassword", "secret99"),
                ],
            );
            assert_eq!(
                status, 200,
                "case \"create a cluster\": expected 200, got {status} with {body}"
            );

            // A cluster AWS returns `creating` can only be deleted once the clock has made
            // it `available`; the emulator never sleeps for that, so the test lets the
            // window pass.
            std::thread::sleep(
                service_com_amazonaws_docdb_amazonrdsv19::state::PROVISIONING_DELAY * 3,
            );

            let (status, _, body) = call(
                &router,
                &runtime,
                "docdb",
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", "front-door-cluster"),
                    ("SkipFinalSnapshot", "false"),
                    ("FinalDBSnapshotIdentifier", "front-door-snapshot"),
                ],
            );
            assert_eq!(
                status, 200,
                "case \"delete the cluster with a final snapshot\": expected 200, got \
                 {status} with {body}"
            );

            // The final snapshot is still being taken, and the clock owes it `available`.
            std::thread::sleep(
                service_com_amazonaws_docdb_amazonrdsv19::state::PROVISIONING_DELAY * 3,
            );

            let (status, content_type, body) = call(
                &router,
                &runtime,
                "docdb",
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", "front-door-snapshot")],
            );
            assert_eq!(
                status, 200,
                "case \"describe the final snapshot\": expected 200, got {status} with {body}"
            );
            assert!(
                content_type.starts_with("text/xml"),
                "case \"describe the final snapshot\": an awsQuery response is text/xml, \
                 got {content_type:?} with {body}"
            );
            assert_eq!(
                element(&body, "DBClusterSnapshotIdentifier").as_deref(),
                Some("front-door-snapshot"),
                "case \"describe the final snapshot\": got {body}"
            );
            assert_eq!(
                element(&body, "DBClusterIdentifier").as_deref(),
                Some("front-door-cluster"),
                "case \"describe the final snapshot\": the snapshot names the cluster it \
                 was taken from, got {body}"
            );
            assert_eq!(
                element(&body, "Status").as_deref(),
                Some("available"),
                "case \"describe the final snapshot\": the clock owed this snapshot \
                 `available`, got {body}"
            );
        })
        .expect("spawn the round-trip thread")
        .join()
        .expect("a snapshot written through the front door must be read back through it");
}
