// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a request that named a resource it does not hold, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is the
//! status, the `x-amzn-errortype` header a client resolves the error shape from, and the
//! `message` an SDK surfaces, not an internal function's return value.
//!
//! The rule under test: AWS IoT has no single not-found sentence. Each resource FAMILY
//! refuses a lookup miss with its own fixed text, whose variable parts come from the
//! request and from this account's state:
//!
//! * the REGISTRY families qualify the name with the account the call was made for —
//!   `<Family> <accountId>:<name> not found.` — with the family's own noun (`Thing`,
//!   `ThingGroup`, `Thing Type`);
//! * the JOBS and COMMANDS planes word it `<Family> <id> cannot be found.`;
//! * the AUTHORIZER family words it `Authorizer <name> not found`;
//! * the BULK THING REGISTRATION task family words it with a sentence carrying NO part
//!   of the request at all;
//! * a `DescribeJobExecution` whose JOB does not exist names that missing job and not
//!   the thing, while one whose job exists but has no execution on the thing names the
//!   pair, because the two conditions are distinguishable;
//! * and an `indexName` that is no fleet index of this service is not a miss at all but
//!   a request the fleet-indexing plane will not accept: `InvalidRequestException`
//!   (400), whose shape name that plane's validation layer qualifies with the service's
//!   API doc namespace in `x-amzn-errortype`.
//!
//! Every operation that addresses ONE resource of a family refuses with that family's
//! sentence, so the table below spans what the rule generalises over — the family, the
//! operation (a read, a delete and an update of the same family), the method, and the
//! form and length of the identifier — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    FLEET_INDEX_NAMES, THING_GROUP_INDEX_NAME, THING_INDEX_NAME,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id this emulator serves, which the registry families qualify a missing
/// name with (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// The region every request in this suite is signed for. None of these refusals is
/// regional, so no assertion depends on it.
const REGION: &str = "eu-west-3";

/// The wire error shape of a resource this service does not hold.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// `InvalidRequestException` as AWS IoT's fleet-indexing validation layer names it in
/// `x-amzn-errortype`: the shape name, a `:`, and the service's API doc namespace.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The `x-amzn-errortype` header, verbatim — including any namespace a plane
    /// qualifies the shape name with.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The `message` the body carries, or `""` when it carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

}

/// One request that names a resource the store does not hold, and the refusal it must
/// answer.
#[derive(Debug)]
struct Case {
    /// What this row is, for the assertion messages.
    case: String,
    method: &'static str,
    path: String,
    /// A body for the methods that carry one; `None` for a read or a delete.
    body: Option<Value>,
    status: u16,
    error_type: &'static str,
    message: String,
}

/// Identifiers of the form `^[a-zA-Z0-9_-]+$` within 1..64 — the jobs and commands
/// planes' `JobId` / `CommandId` constraints — differing in length, case and separator.
fn plane_ids() -> Vec<String> {
    vec![
        "x".to_string(),
        "missing-job-id".to_string(),
        "Missing_Id_42".to_string(),
        "n".repeat(64),
    ]
}

/// Names of the form `^[a-zA-Z0-9:_-]+$` within 1..128 — the registry families'
/// `ThingName` / `ThingGroupName` / `ThingTypeName` constraints.
fn registry_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "missing-resource".to_string(),
        "Missing:Scoped_Name-9".to_string(),
        "r".repeat(128),
    ]
}

/// Names of the form `^[\w=,@-]+$` within 1..128 — the `AuthorizerName` constraint,
/// which admits punctuation the registry names do not.
fn authorizer_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "missing-authorizer".to_string(),
        "Missing_Authorizer=1,2@host".to_string(),
        "z".repeat(128),
    ]
}

/// Every row of the table: each family, over each operation of this service that
/// addresses one resource of it.
fn cases() -> Vec<Case> {
    let mut rows = Vec::new();

    for name in authorizer_names() {
        let message = format!("Authorizer {name} not found");
        // DescribeAuthorizer, DeleteAuthorizer and UpdateAuthorizer all address the one
        // record by name, so all three word the miss the same way.
        for (operation, method, body) in [
            ("DescribeAuthorizer", "GET", None),
            ("DeleteAuthorizer", "DELETE", None),
            ("UpdateAuthorizer", "PUT", Some(json!({}))),
        ] {
            rows.push(Case {
                case: format!("{operation} on authorizer {name:?}"),
                method,
                path: format!("/authorizer/{name}"),
                body,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }
    }

    for id in plane_ids() {
        let message = format!("Command {id} cannot be found.");
        for (operation, method, path, body) in [
            ("GetCommand", "GET", format!("/commands/{id}"), None),
            (
                "UpdateCommand",
                "PATCH",
                format!("/commands/{id}"),
                Some(json!({})),
            ),
        ] {
            rows.push(Case {
                case: format!("{operation} on command {id:?}"),
                method,
                path,
                body,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }

        let message = format!("Job {id} cannot be found.");
        for (operation, method, path) in [
            ("DescribeJob", "GET", format!("/jobs/{id}")),
            ("DeleteJob", "DELETE", format!("/jobs/{id}")),
            ("GetJobDocument", "GET", format!("/jobs/{id}/job-document")),
        ] {
            rows.push(Case {
                case: format!("{operation} on job {id:?}"),
                method,
                path,
                body: None,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }

        // A job execution of a job nobody created: the refusal is about the missing JOB,
        // and the thing in the URI is not part of it.
        rows.push(Case {
            case: format!("DescribeJobExecution on the missing job {id:?}"),
            method: "GET",
            path: format!("/things/some-device/jobs/{id}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: format!("Job execution cannot be found as the Job {id} does not exist."),
        });
    }

    for name in registry_names() {
        // The registry families, each with its own noun and all account-qualified.
        for (operation, method, path, noun) in [
            (
                "DescribeThing",
                "GET",
                format!("/things/{name}"),
                "Thing",
            ),
            (
                "DescribeThingGroup",
                "GET",
                format!("/thing-groups/{name}"),
                "ThingGroup",
            ),
            (
                "DescribeThingType",
                "GET",
                format!("/thing-types/{name}"),
                "Thing Type",
            ),
            (
                "DeprecateThingType",
                "POST",
                format!("/thing-types/{name}/deprecate"),
                "Thing Type",
            ),
        ] {
            rows.push(Case {
                case: format!("{operation} on {noun} {name:?}"),
                method,
                path,
                body: (method == "POST").then(|| json!({})),
                status: 404,
                error_type: NOT_FOUND,
                message: format!("{noun} {ACCOUNT_ID}:{name} not found."),
            });
        }
    }

    // The bulk-registration task family: one fixed sentence, whatever was asked for.
    // `TaskId` is length 0..40 with no pattern, so these differ in form as well as
    // length.
    for task_id in [
        "t",
        "00000000-0000-0000-0000-000000000000",
        "task-nobody-started",
    ] {
        rows.push(Case {
            case: format!("DescribeThingRegistrationTask on task {task_id:?}"),
            method: "GET",
            path: format!("/thing-registration-tasks/{task_id}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: "Task doesn't exist".to_string(),
        });
    }

    // A name that is no fleet index: the fleet-indexing plane will not accept the
    // request, so this is the 400 and not a miss.
    for index_name in ["AWS_Thing", "aws_things", "my-index", "AWS_Things:1"] {
        rows.push(Case {
            case: format!("DescribeIndex on the unrecognised index {index_name:?}"),
            method: "GET",
            path: format!("/indices/{index_name}"),
            body: None,
            status: 400,
            error_type: QUALIFIED_INVALID_REQUEST,
            message: format!("Unrecognized indexName {index_name}"),
        });
    }

    // A name that IS one of the recognised indexes, while the section that drives it is
    // off: the account has no such index, which is the miss.
    for index_name in FLEET_INDEX_NAMES {
        rows.push(Case {
            case: format!("DescribeIndex on the disabled index {index_name:?}"),
            method: "GET",
            path: format!("/indices/{index_name}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: format!("Index {index_name} does not exist"),
        });
    }

    rows
}

/// Every family's refusal: the status, the error shape and the sentence, over every
/// operation of this service that addresses one resource of that family.
#[tokio::test]
async fn each_resource_family_words_a_miss_its_own_way() {
    for case in cases() {
        // A fresh emulator per row: a refusal must not depend on what another row left
        // behind, and nothing here creates the resource it asks about.
        let emulator = Emulator::new();
        let reply = emulator
            .call(case.method, &case.path, case.body.as_ref())
            .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected status {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(case.error_type),
            "case {:?}: expected x-amzn-errortype {:?}, got {:?}",
            case.case,
            case.error_type,
            reply.error_type
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {:?}: expected message {:?}, got {:?}",
            case.case,
            case.message,
            reply.message()
        );
    }
}

/// The bulk-registration family's refusal carries NO part of the request: the requested
/// task id is nowhere in the answer.
#[tokio::test]
async fn a_missing_registration_task_echoes_nothing_of_the_request() {
    let emulator = Emulator::new();
    for task_id in ["abc", "0123456789", "task-nobody-started"] {
        let reply = emulator
            .call("GET", &format!("/thing-registration-tasks/{task_id}"), None)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {task_id:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.message().contains(task_id),
            "case {task_id:?}: this family's refusal must echo no part of the request, \
             got {:?}",
            reply.message()
        );
    }
}

/// A `DescribeJobExecution` whose JOB exists but has no execution on the thing is the
/// OTHER condition, and is worded about the pair — so the two are distinguishable, which
/// is the whole point of resolving the job first.
#[tokio::test]
async fn a_missing_job_and_a_missing_execution_are_distinguishable() {
    let emulator = Emulator::new();
    let registered = emulator
        .call("POST", "/things/targeted", Some(&json!({})))
        .await;
    assert_eq!(
        registered.status, 200,
        "registering the targeted thing must answer 200, got {} {:?}",
        registered.status, registered.body
    );
    let created = emulator
        .call(
            "PUT",
            "/jobs/known-job",
            Some(&json!({
                "targets": [format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/targeted")],
                "document": "{}",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the job must answer 200, got {} {:?}",
        created.status, created.body
    );

    // (case, the job in the URI, the job exists)
    for (case, job_id, exists) in [
        ("a job nobody created", "no-such-job", false),
        ("a job that exists", "known-job", true),
    ] {
        let reply = emulator
            .call("GET", &format!("/things/untargeted/jobs/{job_id}"), None)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        let message = reply.message();
        if exists {
            assert_eq!(
                message,
                format!(
                    "Job execution for thing untargeted cannot be found within job {job_id}."
                ),
                "case {case:?}: a missing EXECUTION of an existing job names the pair, \
                 got {message:?}"
            );
        } else {
            assert_eq!(
                message,
                format!("Job execution cannot be found as the Job {job_id} does not exist."),
                "case {case:?}: a missing JOB is reported as the missing job, got \
                 {message:?}"
            );
        }
    }
}

/// An index this service DOES recognise, once the section that drives it is enabled, is
/// described rather than refused — so the 400 the table pins is about the NAME and not
/// about the fleet-indexing plane refusing everything.
#[tokio::test]
async fn a_recognised_index_is_served_once_it_is_enabled() {
    let emulator = Emulator::new();
    // Enabled through the configuration endpoint, so the index this reads back is one a
    // request created rather than a record the test seeded.
    for body in [
        json!({ "thingIndexingConfiguration": { "thingIndexingMode": "REGISTRY" } }),
        json!({ "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": "ON" } }),
    ] {
        let updated = emulator.call("POST", "/indexing/config", Some(&body)).await;
        assert_eq!(
            updated.status, 200,
            "enabling an index must answer 200, got {} {:?}",
            updated.status, updated.body
        );
    }

    for index_name in [THING_INDEX_NAME, THING_GROUP_INDEX_NAME] {
        let reply = emulator
            .call("GET", &format!("/indices/{index_name}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index_name:?}: an enabled index must be described, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body.get("indexName").and_then(Value::as_str),
            Some(index_name),
            "case {index_name:?}: the description must name the index, got {:?}",
            reply.body
        );
    }
}
