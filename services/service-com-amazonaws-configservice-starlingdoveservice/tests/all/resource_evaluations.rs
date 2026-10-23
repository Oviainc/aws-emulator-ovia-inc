// SPDX-License-Identifier: CC0-1.0

//! The proactive resource-evaluation path, end to end over the wire:
//! `StartResourceEvaluation` persists one execution and `ListResourceEvaluations`
//! reports what is in state.
//!
//! Four rules are pinned here, each over a table of inputs rather than one recorded
//! case:
//!
//! 1. an `EvaluationMode` member the model documents as `PROACTIVE`-only refuses every
//!    other published mode with the operation's modelled `InvalidParameterValueException`
//!    and persists nothing — checked on both members that carry that documentation,
//!    `StartResourceEvaluationRequest$EvaluationMode` and
//!    `ResourceEvaluationFilters$EvaluationMode`;
//! 2. a started evaluation is reported back by the list path with the id the service
//!    minted, the mode it runs in and the instant its execution started, most recent
//!    first, whatever resource details, context or timeout it was started with;
//! 3. the list path selects by every `ResourceEvaluationFilters` member and pages at the
//!    requested `Limit`, with an inverted time window and a token this operation never
//!    issued refused by their modelled errors;
//! 4. a retry carrying a `ClientToken` an evaluation was already started with performs
//!    no further actions, and a retry whose parameters differ fails with
//!    `IdempotentParameterMismatch`.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// Every published `EvaluationMode` value, from
/// `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE`, `PROACTIVE`.
const DETECTIVE: &str = "DETECTIVE";
/// See [`DETECTIVE`].
const PROACTIVE: &str = "PROACTIVE";

/// The only published `ResourceConfigurationSchemaType` value, from
/// `smithy_query(query="shape", shape="ResourceConfigurationSchemaType")`.
const CFN_RESOURCE_SCHEMA: &str = "CFN_RESOURCE_SCHEMA";

/// The published bounds of `ListResourceEvaluationsPageItemLimit` (range 0..100), from
/// `smithy_query(query="shape", shape="ListResourceEvaluationsPageItemLimit")`.
const LIMIT_MIN: i64 = 0;
/// See [`LIMIT_MIN`].
const LIMIT_MAX: i64 = 100;

/// The page size `ListResourceEvaluations` serves when the request asks for no `Limit`
/// (`ListResourceEvaluationsRequest$Limit`: "The default is 10").
const DEFAULT_LIMIT: usize = 10;

/// The published maximum of `EvaluationTimeout` (range 0..3600), from
/// `smithy_query(query="shape", shape="EvaluationTimeout")`.
const EVALUATION_TIMEOUT_MAX: i64 = 3600;

/// Resource types the evaluation rows use. `ResourceDetails$ResourceType` points at the
/// "Resource Type Value" column of *Supported Amazon Web Services resource Types*;
/// these are entries from it.
const RESOURCE_TYPES: [&str; 3] = [
    "AWS::S3::Bucket",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
];

/// Long enough that two evaluations started around it land on either side of an instant
/// the test reads off the clock, well past the resolution an epoch-seconds timestamp
/// carries on the wire.
const APART: Duration = Duration::from_millis(20);

/// A `ClientToken` built from `seed`. `ClientToken`'s modelled length is 64..256, so a
/// shorter token never reaches the handler at all.
fn client_token(seed: &str) -> String {
    format!("{seed:0<64}")
}

/// The `ResourceDetails` body for one resource.
fn resource_details(resource_id: &str, resource_type: &str, configuration: &Value) -> Value {
    json!({
        "ResourceId": resource_id,
        "ResourceType": resource_type,
        "ResourceConfiguration": configuration.to_string(),
    })
}

/// A minimal PROACTIVE `StartResourceEvaluation` body for one resource.
fn start_request(resource_id: &str, resource_type: &str) -> Value {
    json!({
        "EvaluationMode": PROACTIVE,
        "ResourceDetails": resource_details(
            resource_id,
            resource_type,
            &json!({ "Name": resource_id }),
        ),
    })
}

/// Start one evaluation and return the `ResourceEvaluationId` the service minted.
async fn start(case: &str, emulator: &Emulator, body: &Value) -> String {
    let started = emulator.ok("StartResourceEvaluation", body).await;
    started["ResourceEvaluationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ResourceEvaluationId, got {started}")
        })
        .to_string()
}

/// Every evaluation `ListResourceEvaluations` reports for `request`.
async fn listed(case: &str, emulator: &Emulator, request: &Value) -> Vec<Value> {
    let response = emulator.ok("ListResourceEvaluations", request).await;
    response["ResourceEvaluations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ResourceEvaluations list, got {response}")
        })
        .clone()
}

/// The `ResourceEvaluationId` of each evaluation in `evaluations`, in the order
/// reported.
fn ids(case: &str, evaluations: &[Value]) -> Vec<String> {
    evaluations
        .iter()
        .map(|evaluation| {
            evaluation["ResourceEvaluationId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every evaluation carries an id, got {evaluation}")
                })
                .to_string()
        })
        .collect()
}

/// Every evaluation the list path reports, read with the published maximum `Limit` so
/// one page holds them all.
async fn all_listed(case: &str, emulator: &Emulator) -> Vec<Value> {
    listed(case, emulator, &json!({ "Limit": LIMIT_MAX })).await
}

/// The wall clock as epoch seconds, the form a modelled timestamp takes on this
/// service's `awsJson1_1` wire.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the Unix epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn only_the_proactive_evaluation_mode_is_accepted_on_the_resource_evaluation_path() {
    // Rows: every published EvaluationMode value, on each member the model documents as
    // PROACTIVE-only.
    for mode in [PROACTIVE, DETECTIVE] {
        let accepted = mode == PROACTIVE;
        let emulator = Emulator::new();

        let case = format!("StartResourceEvaluation in {mode} mode");
        let mut request = start_request("mode-row", RESOURCE_TYPES[0]);
        request["EvaluationMode"] = json!(mode);
        if accepted {
            let started = emulator.ok("StartResourceEvaluation", &request).await;
            assert!(
                started["ResourceEvaluationId"].is_string(),
                "case {case:?}: expected a minted ResourceEvaluationId, got {started}"
            );
        } else {
            let (status, code) = emulator.refused("StartResourceEvaluation", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected 400 InvalidParameterValueException, got {status} {code}"
            );
            // A refused start persists nothing.
            let evaluations = all_listed(&case, &emulator).await;
            assert!(
                evaluations.is_empty(),
                "case {case:?}: a refused start must persist nothing, got {evaluations:?}"
            );
        }

        let case = format!("ListResourceEvaluations filtered on {mode} mode");
        let request = json!({ "Filters": { "EvaluationMode": mode } });
        if accepted {
            let evaluations = listed(&case, &emulator, &request).await;
            assert_eq!(
                evaluations.len(),
                1,
                "case {case:?}: expected the single PROACTIVE evaluation, got {evaluations:?}"
            );
        } else {
            let (status, code) = emulator.refused("ListResourceEvaluations", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected 400 InvalidParameterValueException, got {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn an_unstarted_account_reports_an_empty_page() {
    let emulator = Emulator::new();
    let response = emulator.ok("ListResourceEvaluations", &json!({})).await;
    let evaluations = response["ResourceEvaluations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResourceEvaluations list, got {response}"));
    assert!(
        evaluations.is_empty() && response["NextToken"].is_null(),
        "expected an empty page and no NextToken before anything is started, got {response}"
    );
}

#[tokio::test]
async fn a_started_evaluation_is_reported_back_most_recent_first() {
    // Rows differ along everything a start call can vary: the resource id, the resource
    // type, the shape of the configuration document, whether the request names an
    // evaluation context, a schema type or a timeout (0, a middling value, the
    // published maximum).
    let cases: [(&str, Value); 4] = [
        (
            "no context, no timeout, no schema type",
            json!({
                "EvaluationMode": PROACTIVE,
                "ResourceDetails": resource_details(
                    "bucket-one",
                    RESOURCE_TYPES[0],
                    &json!({ "BucketName": "bucket-one" }),
                ),
            }),
        ),
        (
            "an evaluation context and a middling timeout",
            json!({
                "EvaluationMode": PROACTIVE,
                "EvaluationContext": { "EvaluationContextIdentifier": "cfn-stack-alpha" },
                "EvaluationTimeout": 60,
                "ResourceDetails": resource_details(
                    "table-one",
                    RESOURCE_TYPES[1],
                    &json!({ "TableName": "table-one", "BillingMode": "PAY_PER_REQUEST" }),
                ),
            }),
        ),
        (
            "the published schema type and the published maximum timeout",
            json!({
                "EvaluationMode": PROACTIVE,
                "EvaluationTimeout": EVALUATION_TIMEOUT_MAX,
                "ResourceDetails": {
                    "ResourceId": "volume-one",
                    "ResourceType": RESOURCE_TYPES[2],
                    "ResourceConfiguration": json!({ "Size": 8, "Encrypted": true }).to_string(),
                    "ResourceConfigurationSchemaType": CFN_RESOURCE_SCHEMA,
                },
            }),
        ),
        (
            "a nested configuration document and the minimum timeout",
            json!({
                "EvaluationMode": PROACTIVE,
                "EvaluationTimeout": 0,
                "EvaluationContext": { "EvaluationContextIdentifier": "cfn-stack-beta" },
                "ResourceDetails": resource_details(
                    "bucket-two",
                    RESOURCE_TYPES[0],
                    &json!({
                        "BucketName": "bucket-two",
                        "Tags": [{ "Key": "team", "Value": "platform" }],
                        "VersioningConfiguration": { "Status": "Enabled" },
                    }),
                ),
            }),
        ),
    ];

    let emulator = Emulator::new();
    let mut started: Vec<(&str, String, f64, f64)> = Vec::new();
    for (case, request) in &cases {
        let before = now_seconds();
        let id = start(case, &emulator, request).await;
        let after = now_seconds();
        assert!(
            started.iter().all(|(_, minted, _, _)| minted != &id),
            "case {case:?}: every evaluation must get a distinct id, got {id:?} again"
        );
        started.push((case, id, before, after));
        tokio::time::sleep(APART).await;
    }

    let evaluations = all_listed("every started evaluation", &emulator).await;
    assert_eq!(
        evaluations.len(),
        started.len(),
        "expected every started evaluation to be listed, got {evaluations:?}"
    );

    // Most recent execution first: the reverse of the order they were started in.
    let reported = ids("every started evaluation", &evaluations);
    let expected: Vec<String> = started
        .iter()
        .rev()
        .map(|(_, id, _, _)| id.clone())
        .collect();
    assert_eq!(
        reported, expected,
        "expected the evaluations most recent first, got {reported:?}"
    );

    for (case, id, before, after) in &started {
        let evaluation = evaluations
            .iter()
            .find(|evaluation| evaluation["ResourceEvaluationId"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {case:?}: {id:?} must be listed, got {evaluations:?}"));
        assert_eq!(
            evaluation["EvaluationMode"].as_str(),
            Some(PROACTIVE),
            "case {case:?}: expected EvaluationMode {PROACTIVE}, got {evaluation}"
        );
        let started_at = evaluation["EvaluationStartTimestamp"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an EvaluationStartTimestamp, got {evaluation}")
            });
        assert!(
            (*before..=*after).contains(&started_at),
            "case {case:?}: expected an EvaluationStartTimestamp inside [{before}, {after}], \
             got {started_at}"
        );
    }

    // A read leaves what a later caller sees untouched.
    let again = all_listed("a repeated read", &emulator).await;
    assert_eq!(
        ids("a repeated read", &again),
        reported,
        "a list must not change what a later caller sees, got {again:?}"
    );
}

#[tokio::test]
async fn the_list_path_selects_by_every_filter_member() {
    let emulator = Emulator::new();
    let before_all = now_seconds();
    tokio::time::sleep(APART).await;

    // Three evaluations: two in one deployment context, one in another, separated in
    // time so a window can cut between them.
    let mut request = start_request("alpha-one", RESOURCE_TYPES[0]);
    request["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "cfn-stack-alpha" });
    let first = start("the first evaluation", &emulator, &request).await;
    tokio::time::sleep(APART).await;
    let between = now_seconds();
    tokio::time::sleep(APART).await;

    let mut request = start_request("alpha-two", RESOURCE_TYPES[1]);
    request["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "cfn-stack-alpha" });
    let second = start("the second evaluation", &emulator, &request).await;
    tokio::time::sleep(APART).await;

    let mut request = start_request("beta-one", RESOURCE_TYPES[2]);
    request["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "cfn-stack-beta" });
    let third = start("the third evaluation", &emulator, &request).await;
    tokio::time::sleep(APART).await;
    let after_all = now_seconds();

    // Rows differ along which filter member selects, and along where the window sits
    // relative to the executions: covering all, cutting between them, and missing them
    // entirely on either side.
    let cases: [(&str, Value, Vec<&String>); 10] = [
        ("no filters at all", json!({}), vec![&third, &second, &first]),
        (
            "an empty filters object",
            json!({ "Filters": {} }),
            vec![&third, &second, &first],
        ),
        (
            "the mode every evaluation runs in",
            json!({ "Filters": { "EvaluationMode": PROACTIVE } }),
            vec![&third, &second, &first],
        ),
        (
            "the context two evaluations belong to",
            json!({ "Filters": { "EvaluationContextIdentifier": "cfn-stack-alpha" } }),
            vec![&second, &first],
        ),
        (
            "the context one evaluation belongs to",
            json!({ "Filters": { "EvaluationContextIdentifier": "cfn-stack-beta" } }),
            vec![&third],
        ),
        (
            "a context nothing belongs to",
            json!({ "Filters": { "EvaluationContextIdentifier": "cfn-stack-absent" } }),
            vec![],
        ),
        (
            "a window around every execution",
            json!({ "Filters": { "TimeWindow": { "StartTime": before_all, "EndTime": after_all } } }),
            vec![&third, &second, &first],
        ),
        (
            "a window that starts after the first execution",
            json!({ "Filters": { "TimeWindow": { "StartTime": between } } }),
            vec![&third, &second],
        ),
        (
            "a window that ends before the second execution",
            json!({ "Filters": { "TimeWindow": { "EndTime": between } } }),
            vec![&first],
        ),
        (
            // Equal bounds are a valid (degenerate) window: it is not inverted, so it
            // is accepted, and the inclusive bounds select only an execution at
            // exactly that instant — there is none between two executions.
            "a window whose bounds are equal",
            json!({ "Filters": { "TimeWindow": { "StartTime": between, "EndTime": between } } }),
            vec![],
        ),
    ];

    for (case, request, expected) in &cases {
        let mut request = request.clone();
        request["Limit"] = json!(LIMIT_MAX);
        let evaluations = listed(case, &emulator, &request).await;
        let reported = ids(case, &evaluations);
        let expected: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // A window around no execution at all selects nothing.
    let case = "a window past every execution";
    let evaluations = listed(
        case,
        &emulator,
        &json!({
            "Filters": {
                "TimeWindow": { "StartTime": after_all + 60.0, "EndTime": after_all + 120.0 },
            },
        }),
    )
    .await;
    assert!(
        evaluations.is_empty(),
        "case {case:?}: expected no evaluation, got {evaluations:?}"
    );

    // A window whose start is not chronologically before its end is not a valid range.
    let case = "an inverted window";
    let (status, code) = emulator
        .refused(
            "ListResourceEvaluations",
            &json!({
                "Filters": {
                    "TimeWindow": { "StartTime": after_all, "EndTime": before_all },
                },
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidTimeRangeException"),
        "case {case:?}: expected 400 InvalidTimeRangeException, got {status} {code}"
    );
}

#[tokio::test]
async fn a_page_carries_the_requested_limit_and_the_token_resumes_the_next_one() {
    let emulator = Emulator::new();
    let started = 12;
    for index in 0..started {
        let request = start_request(
            &format!("paged-{index:02}"),
            RESOURCE_TYPES[index % RESOURCE_TYPES.len()],
        );
        start(&format!("evaluation {index}"), &emulator, &request).await;
    }

    let every = ids(
        "every started evaluation",
        &all_listed("every started evaluation", &emulator).await,
    );
    assert_eq!(
        every.len(),
        started,
        "expected every started evaluation to be listed, got {every:?}"
    );

    // Rows differ along the page size asked for: absent, the published minimum (which
    // the member documents as "use the default"), sizes under and over the number
    // started, and the published maximum.
    let cases: [(&str, Option<i64>, usize); 6] = [
        ("no Limit at all", None, DEFAULT_LIMIT),
        ("the published minimum Limit", Some(LIMIT_MIN), DEFAULT_LIMIT),
        ("a Limit of one", Some(1), 1),
        ("a Limit of five", Some(5), 5),
        ("a Limit of exactly the default", Some(10), DEFAULT_LIMIT),
        ("the published maximum Limit", Some(LIMIT_MAX), 12),
    ];

    for (case, limit, page_size) in cases {
        // The first page carries exactly the requested size, and a NextToken only when
        // evaluations are left.
        let mut request = json!({});
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }
        let response = emulator.ok("ListResourceEvaluations", &request).await;
        let page = ids(case, response["ResourceEvaluations"].as_array().map_or(&[], |page| page.as_slice()));
        assert_eq!(
            page.len(),
            page_size,
            "case {case:?}: expected a page of {page_size}, got {page:?}"
        );
        assert_eq!(
            page.as_slice(),
            &every[..page_size],
            "case {case:?}: expected the first {page_size} evaluations, got {page:?}"
        );
        assert_eq!(
            response["NextToken"].is_string(),
            page_size < started,
            "case {case:?}: expected a NextToken only with evaluations left, got {response}"
        );

        // Paging to the end reports every evaluation exactly once, in the same order.
        let mut walked: Vec<String> = Vec::new();
        let mut next_token = Some(Value::Null);
        let mut pages = 0;
        while let Some(token) = next_token.take() {
            let mut request = request.clone();
            if !token.is_null() {
                request["NextToken"] = token;
            }
            let response = emulator.ok("ListResourceEvaluations", &request).await;
            walked.extend(ids(
                case,
                response["ResourceEvaluations"]
                    .as_array()
                    .map_or(&[], |page| page.as_slice()),
            ));
            next_token = response.get("NextToken").filter(|t| t.is_string()).cloned();
            pages += 1;
            assert!(
                pages <= started + 1,
                "case {case:?}: paging must terminate, got {walked:?}"
            );
        }
        assert_eq!(
            walked, every,
            "case {case:?}: expected paging to report every evaluation once, got {walked:?}"
        );
    }

    // One past the published maximum is refused by the member's modelled range, before
    // this operation runs.
    let case = "one past the published maximum Limit";
    let (status, code) = emulator
        .refused(
            "ListResourceEvaluations",
            &json!({ "Limit": LIMIT_MAX + 1 }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} {code}"
    );
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let emulator = Emulator::new();
    start(
        "one evaluation to page",
        &emulator,
        &start_request("token-row", RESOURCE_TYPES[0]),
    )
    .await;

    // Rows differ along how the token is malformed: not base64 at all, base64 of a body
    // with no key separator, base64 of a key whose instant is not a number, and base64
    // of a key with no id.
    let cases: [(&str, &str); 4] = [
        ("not base64", "**not-base64**"),
        ("base64 with no separator", "MTIzNDU2"),
        (
            "base64 whose instant is not a number",
            "d2hlbjppZA==",
        ),
        ("base64 with no id", "MTIzNDU2Nzg5MDo="),
    ];

    for (case, token) in cases {
        let (status, code) = emulator
            .refused("ListResourceEvaluations", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_retry_with_the_same_client_token_performs_no_further_actions() {
    let token = client_token("retry-token-");
    let request = {
        let mut request = start_request("idempotent-one", RESOURCE_TYPES[0]);
        request["ClientToken"] = json!(token);
        request["EvaluationTimeout"] = json!(60);
        request["EvaluationContext"] = json!({ "EvaluationContextIdentifier": "cfn-stack-alpha" });
        request
    };

    let emulator = Emulator::new();
    let first = start("the first start", &emulator, &request).await;

    // The same token with the same parameters answers with the evaluation it already
    // started, and starts no second one.
    for attempt in 1..=2 {
        let case = format!("retry {attempt} with the same parameters");
        let retried = start(&case, &emulator, &request).await;
        assert_eq!(
            retried, first,
            "case {case:?}: expected the id of the evaluation the token started, got {retried:?}"
        );
        let evaluations = all_listed(&case, &emulator).await;
        assert_eq!(
            evaluations.len(),
            1,
            "case {case:?}: a retry must start no second evaluation, got {evaluations:?}"
        );
    }

    // Rows differ along WHICH parameter the retry changed — each one is a mismatch.
    let cases: [(&str, Value); 5] = [
        ("a different ResourceId", {
            let mut changed = request.clone();
            changed["ResourceDetails"]["ResourceId"] = json!("idempotent-two");
            changed
        }),
        ("a different ResourceType", {
            let mut changed = request.clone();
            changed["ResourceDetails"]["ResourceType"] = json!(RESOURCE_TYPES[1]);
            changed
        }),
        ("a different ResourceConfiguration", {
            let mut changed = request.clone();
            changed["ResourceDetails"]["ResourceConfiguration"] =
                json!(json!({ "Name": "something-else" }).to_string());
            changed
        }),
        ("a different EvaluationContextIdentifier", {
            let mut changed = request.clone();
            changed["EvaluationContext"] =
                json!({ "EvaluationContextIdentifier": "cfn-stack-beta" });
            changed
        }),
        ("a different EvaluationTimeout", {
            let mut changed = request.clone();
            changed["EvaluationTimeout"] = json!(120);
            changed
        }),
    ];

    for (case, changed) in &cases {
        let (status, code) = emulator
            .refused("StartResourceEvaluation", changed)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IdempotentParameterMismatch"),
            "case {case:?}: expected 400 IdempotentParameterMismatch, got {status} {code}"
        );
        let evaluations = all_listed(case, &emulator).await;
        assert_eq!(
            evaluations.len(),
            1,
            "case {case:?}: a mismatched retry must start nothing, got {evaluations:?}"
        );
    }

    // A fresh token starts a second evaluation.
    let case = "a fresh client token";
    let mut fresh = request.clone();
    fresh["ClientToken"] = json!(client_token("fresh-token-"));
    let second = start(case, &emulator, &fresh).await;
    assert_ne!(
        second, first,
        "case {case:?}: expected a new evaluation, got {second:?}"
    );
    let evaluations = all_listed(case, &emulator).await;
    assert_eq!(
        evaluations.len(),
        2,
        "case {case:?}: expected both evaluations, got {evaluations:?}"
    );
}

#[tokio::test]
async fn a_resource_configuration_that_is_not_a_resource_definition_is_refused() {
    // Rows differ along the form of the configuration body: a JSON object of the
    // resource's properties is the resource definition the schema type describes; a
    // body that is not one cannot be evaluated against it.
    let cases: [(&str, &str, bool); 6] = [
        ("an empty JSON object", "{}", true),
        (
            "a JSON object of properties",
            r#"{"BucketName":"bucket","Tags":[]}"#,
            true,
        ),
        ("not JSON at all", "BucketName=bucket", false),
        ("a JSON array", r#"[{"BucketName":"bucket"}]"#, false),
        ("a JSON string", r#""BucketName""#, false),
        ("a JSON number", "42", false),
    ];

    for (case, configuration, accepted) in cases {
        let emulator = Emulator::new();
        let request = json!({
            "EvaluationMode": PROACTIVE,
            "ResourceDetails": {
                "ResourceId": "configuration-row",
                "ResourceType": RESOURCE_TYPES[0],
                "ResourceConfiguration": configuration,
            },
        });
        if accepted {
            let started = start(case, &emulator, &request).await;
            let evaluations = all_listed(case, &emulator).await;
            assert_eq!(
                ids(case, &evaluations),
                vec![started.clone()],
                "case {case:?}: expected {started:?} to be listed, got {evaluations:?}"
            );
        } else {
            let (status, code) = emulator
                .refused("StartResourceEvaluation", &request)
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected 400 InvalidParameterValueException, got {status} {code}"
            );
            let evaluations = all_listed(case, &emulator).await;
            assert!(
                evaluations.is_empty(),
                "case {case:?}: a refused start must persist nothing, got {evaluations:?}"
            );
        }
    }
}
