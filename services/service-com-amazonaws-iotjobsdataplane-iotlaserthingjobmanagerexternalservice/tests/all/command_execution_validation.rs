// SPDX-License-Identifier: CC0-1.0

//! What this service REFUSES on the command-execution surface, and how it says so.
//!
//! Three rules, each asserted over a set of inputs rather than one case:
//!
//! 1. an execution starts only for a command that EXISTS — an unregistered
//!    `commandArn` is refused 400 `ValidationException` naming the command, and
//!    nothing is persisted;
//! 2. a `targetArn` the service cannot target is refused 400 with the value READ
//!    BACK out of the request — the resource type when it names an IoT resource,
//!    the value itself when it does not — so two rejections of two targets are
//!    distinguishable;
//! 3. a per-execution read or write for a job the thing has no execution of is
//!    404, reporting the job and the thing the request named — one form, from
//!    both operations that address a single execution.
//!
//! Every request goes over the wire (restJson1 through the multi-service router),
//! so what is asserted is the HTTP behaviour a client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::command_registry;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator whose shared IoT command index holds exactly `commands`.
    ///
    /// The index is the control plane's (`iot:CreateCommand` registers an ARN
    /// there; it is still a scaffold in its own crate, under its own issue), so a
    /// test that needs a command to exist writes it the way that create will.
    fn with_commands(commands: &[&str]) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        {
            let index = command_registry(&*engine).expect("the shared IoT command index");
            let mut index = index.lock();
            for command_arn in commands {
                index
                    .register(command_arn)
                    .unwrap_or_else(|refusal| panic!("register {command_arn}: {refusal}"));
            }
        }
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://data.jobs.iot.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGV4)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn start_command(&self, target_arn: &str, command_arn: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/command-executions",
            Some(json!({"targetArn": target_arn, "commandArn": command_arn})),
        )
        .await
    }

    async fn pending(&self, thing_name: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs"), None)
            .await
    }

    async fn describe(&self, thing_name: &str, job_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs/{job_id}"), None)
            .await
    }

    async fn update(&self, thing_name: &str, job_id: &str) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/things/{thing_name}/jobs/{job_id}"),
            Some(json!({"status": "IN_PROGRESS"})),
        )
        .await
    }
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `message` of a modeled failure.
fn message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or("")
}

/// A thing ARN for `thing_name` in the `aws` partition — the accepted target
/// form, used as the fixed half of the command-existence rows.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}")
}

/// RULE: `StartCommandExecution` starts an execution only for a command that
/// EXISTS ("using the command created with the CreateCommand API, start a command
/// execution on a specific device"). A `commandArn` naming no command is refused
/// 400 `ValidationException` whose message names that command, and nothing is
/// persisted for the target; a registered one is accepted and the execution is
/// readable back.
///
/// The rows differ in command id, region, partition and account — a command is
/// identified by its whole ARN, so the same id in another region is another
/// command.
#[tokio::test]
async fn an_execution_starts_only_for_a_command_that_exists() {
    const REGISTERED: &[&str] = &[
        "arn:aws:iot:us-east-1:111122223333:command/reboot",
        "arn:aws-cn:iot:cn-north-1:444455556666:command/install_firmware",
    ];
    let emulator = Emulator::with_commands(REGISTERED);

    // (case, thing, commandArn, exists)
    let rows: [(&str, &str, &str, bool); 6] = [
        (
            "a registered command",
            "laser-exists-1",
            REGISTERED[0],
            true,
        ),
        (
            "a registered command in another partition and region",
            "laser-exists-2",
            REGISTERED[1],
            true,
        ),
        (
            "a command id nobody created",
            "laser-absent-1",
            "arn:aws:iot:us-east-1:111122223333:command/no-such-command",
            false,
        ),
        (
            "a registered id in another region",
            "laser-absent-2",
            "arn:aws:iot:eu-west-1:111122223333:command/reboot",
            false,
        ),
        (
            "a registered id in another account",
            "laser-absent-3",
            "arn:aws:iot:us-east-1:999988887777:command/reboot",
            false,
        ),
        (
            "a command id that only prefixes a registered one",
            "laser-absent-4",
            "arn:aws:iot:us-east-1:111122223333:command/reboo",
            false,
        ),
    ];

    for (case, thing, command_arn, exists) in rows {
        let (status, body) = emulator.start_command(&thing_arn(thing), command_arn).await;
        if exists {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for a command that exists, got {status} {body}"
            );
            let execution_id = body["executionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an executionId, got {body}"))
                .to_string();

            // Round trip: the execution the accept created is readable back, and
            // its job document names the command it was started for.
            let (status, described) = emulator.describe(thing, &execution_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the started execution to be readable, got {status} {described}"
            );
            let document: Value = serde_json::from_str(
                described["execution"]["jobDocument"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no jobDocument in {described}")),
            )
            .unwrap_or_else(|error| panic!("case {case:?}: the job document must be JSON: {error}"));
            assert_eq!(
                document["commandArn"], command_arn,
                "case {case:?}: the job document names the command, got {document}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a command that does not exist, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let command_name = command_arn.rsplit('/').next().unwrap_or_default();
        assert_eq!(
            message(&body),
            format!("The command {command_name} was not found."),
            "case {case:?}: the refusal must name the missing command, got {body}"
        );

        // A refused start persists NOTHING: the target keeps an empty queue.
        let (status, pending) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the queue read, got {status} {pending}"
        );
        assert_eq!(
            pending["queuedJobs"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused start must queue nothing, got {pending}"
        );
    }
}

/// RULE: a `targetArn` the service cannot target is refused 400
/// `ValidationException` with a message built FROM THE REQUEST: it states how the
/// supplied value was READ — the resource type parsed out of it when it names an
/// IoT resource, the value itself when no IoT resource type can be read out of it
/// — followed by "not supported as a target". The one supported type is an IoT
/// `thing` (`targetArn` is "the ARN of the device where the command execution is
/// occurring").
///
/// The rows differ in the resource type named, in which of the two forms AWS
/// writes a resource in (`<type>/<id>` and `<type>:<id>`), in the partition and
/// region, in whether the ARN carries an id at all, in whether it is this
/// service's ARN, and in whether the value parses as an ARN. The types are those
/// of the IoT resource ARNs the service's own models write — `job/<jobId>`,
/// `thinggroup/<name>`, `command/<commandId>` (the form
/// `StartCommandExecution$commandArn` documents: "arn:aws:iot:::command/"),
/// `policy/<name>`, `cert/<id>` — plus one type no AWS service defines, to show
/// the message is parsed and not looked up in a list.
#[tokio::test]
async fn an_unsupported_target_is_read_back_in_the_refusal() {
    const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";
    let emulator = Emulator::with_commands(&[COMMAND_ARN]);

    // (case, targetArn, how the message must say the value was read)
    let rows: [(&str, &str, &str); 13] = [
        (
            "a job",
            "arn:aws:iot:us-east-1:111122223333:job/upgrade-42",
            "Resource type job",
        ),
        (
            "a thing group",
            "arn:aws:iot:us-east-1:111122223333:thinggroup/plant-floor",
            "Resource type thinggroup",
        ),
        (
            "a command",
            "arn:aws:iot:us-east-1:111122223333:command/reboot",
            "Resource type command",
        ),
        (
            "a policy in another partition",
            "arn:aws-us-gov:iot:us-gov-west-1:111122223333:policy/device-policy",
            "Resource type policy",
        ),
        (
            "a certificate written with a colon",
            "arn:aws:iot:eu-west-1:111122223333:cert:9f3c",
            "Resource type cert",
        ),
        (
            "a type no service defines",
            "arn:aws:iot:us-east-1:111122223333:unicorn/sparkle",
            "Resource type unicorn",
        ),
        (
            "a resource with no id at all",
            "arn:aws:iot:us-east-1:111122223333:job",
            "Resource type job",
        ),
        (
            "a thing of another service",
            "arn:aws:s3:us-east-1:111122223333:thing/laser-1",
            "Invalid target arn arn:aws:s3:us-east-1:111122223333:thing/laser-1",
        ),
        ("not an ARN", "not-an-arn", "Invalid target arn not-an-arn"),
        (
            "an ARN with too few fields",
            "arn:aws:iot:thing/laser-1",
            "Invalid target arn arn:aws:iot:thing/laser-1",
        ),
        (
            "an ARN with an empty resource",
            "arn:aws:iot:us-east-1:111122223333:",
            "Invalid target arn arn:aws:iot:us-east-1:111122223333:",
        ),
        (
            // The supported TYPE, but naming no thing: read back as the value it
            // is, since "Resource type thing not supported" would deny the one
            // type that is supported. An empty name reads the same way as none.
            "an IoT thing ARN with no name",
            "arn:aws:iot:us-east-1:111122223333:thing",
            "Invalid target arn arn:aws:iot:us-east-1:111122223333:thing",
        ),
        (
            "an IoT thing ARN with an empty name",
            "arn:aws:iot:us-east-1:111122223333:thing/",
            "Invalid target arn arn:aws:iot:us-east-1:111122223333:thing/",
        ),
    ];

    let mut reported: Vec<String> = Vec::new();
    for (case, target_arn, reading) in rows {
        let (status, body) = emulator.start_command(target_arn, COMMAND_ARN).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an unsupported target, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            format!("{reading} not supported as a target"),
            "case {case:?}: the refusal must read the supplied target back, got {body}"
        );
        reported.push(message(&body).to_string());
    }

    // Two rejections of two different targets are DISTINGUISHABLE, which a fixed
    // template could not be: only the two `job` rows (with and without an id)
    // share a message, so 13 rows report 12 distinct messages.
    let distinct: std::collections::BTreeSet<&String> = reported.iter().collect();
    assert_eq!(
        distinct.len(),
        12,
        "expected one message per rejected target, got {reported:?}"
    );

    // The supported type is accepted, so the refusal is about what the value
    // names and not about the ARN's shape.
    let (status, body) = emulator
        .start_command(&thing_arn("laser-target-ok"), COMMAND_ARN)
        .await;
    assert_eq!(
        status, 200,
        "case \"a thing\": expected 200 for the supported target type, got {status} {body}"
    );
}

/// RULE: an operation that addresses ONE job execution takes the job and the
/// thing as required path labels, so when the thing has no execution of that job
/// it answers 404 `ResourceNotFoundException` with a message built from those two
/// request members — the same form from `DescribeJobExecution` and
/// `UpdateJobExecution`.
///
/// The rows differ in whether the thing is unknown or merely has other
/// executions, and in the form of the job id (hyphens, underscores, digits,
/// mixed case, a 64-character id).
#[tokio::test]
async fn a_missing_execution_reports_the_job_and_the_thing_it_was_asked_for() {
    const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";
    let emulator = Emulator::with_commands(&[COMMAND_ARN]);

    // A thing that DOES hold an execution, so a missing job on a known thing is
    // covered as well as one on a thing nobody has ever targeted.
    let (status, started) = emulator
        .start_command(&thing_arn("laser-known"), COMMAND_ARN)
        .await;
    assert_eq!(status, 200, "seeding an execution: got {status} {started}");

    // (case, thing, jobId)
    let rows: [(&str, &str, &str); 5] = [
        ("a thing with no executions", "laser-unknown", "upgrade-7"),
        ("a thing that holds another execution", "laser-known", "upgrade-7"),
        ("an underscored job id", "laser-known", "upgrade_firmware_2"),
        ("a numeric job id", "laser-unknown", "20261022"),
        (
            "a 64-character job id",
            "laser-unknown",
            "jjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjj",
        ),
    ];

    for (case, thing, job_id) in rows {
        let expected = format!("Job {job_id} not found for {thing}");
        for (operation, (status, body)) in [
            ("DescribeJobExecution", emulator.describe(thing, job_id).await),
            ("UpdateJobExecution", emulator.update(thing, job_id).await),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?} ({operation}): expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} ({operation}): expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?} ({operation}): expected {expected:?}, got {body}"
            );
        }
    }

    // The execution that DOES exist is still found, so the 404 is about the job
    // and not about the operation.
    let execution_id = started["executionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an executionId, got {started}"));
    let (status, body) = emulator.describe("laser-known", execution_id).await;
    assert_eq!(
        status, 200,
        "the seeded execution must still be readable, got {status} {body}"
    );
}
