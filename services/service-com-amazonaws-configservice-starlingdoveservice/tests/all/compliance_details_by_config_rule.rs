// SPDX-License-Identifier: CC0-1.0

//! What `GetComplianceDetailsByConfigRule` reports, end to end over the wire.
//!
//! The rule under test: the operation reports exactly the evaluation results the named
//! rule holds in state — each one with the qualifier and the times recorded for it — in
//! the store's stable (resource type, resource id, evaluation mode) order, narrowed to
//! the `ComplianceTypes` the request filters by and cut into pages of the requested
//! `Limit`, where 0 asks for the operation's documented default of 10, handing back a
//! `NextToken` exactly when results remain. It writes nothing, and it refuses a rule
//! this account does not hold (`NoSuchConfigRuleException`), a `NextToken` it did not
//! issue (`InvalidNextTokenException`) and a filter naming a `ComplianceType` no filter
//! may name (`InvalidParameterValueException`).
//!
//! No operation of this API can record an evaluation result yet — `PutEvaluations`
//! identifies the rule only through a `ResultToken` minted by the Lambda invocation that
//! produced the evaluation, which this emulator does not run — so each case seeds that
//! inventory in the shared engine the router hands the handlers, exactly as
//! `pending_aggregation_requests` seeds the inventory no operation of this API mints,
//! and then drives the service the way a client does: an `awsJson1_1` `POST /` through
//! the same `MultiServiceRouter` the emulator binary mounts.

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, EvaluationResultEntry,
};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// The page size the operation serves when the request asks for none of its own, from
/// `smithy_query(query="operation", operation="GetComplianceDetailsByConfigRule")`
/// (`Limit`, range 0..100: "The default is 10. ... If you specify 0, Config uses the
/// default").
const DEFAULT_LIMIT: usize = 10;

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`]. Reported on a result that carries it, but no filter may name it
/// (`GetComplianceDetailsByConfigRuleRequest$ComplianceTypes`: "INSUFFICIENT_DATA
/// cannot be used as a ComplianceType for filtering results").
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Every published `EvaluationMode` value, from
/// `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE`, `PROACTIVE`.
const EVALUATION_MODES: [&str; 2] = ["DETECTIVE", "PROACTIVE"];

/// Resource types the seeded results name. `EvaluationResultQualifier$ResourceType`
/// points at the "Resource Type Value" column of *Supported Amazon Web Services
/// resource Types*; these are entries from it, chosen so they do not sort in the order
/// the results are inserted.
const RESOURCE_TYPES: [&str; 3] = [
    "AWS::S3::Bucket",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
];

/// The instant the first generated result was evaluated at. Whole seconds, so the
/// epoch-second timestamps the wire carries compare exactly.
const FIRST_EVALUATED_AT: u64 = 1_700_000_000;

/// The rule every case evaluates.
const RULE: &str = "details-rule";

/// An emulator holding the rule [`RULE`], ready for results to be seeded against it.
async fn emulator_with_rule() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(RULE)).await;
    emulator
}

/// Seed `results` as the evaluation results recorded against the rule `rule`, reaching
/// the store the way a handler does.
fn seed(emulator: &Emulator, rule: &str, results: &[EvaluationResultEntry]) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let entry = store
        .config_rules
        .get_mut(rule)
        .expect("the rule was created through PutConfigRule");
    for result in results {
        entry.evaluation_results.insert(result.key(), result.clone());
    }
}

/// `count` generated results for one rule, spread over three resource types, both
/// evaluation modes and every published `ComplianceType`, with resource ids that do not
/// ascend with the insertion order and an annotation on every third one.
fn generated(count: usize) -> Vec<EvaluationResultEntry> {
    let compliance_types = [COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE, INSUFFICIENT_DATA];
    (0..count)
        .map(|index| {
            let mut entry = EvaluationResultEntry::new(
                RESOURCE_TYPES[index % RESOURCE_TYPES.len()],
                &format!("res-{:04}", 9_000 - index * 7),
                EVALUATION_MODES[index % EVALUATION_MODES.len()],
                compliance_types[index % compliance_types.len()],
                SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_EVALUATED_AT + index as u64),
            );
            if index % 3 == 0 {
                entry.annotation = Some(format!("evaluated at position {index}"));
            }
            entry
        })
        .collect()
}

/// `at` as the epoch seconds the wire carries a modelled timestamp as.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("every seeded instant is after the epoch")
        .as_secs_f64()
}

/// `value` with every number widened to f64 and every null member dropped, so an
/// epoch-second timestamp compares equal however the serializer wrote it and an absent
/// member compares equal to one serialized as null.
fn normalised(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(normalised).collect()),
        Value::Object(members) => {
            let kept: Map<String, Value> = members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), normalised(member)))
                .collect();
            Value::Object(kept)
        }
        other => other.clone(),
    }
}

/// `result` as the wire object the operation must report it as.
fn wire(rule: &str, result: &EvaluationResultEntry) -> Value {
    let mut value = json!({
        "EvaluationResultIdentifier": {
            "EvaluationResultQualifier": {
                "ConfigRuleName": rule,
                "ResourceType": result.resource_type,
                "ResourceId": result.resource_id,
                "EvaluationMode": result.evaluation_mode,
            },
            "OrderingTimestamp": epoch_seconds(result.ordering_timestamp),
        },
        "ComplianceType": result.compliance_type,
        "ResultRecordedTime": epoch_seconds(result.result_recorded_time),
        "ConfigRuleInvokedTime": epoch_seconds(result.config_rule_invoked_time),
    });
    if let Some(annotation) = result.annotation.as_deref() {
        value["Annotation"] = json!(annotation);
    }
    if let Some(resource_evaluation_id) = result.resource_evaluation_id.as_deref() {
        value["EvaluationResultIdentifier"]["ResourceEvaluationId"] =
            json!(resource_evaluation_id);
    }
    normalised(&value)
}

/// The results of `rule` that a filter of `wanted` selects, in the store's key order,
/// as the wire objects they must be reported as.
fn expected(rule: &str, results: &[EvaluationResultEntry], wanted: &[&str]) -> Vec<Value> {
    let mut selected: Vec<&EvaluationResultEntry> = results
        .iter()
        .filter(|result| wanted.is_empty() || wanted.contains(&result.compliance_type.as_str()))
        .collect();
    selected.sort_by_key(|result| result.key());
    selected.into_iter().map(|result| wire(rule, result)).collect()
}

/// The sizes of the pages `total` results are served in at `limit` per page: a page
/// carries at most `limit`, and a rule with no results still answers one empty page.
fn page_sizes(total: usize, limit: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut remaining = total;
    while remaining > 0 {
        let size = remaining.min(limit);
        sizes.push(size);
        remaining -= size;
    }
    sizes
}

/// Every page of `GetComplianceDetailsByConfigRule` for `rule` at `limit`, walked
/// through its `NextToken`: the pages' contents in order, and the size of each page.
async fn walk_pages(
    case: &str,
    emulator: &Emulator,
    rule: &str,
    wanted: &[&str],
    limit: i64,
    steps: usize,
) -> (Vec<Value>, Vec<usize>) {
    let mut reported: Vec<Value> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..=steps {
        let mut request = json!({ "ConfigRuleName": rule, "Limit": limit });
        if !wanted.is_empty() {
            request["ComplianceTypes"] = json!(wanted);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator
            .ok("GetComplianceDetailsByConfigRule", &request)
            .await;
        let page = body["EvaluationResults"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an EvaluationResults list, got {body}")
            })
            .clone();
        sizes.push(page.len());
        reported.extend(page.iter().map(normalised));
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return (reported, sizes),
        }
    }
    panic!("case {case:?}: the page walk did not finish within {steps} pages, got {sizes:?}");
}

#[tokio::test]
async fn every_result_the_rule_holds_is_reported_in_store_order_one_page_at_a_time() {
    // Rows differ along how many results the rule holds — none, one, under the default
    // page, exactly the default page, one past it, and well past it — crossed with the
    // page size the request asks for, including 0 (the documented default) and the
    // modelled maximum of 100.
    for total in [0_usize, 1, 7, DEFAULT_LIMIT, DEFAULT_LIMIT + 1, 23] {
        for limit in [0_i64, 1, 3, 100] {
            let case = format!("{total} results at Limit {limit}");
            let emulator = emulator_with_rule().await;
            let results = generated(total);
            seed(&emulator, RULE, &results);

            let effective = if limit == 0 {
                DEFAULT_LIMIT
            } else {
                usize::try_from(limit).expect("every row's limit is positive")
            };
            let (reported, sizes) =
                walk_pages(&case, &emulator, RULE, &[], limit, total + 2).await;

            assert_eq!(
                reported,
                expected(RULE, &results, &[]),
                "case {case:?}: expected the rule's results in store order, got {reported:#?}"
            );
            assert_eq!(
                sizes,
                page_sizes(total, effective),
                "case {case:?}: expected pages of at most {effective} results, got {sizes:?}"
            );
        }
    }
}

#[tokio::test]
async fn results_are_narrowed_to_the_compliance_types_the_filter_names() {
    // Rows differ along the filter: absent, each filterable ComplianceType alone, a
    // pair, and every filterable value at once (the modelled ComplianceTypes maximum
    // of three).
    let cases: [(&str, &[&str]); 6] = [
        ("no filter at all", &[]),
        ("compliant only", &[COMPLIANT]),
        ("noncompliant only", &[NON_COMPLIANT]),
        ("not applicable only", &[NOT_APPLICABLE]),
        ("compliant and noncompliant", &[COMPLIANT, NON_COMPLIANT]),
        (
            "every filterable value",
            &[COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE],
        ),
    ];

    // Twelve results cover all four published ComplianceTypes, so a filter both selects
    // and excludes, and INSUFFICIENT_DATA results are reported only when nothing
    // filters them out.
    let results = generated(12);

    for (case, wanted) in cases {
        let emulator = emulator_with_rule().await;
        seed(&emulator, RULE, &results);

        let (reported, _) = walk_pages(case, &emulator, RULE, wanted, 100, 14).await;
        let expected = expected(RULE, &results, wanted);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the {} results the filter selects, got {} in \
             {reported:#?}",
            expected.len(),
            reported.len()
        );
    }
}

#[tokio::test]
async fn a_result_reports_the_qualifier_and_the_times_recorded_against_it() {
    // Rows differ along what the recorded result holds: the resource type and id, the
    // evaluation mode, the compliance type (including the INSUFFICIENT_DATA no filter
    // may name), whether it carries an annotation, whether it came from a resource
    // evaluation run, and whether its three timestamps are one instant or three.
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_EVALUATED_AT);
    let mut annotated = EvaluationResultEntry::new(
        "AWS::DynamoDB::Table",
        "orders",
        EVALUATION_MODES[0],
        NON_COMPLIANT,
        base,
    );
    annotated.annotation = Some("point-in-time recovery is disabled".to_string());
    let mut proactive = EvaluationResultEntry::new(
        "AWS::EC2::Volume",
        "vol-0abcdef1234567890",
        EVALUATION_MODES[1],
        COMPLIANT,
        base,
    );
    proactive.resource_evaluation_id = Some("re-0123456789abcdef".to_string());
    proactive.config_rule_invoked_time = base + Duration::from_secs(1);
    proactive.result_recorded_time = base + Duration::from_secs(2);

    let cases: [(&str, EvaluationResultEntry); 4] = [
        (
            "a plain detective result",
            EvaluationResultEntry::new(
                "AWS::S3::Bucket",
                "logs-bucket",
                EVALUATION_MODES[0],
                NOT_APPLICABLE,
                base,
            ),
        ),
        ("an annotated noncompliant result", annotated),
        ("a proactive result of a resource evaluation", proactive),
        (
            "a result a rule could not be evaluated for",
            EvaluationResultEntry::new(
                "AWS::IAM::Role",
                "service-role",
                EVALUATION_MODES[0],
                INSUFFICIENT_DATA,
                base + Duration::from_secs(3_600),
            ),
        ),
    ];

    for (case, result) in cases {
        let emulator = emulator_with_rule().await;
        seed(&emulator, RULE, std::slice::from_ref(&result));

        let body = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": RULE }),
            )
            .await;
        let reported = normalised(&body["EvaluationResults"][0]);
        assert_eq!(
            reported,
            wire(RULE, &result),
            "case {case:?}: expected the result as recorded, got {body}"
        );

        // The result token associates an evaluation with the `PutEvaluations` call that
        // delivered it; the read path hands none back.
        assert!(
            body["EvaluationResults"][0]["ResultToken"].is_null(),
            "case {case:?}: the read path must not hand back a ResultToken, got {body}"
        );
    }
}

#[tokio::test]
async fn the_report_refuses_a_rule_a_token_and_a_filter_it_cannot_serve() {
    let emulator = emulator_with_rule().await;
    seed(&emulator, RULE, &generated(3));

    // Rows differ along what makes the request unservable: the rule named, the token
    // offered, and the compliance types filtered on.
    let issued_token = {
        let body = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": RULE, "Limit": 1 }),
            )
            .await;
        body["NextToken"]
            .as_str()
            .expect("three results at Limit 1 leave a next page")
            .to_string()
    };

    let cases: [(&str, Value, &str); 9] = [
        (
            "a rule this account does not hold",
            json!({ "ConfigRuleName": "no-such-rule" }),
            "NoSuchConfigRuleException",
        ),
        (
            "a rule name differing only in case",
            json!({ "ConfigRuleName": RULE.to_uppercase() }),
            "NoSuchConfigRuleException",
        ),
        (
            "an empty next token",
            json!({ "ConfigRuleName": RULE, "NextToken": "" }),
            "InvalidNextTokenException",
        ),
        (
            "a next token that is not base64",
            json!({ "ConfigRuleName": RULE, "NextToken": "not-a-token!!" }),
            "InvalidNextTokenException",
        ),
        (
            "a next token another operation issued",
            json!({ "ConfigRuleName": RULE, "NextToken": "ZGV0YWlscy1ydWxl" }),
            "InvalidNextTokenException",
        ),
        (
            "a next token whose fields do not account for its bytes",
            json!({ "ConfigRuleName": RULE, "NextToken": "OTk6YWJj" }),
            "InvalidNextTokenException",
        ),
        (
            "a filter naming INSUFFICIENT_DATA",
            json!({ "ConfigRuleName": RULE, "ComplianceTypes": [INSUFFICIENT_DATA] }),
            "InvalidParameterValueException",
        ),
        (
            "a filter naming INSUFFICIENT_DATA beside a filterable value",
            json!({
                "ConfigRuleName": RULE,
                "ComplianceTypes": [COMPLIANT, INSUFFICIENT_DATA],
            }),
            "InvalidParameterValueException",
        ),
        (
            "an unknown rule asked for with the token of another request",
            json!({ "ConfigRuleName": "no-such-rule", "NextToken": issued_token }),
            "NoSuchConfigRuleException",
        ),
    ];

    for (case, request, expected_code) in cases {
        let (status, code) = emulator
            .refused("GetComplianceDetailsByConfigRule", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
    }

    // The token the operation did issue is served, so the refusals above are about the
    // tokens and not about paging at all.
    let resumed = emulator
        .ok(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": RULE, "Limit": 1, "NextToken": issued_token }),
        )
        .await;
    assert_eq!(
        resumed["EvaluationResults"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "the NextToken this operation issued must serve the next page, got {resumed}"
    );
}

#[tokio::test]
async fn the_report_is_a_read_and_the_rule_rolls_up_the_same_results() {
    // Rows differ along the compliance types the rule's results carry, which is what
    // the rule's own compliance rolls up from: nothing evaluated, one of each
    // filterable value, and mixtures where one noncompliant result decides the rule.
    let cases: [(&str, &[&str], u64, u64); 6] = [
        ("nothing evaluated", &[], 0, 0),
        ("one compliant resource", &[COMPLIANT], 1, 0),
        ("one noncompliant resource", &[NON_COMPLIANT], 0, 1),
        ("only not-applicable results", &[NOT_APPLICABLE], 0, 0),
        (
            "compliant beside noncompliant",
            &[COMPLIANT, NON_COMPLIANT, COMPLIANT],
            0,
            1,
        ),
        (
            "compliant beside insufficient data",
            &[COMPLIANT, INSUFFICIENT_DATA],
            1,
            0,
        ),
    ];

    for (case, compliance_types, compliant_rules, non_compliant_rules) in cases {
        let emulator = emulator_with_rule().await;
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_EVALUATED_AT);
        let results: Vec<EvaluationResultEntry> = compliance_types
            .iter()
            .enumerate()
            .map(|(index, compliance_type)| {
                EvaluationResultEntry::new(
                    RESOURCE_TYPES[index % RESOURCE_TYPES.len()],
                    &format!("res-{index}"),
                    EVALUATION_MODES[0],
                    compliance_type,
                    base,
                )
            })
            .collect();
        seed(&emulator, RULE, &results);

        let request = json!({ "ConfigRuleName": RULE });
        let before = emulator
            .ok("GetComplianceDetailsByConfigRule", &request)
            .await;
        let again = emulator
            .ok("GetComplianceDetailsByConfigRule", &request)
            .await;
        assert_eq!(
            normalised(&again),
            normalised(&before),
            "case {case:?}: a second read must report the same results, got {again}"
        );
        assert_eq!(
            normalised(&before["EvaluationResults"]),
            Value::Array(expected(RULE, &results, &[])),
            "case {case:?}: expected every recorded result, got {before}"
        );

        // The results the details report are the ones the rule's compliance rolls up
        // from: one store, read two ways.
        let summary = emulator
            .ok("GetComplianceSummaryByConfigRule", &json!({}))
            .await;
        for (member, rules) in [
            ("CompliantResourceCount", compliant_rules),
            ("NonCompliantResourceCount", non_compliant_rules),
        ] {
            assert_eq!(
                summary["ComplianceSummary"][member]["CappedCount"],
                json!(rules),
                "case {case:?}: expected {member}.CappedCount {rules} for the seeded \
                 results, got {summary}"
            );
        }

        // Reading the details must not change what a later caller sees.
        let rules_after = emulator.ok("DescribeConfigRules", &json!({})).await;
        emulator
            .ok("GetComplianceDetailsByConfigRule", &request)
            .await;
        assert_eq!(
            emulator.ok("DescribeConfigRules", &json!({})).await,
            rules_after,
            "case {case:?}: reporting compliance details must not change the rules"
        );
    }
}
