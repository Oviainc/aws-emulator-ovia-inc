// SPDX-License-Identifier: CC0-1.0

//! What a refusal may SAY, over the wire: the `message` of every error this service answers
//! with has to fit the member it travels in.
//!
//! Every error shape of this service carries its text in a `message` targeting `Message`,
//! which is `len 1..2048` (source:
//! `smithy_query(query="shape",shape="Message,ValidationException,ResourceNotFoundException")`),
//! while the ARN-typed members a refusal is built from are themselves `len 20..2048`
//! (`AmazonResourceName`, source: `smithy_query(query="operation",operation="GetEndpoint")`).
//! A refusal that stated such a value WHOLE would therefore answer a body the model says
//! cannot exist, and nothing downstream catches it: the framework validates a request's input
//! against the model, not an error's members.
//!
//! The rule asserted here is one sentence: a refusal states a message inside the bound its own
//! modelled member declares, with the whole of its fixed text, and states the request-supplied
//! value in full only while a message can hold it.
//!
//! The tables span what the rule generalises over — the LENGTH of the value (the member's
//! minimum, the length of an ARN this service itself mints, the member's maximum) and the FORM
//! of its characters (plain ASCII, a well-formed ARN of another service, multi-byte
//! characters, characters with no printable form, quote characters) — crossed with every
//! operation and member a refusal is built from. Each case drives a real HTTP request through
//! the same `MultiServiceRouter` the emulator serves on a socket.

use serde_json::json;

use super::endpoint::percent_encode;
use super::suite_lifecycle::{thing_arn, Answer, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// The bounds `Message` declares, which every error member carrying refusal text targets.
///
/// Written out from the MODEL rather than imported from `crate::state`: what these cases
/// assert is the model's bound, so a change to the crate's own constant has to fail here
/// instead of moving the expectation with it.
const MESSAGE_MIN: usize = 1;
const MESSAGE_MAX: usize = 2048;

/// The bounds `AmazonResourceName` declares: both are rows below.
const ARN_MIN: usize = 20;
const ARN_MAX: usize = 2048;

/// One request-supplied value, and how a refusal built from it must state it.
struct Row {
    case: String,
    value: String,
    /// True when a message can hold the whole value, so the refusal has to state all of it.
    stated_whole: bool,
    /// True when the value IS a well-formed ARN of this service, so the tagging surface
    /// answers it as a resource it does not hold rather than as a bad request.
    of_this_service: bool,
}

/// A `suitedefinition/<id>` ARN of this service holding `id`.
fn definition_arn(id: &str) -> String {
    format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/{id}")
}

/// The values every ARN-form refusal below is driven with: none of them is an ARN of the
/// service the member carrying it names, so each one reaches the refusal, and together they
/// span the member's length bounds and the character forms a value can be built from.
fn value_rows() -> Vec<Row> {
    let run_arn = format!(
        "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/\
         11111111-2222-3333-4444-555555555555/66666666-7777-8888-9999-aaaaaaaaaaaa"
    );
    // A well-formed ARN of a service that is not the member's, padded to the member's maximum.
    let other_service = format!("arn:aws:iotwireless:{REGION}:000000000000:thing/");
    let foreign_arn = format!(
        "{other_service}{}",
        "t".repeat(ARN_MAX - other_service.chars().count())
    );

    vec![
        Row {
            case: "a value at the member's minimum length".to_string(),
            value: "x".repeat(ARN_MIN),
            stated_whole: true,
            of_this_service: false,
        },
        Row {
            // The longest ARN this service MINTS: a suite run's, whose resource portion
            // carries a definition id and a run id. A value of a form the service itself
            // produces has to be stated in full.
            case: "a suite run ARN of this service".to_string(),
            value: run_arn,
            stated_whole: true,
            of_this_service: true,
        },
        Row {
            case: "a value at the member's maximum length".to_string(),
            value: "x".repeat(ARN_MAX),
            stated_whole: false,
            of_this_service: false,
        },
        Row {
            case: "a maximum-length ARN of another service".to_string(),
            value: foreign_arn,
            stated_whole: false,
            of_this_service: false,
        },
        Row {
            case: "a maximum-length value of multi-byte characters".to_string(),
            value: "é".repeat(ARN_MAX),
            stated_whole: false,
            of_this_service: false,
        },
        Row {
            // Quoting a value escapes what has no printable form, and an escape is longer
            // than the character it stands for, so these rows are the ones that stress the
            // bound hardest.
            case: "a maximum-length value of characters with no printable form".to_string(),
            value: "\n".repeat(ARN_MAX),
            stated_whole: false,
            of_this_service: false,
        },
        Row {
            case: "a maximum-length value of quote characters".to_string(),
            value: "\"".repeat(ARN_MAX),
            stated_whole: false,
            of_this_service: false,
        },
    ]
}

/// The `message` an answer carries, or a panic naming the case when it carries none: a
/// refusal with no message states nothing, which the member's minimum of one character
/// forbids.
fn message(case: &str, answer: &Answer) -> String {
    answer
        .member("message")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message, got {}", answer.body))
        .to_string()
}

/// Assert one refusal against the rule: the status and code the model declares for it, a
/// message inside `Message`'s bounds, the whole of the template's fixed text (`fragments`),
/// and the value stated in full exactly while a message can hold it.
fn assert_bounded_refusal(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    fragments: &[&str],
    row: &Row,
) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (status, Some(code)),
        "case {case:?}: expected HTTP {status} {code}, got {} {:?} {}",
        answer.status,
        answer.error_code,
        answer.body
    );

    let message = message(case, answer);
    let length = message.chars().count();
    assert!(
        (MESSAGE_MIN..=MESSAGE_MAX).contains(&length),
        "case {case:?}: a message must stay inside Message's len {MESSAGE_MIN}..{MESSAGE_MAX}, \
         got {length} characters: {message:?}"
    );
    for fragment in fragments {
        assert!(
            message.contains(fragment),
            "case {case:?}: the refusal must state {fragment:?}, got {message:?}"
        );
    }

    if row.stated_whole {
        assert!(
            message.contains(&row.value),
            "case {case:?}: a value a message can hold must be stated in full, got {message:?}"
        );
    } else {
        assert!(
            !message.contains(&row.value),
            "case {case:?}: a value longer than a message can hold must not be stated whole, \
             got {length} characters"
        );
        assert!(
            message.contains("..."),
            "case {case:?}: a value stated only in part must be marked as cut, got {message:?}"
        );
    }
}

/// Every refusal built from an ARN-typed member a request supplied — the device members
/// `GetEndpoint` reads from its query, the role and device ARNs both operations taking a
/// definition configuration read from the body, the primary device a run start reads from the
/// body, and the `resourceArn` the tagging surface reads from the path — states a message
/// inside the modelled bound, keeps the whole of its own template, and states the value only
/// as far as a message can hold it.
#[tokio::test]
async fn an_arn_refusal_states_a_message_inside_the_modelled_bound() {
    let emulator = Emulator::new();
    let definition_id = {
        let created = emulator
            .create_definition(
                &json!({
                    "suiteDefinitionName": "message-bounds",
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                    "devices": [{ "thingArn": thing_arn(0) }],
                }),
                REGION,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup CreateSuiteDefinition failed with {} {}",
            created.status, created.body
        );
        created.string("suiteDefinitionId")
    };

    for row in value_rows() {
        // GetEndpoint: the device ARNs travel in the query.
        for member in ["thingArn", "certificateArn", "deviceRoleArn"] {
            let answer = emulator
                .call(
                    "GET",
                    &format!("/endpoint?{member}={}", percent_encode(&row.value)),
                    REGION,
                    None,
                )
                .await;
            assert_bounded_refusal(
                &format!("GetEndpoint {member} is {}", row.case),
                &answer,
                400,
                "ValidationException",
                &[&format!(
                    "{member} must be an Amazon Resource Name of the form"
                )],
                &row,
            );
        }

        // CreateSuiteDefinition and UpdateSuiteDefinition: the role the suite points at, and
        // a device under test, both inside the configuration in the body.
        for (member, configuration) in [
            (
                "suiteDefinitionConfiguration.devicePermissionRoleArn",
                json!({
                    "suiteDefinitionName": "bad-role",
                    "rootGroup": "g",
                    "devicePermissionRoleArn": row.value,
                }),
            ),
            (
                "suiteDefinitionConfiguration.devices[0].thingArn",
                json!({
                    "suiteDefinitionName": "bad-device",
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                    "devices": [{ "thingArn": row.value }],
                }),
            ),
        ] {
            let created = emulator.create_definition(&configuration, REGION).await;
            assert_bounded_refusal(
                &format!("CreateSuiteDefinition {member} is {}", row.case),
                &created,
                400,
                "ValidationException",
                &[&format!(
                    "{member} must be an Amazon Resource Name of the form"
                )],
                &row,
            );

            let updated = emulator
                .call(
                    "PATCH",
                    &format!("/suiteDefinitions/{definition_id}"),
                    REGION,
                    Some(json!({ "suiteDefinitionConfiguration": configuration })),
                )
                .await;
            assert_bounded_refusal(
                &format!("UpdateSuiteDefinition {member} is {}", row.case),
                &updated,
                400,
                "ValidationException",
                &[&format!(
                    "{member} must be an Amazon Resource Name of the form"
                )],
                &row,
            );
        }

        // StartSuiteRun: the primary device, inside the run configuration in the body.
        let started = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                REGION,
                Some(json!({
                    "suiteRunConfiguration": { "primaryDevice": { "thingArn": row.value } }
                })),
            )
            .await;
        assert_bounded_refusal(
            &format!("StartSuiteRun primaryDevice.thingArn is {}", row.case),
            &started,
            400,
            "ValidationException",
            &["suiteRunConfiguration.primaryDevice.thingArn must be an Amazon Resource Name"],
            &row,
        );

        // The tagging surface reads its `resourceArn` from the path: a value that is not one
        // of this service's ARNs is a bad request, and one that IS but names nothing the
        // store holds is a missing resource — two templates, one rule about what they may
        // state.
        let (status, code, fragment) = if row.of_this_service {
            (
                404,
                "ResourceNotFoundException",
                "Unable to find resource with resourceArn: ",
            )
        } else {
            (
                400,
                "ValidationException",
                "is not an IoT Device Advisor suite definition or suite run ARN",
            )
        };
        for (operation, method, query, body) in [
            ("ListTagsForResource", "GET", "", None),
            ("UntagResource", "DELETE", "?tagKeys=env", None),
            (
                "TagResource",
                "POST",
                "",
                Some(json!({ "tags": { "env": "test" } })),
            ),
        ] {
            let answer = emulator
                .call(
                    method,
                    &format!("/tags/{}{query}", percent_encode(&row.value)),
                    REGION,
                    body,
                )
                .await;
            assert_bounded_refusal(
                &format!("{operation} resourceArn is {}", row.case),
                &answer,
                status,
                code,
                &[fragment],
                &row,
            );
        }
    }
}

/// The well-formed ARNs of THIS service the store holds no resource for: a definition id at
/// the shortest legal length, a suite run ARN, and a definition id filling the member to its
/// maximum length.
fn unheld_arns() -> Vec<Row> {
    // `UUID` is `len 12..36` (source: `smithy_query(query="shape",shape="UUID")`).
    let shortest = definition_arn("abcdefghijkl");
    let longest_prefix = definition_arn("");
    vec![
        Row {
            case: "a definition id at the shortest legal length".to_string(),
            value: shortest,
            stated_whole: true,
            of_this_service: true,
        },
        Row {
            case: "a suite run of a definition the store does not hold".to_string(),
            value: format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/\
                 11111111-2222-3333-4444-555555555555/66666666-7777-8888-9999-aaaaaaaaaaaa"
            ),
            stated_whole: true,
            of_this_service: true,
        },
        Row {
            case: "a definition id filling the member to its maximum length".to_string(),
            value: format!(
                "{longest_prefix}{}",
                "d".repeat(ARN_MAX - longest_prefix.chars().count())
            ),
            stated_whole: false,
            of_this_service: true,
        },
    ]
}

/// A lookup of a resource the store does not hold answers the service's single not-found
/// template, and that message too stays inside the modelled bound: the template's words
/// survive whole, and the `resourceArn` the caller sent is stated only as far as a message
/// can hold it.
#[tokio::test]
async fn a_not_found_refusal_states_a_message_inside_the_modelled_bound() {
    let emulator = Emulator::new();

    for row in unheld_arns() {
        for (operation, method, query, body) in [
            ("ListTagsForResource", "GET", "", None),
            ("UntagResource", "DELETE", "?tagKeys=env", None),
            (
                "TagResource",
                "POST",
                "",
                Some(json!({ "tags": { "env": "test" } })),
            ),
        ] {
            let answer = emulator
                .call(
                    method,
                    &format!("/tags/{}{query}", percent_encode(&row.value)),
                    REGION,
                    body,
                )
                .await;
            assert_bounded_refusal(
                &format!("{operation} with {}", row.case),
                &answer,
                404,
                "ResourceNotFoundException",
                &["Unable to find resource with resourceArn: "],
                &row,
            );
        }
    }
}

/// A value PAST the member's maximum length never reaches a handler — the framework refuses
/// it against the model — and that refusal carries a message inside the same bound, so the
/// rule holds on both sides of the published bound.
#[tokio::test]
async fn a_value_past_the_members_maximum_is_refused_inside_the_bound() {
    let emulator = Emulator::new();

    let rows = [
        ("one character past the maximum", ARN_MAX + 1),
        ("far past the maximum", ARN_MAX * 8),
    ];
    for (case, length) in rows {
        let answer = emulator
            .call(
                "GET",
                &format!("/endpoint?thingArn={}", "x".repeat(length)),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a value outside the modelled length must be refused as a bad \
             request, got {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let message = message(case, &answer);
        let stated = message.chars().count();
        assert!(
            (MESSAGE_MIN..=MESSAGE_MAX).contains(&stated),
            "case {case:?}: a message must stay inside Message's len {MESSAGE_MIN}..{MESSAGE_MAX}, \
             got {stated} characters"
        );
    }
}
