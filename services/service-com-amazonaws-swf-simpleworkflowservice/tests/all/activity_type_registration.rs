// SPDX-License-Identifier: CC0-1.0

//! `RegisterActivityType` and the read that proves it, `DescribeActivityType`, end to
//! end over this service's own wire protocol (dispatched through the same
//! `MultiServiceRouter` the emulator mounts in production, via the harness in
//! `workflow_execution_lifecycle`).
//!
//! The rule every case here asserts: registering an activity type stores ONE record
//! under `(domain, name, version)` carrying exactly the configuration the request
//! supplied — an absent or empty optional member stays absent — and
//! `DescribeActivityType` answers from that stored record alone, so a second
//! registration of the same pair conflicts, a different version does not, and the
//! lookup never leaves the domain it was asked about.
//!
//! Rows vary what the rule generalises over: which defaults are supplied, their values
//! and form (seconds, zero-padded seconds, the documented `NONE`, the published task
//! priority bounds), the shape of the name and version (ASCII, unicode, digits,
//! embedded spaces, the modelled maximum length), whether a description is given, and
//! whether a member arrives as the empty string. No row is privileged.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{merge, register_domain_body, string_at, Emulator};

/// Every member of the modelled `ActivityTypeConfiguration`. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.swf#ActivityTypeConfiguration")`.
const CONFIGURATION_MEMBERS: [&str; 6] = [
    "defaultTaskStartToCloseTimeout",
    "defaultTaskHeartbeatTimeout",
    "defaultTaskList",
    "defaultTaskPriority",
    "defaultTaskScheduleToStartTimeout",
    "defaultTaskScheduleToCloseTimeout",
];

/// The four `RegisterActivityType` members documented as "The duration is specified in
/// seconds, an integer greater than or equal to 0. You can use NONE to specify
/// unlimited duration." Source: those members' docs in this service's Smithy model.
const DURATION_MEMBERS: [&str; 4] = [
    "defaultTaskStartToCloseTimeout",
    "defaultTaskHeartbeatTimeout",
    "defaultTaskScheduleToStartTimeout",
    "defaultTaskScheduleToCloseTimeout",
];

/// One registration: the request's optional members, and what the stored record must
/// report back.
struct RegisterCase {
    case: &'static str,
    name: String,
    version: String,
    /// Optional members merged into the `RegisterActivityType` request.
    supplied: Value,
    /// Exactly what `/configuration` must report; a member absent here must be absent
    /// in the response.
    expected_config: Value,
    expected_description: Option<&'static str>,
}

fn register_cases() -> Vec<RegisterCase> {
    vec![
        RegisterCase {
            case: "no-defaults",
            name: "plain".to_string(),
            version: "1.0".to_string(),
            supplied: json!({}),
            expected_config: json!({}),
            expected_description: None,
        },
        RegisterCase {
            case: "task-list-only",
            name: "list-only".to_string(),
            version: "2".to_string(),
            supplied: json!({ "defaultTaskList": { "name": "only-tasks" } }),
            expected_config: json!({ "defaultTaskList": { "name": "only-tasks" } }),
            expected_description: None,
        },
        RegisterCase {
            case: "every-default-in-seconds",
            name: "all-defaults".to_string(),
            version: "3.14".to_string(),
            supplied: json!({
                "description": "all defaults",
                "defaultTaskList": { "name": "all-tasks" },
                "defaultTaskPriority": "10",
                "defaultTaskStartToCloseTimeout": "300",
                "defaultTaskHeartbeatTimeout": "60",
                "defaultTaskScheduleToStartTimeout": "90",
                "defaultTaskScheduleToCloseTimeout": "600",
            }),
            expected_config: json!({
                "defaultTaskList": { "name": "all-tasks" },
                "defaultTaskPriority": "10",
                "defaultTaskStartToCloseTimeout": "300",
                "defaultTaskHeartbeatTimeout": "60",
                "defaultTaskScheduleToStartTimeout": "90",
                "defaultTaskScheduleToCloseTimeout": "600",
            }),
            expected_description: Some("all defaults"),
        },
        RegisterCase {
            // "You can use NONE to specify unlimited duration", and "0" / a zero-padded
            // "0007" are both the documented integer form.
            case: "documented-duration-forms",
            name: "duration forms".to_string(),
            version: "0001".to_string(),
            supplied: json!({
                "defaultTaskStartToCloseTimeout": "NONE",
                "defaultTaskHeartbeatTimeout": "0",
                "defaultTaskScheduleToStartTimeout": "0007",
                "defaultTaskScheduleToCloseTimeout": "31536000",
            }),
            expected_config: json!({
                "defaultTaskStartToCloseTimeout": "NONE",
                "defaultTaskHeartbeatTimeout": "0",
                "defaultTaskScheduleToStartTimeout": "0007",
                "defaultTaskScheduleToCloseTimeout": "31536000",
            }),
            expected_description: None,
        },
        RegisterCase {
            // An empty optional member is an absent one: the model gives every
            // overridable duration a minimum length of 0, so "" is on the wire.
            case: "empty-optional-members",
            name: "empties".to_string(),
            version: "1".to_string(),
            supplied: json!({
                "defaultTaskPriority": "",
                "defaultTaskStartToCloseTimeout": "",
                "defaultTaskHeartbeatTimeout": "",
                "defaultTaskScheduleToStartTimeout": "",
                "defaultTaskScheduleToCloseTimeout": "",
            }),
            expected_config: json!({}),
            expected_description: None,
        },
        RegisterCase {
            // `defaultTaskPriority`'s doc: "Valid values are integers that range from
            // Java's Integer.MIN_VALUE (-2147483648) to Integer.MAX_VALUE
            // (2147483647)" — both published bounds, stored verbatim.
            case: "priority-lower-bound",
            name: "priority-low".to_string(),
            version: "1".to_string(),
            supplied: json!({ "defaultTaskPriority": "-2147483648" }),
            expected_config: json!({ "defaultTaskPriority": "-2147483648" }),
            expected_description: None,
        },
        RegisterCase {
            case: "priority-upper-bound",
            name: "priority-high".to_string(),
            version: "1".to_string(),
            supplied: json!({ "defaultTaskPriority": "2147483647" }),
            expected_config: json!({ "defaultTaskPriority": "2147483647" }),
            expected_description: None,
        },
        RegisterCase {
            case: "unicode-identity",
            name: "活動タイプ".to_string(),
            version: "ß-1".to_string(),
            supplied: json!({ "description": "unicode name" }),
            expected_config: json!({}),
            expected_description: Some("unicode name"),
        },
        RegisterCase {
            // "arn" is refused as a whole, so a name that merely CONTAINS it is fine.
            case: "arn-substring-identity",
            name: "arnold".to_string(),
            version: "arn-ish".to_string(),
            supplied: json!({}),
            expected_config: json!({}),
            expected_description: None,
        },
        RegisterCase {
            // The modelled maxima: `Name` is length 1..256, `Version` 1..64.
            case: "maximum-length-identity",
            name: "n".repeat(256),
            version: "v".repeat(64),
            supplied: json!({}),
            expected_config: json!({}),
            expected_description: None,
        },
    ]
}

/// The `RegisterActivityType` request body for one case.
fn register_body(domain: &str, case: &RegisterCase) -> Value {
    let mut body = json!({
        "domain": domain,
        "name": case.name,
        "version": case.version,
    });
    merge(&mut body, case.supplied.clone());
    body
}

fn describe_body(domain: &str, name: &str, version: &str) -> Value {
    json!({
        "domain": domain,
        "activityType": { "name": name, "version": version },
    })
}

#[tokio::test]
async fn register_activity_type_is_read_back_from_the_stored_record() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &register_domain_body("activities", "7"),
        )
        .await;

    for case in register_cases() {
        let body = register_body("activities", &case);
        let registered = emulator.ok(case.case, "RegisterActivityType", &body).await;
        assert_eq!(
            registered,
            json!({}),
            "case {:?}: RegisterActivityType's output shape is Unit, got {registered:?}",
            case.case
        );

        let described = emulator
            .ok(
                case.case,
                "DescribeActivityType",
                &describe_body("activities", &case.name, &case.version),
            )
            .await;

        assert_eq!(
            described.pointer("/typeInfo/activityType"),
            Some(&json!({ "name": case.name, "version": case.version })),
            "case {:?}: DescribeActivityType must report the stored identity, got {described:?}",
            case.case
        );
        let status = string_at(&described, "/typeInfo/status");
        assert_eq!(
            status, "REGISTERED",
            "case {:?}: a freshly registered type is REGISTERED, got {described:?}",
            case.case
        );
        assert!(
            REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
            "case {:?}: status must be a modelled RegistrationStatus, got {status:?}",
            case.case
        );
        assert!(
            described
                .pointer("/typeInfo/creationDate")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {:?}: the stored creationDate must be reported, got {described:?}",
            case.case
        );
        assert_eq!(
            described.pointer("/typeInfo/deprecationDate"),
            None,
            "case {:?}: a type that was never deprecated carries no deprecationDate, \
             got {described:?}",
            case.case
        );
        assert_eq!(
            described
                .pointer("/typeInfo/description")
                .and_then(Value::as_str),
            case.expected_description,
            "case {:?}: description must mirror what registration stored, got {described:?}",
            case.case
        );

        // Exactly the defaults that were registered come back, and no others.
        for member in CONFIGURATION_MEMBERS {
            let expected = case.expected_config.get(member);
            let actual = described.pointer(&format!("/configuration/{member}"));
            assert_eq!(
                actual,
                expected,
                "case {:?}: configuration.{member} must mirror what registration stored, \
                 got {actual:?}",
                case.case
            );
        }

        // A read does not change what a later caller sees.
        let again = emulator
            .ok(
                case.case,
                "DescribeActivityType",
                &describe_body("activities", &case.name, &case.version),
            )
            .await;
        assert_eq!(
            again, described,
            "case {:?}: two describes of one type must agree, got {again:?}",
            case.case
        );

        // A second registration of the same (name, version) is a conflict, and it
        // leaves the stored record alone.
        emulator
            .refused(
                case.case,
                "RegisterActivityType",
                &body,
                "TypeAlreadyExistsFault",
            )
            .await;
        let after_conflict = emulator
            .ok(
                case.case,
                "DescribeActivityType",
                &describe_body("activities", &case.name, &case.version),
            )
            .await;
        assert_eq!(
            after_conflict, described,
            "case {:?}: a refused re-registration must not touch the stored record, \
             got {after_conflict:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn one_name_at_several_versions_is_several_independent_types() {
    let emulator = Emulator::new();
    emulator
        .ok("seed", "RegisterDomain", &register_domain_body("versions", "7"))
        .await;

    // Same name, three versions, each with its own heartbeat default: the record is
    // keyed by the PAIR, so none of them can read another's configuration.
    let versions = [("1.0", "30"), ("2.0", "NONE"), ("10", "600")];
    for (version, heartbeat) in versions {
        emulator
            .ok(
                version,
                "RegisterActivityType",
                &json!({
                    "domain": "versions",
                    "name": "shared-name",
                    "version": version,
                    "defaultTaskHeartbeatTimeout": heartbeat,
                }),
            )
            .await;
    }
    for (version, heartbeat) in versions {
        let described = emulator
            .ok(
                version,
                "DescribeActivityType",
                &describe_body("versions", "shared-name", version),
            )
            .await;
        assert_eq!(
            string_at(&described, "/configuration/defaultTaskHeartbeatTimeout"),
            heartbeat,
            "case {version:?}: each version reports its own configuration, got {described:?}"
        );
    }
}

#[tokio::test]
async fn an_activity_type_and_a_workflow_type_may_share_one_name_and_version() {
    let emulator = Emulator::new();
    emulator
        .ok("seed", "RegisterDomain", &register_domain_body("both", "7"))
        .await;
    for operation in ["RegisterWorkflowType", "RegisterActivityType"] {
        emulator
            .ok(
                operation,
                operation,
                &json!({ "domain": "both", "name": "twin", "version": "1.0" }),
            )
            .await;
    }

    let activity = emulator
        .ok(
            "activity",
            "DescribeActivityType",
            &describe_body("both", "twin", "1.0"),
        )
        .await;
    assert_eq!(
        activity.pointer("/typeInfo/activityType"),
        Some(&json!({ "name": "twin", "version": "1.0" })),
        "case \"activity\": the activity record is its own, got {activity:?}"
    );
    let workflow = emulator
        .ok(
            "workflow",
            "DescribeWorkflowType",
            &json!({
                "domain": "both",
                "workflowType": { "name": "twin", "version": "1.0" },
            }),
        )
        .await;
    assert_eq!(
        workflow.pointer("/typeInfo/workflowType"),
        Some(&json!({ "name": "twin", "version": "1.0" })),
        "case \"workflow\": the workflow record is its own, got {workflow:?}"
    );
}

#[tokio::test]
async fn an_unknown_domain_or_activity_type_is_an_unknown_resource() {
    let emulator = Emulator::new();
    for domain in ["known", "other"] {
        emulator
            .ok(domain, "RegisterDomain", &register_domain_body(domain, "7"))
            .await;
    }
    emulator
        .ok(
            "seed",
            "RegisterActivityType",
            &json!({ "domain": "known", "name": "act", "version": "1.0" }),
        )
        .await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "register in a domain that was never created",
            "RegisterActivityType",
            json!({ "domain": "absent", "name": "act", "version": "1.0" }),
        ),
        (
            "describe in a domain that was never created",
            "DescribeActivityType",
            describe_body("absent", "act", "1.0"),
        ),
        (
            "describe a name no registration stored",
            "DescribeActivityType",
            describe_body("known", "never-registered", "1.0"),
        ),
        (
            "describe a version no registration stored",
            "DescribeActivityType",
            describe_body("known", "act", "9.9"),
        ),
        (
            // The lookup is domain-scoped: the type exists, just not here.
            "describe across domains",
            "DescribeActivityType",
            describe_body("other", "act", "1.0"),
        ),
    ];
    for (case, operation, body) in cases {
        emulator
            .refused(case, operation, &body, "UnknownResourceFault")
            .await;
    }

    // The refusals left the one stored record readable where it belongs.
    let described = emulator
        .ok(
            "still readable",
            "DescribeActivityType",
            &describe_body("known", "act", "1.0"),
        )
        .await;
    assert_eq!(
        string_at(&described, "/typeInfo/status"),
        "REGISTERED",
        "case \"still readable\": the stored record survives every refusal, got {described:?}"
    );
}

#[tokio::test]
async fn a_default_duration_that_is_not_the_documented_form_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &register_domain_body("bad-durations", "7"),
        )
        .await;

    // Not the documented form: a signed or non-decimal value, the wrong case of the
    // documented literal, or a value with surrounding space or a unit suffix.
    let rejected = ["abc", "+5", "-1", "none", " 5", "5s", "1.5"];
    for (index, member) in DURATION_MEMBERS.iter().enumerate() {
        for (offset, value) in rejected.iter().enumerate() {
            let case = format!("{member}={value:?}");
            let name = format!("bad-{index}-{offset}");
            let mut body = json!({
                "domain": "bad-durations",
                "name": name,
                "version": "1.0",
            });
            merge(&mut body, json!({ (*member): value }));
            emulator
                .refused(&case, "RegisterActivityType", &body, "ValidationException")
                .await;
            // The refusal persisted nothing.
            emulator
                .refused(
                    &case,
                    "DescribeActivityType",
                    &describe_body("bad-durations", &name, "1.0"),
                    "UnknownResourceFault",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn activity_type_names_outside_the_swf_naming_rule_are_refused() {
    // Source: the `name` / `version` member docs on RegisterActivityType — "The
    // specified string must not contain a : (colon), / (slash), | (vertical bar), or
    // any control characters (\u0000-\u001f | \u007f-\u009f). Also, it must not be the
    // literal string arn." (and the shared SWF rule on leading/trailing whitespace).
    let rejected = [
        "with:colon",
        "with/slash",
        "with|bar",
        "with\u{1}control",
        "with\u{7f}control",
        " leading",
        "trailing ",
        "arn",
    ];
    let emulator = Emulator::new();
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &register_domain_body("naming", "7"),
        )
        .await;

    for value in rejected {
        for member in ["name", "version"] {
            let case = format!("{member}={value:?}");
            let mut body = json!({ "domain": "naming", "name": "ok-name", "version": "1.0" });
            merge(&mut body, json!({ member: value }));
            emulator
                .refused(&case, "RegisterActivityType", &body, "ValidationException")
                .await;
        }
        let case = format!("defaultTaskList.name={value:?}");
        emulator
            .refused(
                &case,
                "RegisterActivityType",
                &json!({
                    "domain": "naming",
                    "name": "ok-name",
                    "version": "1.0",
                    "defaultTaskList": { "name": value },
                }),
                "ValidationException",
            )
            .await;
    }

    // None of those refusals stored the type they named.
    emulator
        .refused(
            "nothing stored",
            "DescribeActivityType",
            &describe_body("naming", "ok-name", "1.0"),
            "UnknownResourceFault",
        )
        .await;

    // `TaskList`'s `name` is modelled length 1..256, so an empty one is refused at the
    // boundary with the same code the handler uses for the naming rule.
    emulator
        .refused(
            "defaultTaskList.name=\"\"",
            "RegisterActivityType",
            &json!({
                "domain": "naming",
                "name": "ok-name",
                "version": "1.0",
                "defaultTaskList": { "name": "" },
            }),
            "ValidationException",
        )
        .await;
}
