// SPDX-License-Identifier: CC0-1.0

//! How Braket WORDS the client errors it answers, over HTTP.
//!
//! The rule these suites pin: the message of a client error belongs to the OPERATION
//! that raised it. Amazon Braket words the identical input defect differently from one
//! operation to the next — the same malformed `quantumTaskArn` is `"quantumTaskArn is
//! invalid."` to one door and `"quantumTaskArn is not valid."` to another, and the same
//! `jobArn` is named with its rejected value by one door and without it by another — so
//! no sentence derived once for the service can serve them all, and none of these
//! messages explains a grammar the service does not state.
//!
//! The refusals that name a resource the caller cannot resolve follow the same rule from
//! the other side: an operation that models no not-found error answers one FIXED sentence
//! pointing at the operation that enumerates the device inventory, while `GetDevice`,
//! which does model one, states the ARN it could not resolve, the region the request was
//! signed for (device availability is region-scoped) and the documentation that lists
//! which regions publish which device.
//!
//! Each suite asserts its rule over a TABLE spanning the class: the rows differ along the
//! operation, the HTTP method, the member, the part of the ARN grammar the value misses
//! and — where the message states it — the region the request was signed for. The
//! recorded calls behind the rule are one row each.
//!
//! Everything goes through the production router, reusing the harness in
//! `quantum_task_loop.rs`, so what is pinned is what a Braket client sees.

use serde_json::{json, Value};

use crate::quantum_task_loop::{create_body, encode, Braket, Reply};

/// Values that miss an ARN `@pattern`, one row per part of the grammar a value can get
/// wrong. `{resource}` is replaced by the resource type the parameter under test names,
/// so one table serves every ARN parameter. `(case, value)`.
const MALFORMED_ARNS: &[(&str, &str)] = &[
    // The value the recorded calls behind this rule sent.
    ("not an ARN at all", "not-an-arn"),
    ("a bare resource id", "r1"),
    ("the arn prefix in upper case", "ARN:aws:braket:us-east-1:000000000000:{resource}/r1"),
    ("a partition outside the aws family", "arn:azure:braket:us-east-1:000000000000:{resource}/r1"),
    ("another service's namespace", "arn:aws:s3:us-east-1:000000000000:{resource}/r1"),
    ("another Braket resource type", "arn:aws:braket:us-east-1:000000000000:other-resource/r1"),
    ("an account id shorter than 12 digits", "arn:aws:braket:us-east-1:12345:{resource}/r1"),
];

/// One more row for the parameters whose shape carries a `@pattern`: that expression
/// requires a NON-EMPTY region (`...:braket:[a-z0-9\-]+:...`), so an ARN without one
/// misses it. It is not in the table above because an empty region is not what makes a
/// DEVICE ARN malformed — the device ARNs Amazon Braket publishes carry neither region
/// nor account. `(case, value)`.
const MALFORMED_ONLY_AGAINST_A_PATTERN: &[(&str, &str)] =
    &[("an empty region", "arn:aws:braket::000000000000:{resource}/r1")];

/// One door that takes an ARN-shaped PARAMETER outside the request body whose target
/// shape carries a `@pattern` — every one the model declares
/// (`smithy_query(query="operation",operation="GetJob,CancelJob,DeleteSpendingLimit,UpdateSpendingLimit")`
/// over `JobArn` and `SpendingLimitArn`, the only two ARN shapes with a `@pattern` that
/// are bound to a URI label).
struct ArnParameterDoor {
    operation: &'static str,
    method: &'static str,
    /// The request path, with `{arn}` where the parameter's value goes.
    path: &'static str,
    /// The parameter the refusal names.
    parameter: &'static str,
    /// The resource type the parameter's ARN names, for [`MALFORMED_ARNS`].
    resource: &'static str,
    /// A body every modelled rule of the operation accepts, so the only violation a row
    /// carries is its parameter's.
    body: Option<fn() -> Value>,
    /// The message THIS door answers, built from the parameter it names and the value it
    /// rejected.
    wording: fn(&str, &str) -> String,
}

/// `"<parameter> parameter is not valid."` — `GetJob`'s sentence.
fn parameter_is_not_valid(parameter: &str, _value: &str) -> String {
    format!("{parameter} parameter is not valid.")
}

/// `"<parameter> <value> is not a valid ARN."` — `CancelJob`'s sentence, the one form
/// that states the rejected value.
fn value_is_not_a_valid_arn(parameter: &str, value: &str) -> String {
    format!("{parameter} {value} is not a valid ARN.")
}

/// `"<parameter> is not valid."` — the spending-limit doors' sentence.
fn is_not_valid(parameter: &str, _value: &str) -> String {
    format!("{parameter} is not valid.")
}

/// The body `UpdateSpendingLimit` requires: `clientToken` is `@required`, so a row that
/// omitted it would be refused for its BODY instead of for its parameter.
fn update_body() -> Value {
    json!({ "clientToken": "tok-wording-update", "spendingLimit": "10" })
}

/// Every door of the class, with the sentence each one words the refusal in.
const ARN_PARAMETER_DOORS: &[ArnParameterDoor] = &[
    ArnParameterDoor {
        operation: "GetJob",
        method: "GET",
        path: "/job/{arn}",
        parameter: "jobArn",
        resource: "job",
        body: None,
        wording: parameter_is_not_valid,
    },
    ArnParameterDoor {
        operation: "CancelJob",
        method: "PUT",
        path: "/job/{arn}/cancel",
        parameter: "jobArn",
        resource: "job",
        body: None,
        wording: value_is_not_a_valid_arn,
    },
    ArnParameterDoor {
        operation: "DeleteSpendingLimit",
        method: "DELETE",
        path: "/spending-limit/{arn}/delete",
        parameter: "spendingLimitArn",
        resource: "spending-limit",
        body: None,
        wording: is_not_valid,
    },
    ArnParameterDoor {
        operation: "UpdateSpendingLimit",
        method: "PATCH",
        path: "/spending-limit/{arn}/update",
        parameter: "spendingLimitArn",
        resource: "spending-limit",
        body: Some(update_body),
        wording: is_not_valid,
    },
];

/// Assert `reply` is a 400 carrying the modelled `ValidationException` and exactly
/// `expected`.
fn assert_validation(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} ({})",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected the modelled ValidationException, got {:?} ({})",
        reply.error,
        reply.body
    );
    assert_eq!(
        reply.message(),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        reply.message()
    );
}

/// An ARN-shaped parameter outside its modelled `@pattern` is refused in the sentence
/// the SERVING OPERATION words it in — and the two doors that take the same `jobArn`
/// word it differently, which is why no single sentence is derived for the class.
#[tokio::test]
async fn an_invalid_arn_parameter_is_worded_by_the_operation_that_refused_it() {
    let braket = Braket::new();

    for door in ARN_PARAMETER_DOORS {
        for (grammar, template) in MALFORMED_ARNS
            .iter()
            .chain(MALFORMED_ONLY_AGAINST_A_PATTERN)
        {
            let value = template.replace("{resource}", door.resource);
            let case = format!("{} with {grammar} ({value})", door.operation);
            let reply = braket
                .call(
                    door.method,
                    &door.path.replace("{arn}", &encode(&value)),
                    door.body.map(|body| body()),
                )
                .await;
            assert_validation(&reply, &case, &(door.wording)(door.parameter, &value));
        }
    }

    // The same input, one door to the next: the sentences are the operations' own, so
    // the pair that shares a parameter does not share a message.
    let get = braket.call("GET", "/job/not-an-arn", None).await;
    let cancel = braket.call("PUT", "/job/not-an-arn/cancel", None).await;
    assert_ne!(
        get.message(),
        cancel.message(),
        "case GetJob and CancelJob on the same malformed jobArn: expected each operation's \
         own sentence, got {:?} from both",
        get.message()
    );
}

/// A parameter that breaks some OTHER modelled rule broke something else, and keeps the
/// framework's wording, which states the constraint it broke rather than any of the
/// sentences above.
#[tokio::test]
async fn a_parameter_outside_another_constraint_keeps_the_constraint_wording() {
    let braket = Braket::new();

    // `DeviceArn` is @length 1..256 (`smithy_query(query="shape",shape="DeviceArn")`),
    // so a longer label breaks the bound rather than any ARN syntax.
    let long = "a".repeat(300);
    let reply = braket.call("GET", &format!("/device/{long}"), None).await;
    assert_eq!(
        reply.error.as_deref(),
        Some("ValidationException"),
        "case a label past its @length bound: expected ValidationException, got {:?} ({})",
        reply.error,
        reply.body
    );
    let message = reply.message();
    assert!(
        message.contains("failed to satisfy constraint") && message.contains("deviceArn"),
        "case a label past its @length bound: expected the constraint wording naming the \
         member, got {message:?}"
    );
}

/// Values that are not Braket quantum-task ARNs. `QuantumTaskArn` carries no `@pattern`,
/// so these reach the handler, which words the refusal itself.
const MALFORMED_QUANTUM_TASK_ARNS: &[(&str, &str)] = &[
    ("not an ARN at all", "not-a-valid-quantum-task-arn"),
    ("a bare resource id", "00000000-0000-4000-8000-000000000000"),
    ("another service's namespace", "arn:aws:s3:us-east-1:000000000000:quantum-task/t1"),
    ("another Braket resource type", "arn:aws:braket:us-east-1:000000000000:job/j1"),
    ("an empty resource id", "arn:aws:braket:us-east-1:000000000000:quantum-task/"),
    ("a region in upper case", "arn:aws:braket:US-EAST-1:000000000000:quantum-task/t1"),
];

/// The two quantum-task doors word the SAME malformed label differently, each in its own
/// sentence, over every form the value can take.
#[tokio::test]
async fn each_quantum_task_door_words_a_malformed_label_in_its_own_sentence() {
    let braket = Braket::new();

    for (index, (grammar, value)) in MALFORMED_QUANTUM_TASK_ARNS.iter().enumerate() {
        let read = braket.get_task(value).await;
        assert_validation(
            &read,
            &format!("GetQuantumTask with {grammar} ({value})"),
            "quantumTaskArn is invalid.",
        );

        let cancel = braket
            .cancel_task(value, &format!("tok-wording-cancel-{index}"))
            .await;
        assert_validation(
            &cancel,
            &format!("CancelQuantumTask with {grammar} ({value})"),
            "quantumTaskArn is not valid.",
        );
    }
}

/// Well-formed device ARNs the inventory does not publish: the shape of the value varies,
/// the refusal's template does not. `(case, value)`.
const ABSENT_DEVICE_ARNS: &[(&str, &str)] = &[
    (
        "a simulator that is not published",
        "arn:aws:braket:::device/quantum-simulator/amazon/nope",
    ),
    (
        "a QPU that is not published",
        "arn:aws:braket:us-west-2::device/qpu/rigetti/Nothing",
    ),
    (
        "another partition",
        "arn:aws-cn:braket:cn-north-1:123456789012:device/qpu/nobody/Nothing",
    ),
];

/// The regions a row is signed for: the message states the region the REQUEST named, so
/// the same absent ARN is reported differently depending on where it was asked for.
const REGIONS: &[&str] = &["us-west-2", "eu-west-1", "ap-northeast-1"];

/// `GetDevice` reports a device ARN the inventory does not publish as a missing resource
/// that states the ARN, the region the request was signed for and the device list in the
/// documentation.
#[tokio::test]
async fn get_device_states_the_arn_the_region_and_the_documentation() {
    let braket = Braket::new();

    for (case, value) in ABSENT_DEVICE_ARNS {
        for region in REGIONS {
            let reply = braket
                .call_in_region(region, "GET", &format!("/device/{}", encode(value)), None)
                .await;
            let case = format!("{case} asked for in {region}");
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404, got {} ({})",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the modelled ResourceNotFoundException, got {:?} ({})",
                reply.error,
                reply.body
            );
            let expected = format!(
                "Braket device '{value}' not found in {region}. You can find a list of all \
                 supported device ARNs and the regions in which they are available in the \
                 documentation: \
                 https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html"
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // A device the inventory DOES publish is still answered, in every region: what the
    // message above reports is the inventory's answer, not a refusal of the region.
    let device_arn = braket.an_online_device().await;
    for region in REGIONS {
        let reply = braket
            .call_in_region(
                region,
                "GET",
                &format!("/device/{}", encode(&device_arn)),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case a published device asked for in {region}: expected HTTP 200, got {} ({})",
            reply.status, reply.body
        );
    }
}

/// A `CreateJob` body every modelled rule accepts, with `deviceConfig.device` set to
/// `device_arn`.
fn job_body(client_token: &str, job_name: &str, device_arn: &str) -> Value {
    json!({
        "clientToken": client_token,
        "jobName": job_name,
        "roleArn": "arn:aws:iam::123456789012:role/BraketJobsRole",
        "algorithmSpecification": {
            "containerImage": {
                "uri": "000000000000.dkr.ecr.eu-west-1.amazonaws.com/braket:latest"
            }
        },
        "outputDataConfig": { "s3Path": "s3://amazon-braket-emulator/jobs" },
        "instanceConfig": { "instanceType": "ml.m5.large", "volumeSizeInGb": 30 },
        "deviceConfig": { "device": device_arn },
    })
}

/// A `CreateSpendingLimit` body every modelled rule accepts, on `device_arn`.
fn spending_limit_body(client_token: &str, device_arn: &str) -> Value {
    json!({
        "clientToken": client_token,
        "deviceArn": device_arn,
        "spendingLimit": "100.50",
    })
}

/// The sentence every operation that resolves a device ARN against the inventory and
/// models no not-found error answers for an ARN naming no published device: fixed text
/// that interpolates nothing and points at the operation which enumerates the inventory.
const DEVICE_ARN_NOT_FOUND: &str =
    "Device ARN not found. Please use SearchDevices operation to get a list of supported \
     devices.";

/// Well-formed Braket device ARNs whose category segment names no kind of device Amazon
/// Braket publishes devices under, so the value resolves to no device at all whatever an
/// inventory holds. `(case, value)`.
const UNKNOWN_KIND_DEVICE_ARNS: &[(&str, &str)] = &[
    (
        "a category nothing is published under",
        "arn:aws:braket:us-west-2::device/quantum-annealer/acme/Advantage",
    ),
    (
        "a device ARN carrying no category at all",
        "arn:aws:braket:::device/sv1",
    ),
];

/// Each create that takes a device ARN in its BODY refuses one that resolves to no device
/// with the service's fixed inventory pointer, and one that is not an ARN at all
/// by naming the member it read it from — the two are different refusals, and neither
/// echoes the value AWS does not echo.
///
/// What "resolves to no device" means differs with what the create DOES: the two creates
/// that dispatch work to a device resolve the ARN against the published inventory, while
/// `CreateSpendingLimit` — a cost control about the device an ARN names — resolves only the
/// KIND the ARN itself states (`spending_limits.rs` pins that admission).
#[tokio::test]
async fn a_create_refuses_an_unresolvable_device_with_the_inventory_pointer() {
    let braket = Braket::new();

    // (case, member the malformed refusal names, whether the create resolves the ARN
    // against the inventory, the request for one device ARN)
    #[allow(clippy::type_complexity)]
    let creates: Vec<(&str, &str, bool, Box<dyn Fn(usize, &str) -> (&'static str, Value)>)> = vec![
        (
            "CreateQuantumTask",
            "deviceArn",
            true,
            Box::new(|index: usize, device_arn: &str| {
                (
                    "/quantum-task",
                    create_body(&format!("tok-wording-task-{index}"), device_arn, 1),
                )
            }),
        ),
        (
            "CreateJob",
            "deviceConfig.device",
            true,
            Box::new(|index: usize, device_arn: &str| {
                (
                    "/job",
                    job_body(
                        &format!("tok-wording-job-{index}"),
                        &format!("job-wording-{index}"),
                        device_arn,
                    ),
                )
            }),
        ),
        (
            "CreateSpendingLimit",
            "deviceArn",
            false,
            Box::new(|index: usize, device_arn: &str| {
                (
                    "/spending-limit",
                    spending_limit_body(&format!("tok-wording-limit-{index}"), device_arn),
                )
            }),
        ),
    ];

    let mut index = 0;
    for (operation, member, resolves_inventory, request) in &creates {
        let unresolvable = if *resolves_inventory {
            ABSENT_DEVICE_ARNS
        } else {
            UNKNOWN_KIND_DEVICE_ARNS
        };
        for (case, device_arn) in unresolvable {
            index += 1;
            let (path, body) = request(index, device_arn);
            let reply = braket.call("POST", path, Some(body)).await;
            assert_validation(
                &reply,
                &format!("{operation} on {case} ({device_arn})"),
                DEVICE_ARN_NOT_FOUND,
            );
        }

        for (grammar, template) in MALFORMED_ARNS {
            index += 1;
            let value = template.replace("{resource}", "device");
            let (path, body) = request(index, &value);
            let reply = braket.call("POST", path, Some(body)).await;
            assert_validation(
                &reply,
                &format!("{operation} with {grammar} ({value})"),
                &format!("{member} is not valid."),
            );
        }
    }
}
