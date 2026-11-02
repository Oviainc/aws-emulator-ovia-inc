// SPDX-License-Identifier: CC0-1.0

//! Which error shapes the tagging calls can answer with, driven over the wire.
//!
//! Every case goes through the real router, so the restJson1 routing of
//! `/tags/{resourceArn}`, the boundary constraint walk, the handler and the error
//! serialization all run exactly as they do in the emulator binary.
//!
//! The rule under test: a call answers only the error shapes its own operation declares,
//! plus the service's common errors and the not-found class the tagging routes
//! themselves carry — so a failure of the EMULATOR that none of them describes is
//! reported as the cross-service `InternalFailure` (HTTP 500), and never as a modelled
//! shape the model declares for neither the operation nor the service.
//!
//! The table spans every operation of this service whose modelled error set carries no
//! server fault — the three the model binds to `/tags/{resourceArn}` (`GET` lists,
//! `POST` tags, `DELETE` untags) — and, for each, the ways a call can fail: its own
//! store out of reach, which is the emulator's own fault; an ARN that names no resource
//! here; an ARN the modelled `@pattern` refuses before any handler runs; and a
//! well-formed call that is served.

use serde_json::Value;
use service_com_amazonaws_ssmsap_ssmsap::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_lifecycle::{dispatch, Emulator, Reply};
use crate::resource_tags::{application_arn, database_arn, register, registration, tags_path};
use crate::tag_resource::{pairs, tags_body};
use crate::untag_resource::untag_path;
use crate::validation_message_rendering::message;

/// The region every case signs for; it is the region each expected ARN carries.
const REGION: &str = "us-east-1";

/// The error code AWS reports a server-side fault with across its services, which is
/// what an operation whose model declares no server-fault shape has to answer an
/// emulator failure with. HTTP 500.
const INTERNAL_FAILURE: &str = "InternalFailure";

/// The error code an ARN naming no resource of this account and Region is answered with:
/// the class the `/tags/{resourceArn}` routes really carry (HTTP 404), which the model
/// declares as no shape — the modelled `ResourceNotFoundException` is the shape name, not
/// the code those routes answer.
const TAGGING_NOT_FOUND_CODE: &str =
    service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;

/// The modelled error code a request the constraint walk refuses is answered with: the
/// service's one common error (`smithy_query(query="service_info")`:
/// `common_errors: ["ValidationException"]`, `@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The WHOLE set of error codes a tagging call may answer with: the three shapes each of
/// the three operations declares (`smithy_query(query="operation",
/// operation="ListTagsForResource,TagResource,UntagResource")`: `ConflictException`,
/// `ResourceNotFoundException`, `ValidationException`), the not-found class those routes
/// carry instead of the modelled shape name ([`TAGGING_NOT_FOUND_CODE`]), plus the
/// cross-service `InternalFailure` for a fault of the emulator itself.
const ANSWERABLE_CODES: &[&str] = &[
    "ConflictException",
    "ResourceNotFoundException",
    TAGGING_NOT_FOUND_CODE,
    VALIDATION_EXCEPTION,
    INTERNAL_FAILURE,
];

/// The error shapes this service's model declares SOMEWHERE but not on the tagging
/// operations, and not on the service either (`smithy_query(query="digest")`: the other
/// operations declare `InternalServerException` and `UnauthorizedException`, and
/// `common_errors` is `ValidationException` alone). No tagging call may answer with one
/// of these: a client of these operations is never told to expect them.
const CODES_THESE_OPERATIONS_NEVER_ANSWER: &[&str] =
    &["InternalServerException", "UnauthorizedException"];

/// One tagging operation and how a client addresses it, from the model's own HTTP
/// binding (`smithy_query(query="operation",
/// operation="ListTagsForResource,TagResource,UntagResource")`: `GET`, `POST` and
/// `DELETE` on `/tags/{resourceArn}`, with `tags` in the body of the `POST` and `tagKeys`
/// in the query string of the `DELETE`).
struct Tagging {
    /// How an assertion names the operation.
    operation: &'static str,
    method: &'static str,
    /// The URI and body a call of this operation carries for one resource ARN.
    request: fn(&str) -> (String, Option<Value>),
}

/// `ListTagsForResource`: every member is bound to the URI, so the call carries no body.
fn list_request(resource_arn: &str) -> (String, Option<Value>) {
    (tags_path(resource_arn), None)
}

/// `TagResource`: `tags` is the body member.
fn tag_request(resource_arn: &str) -> (String, Option<Value>) {
    (
        tags_path(resource_arn),
        Some(tags_body(&pairs(&[("env", "dev")]))),
    )
}

/// `UntagResource`: `tagKeys` is bound to the query string, so the call carries no body.
fn untag_request(resource_arn: &str) -> (String, Option<Value>) {
    (untag_path(resource_arn, &["env".to_string()]), None)
}

/// The rows: every operation of this service whose modelled errors carry no server
/// fault.
const TAGGING_OPERATIONS: &[Tagging] = &[
    Tagging {
        operation: "ListTagsForResource",
        method: "GET",
        request: list_request,
    },
    Tagging {
        operation: "TagResource",
        method: "POST",
        request: tag_request,
    },
    Tagging {
        operation: "UntagResource",
        method: "DELETE",
        request: untag_request,
    },
];

/// An engine that is NOT the one this service keeps its records in, so a handler mounted
/// on it resolves no store at all.
///
/// This is the only way the store-unavailable branch is reachable: a service mounted on
/// the shared in-memory engine always has its own partition, which is why the branch
/// needs a router of its own rather than the harness every other suite drives.
#[derive(Debug)]
struct StorelessEngine;

/// The service mounted on [`StorelessEngine`], reached over the same wire as any other
/// suite.
fn storeless_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(std::sync::Arc::new(StorelessEngine)));
    router
}

/// Assert the reply carries a code the operation may answer with at all, and none of the
/// shapes the model declares elsewhere in this service.
fn assert_answerable_code(case: &str, reply: &Reply) {
    let code = reply.code();
    assert!(
        !CODES_THESE_OPERATIONS_NEVER_ANSWER.contains(&code),
        "case {case:?}: the model declares {code:?} for neither this operation nor the \
         service, so no client of it is told to expect that shape; got {reply:?}"
    );
    assert!(
        code.is_empty() || ANSWERABLE_CODES.contains(&code),
        "case {case:?}: expected one of {ANSWERABLE_CODES:?}, got {code:?} in {reply:?}"
    );
}

/// RULE: a failure of the emulator itself — its own store out of reach — is answered with
/// the cross-service `InternalFailure` (HTTP 500), because no shape these operations
/// declare describes one; a request the modelled constraints refuse is still the
/// modelled `ValidationException` (HTTP 400), because the boundary refuses it before any
/// handler looks for the store.
#[tokio::test]
async fn a_fault_of_the_emulator_is_answered_with_the_cross_service_internal_failure() {
    let router = storeless_router();

    // Rows: the forms a `resourceArn` can take — this service's own application and
    // database ARNs, another service's resource, and a value the modelled `@pattern`
    // refuses — paired with the channel each must be answered on.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an application ARN",
            application_arn(REGION, "tags-no-store"),
            500,
            INTERNAL_FAILURE,
        ),
        (
            "a database ARN",
            database_arn(REGION, "tags-no-store", "HDB"),
            500,
            INTERNAL_FAILURE,
        ),
        (
            "another service's resource",
            "arn:aws:s3:::a-bucket/an-object".to_string(),
            500,
            INTERNAL_FAILURE,
        ),
        (
            "a value the modelled pattern refuses",
            "not-an-arn".to_string(),
            400,
            VALIDATION_EXCEPTION,
        ),
    ];

    for op in TAGGING_OPERATIONS {
        for (label, arn, expected_status, expected_code) in &rows {
            let case = format!("{}/{label}", op.operation);
            let (path, body) = (op.request)(arn);
            let reply = dispatch(&router, op.method, REGION, &path, body).await;

            assert_eq!(
                reply.status, *expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                *expected_code,
                "case {case:?}: expected the error code {expected_code:?}, got {:?}",
                reply.code()
            );
            assert_answerable_code(&case, &reply);
            assert!(
                !message(&reply).is_empty(),
                "case {case:?}: an error says what went wrong, got {reply:?}"
            );
        }
    }
}

/// RULE: with its store in reach, each tagging call answers only the classes its route
/// really carries — its own not-found class for an ARN naming no resource here, the
/// modelled `ValidationException` for a request the constraints refuse, and a served call
/// with no error at all.
#[tokio::test]
async fn a_served_call_answers_only_the_shapes_its_own_model_declares() {
    let emulator = Emulator::new();
    register(
        &emulator,
        "a registered application",
        registration("tags-channel", &[], Some(&["HDB"])),
    )
    .await;

    // Rows: the ARN each call names, and what the operation must answer for it.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "the registered application",
            application_arn(REGION, "tags-channel"),
            200,
            "",
        ),
        (
            "a database of that registration",
            database_arn(REGION, "tags-channel", "HDB"),
            200,
            "",
        ),
        (
            "an application never registered",
            application_arn(REGION, "tags-absent"),
            404,
            TAGGING_NOT_FOUND_CODE,
        ),
        (
            "a database the registration never named",
            database_arn(REGION, "tags-channel", "NOPE"),
            404,
            TAGGING_NOT_FOUND_CODE,
        ),
        (
            "a value the modelled pattern refuses",
            "not-an-arn".to_string(),
            400,
            VALIDATION_EXCEPTION,
        ),
    ];

    for op in TAGGING_OPERATIONS {
        for (label, arn, expected_status, expected_code) in &rows {
            let case = format!("{}/{label}", op.operation);
            let (path, body) = (op.request)(arn);
            let reply = emulator
                .send_request(op.method, REGION, &path, body)
                .await;

            assert_eq!(
                reply.status, *expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                *expected_code,
                "case {case:?}: expected the error code {expected_code:?}, got {:?}",
                reply.code()
            );
            assert_answerable_code(&case, &reply);
        }
    }
}
