// SPDX-License-Identifier: CC0-1.0

//! What `GetComplianceDetailsByResource` reports, end to end over the wire.
//!
//! The rule under test: the operation reports, for ONE resource, exactly the evaluation
//! results EVERY Config rule the account holds recorded for it — each one with the
//! qualifier and the times recorded for it — in the store's stable (rule name,
//! evaluation mode) order, narrowed to the `ComplianceTypes` the request filters by and
//! cut into pages, handing back a `NextToken` exactly when results remain. The resource
//! is named either by its own `ResourceType` and `ResourceId` or by the
//! `ResourceEvaluationId` of an execution, whose resource and evaluation mode then say
//! which results are its own. It writes nothing, and every request it cannot serve — one
//! naming no resource, an execution this account does not hold, a `NextToken` it did not
//! issue, or a `ComplianceType` no filter may name — is refused with
//! `InvalidParameterValueException`, the only error the model declares for it
//! (`smithy_query(query="operation", operation="GetComplianceDetailsByResource")`).
//!
//! Recording an evaluation result for a MANAGED rule is something Config does itself
//! from the configuration state it recorded, so each case seeds that inventory in the
//! shared engine the router hands the handlers — exactly as
//! `compliance_details_by_config_rule` does — and then drives the service the way a
//! client does: an `awsJson1_1` `POST /` through the same `MultiServiceRouter` the
//! emulator binary mounts. The wire shape of one reported result is the one
//! `compliance_details_by_config_rule` already pins, reused from there.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, EvaluationResultEntry, GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE,
};

use crate::compliance_details_by_config_rule::{normalised, wire};
use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`]. Reported on a result that carries it, but no filter may name it
/// (`GetComplianceDetailsByResourceRequest$ComplianceTypes`: "INSUFFICIENT_DATA cannot
/// be used as a ComplianceType for filtering results").
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Every published value, in the order the seeded results cycle through them.
const COMPLIANCE_TYPES: [&str; 4] = [COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE, INSUFFICIENT_DATA];

/// Every published `EvaluationMode` value, from
/// `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE`, `PROACTIVE`.
const DETECTIVE: &str = "DETECTIVE";
/// See [`DETECTIVE`]. The only mode `StartResourceEvaluation` runs an execution in
/// (`StartResourceEvaluationRequest$EvaluationMode`: "The only valid value for this API
/// is PROACTIVE").
const PROACTIVE: &str = "PROACTIVE";

/// Both modes, as a rule that evaluates in both records its results under.
const BOTH_MODES: [&str; 2] = [DETECTIVE, PROACTIVE];

/// Resource types the seeded results name. `EvaluationResultQualifier$ResourceType`
/// points at the "Resource Type Value" column of *Supported Amazon Web Services resource
/// Types*; these are entries from it, plus the account-level type Config accepts for an
/// evaluation of the account itself (`AWS::::Account`).
const OTHER_TYPES: [&str; 3] = [
    "AWS::EC2::Volume",
    "AWS::Lambda::Function",
    "AWS::IAM::Role",
];

/// The resource a case reports on, as (`ResourceType`, `ResourceId`).
type Resource = (&'static str, &'static str);

/// The resources the cases below report on: a plain one, one whose id carries the
/// characters a page key could be split on (so the `NextToken` cannot depend on a
/// separator), one whose id is the longest a `BaseResourceId` may be (length 1..768) and
/// the account itself.
const PLAIN: Resource = ("AWS::S3::Bucket", "config-details-bucket");
/// See [`PLAIN`].
const PUNCTUATED: Resource = ("AWS::DynamoDB::Table", "orders:2026#live/v2");
/// See [`PLAIN`].
const ACCOUNT: Resource = ("AWS::::Account", "000000000000");

/// The instant the first generated result was evaluated at. Whole seconds, so the
/// epoch-second timestamps the wire carries compare exactly.
const FIRST_EVALUATED_AT: u64 = 1_700_000_000;

/// One Config rule a case seeds: its name, and the evaluation results it recorded.
type Seeded = (String, Vec<EvaluationResultEntry>);

/// The instant `offset` seconds after [`FIRST_EVALUATED_AT`].
fn at(offset: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_EVALUATED_AT + offset)
}

/// `rule_count` rules, each recording one result for `target` per mode in `modes` and
/// `others` results for OTHER resources, so a report about `target` has to leave those
/// out.
///
/// The rule names do not ascend with the order the rules are created in, so the report's
/// ordering cannot come from the creation order.
fn seed_plan(
    target: Resource,
    rule_count: usize,
    modes: &[&str],
    others: usize,
) -> Vec<Seeded> {
    (0..rule_count)
        .map(|index| {
            let name = format!("rule-{:03}", 900 - index * 7);
            let mut results = Vec::new();
            for (slot, mode) in modes.iter().enumerate() {
                let position = index * modes.len() + slot;
                let mut result = EvaluationResultEntry::new(
                    target.0,
                    target.1,
                    mode,
                    COMPLIANCE_TYPES[position % COMPLIANCE_TYPES.len()],
                    at(position as u64),
                );
                if position.is_multiple_of(3) {
                    result.annotation = Some(format!("evaluated in {mode} mode"));
                }
                result.config_rule_invoked_time = at(position as u64 + 1);
                result.result_recorded_time = at(position as u64 + 2);
                results.push(result);
            }
            for other in 0..others {
                results.push(EvaluationResultEntry::new(
                    OTHER_TYPES[other % OTHER_TYPES.len()],
                    &format!("other-{:04}", 9_000 - other * 13),
                    BOTH_MODES[other % BOTH_MODES.len()],
                    COMPLIANCE_TYPES[other % COMPLIANCE_TYPES.len()],
                    at(1_000 + other as u64),
                ));
            }
            (name, results)
        })
        .collect()
}

/// An emulator holding one managed Config rule per entry of `plan`, each carrying the
/// results that entry names, reaching the store the way a handler does for the results
/// no operation of this API can record.
async fn emulator_holding(plan: &[Seeded]) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for (name, results) in plan {
        emulator.ok("PutConfigRule", &managed_rule(name)).await;
        let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
        let mut store = state.lock();
        let rule = store
            .config_rules
            .get_mut(name)
            .expect("the rule was created through PutConfigRule");
        for result in results {
            rule.record_evaluation_result(result.clone());
        }
    }
    emulator
}

/// The results of `plan` a report about `target` must carry, in (rule name, evaluation
/// mode) order, narrowed to `only_mode` when an execution fixed one and to the
/// compliance types `wanted` names.
fn expected(
    plan: &[Seeded],
    target: Resource,
    only_mode: Option<&str>,
    wanted: &[&str],
) -> Vec<Value> {
    let mut rules: Vec<&Seeded> = plan.iter().collect();
    rules.sort_by(|left, right| left.0.cmp(&right.0));
    let mut reported = Vec::new();
    for (name, results) in rules {
        let mut selected: Vec<&EvaluationResultEntry> = results
            .iter()
            .filter(|result| result.resource_type == target.0 && result.resource_id == target.1)
            .filter(|result| only_mode.is_none_or(|mode| result.evaluation_mode == mode))
            .filter(|result| {
                wanted.is_empty() || wanted.contains(&result.compliance_type.as_str())
            })
            .collect();
        selected.sort_by_key(|result| result.key());
        reported.extend(selected.into_iter().map(|result| wire(name, result)));
    }
    reported
}

/// The sizes of the pages `total` results are served in: a page carries at most the
/// operation's page size, and a resource with no results still answers one empty page.
fn page_sizes(total: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut remaining = total;
    while remaining > 0 {
        let size = remaining.min(GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE);
        sizes.push(size);
        remaining -= size;
    }
    sizes
}

/// Every page of `GetComplianceDetailsByResource` for `request`, walked through its
/// `NextToken`: the pages' contents in order, and the size of each page.
async fn walk_pages(
    case: &str,
    emulator: &Emulator,
    request: &Value,
    steps: usize,
) -> (Vec<Value>, Vec<usize>) {
    let mut reported: Vec<Value> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..=steps {
        let mut asked = request.clone();
        if let Some(token) = next_token.as_deref() {
            asked["NextToken"] = json!(token);
        }
        let body = emulator
            .ok("GetComplianceDetailsByResource", &asked)
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

/// The request that asks for `target`'s evaluation details, filtered by `wanted`.
fn by_resource(target: Resource, wanted: &[&str]) -> Value {
    let mut request = json!({ "ResourceType": target.0, "ResourceId": target.1 });
    if !wanted.is_empty() {
        request["ComplianceTypes"] = json!(wanted);
    }
    request
}

#[tokio::test]
async fn every_rule_s_result_for_the_named_resource_is_reported_in_store_order() {
    // Rows differ along the resource reported on (including an id carrying the
    // characters a page key could be split on), how many rules evaluated it, how many
    // modes each recorded it in — so a page spans rules, and a rule's own results span
    // modes — and how many results for OTHER resources are in state beside them. The
    // counts straddle the operation's page size: under it, exactly it, and past it.
    let cases: [(&str, Resource, usize, &[&str], usize); 8] = [
        ("no rule at all", PLAIN, 0, &[], 0),
        ("rules that evaluated other resources only", PLAIN, 3, &[], 4),
        ("one rule, one mode", PLAIN, 1, &[DETECTIVE], 0),
        ("one rule, both modes, beside other resources", PLAIN, 1, &BOTH_MODES, 5),
        ("three rules, one mode", PUNCTUATED, 3, &[DETECTIVE], 2),
        (
            "exactly one page over five rules in both modes",
            PUNCTUATED,
            GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE / 2,
            &BOTH_MODES,
            1,
        ),
        (
            "one past a page over eleven rules",
            ACCOUNT,
            GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE + 1,
            &[PROACTIVE],
            0,
        ),
        ("well past a page over nine rules in both modes", ACCOUNT, 9, &BOTH_MODES, 3),
    ];

    for (case, target, rule_count, modes, others) in cases {
        let plan = seed_plan(target, rule_count, modes, others);
        let emulator = emulator_holding(&plan).await;

        let expected = expected(&plan, target, None, &[]);
        let (reported, sizes) = walk_pages(
            case,
            &emulator,
            &by_resource(target, &[]),
            expected.len() + 2,
        )
        .await;

        assert_eq!(
            reported, expected,
            "case {case:?}: expected the {} results the account's rules recorded for the \
             resource, got {} in {reported:#?}",
            expected.len(),
            reported.len()
        );
        assert_eq!(
            sizes,
            page_sizes(expected.len()),
            "case {case:?}: expected pages of at most \
             {GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE} results, got {sizes:?}"
        );
    }
}

#[tokio::test]
async fn results_are_narrowed_to_the_compliance_types_the_filter_names() {
    // Rows differ along the filter: absent, each filterable ComplianceType alone, a pair,
    // and every filterable value at once (the modelled ComplianceTypes maximum of three).
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

    // Six rules recording the resource in both modes cover all four published
    // ComplianceTypes three times over, so a filter both selects and excludes, and the
    // INSUFFICIENT_DATA results are reported only when nothing filters them out.
    let plan = seed_plan(PLAIN, 6, &BOTH_MODES, 2);

    for (case, wanted) in cases {
        let emulator = emulator_holding(&plan).await;

        let expected = expected(&plan, PLAIN, None, wanted);
        let (reported, _) = walk_pages(
            case,
            &emulator,
            &by_resource(PLAIN, wanted),
            expected.len() + 2,
        )
        .await;
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
async fn an_execution_names_the_resource_and_the_mode_whose_results_are_its_own() {
    // Rows differ along the resource the execution was started for and what the rules
    // recorded for it: results in the execution's own mode beside results in the other
    // mode, results in the other mode only, and a resource nothing evaluated.
    let cases: [(&str, Resource, usize, &[&str]); 4] = [
        ("both modes recorded", PLAIN, 4, &BOTH_MODES),
        ("the execution's mode only", PUNCTUATED, 3, &[PROACTIVE]),
        ("the other mode only", ACCOUNT, 3, &[DETECTIVE]),
        ("a resource nothing evaluated", PLAIN, 2, &[]),
    ];

    for (case, target, rule_count, modes) in cases {
        let plan = seed_plan(target, rule_count, modes, 3);
        let emulator = emulator_holding(&plan).await;

        let started = emulator
            .ok(
                "StartResourceEvaluation",
                &json!({
                    "ResourceDetails": {
                        "ResourceId": target.1,
                        "ResourceType": target.0,
                        "ResourceConfiguration": "{}",
                    },
                    "EvaluationMode": PROACTIVE,
                }),
            )
            .await;
        let id = started["ResourceEvaluationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {started}"))
            .to_string();

        // The execution evaluates its resource in ITS mode, so the results it owns are
        // the ones recorded in that mode — never the other mode's.
        let expected_for_execution = expected(&plan, target, Some(PROACTIVE), &[]);
        let (reported, _) = walk_pages(
            case,
            &emulator,
            &json!({ "ResourceEvaluationId": id }),
            expected_for_execution.len() + 2,
        )
        .await;
        assert_eq!(
            reported, expected_for_execution,
            "case {case:?}: expected the {} results recorded in the execution's mode, got \
             {} in {reported:#?}",
            expected_for_execution.len(),
            reported.len()
        );

        // The same resource named by its own type and id reports every mode's results,
        // so the narrowing above is the execution's and not the resource's.
        let expected_for_resource = expected(&plan, target, None, &[]);
        let (by_type_and_id, _) = walk_pages(
            case,
            &emulator,
            &by_resource(target, &[]),
            expected_for_resource.len() + 2,
        )
        .await;
        assert_eq!(
            by_type_and_id, expected_for_resource,
            "case {case:?}: expected every mode's results for the resource itself, got \
             {by_type_and_id:#?}"
        );

        // An execution identifies the resource on its own ("You need to only provide
        // either a ResourceEvaluationID or a ResourceID and ResourceType"), so a request
        // carrying both reports the execution's resource.
        let (both, _) = walk_pages(
            case,
            &emulator,
            &json!({
                "ResourceEvaluationId": id,
                "ResourceType": OTHER_TYPES[0],
                "ResourceId": "other-9000",
            }),
            expected_for_execution.len() + 2,
        )
        .await;
        assert_eq!(
            both, expected_for_execution,
            "case {case:?}: an execution names the resource it evaluated, got {both:#?}"
        );
    }
}

#[tokio::test]
async fn the_report_refuses_a_request_it_cannot_serve() {
    let plan = seed_plan(PLAIN, 6, &BOTH_MODES, 2);
    let emulator = emulator_holding(&plan).await;

    let issued_token = {
        let body = emulator
            .ok("GetComplianceDetailsByResource", &by_resource(PLAIN, &[]))
            .await;
        body["NextToken"]
            .as_str()
            .expect("twelve results leave a next page")
            .to_string()
    };

    // Rows differ along what makes the request unservable: which half of the resource
    // identification is missing, the execution named, the token offered, and the
    // compliance types filtered on. Every one of them is the single error this operation
    // models, so the assertion is on the status and the code together.
    let cases: [(&str, Value); 10] = [
        ("no resource identification at all", json!({})),
        (
            "a resource id with no resource type",
            json!({ "ResourceId": PLAIN.1 }),
        ),
        (
            "a resource type with no resource id",
            json!({ "ResourceType": PLAIN.0 }),
        ),
        (
            "a filter but no resource at all",
            json!({ "ComplianceTypes": [COMPLIANT] }),
        ),
        (
            "an execution this account does not hold",
            json!({ "ResourceEvaluationId": "no-such-evaluation" }),
        ),
        (
            "an empty next token",
            json!({ "ResourceType": PLAIN.0, "ResourceId": PLAIN.1, "NextToken": "" }),
        ),
        (
            "a next token that is not base64",
            json!({ "ResourceType": PLAIN.0, "ResourceId": PLAIN.1, "NextToken": "not-a-token!!" }),
        ),
        (
            "a next token whose fields do not account for its bytes",
            json!({ "ResourceType": PLAIN.0, "ResourceId": PLAIN.1, "NextToken": "OTk6YWJj" }),
        ),
        (
            "a filter naming INSUFFICIENT_DATA",
            json!({
                "ResourceType": PLAIN.0,
                "ResourceId": PLAIN.1,
                "ComplianceTypes": [INSUFFICIENT_DATA],
            }),
        ),
        (
            "a filter naming INSUFFICIENT_DATA beside a filterable value",
            json!({
                "ResourceType": PLAIN.0,
                "ResourceId": PLAIN.1,
                "ComplianceTypes": [COMPLIANT, INSUFFICIENT_DATA],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, code) = emulator
            .refused("GetComplianceDetailsByResource", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected HTTP 400 InvalidParameterValueException, got \
             {status} {code}"
        );
    }

    // The token the operation did issue is served, so the refusals above are about the
    // tokens and not about paging at all.
    let mut resumed_request = by_resource(PLAIN, &[]);
    resumed_request["NextToken"] = json!(issued_token);
    let resumed = emulator
        .ok("GetComplianceDetailsByResource", &resumed_request)
        .await;
    let expected = expected(&plan, PLAIN, None, &[]);
    assert_eq!(
        normalised(&resumed["EvaluationResults"]),
        Value::Array(expected[GET_COMPLIANCE_DETAILS_BY_RESOURCE_PAGE_SIZE..].to_vec()),
        "the NextToken this operation issued must serve the results the first page left, \
         got {resumed}"
    );
}

#[tokio::test]
async fn reporting_a_resource_s_details_changes_nothing_a_later_caller_sees() {
    // Rows differ along what the account holds for the resource: nothing, one verdict,
    // and several rules' verdicts in both modes.
    let cases: [(&str, usize, &[&str]); 3] = [
        ("nothing evaluated", 0, &[]),
        ("one rule, one mode", 1, &[DETECTIVE]),
        ("three rules, both modes", 3, &BOTH_MODES),
    ];

    for (case, rule_count, modes) in cases {
        let plan = seed_plan(PLAIN, rule_count, modes, 2);
        let emulator = emulator_holding(&plan).await;
        let request = by_resource(PLAIN, &[]);

        let before = emulator
            .ok("GetComplianceDetailsByResource", &request)
            .await;
        let again = emulator
            .ok("GetComplianceDetailsByResource", &request)
            .await;
        assert_eq!(
            normalised(&again),
            normalised(&before),
            "case {case:?}: a second read must report the same results, got {again}"
        );

        // The results reported here are the ones the owning rule's own details report:
        // one store, read two ways.
        for (name, _) in &plan {
            let by_rule = emulator
                .ok(
                    "GetComplianceDetailsByConfigRule",
                    &json!({ "ConfigRuleName": name, "Limit": 100 }),
                )
                .await;
            let reported_by_rule = normalised(&by_rule["EvaluationResults"]);
            for result in expected(&plan, PLAIN, None, &[]) {
                let owned_by_this_rule = result["EvaluationResultIdentifier"]
                    ["EvaluationResultQualifier"]["ConfigRuleName"]
                    == json!(name);
                let carried = reported_by_rule
                    .as_array()
                    .is_some_and(|results| results.contains(&result));
                assert_eq!(
                    carried, owned_by_this_rule,
                    "case {case:?}: the rule {name:?} must report exactly its own results \
                     for the resource, got {by_rule}"
                );
            }
        }

        // Reading the details must not change what a later caller sees.
        let rules_after = emulator.ok("DescribeConfigRules", &json!({})).await;
        emulator
            .ok("GetComplianceDetailsByResource", &request)
            .await;
        assert_eq!(
            emulator.ok("DescribeConfigRules", &json!({})).await,
            rules_after,
            "case {case:?}: reporting a resource's compliance details must not change the \
             rules"
        );
    }
}
