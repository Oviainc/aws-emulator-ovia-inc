// SPDX-License-Identifier: CC0-1.0

//! The TEXT the configuration-recorder refusals carry, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — and reads the
//! `message` out of the modelled error body.
//!
//! The rule pinned here: where the service FIXES the text of a refusal — the model
//! states the sentence, or the condition is one the service words
//! request-independently — the emulator emits that sentence, the same one however the
//! request named the recorder or the service, so a client that matches it separates the
//! legs of an error code that is shared between several conditions. Each sentence is
//! spelled out in this suite rather than read from the handler's own constant, so these
//! cases PIN the text instead of restating whatever the code happens to say.
//!
//! The rows of every table differ along what the rule generalises over: which service
//! the recorder is linked to, which operation refused the call, and which name carried
//! the reserved prefix — never along the one request a recording happened to make.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    SERVICE_LINKED_PRINCIPALS, SERVICE_LINKED_RECORDER_NAME_PREFIX,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation that creates a service-linked configuration recorder.
const PUT_SERVICE_LINKED: &str = "PutServiceLinkedConfigurationRecorder";

/// The sentence Config answers a second service-linked create for a service that
/// already holds one with (`ConflictException`: "you cannot create a service-linked
/// recorder because a service-linked recorder already exists for the specified
/// service"). It names the CONDITION and quotes nothing the caller sent.
const CONFLICT_MESSAGE: &str =
    "A service-linked configuration recorder with the specified service principal \
     already exists.";

/// The sentence `StartConfigurationRecorder` and `StopConfigurationRecorder` answer a
/// SERVICE-LINKED recorder with, stated verbatim by the model
/// (`UnmodifiableEntityException`: "these operations do not affect service-linked
/// configuration recorders. Service-linked configuration recorders are always
/// recording. To stop recording, you must delete the service-linked configuration
/// recorder. Use the DeleteServiceLinkedConfigurationRecorder operation to delete a
/// service-linked configuration recorder").
const ALWAYS_RECORDING_MESSAGE: &str =
    "This operation does not affect service-linked configuration recorders. \
     Service-linked configuration recorders are always recording. To stop recording, \
     you must delete the service-linked configuration recorder. Use the \
     DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked \
     configuration recorder.";

/// The sentence `DeleteConfigurationRecorder` answers a service-linked recorder with,
/// also stated by `UnmodifiableEntityException`'s own documentation.
const NOT_DELETABLE_HERE_MESSAGE: &str =
    "You cannot use this operation to delete a service-linked configuration recorder. \
     Use the DeleteServiceLinkedConfigurationRecorder operation to delete a \
     service-linked configuration recorder.";

/// The sentence `PutConfigurationRecorder` answers a request asking it for a
/// service-linked recorder with, from the same documentation.
const NOT_CREATABLE_HERE_MESSAGE: &str =
    "You cannot use this operation to create a service-linked configuration recorder. \
     Use the PutServiceLinkedConfigurationRecorder operation to create a service-linked \
     configuration recorder.";

/// The sentence `PutConfigurationRecorder` answers a name carrying the reserved prefix
/// with, from `InvalidConfigurationRecorderNameException`'s own documentation ("The
/// configuration recorder name is not valid. The prefix \"AWSConfigurationRecorderFor\"
/// is reserved for service-linked configuration recorders"), as the service renders it.
const RESERVED_PREFIX_MESSAGE: &str =
    "The configuration recorder name is not valid, the prefix \
     AWSConfigurationRecorderFor is reserved for service-linked configuration recorders.";

/// The `message` an error body carries, under either spelling the wire may use.
fn error_message(body: &Value) -> String {
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    format!("<no message in {body}>")
}

/// Call `operation`, require HTTP 400 with the modelled `expected_code`, and answer the
/// message the refusal carried.
async fn refusal(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    expected_code: &str,
) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?}: expected 400 {expected_code} from {operation}, got {status} {code:?} \
         with body {parsed}"
    );
    error_message(&parsed)
}

/// Create the service-linked configuration recorder of `service_principal` and answer
/// the name Config assigned it.
async fn put_service_linked(emulator: &Emulator, service_principal: &str) -> String {
    let created = emulator
        .ok(
            PUT_SERVICE_LINKED,
            &json!({ "ServicePrincipal": service_principal }),
        )
        .await;
    created["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("{PUT_SERVICE_LINKED} must answer a Name, got {created}"))
        .to_string()
}

#[tokio::test]
async fn the_duplicate_service_linked_conflict_states_the_condition_and_quotes_no_principal() {
    // Rows: every service Config links a configuration recorder to
    // (`state::SERVICE_LINKED_PRINCIPALS`), so the text is shown to be the same
    // whichever service the request named.
    for service_principal in SERVICE_LINKED_PRINCIPALS {
        let case = format!("a second recorder for {service_principal}");
        let case = case.as_str();
        let emulator = Emulator::new();
        put_service_linked(&emulator, service_principal).await;

        let message = refusal(
            &emulator,
            case,
            PUT_SERVICE_LINKED,
            &json!({ "ServicePrincipal": service_principal }),
            "ConflictException",
        )
        .await;
        assert_eq!(
            message, CONFLICT_MESSAGE,
            "case {case:?}: expected {CONFLICT_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(service_principal),
            "case {case:?}: the conflict text must not quote the service principal, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn starting_or_stopping_a_service_linked_recorder_reports_that_it_always_records() {
    // Rows differ along the operation the caller used and the service the recorder is
    // linked to: the property the refusal is about — the recorder always records — is
    // the same for all of them.
    for service_principal in SERVICE_LINKED_PRINCIPALS {
        let emulator = Emulator::new();
        let name = put_service_linked(&emulator, service_principal).await;

        for operation in ["StartConfigurationRecorder", "StopConfigurationRecorder"] {
            let case = format!("{operation} on {name}");
            let case = case.as_str();
            let message = refusal(
                &emulator,
                case,
                operation,
                &json!({ "ConfigurationRecorderName": name }),
                "UnmodifiableEntityException",
            )
            .await;
            assert_eq!(
                message, ALWAYS_RECORDING_MESSAGE,
                "case {case:?}: expected {ALWAYS_RECORDING_MESSAGE:?}, got {message:?}"
            );
            assert!(
                message.contains("always recording"),
                "case {case:?}: the refusal must report that the recorder always records, \
                 got {message:?}"
            );
        }

        // The standard delete is refused with the sentence that names the operation
        // that does delete this recorder.
        let case = format!("DeleteConfigurationRecorder on {name}");
        let case = case.as_str();
        let message = refusal(
            &emulator,
            case,
            "DeleteConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": name }),
            "UnmodifiableEntityException",
        )
        .await;
        assert_eq!(
            message, NOT_DELETABLE_HERE_MESSAGE,
            "case {case:?}: expected {NOT_DELETABLE_HERE_MESSAGE:?}, got {message:?}"
        );

        // Every refusal left the recorder recording for its service.
        let status = emulator
            .ok(
                "DescribeConfigurationRecorderStatus",
                &json!({ "ServicePrincipal": service_principal }),
            )
            .await;
        assert_eq!(
            status["ConfigurationRecordersStatus"][0]["recording"],
            json!(true),
            "the service-linked recorder is still recording, got {status}"
        );
    }
}

#[tokio::test]
async fn a_name_carrying_the_reserved_prefix_is_refused_with_the_documented_sentence() {
    // Rows differ along the name the request carried: the prefix on its own, the prefix
    // with a suffix of the caller's own, the name Config itself assigns a supported
    // service, and the prefix carrying digits. The prefix is reserved however the rest
    // of the name reads, and the sentence does not quote it.
    let assigned = {
        let emulator = Emulator::new();
        put_service_linked(&emulator, SERVICE_LINKED_PRINCIPALS[0]).await
    };
    let names: Vec<(&str, String)> = vec![
        (
            "the reserved prefix on its own",
            SERVICE_LINKED_RECORDER_NAME_PREFIX.to_string(),
        ),
        (
            "the prefix with a suffix of the caller's own",
            format!("{SERVICE_LINKED_RECORDER_NAME_PREFIX}MyService"),
        ),
        (
            "the name Config assigns a linked service",
            assigned.clone(),
        ),
        (
            "the prefix carrying digits",
            format!("{SERVICE_LINKED_RECORDER_NAME_PREFIX}123"),
        ),
    ];

    for (case, name) in names {
        let emulator = Emulator::new();
        let message = refusal(
            &emulator,
            case,
            "PutConfigurationRecorder",
            &recorder_request(json!({ "name": name, "roleARN": ROLE_ARN })),
            "InvalidConfigurationRecorderNameException",
        )
        .await;
        assert_eq!(
            message, RESERVED_PREFIX_MESSAGE,
            "case {case:?}: expected {RESERVED_PREFIX_MESSAGE:?}, got {message:?}"
        );
        assert!(
            message.contains(SERVICE_LINKED_RECORDER_NAME_PREFIX),
            "case {case:?}: the sentence states the reserved prefix, got {message:?}"
        );

        // The refused create persisted nothing.
        let described = emulator
            .ok("DescribeConfigurationRecorders", &json!({}))
            .await;
        assert_eq!(
            described["ConfigurationRecorders"],
            json!([]),
            "case {case:?}: the refused create must persist no recorder, got {described}"
        );
    }

    // A name that does NOT carry the prefix is accepted, so the refusal is about the
    // prefix and nothing else.
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "name": "aws-config-recorder", "roleARN": ROLE_ARN })),
        )
        .await;
}

#[tokio::test]
async fn asking_the_standard_create_for_a_service_linked_recorder_names_the_right_operation() {
    // Rows differ along WHICH member of the request asks for a service-linked recorder:
    // the service principal it would be linked to, the connector it would record
    // through, and both at once.
    let connector_arn = "arn:aws:config:us-east-1:000000000000:connector/azure-tenant-1";
    let cases: Vec<(&str, Value)> = vec![
        (
            "a request naming a service principal",
            json!({ "roleARN": ROLE_ARN, "servicePrincipal": SERVICE_LINKED_PRINCIPALS[0] }),
        ),
        (
            "a request naming a connector",
            json!({ "roleARN": ROLE_ARN, "connectorArn": connector_arn }),
        ),
        (
            "a request naming both",
            json!({
                "roleARN": ROLE_ARN,
                "servicePrincipal": SERVICE_LINKED_PRINCIPALS[1],
                "connectorArn": connector_arn,
            }),
        ),
    ];

    for (case, recorder) in cases {
        let emulator = Emulator::new();
        let message = refusal(
            &emulator,
            case,
            "PutConfigurationRecorder",
            &recorder_request(recorder),
            "UnmodifiableEntityException",
        )
        .await;
        assert_eq!(
            message, NOT_CREATABLE_HERE_MESSAGE,
            "case {case:?}: expected {NOT_CREATABLE_HERE_MESSAGE:?}, got {message:?}"
        );
    }
}
