// SPDX-License-Identifier: CC0-1.0

//! What the resource-evaluation surface REPORTS, end to end over the wire: the
//! terminal verdict of an execution, the members a listed evaluation carries and the
//! envelopes its three refusals travel in.
//!
//! Five rules are pinned here, each over a table of inputs rather than the one recorded
//! case:
//!
//! 1. `GetResourceEvaluationSummary`'s terminal `EvaluationStatus$Status` follows
//!    whether there was an execution to run: `SUCCEEDED` only when the account has rule
//!    configuration for the evaluation's mode and every rule covering the resource had
//!    already placed it, `FAILED` otherwise (nothing configured to run, or a covering
//!    rule whose verdict this emulator cannot produce);
//! 2. the terminal `Compliance` is the roll-up of the verdicts the covering rules
//!    placed the resource at — `NON_COMPLIANT` over `COMPLIANT` over
//!    `INSUFFICIENT_DATA` (a rule applies but decided nothing) over `NOT_APPLICABLE`
//!    (no rule verdict applies at all);
//! 3. a `ListResourceEvaluations` entry carries exactly `ResourceEvaluationId` and
//!    `EvaluationStartTimestamp`;
//! 4. `IdempotentParameterMismatch` and `ResourceNotFoundException` carry ONE template
//!    each, whose only request-derived part is the `ClientToken` / the
//!    `ResourceEvaluationId`;
//! 5. `ListResourceEvaluations`'s `InvalidTimeRangeException` carries its code alone:
//!    the error envelope populates no `message` member.
//!
//! The harness (`Emulator`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds; verdicts a rule recorded are seeded
//! through `service_state`, the way the other suites of this crate seed state.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, EvaluationResultEntry,
};

use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator};

/// The published `EvaluationMode` values, from
/// `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE`, `PROACTIVE`.
const DETECTIVE: &str = "DETECTIVE";
/// See [`DETECTIVE`].
const PROACTIVE: &str = "PROACTIVE";

/// The published `ResourceEvaluationStatus` values, from
/// `smithy_query(query="shape", shape="ResourceEvaluationStatus")`: `IN_PROGRESS`,
/// `FAILED`, `SUCCEEDED`.
const SUCCEEDED: &str = "SUCCEEDED";
/// See [`SUCCEEDED`].
const FAILED: &str = "FAILED";

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The resource every evaluation below is handed.
const RESOURCE_TYPE: &str = "AWS::S3::Bucket";
/// See [`RESOURCE_TYPE`].
const RESOURCE_ID: &str = "surface-bucket";

/// A resource type no rule of these cases is scoped to.
const OTHER_RESOURCE_TYPE: &str = "AWS::DynamoDB::Table";

/// Longer than `EXECUTION_SETTLES_AFTER`, the span after which the clock owes a
/// started execution its terminal status.
const PAST_THE_SETTLING_SPAN: Duration = Duration::from_millis(700);

/// One Config rule a row of the terminal-verdict table puts into the account.
struct RuleRow {
    /// The rule's name, which a failure reason may name.
    name: &'static str,
    /// The `EvaluationModes` entry it evaluates in.
    mode: &'static str,
    /// `Some(type)` scopes it to that resource type; `None` leaves it unscoped, so it
    /// covers every resource.
    scoped_to_type: Option<&'static str>,
    /// The `ComplianceType` this rule has already recorded for the evaluated resource,
    /// if any. `None` means its verdict would have to be computed by running the
    /// rule's implementation.
    verdict: Option<&'static str>,
}

/// One row of the terminal-verdict table.
struct TerminalCase {
    name: &'static str,
    rules: &'static [RuleRow],
    status: &'static str,
    compliance: &'static str,
}

/// The `StartResourceEvaluation` body every case uses.
fn start_body() -> Value {
    json!({
        "EvaluationMode": PROACTIVE,
        "ResourceDetails": {
            "ResourceId": RESOURCE_ID,
            "ResourceType": RESOURCE_TYPE,
            "ResourceConfiguration": json!({ "BucketName": RESOURCE_ID }).to_string(),
        },
    })
}

/// Start one evaluation and return the id the service minted.
async fn start(case: &str, emulator: &Emulator, body: &Value) -> String {
    let started = emulator.ok("StartResourceEvaluation", body).await;
    started["ResourceEvaluationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceEvaluationId, got {started}"))
        .to_string()
}

/// Put `rule` into `emulator`'s account and seed the verdict it has already recorded
/// for the evaluated resource, if any.
async fn put_rule(emulator: &Emulator, rule: &RuleRow) {
    let mut config_rule = json!({
        "ConfigRuleName": rule.name,
        "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
        "EvaluationModes": [{ "Mode": rule.mode }],
    });
    if let Some(resource_type) = rule.scoped_to_type {
        config_rule["Scope"] = json!({ "ComplianceResourceTypes": [resource_type] });
    }
    emulator
        .ok("PutConfigRule", &json!({ "ConfigRule": config_rule }))
        .await;

    if let Some(verdict) = rule.verdict {
        let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
        let mut store = state.lock();
        let entry = store
            .config_rules
            .get_mut(rule.name)
            .expect("the rule was created through PutConfigRule");
        let recorded = EvaluationResultEntry::new(
            RESOURCE_TYPE,
            RESOURCE_ID,
            rule.mode,
            verdict,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        entry.evaluation_results.insert(recorded.key(), recorded);
    }
}

/// The terminal status of an execution says whether there was an execution to run, and
/// its compliance says where the rules that cover the resource placed it.
#[tokio::test]
async fn the_terminal_verdict_follows_the_rules_that_cover_the_resource() {
    // Rows differ along the account's rule configuration: none at all, a rule in the
    // other mode, a rule that covers another resource type, and covering rules with
    // every published verdict recorded, with none recorded, and in combination.
    let cases: [TerminalCase; 11] = [
        TerminalCase {
            name: "no rule at all",
            rules: &[],
            status: FAILED,
            compliance: NOT_APPLICABLE,
        },
        TerminalCase {
            name: "a rule that evaluates in the other mode only",
            rules: &[RuleRow {
                name: "detective-only",
                mode: DETECTIVE,
                scoped_to_type: None,
                verdict: None,
            }],
            status: FAILED,
            compliance: NOT_APPLICABLE,
        },
        TerminalCase {
            name: "a proactive rule scoped to another resource type",
            rules: &[RuleRow {
                name: "proactive-elsewhere",
                mode: PROACTIVE,
                scoped_to_type: Some(OTHER_RESOURCE_TYPE),
                verdict: None,
            }],
            status: SUCCEEDED,
            compliance: NOT_APPLICABLE,
        },
        TerminalCase {
            name: "a covering rule that recorded nothing",
            rules: &[RuleRow {
                name: "covering-undecided",
                mode: PROACTIVE,
                scoped_to_type: None,
                verdict: None,
            }],
            status: FAILED,
            compliance: INSUFFICIENT_DATA,
        },
        TerminalCase {
            name: "a covering rule that recorded COMPLIANT",
            rules: &[RuleRow {
                name: "covering-compliant",
                mode: PROACTIVE,
                scoped_to_type: Some(RESOURCE_TYPE),
                verdict: Some(COMPLIANT),
            }],
            status: SUCCEEDED,
            compliance: COMPLIANT,
        },
        TerminalCase {
            name: "a covering rule that recorded NON_COMPLIANT",
            rules: &[RuleRow {
                name: "covering-noncompliant",
                mode: PROACTIVE,
                scoped_to_type: Some(RESOURCE_TYPE),
                verdict: Some(NON_COMPLIANT),
            }],
            status: SUCCEEDED,
            compliance: NON_COMPLIANT,
        },
        TerminalCase {
            name: "a covering rule that recorded NOT_APPLICABLE",
            rules: &[RuleRow {
                name: "covering-not-applicable",
                mode: PROACTIVE,
                scoped_to_type: None,
                verdict: Some(NOT_APPLICABLE),
            }],
            status: SUCCEEDED,
            compliance: NOT_APPLICABLE,
        },
        TerminalCase {
            name: "a covering rule that recorded INSUFFICIENT_DATA",
            rules: &[RuleRow {
                name: "covering-insufficient",
                mode: PROACTIVE,
                scoped_to_type: None,
                verdict: Some(INSUFFICIENT_DATA),
            }],
            status: SUCCEEDED,
            compliance: INSUFFICIENT_DATA,
        },
        TerminalCase {
            name: "two covering rules, one of them noncompliant",
            rules: &[
                RuleRow {
                    name: "covering-compliant-of-two",
                    mode: PROACTIVE,
                    scoped_to_type: None,
                    verdict: Some(COMPLIANT),
                },
                RuleRow {
                    name: "covering-noncompliant-of-two",
                    mode: PROACTIVE,
                    scoped_to_type: Some(RESOURCE_TYPE),
                    verdict: Some(NON_COMPLIANT),
                },
            ],
            status: SUCCEEDED,
            compliance: NON_COMPLIANT,
        },
        TerminalCase {
            name: "a compliant verdict beside a rule that recorded nothing",
            rules: &[
                RuleRow {
                    name: "covering-compliant-beside",
                    mode: PROACTIVE,
                    scoped_to_type: None,
                    verdict: Some(COMPLIANT),
                },
                RuleRow {
                    name: "covering-undecided-beside",
                    mode: PROACTIVE,
                    scoped_to_type: None,
                    verdict: None,
                },
            ],
            status: FAILED,
            compliance: COMPLIANT,
        },
        TerminalCase {
            name: "a not-applicable verdict beside a rule that recorded nothing",
            rules: &[
                RuleRow {
                    name: "covering-not-applicable-beside",
                    mode: PROACTIVE,
                    scoped_to_type: None,
                    verdict: Some(NOT_APPLICABLE),
                },
                RuleRow {
                    name: "covering-undecided-beside-two",
                    mode: PROACTIVE,
                    scoped_to_type: None,
                    verdict: None,
                },
            ],
            status: FAILED,
            compliance: INSUFFICIENT_DATA,
        },
    ];

    for case in &cases {
        let name = case.name;
        // One account per row: the rules a row puts must not leak into the next.
        let emulator = Emulator::new();
        if !case.rules.is_empty() {
            with_recorder_and_channel(&emulator).await;
            for rule in case.rules {
                put_rule(&emulator, rule).await;
            }
        }

        let id = start(name, &emulator, &start_body()).await;
        tokio::time::sleep(PAST_THE_SETTLING_SPAN).await;
        let reported = emulator
            .ok(
                "GetResourceEvaluationSummary",
                &json!({ "ResourceEvaluationId": id }),
            )
            .await;

        assert_eq!(
            reported["EvaluationStatus"]["Status"].as_str(),
            Some(case.status),
            "case {name:?}: expected the execution to end {:?}, got {reported}",
            case.status
        );
        assert_eq!(
            reported["Compliance"].as_str(),
            Some(case.compliance),
            "case {name:?}: expected Compliance {:?}, got {reported}",
            case.compliance
        );
        // A failed execution says why; one that did not fail carries no reason.
        assert_eq!(
            reported["EvaluationStatus"]
                .get("FailureReason")
                .is_some(),
            case.status == FAILED,
            "case {name:?}: a failed execution must say why and a successful one must \
             not, got {reported}"
        );
        // The read is reporting only: a second read sees exactly the same verdict.
        let again = emulator
            .ok(
                "GetResourceEvaluationSummary",
                &json!({ "ResourceEvaluationId": id }),
            )
            .await;
        assert_eq!(
            again, reported,
            "case {name:?}: reading a summary must not change what a later read sees, \
             got {again}"
        );
    }
}

/// A listed evaluation carries exactly its id and the instant its execution started:
/// the mode belongs to the request's `ResourceEvaluationFilters` vocabulary, not to the
/// listed item.
#[tokio::test]
async fn a_listed_evaluation_carries_only_its_id_and_start_instant() {
    let emulator = Emulator::new();
    for index in 0..3 {
        let mut body = start_body();
        body["ResourceDetails"]["ResourceId"] = json!(format!("{RESOURCE_ID}-{index}"));
        body["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "cfn-stack" });
        start("a listed evaluation", &emulator, &body).await;
    }

    // Rows differ along the request that lists them: unfiltered, filtered by the mode,
    // filtered by the context, and page-limited.
    let cases: [(&str, Value); 4] = [
        ("no filters", json!({})),
        (
            "filtered by the mode",
            json!({ "Filters": { "EvaluationMode": PROACTIVE } }),
        ),
        (
            "filtered by the context",
            json!({ "Filters": { "EvaluationContextIdentifier": "cfn-stack" } }),
        ),
        ("one entry per page", json!({ "Limit": 1 })),
    ];

    for (case, request) in cases {
        let listed = emulator.ok("ListResourceEvaluations", &request).await;
        let entries = listed["ResourceEvaluations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {listed}"))
            .clone();
        assert!(
            !entries.is_empty(),
            "case {case:?}: expected at least one entry, got {listed}"
        );
        for entry in &entries {
            let mut members: Vec<&str> = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is an object, got {entry}"))
                .keys()
                .map(String::as_str)
                .collect();
            members.sort_unstable();
            assert_eq!(
                members,
                ["EvaluationStartTimestamp", "ResourceEvaluationId"],
                "case {case:?}: expected exactly ResourceEvaluationId and \
                 EvaluationStartTimestamp, got {members:?}"
            );
        }
    }
}

/// `IdempotentParameterMismatch` carries one template, whose only request-derived part
/// is the client token.
#[tokio::test]
async fn an_idempotent_parameter_mismatch_carries_one_template() {
    // `ClientToken`'s modelled length is 64..256
    // (`smithy_query(query="shape", shape="ClientToken")`), so the rows span that
    // range: the minimum, a middling token and the maximum.
    let tokens = [
        "a".repeat(64),
        format!("{:-<100}", "sim-probe-token"),
        "z".repeat(256),
    ];

    /// One row of the mismatch table: the parameter the retry changed.
    type Change = (&'static str, fn(&mut Value));

    // Rows differ along WHICH parameter the retry changed.
    let changes: [Change; 4] = [
        ("a different resource id", |body| {
            body["ResourceDetails"]["ResourceId"] = json!("another-bucket");
        }),
        ("a different resource type", |body| {
            body["ResourceDetails"]["ResourceType"] = json!(OTHER_RESOURCE_TYPE);
        }),
        ("a different configuration", |body| {
            body["ResourceDetails"]["ResourceConfiguration"] =
                json!(json!({ "BucketName": "renamed" }).to_string());
        }),
        ("a different evaluation context", |body| {
            body["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "other-stack" });
        }),
    ];

    for token in &tokens {
        for (case, change) in changes {
            let emulator = Emulator::new();
            let mut body = start_body();
            body["ClientToken"] = json!(token);
            let started = start(case, &emulator, &body).await;

            // The same token with the same parameters performs no further actions.
            let retried = emulator.ok("StartResourceEvaluation", &body).await;
            assert_eq!(
                retried["ResourceEvaluationId"].as_str(),
                Some(started.as_str()),
                "case {case:?}: an identical retry must answer the same evaluation, got \
                 {retried}"
            );

            let mut mismatched = body.clone();
            change(&mut mismatched);
            let (status, parsed) = emulator
                .call("StartResourceEvaluation", &mismatched)
                .await;
            let code = parsed
                .get("__type")
                .and_then(Value::as_str)
                .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
                .unwrap_or("<no __type>");
            assert_eq!(
                (status, code),
                (400, "IdempotentParameterMismatch"),
                "case {case:?}: expected 400 IdempotentParameterMismatch, got {status} \
                 {parsed}"
            );
            let expected = format!(
                "The request inputs are not associated with the supplied client token \
                 {token}. You are using the same client token with one or more different \
                 parameters. Specify a new client token with the parameter changes and try \
                 again."
            );
            let message = parsed.get("message").and_then(Value::as_str).unwrap_or("");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// `ResourceNotFoundException` quotes the `ResourceEvaluationId` the request carried in
/// one not-found template.
#[tokio::test]
async fn an_unknown_evaluation_id_is_reported_as_not_found() {
    let emulator = Emulator::new();
    let known = start("a started execution", &emulator, &start_body()).await;
    let appended = format!("{known}f");
    let longest = "9".repeat(128);

    // Rows differ along the form of the id asked for: a hex id of the service's own
    // form, a free-form string, an id one character longer than one it minted, a
    // single character and the modelled `ResourceEvaluationId` length maximum (1..128).
    let cases = [
        "00000000000000000000000000000000",
        "not-a-resource-evaluation-id",
        appended.as_str(),
        "x",
        longest.as_str(),
    ];

    for unknown in cases {
        let (status, parsed) = emulator
            .call(
                "GetResourceEvaluationSummary",
                &json!({ "ResourceEvaluationId": unknown }),
            )
            .await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<no __type>");
        assert_eq!(
            (status, code),
            (400, "ResourceNotFoundException"),
            "case {unknown:?}: expected 400 ResourceNotFoundException, got {status} {parsed}"
        );
        let expected = format!("Provided ResourceEvaluationId {unknown} not found.");
        let message = parsed.get("message").and_then(Value::as_str).unwrap_or("");
        assert_eq!(
            message, expected,
            "case {unknown:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// An inverted `Filters.TimeWindow` is refused with the code alone: the envelope
/// populates no `message` member. Equal bounds are a valid window and are served.
#[tokio::test]
async fn an_inverted_time_window_is_refused_without_a_message() {
    let emulator = Emulator::new();
    start("a started execution", &emulator, &start_body()).await;

    let noon: f64 = 1_700_000_000.0;
    // Rows differ along how far the window is inverted: by a second, by a day, by
    // years, and with the start at the epoch's far side of the end.
    let inverted: [(&str, f64, f64); 4] = [
        ("inverted by one second", noon + 1.0, noon),
        ("inverted by a day", noon + 86_400.0, noon),
        ("inverted by years", noon, noon - 3.0 * 31_536_000.0),
        ("an end at the Unix epoch", noon, 0.0),
    ];

    for (case, start_time, end_time) in inverted {
        let (status, parsed) = emulator
            .call(
                "ListResourceEvaluations",
                &json!({
                    "Filters": { "TimeWindow": { "StartTime": start_time, "EndTime": end_time } },
                }),
            )
            .await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<no __type>");
        assert_eq!(
            (status, code),
            (400, "InvalidTimeRangeException"),
            "case {case:?}: expected 400 InvalidTimeRangeException, got {status} {parsed}"
        );
        assert!(
            parsed.get("message").is_none_or(Value::is_null),
            "case {case:?}: the refusal must carry no message member, got {parsed}"
        );
    }

    // The boundary the rule is stated at: a window whose bounds are equal is valid.
    for at in [noon, 0.0] {
        let listed = emulator
            .ok(
                "ListResourceEvaluations",
                &json!({ "Filters": { "TimeWindow": { "StartTime": at, "EndTime": at } } }),
            )
            .await;
        assert!(
            listed["ResourceEvaluations"].is_array(),
            "case \"equal bounds at {at}\": a degenerate window is valid, got {listed}"
        );
    }
}
