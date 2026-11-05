// SPDX-License-Identifier: CC0-1.0

//! How this service IDENTIFIES things in an error body.
//!
//! Three rules, each asserted over a set of inputs rather than on one recorded request:
//!
//! 1. An error about a service level objective that cannot be RESOLVED identifies it by its
//!    ARN, and reports the resource's capitalised type name — never the bare name the
//!    request happened to send and never the lower-case segment an ARN spells.
//! 2. A create refused because the identifier it sent is taken states that generically: the
//!    identifier at fault is the caller's own `Name`, so the conflict names no resource, no
//!    name and no ARN.
//! 3. A constraint-violation message states the member path and the requirement it
//!    violated, in AWS's `N validation error(s) detected: ` envelope, with no echo of the
//!    rejected value and with a `@pattern` requirement rendered without the `^`/`$` anchors
//!    the model spells it with.
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` already drives
//! this service's `restJson1` surface with, so both suites speak to one harness.
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` already drives
//! this service's `restJson1` surface with, so both suites speak to one harness.

use serde_json::{json, Value};

use crate::slo_lifecycle::{
    arn_path_segment, call, create, name_path_segment, router, sli_config, ACCOUNT, REGION,
};
// The work-item wording the update path answers, asserted over more inputs in the suite that
// owns that rule.
use crate::refusal_wording::assert_work_item_wording;
// The sentence a create whose identifier is already taken is refused with, owned by the suite
// that holds this service's refusal prose and asserted here over this suite's rows too.
use crate::refusal_prose::IDENTIFIER_TAKEN;

/// The `ResourceType` every SLO failure reports: the resource's type NAME, capitalised.
const SLO_RESOURCE_TYPE: &str = "Slo";

/// The ARN of the SLO called `name` in this account and region, as the modelled
/// `ServiceLevelObjectiveArn` pattern spells it
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`).
fn slo_arn(name: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}")
}

/// Rule 1 on the not-found path: `GetServiceLevelObjective`,
/// `UpdateServiceLevelObjective` and `DeleteServiceLevelObjective` all report
/// `ResourceType` = the capitalised type name and `ResourceId` = the SLO's ARN, whichever
/// of the two id forms the modelled `ServiceLevelObjectiveId` admits the caller used.
///
/// The MESSAGE is the reading layer's, not the shape's: the two reads state the resource and
/// its ARN, while the update — which resolves the SLO through the work item that holds it —
/// states that work item. `slo_work_item_wording` is where that form is asserted.
///
/// The rows differ along the name's form (the character classes the modelled
/// `ServiceLevelObjectiveName` pattern admits: plain, dotted, underscored, spaced, and the
/// shortest legal name), the id form the request named the SLO by (bare name vs. full ARN),
/// and whether the SLO was never created or was created and then deleted.
#[tokio::test]
async fn a_missing_slo_is_identified_by_its_arn_on_every_read_write_and_delete() {
    struct Case {
        name: &'static str,
        /// Whether the SLO is created and deleted before the missing-id calls, instead of
        /// never having existed.
        created_then_deleted: bool,
        /// Whether the request names the SLO by its ARN instead of its bare name.
        by_arn: bool,
    }

    // Names drawn from the modelled `ServiceLevelObjectiveName` pattern
    // `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$` (`smithy_query(shape=
    // "ServiceLevelObjective")`, member `Name`): alphanumeric ends, and `-`, `.`, `_`, ` `
    // in the middle. `9z` is the shortest name the pattern admits (two characters).
    let cases = [
        Case { name: "absent-slo", created_then_deleted: false, by_arn: false },
        Case { name: "absent-slo", created_then_deleted: false, by_arn: true },
        Case { name: "absent.slo.with.dots", created_then_deleted: false, by_arn: true },
        Case { name: "absent_slo_underscored", created_then_deleted: false, by_arn: false },
        Case { name: "absent slo with spaces", created_then_deleted: false, by_arn: false },
        Case { name: "9z", created_then_deleted: false, by_arn: true },
        Case { name: "deleted-slo", created_then_deleted: true, by_arn: false },
        Case { name: "deleted-slo-by-arn", created_then_deleted: true, by_arn: true },
    ];

    for case in &cases {
        let router = router();
        if case.created_then_deleted {
            let (status, created) = create(
                &router,
                json!({ "Name": case.name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the setup create to succeed, got {status} with {created}",
                case.name
            );
            let (status, deleted) = call(
                &router,
                "DELETE",
                &format!("/slo/{}", name_path_segment(case.name)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the setup delete to succeed, got {status} with {deleted}",
                case.name
            );
        }

        let path = if case.by_arn {
            arn_path_segment(case.name)
        } else {
            name_path_segment(case.name)
        };
        let arn = slo_arn(case.name);

        for (method, body) in [
            ("GET", None),
            ("PATCH", Some(json!({ "Description": "a new description" }))),
            ("DELETE", None),
        ] {
            let (status, error) = call(&router, method, &format!("/slo/{path}"), body).await;
            let label = format!("{method} {:?} by_arn={}", case.name, case.by_arn);
            assert_eq!(
                status, 404,
                "case {label}: expected HTTP 404 for an SLO that is not stored, got {status} with {error}"
            );
            assert_eq!(
                error["ResourceType"].as_str(),
                Some(SLO_RESOURCE_TYPE),
                "case {label}: expected ResourceType {SLO_RESOURCE_TYPE:?}, got {:?}",
                error["ResourceType"]
            );
            assert_eq!(
                error["ResourceId"].as_str(),
                Some(arn.as_str()),
                "case {label}: expected ResourceId {arn:?}, got {:?}",
                error["ResourceId"]
            );
            let message = error["Message"].as_str().unwrap_or_default();
            if method == "PATCH" {
                assert_work_item_wording(&label, case.name, message);
            } else {
                assert_eq!(
                    message,
                    format!("{SLO_RESOURCE_TYPE} with arn '{arn}' does not exist."),
                    "case {label}: expected the message to name the ARN, got {message:?}"
                );
            }
        }
    }
}

/// Rule 2: a create under a name already stored is refused with the one generic sentence,
/// which identifies nothing — not the SLO, not its name and not its ARN.
///
/// The rows differ along the name's form and along the rest of the repeated request, which
/// the wording must not depend on: a different description, the other (mutually
/// exclusive) SLI config, a goal and burn rates the stored record does not carry.
#[tokio::test]
async fn a_repeated_create_conflicts_without_identifying_anything() {
    for name in [
        "conflict-slo",
        "conflict.slo.dotted",
        "conflict slo spaced",
        "7A",
    ] {
        let router = router();
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first create to succeed, got {status} with {created}"
        );
        let stored_arn = created["Slo"]["Arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            stored_arn,
            slo_arn(name),
            "case {name:?}: expected the stored ARN to be the minted one, got {stored_arn:?}"
        );

        let repeats = [
            json!({ "Name": name, "Description": "a different description",
                    "SliConfig": sli_config("orders", "POST /orders", 999.0) }),
            json!({ "Name": name, "RequestBasedSliConfig": {
                    "RequestBasedSliMetricConfig": { "MetricType": "AVAILABILITY" } } }),
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0),
                    "Goal": { "AttainmentGoal": 95.0 },
                    "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }] }),
        ];
        for (index, body) in repeats.iter().enumerate() {
            let (status, error) = create(&router, body.clone()).await;
            assert_eq!(
                status, 409,
                "case {name:?}/{index}: expected HTTP 409 for a repeated name, got {status} with {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, IDENTIFIER_TAKEN,
                "case {name:?}/{index}: expected {IDENTIFIER_TAKEN:?}, got {message:?}"
            );
            for identifier in [name, stored_arn.as_str(), SLO_RESOURCE_TYPE] {
                assert!(
                    !message.contains(identifier),
                    "case {name:?}/{index}: the conflict must identify nothing, but it states \
                     {identifier:?}: {message:?}"
                );
            }
        }
    }
}

/// One `CreateInstrumentationConfiguration` request that satisfies every `@required` member
/// of the operation (`smithy_query(query="closure",operation=
/// "CreateInstrumentationConfiguration")`: `InstrumentationType`, `Service`, `Environment`,
/// `SignalType`, `Location`, `CaptureConfiguration`), with `Service` set to `service`.
///
/// Every member but `Service` is valid, so the only constraint a row can violate is the one
/// it sets out to violate.
fn instrumentation_request(service: &str) -> Value {
    json!({
        "InstrumentationType": "BREAKPOINT",
        "Service": service,
        "Environment": "prod",
        "SignalType": "SNAPSHOT",
        "Location": {
            "CodeLocation": { "Language": "Java", "FilePath": "src/main/java/App.java" }
        },
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": { "MaxHits": 1 } } }
    })
}

/// Rule 3: a refused request is answered with AWS's constraint-violation envelope, naming
/// the member and the requirement and NOTHING of the value that was rejected; a `@pattern`
/// requirement is the model's expression minus its anchors.
///
/// The rows differ along the operation, the violated constraint class (`@pattern`,
/// `@length` min and max, `@range` min and max, an absent `@required` member) and the member
/// the violation is on — including each published bound of a `@range` and one step past it.
/// Every pattern and bound quoted below is the one the model declares for that member.
#[tokio::test]
async fn a_constraint_violation_names_the_member_and_the_requirement_only() {
    struct Case {
        label: &'static str,
        method: &'static str,
        uri: String,
        body: Option<Value>,
        /// The value the request put in the offending member; it must not appear in the
        /// message. `None` for an absent member, which has no value to echo.
        rejected: Option<String>,
        /// The whole message the service must answer with.
        message: String,
    }

    // The two `@pattern`s the issue's own operations declare, and the account-id pattern on
    // the list selector, each rendered WITHOUT the `^`/`$` the model spells it with.
    let slo_name_pattern = "[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]";
    let instrumentation_pattern = "[A-Za-z0-9:/+=,.@_-]+";
    let account_pattern = "[0-9]{12}";
    let long_description = "d".repeat(1025);

    let cases = vec![
        // CreateServiceLevelObjective.Name: @pattern on the SLO name.
        Case {
            label: "CreateServiceLevelObjective.Name below the pattern",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Name": "a" })),
            // `a` is a character of the pattern text itself, so "the value is absent from
            // the message" cannot be asserted on this row; the row below sends a one
            // character name that the pattern text does not contain.
            rejected: None,
            message: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {slo_name_pattern}"
            ),
        },
        Case {
            label: "CreateServiceLevelObjective.Name shorter than the pattern admits",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Name": "q" })),
            rejected: Some("q".to_string()),
            message: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {slo_name_pattern}"
            ),
        },
        Case {
            label: "CreateServiceLevelObjective.Name with an illegal character",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Name": "has/a/slash" })),
            rejected: Some("has/a/slash".to_string()),
            message: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {slo_name_pattern}"
            ),
        },
        // CreateInstrumentationConfiguration.Service: a different operation, a different
        // @pattern — the same builder words both.
        Case {
            label: "CreateInstrumentationConfiguration.Service with spaces",
            method: "POST",
            uri: "/create-instrumentation-configuration".to_string(),
            body: Some(instrumentation_request("bad service name")),
            rejected: Some("bad service name".to_string()),
            message: format!(
                "1 validation error detected: Value at 'service' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {instrumentation_pattern}"
            ),
        },
        // ListServiceLevelObjectives.SloOwnerAwsAccountId: @pattern on a query-bound member.
        Case {
            label: "ListServiceLevelObjectives.SloOwnerAwsAccountId too short",
            method: "POST",
            uri: "/slos?SloOwnerAwsAccountId=12345".to_string(),
            body: Some(json!({})),
            rejected: Some("12345".to_string()),
            message: format!(
                "1 validation error detected: Value at 'sloOwnerAwsAccountId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {account_pattern}"
            ),
        },
        // CreateServiceLevelObjective.Description: @length 1..1024, both bounds.
        Case {
            label: "CreateServiceLevelObjective.Description below the length minimum",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Name": "length-case", "Description": "" })),
            rejected: None,
            message: "1 validation error detected: Value at 'description' failed to satisfy \
                      constraint: Member must have length greater than or equal to 1"
                .to_string(),
        },
        Case {
            label: "CreateServiceLevelObjective.Description above the length maximum",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Name": "length-case", "Description": long_description.clone() })),
            rejected: Some(long_description.clone()),
            message: "1 validation error detected: Value at 'description' failed to satisfy \
                      constraint: Member must have length less than or equal to 1024"
                .to_string(),
        },
        // ListServiceLevelObjectives.MaxResults: @range 1..50, one step past each bound.
        Case {
            label: "ListServiceLevelObjectives.MaxResults below the range minimum",
            method: "POST",
            uri: "/slos?MaxResults=0".to_string(),
            body: Some(json!({})),
            rejected: Some("0".to_string()),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value greater than or equal to 1"
                .to_string(),
        },
        Case {
            label: "ListServiceLevelObjectives.MaxResults above the range maximum",
            method: "POST",
            uri: "/slos?MaxResults=51".to_string(),
            body: Some(json!({})),
            rejected: Some("51".to_string()),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 50"
                .to_string(),
        },
        // An absent @required member: stated by the member and the requirement alone, with
        // nothing where a value would go, as the recorded absent-member refusal reads.
        Case {
            label: "CreateServiceLevelObjective without Name",
            method: "POST",
            uri: "/slo".to_string(),
            body: Some(json!({ "Description": "no name at all" })),
            rejected: None,
            message: "1 validation error detected: Value at 'name' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
    ];

    let router = router();
    for case in &cases {
        let (status, error) = call(&router, case.method, &case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {error}",
            case.label
        );
        let message = error["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, case.message,
            "case {:?}: expected the message {:?}, got {message:?}",
            case.label, case.message
        );
        if let Some(rejected) = case.rejected.as_deref() {
            assert!(
                !message.contains(rejected),
                "case {:?}: the message must not echo the rejected value {rejected:?}, got {message:?}",
                case.label
            );
        }
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {:?}: a reported pattern carries no anchors, got {message:?}",
            case.label
        );
    }
}
