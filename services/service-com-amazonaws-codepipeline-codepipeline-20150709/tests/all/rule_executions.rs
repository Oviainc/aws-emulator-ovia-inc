// SPDX-License-Identifier: CC0-1.0

//! `ListRuleExecutions`, over the wire the service actually speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.ListRuleExecutions`).
//!
//! The rule under test: a rule execution exists only once a stage condition's rule has
//! been evaluated, so a pipeline configured with no condition rules has none to report
//! — the listing answers one empty page and issues no cursor — while a pipeline that
//! does configure condition rules would have rule executions no plane in this emulator
//! records, so it is refused rather than answered with "none ran". Everything the
//! request names is checked against the store: the pipeline must exist and an
//! execution id in the filter must name an execution of that pipeline.
//!
//! The harness comes from the sibling suite (`tests/all/pipeline_lifecycle.rs`), so
//! every assertion here goes through the same router the production emulator builds.

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{declaration, source_stage, Emulator, ROLE_ARN};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    encode_page_token, CURSOR_EXECUTION, CURSOR_PIPELINE,
};

/// A well-formed `PipelineExecutionId` (its modelled pattern is
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no start
/// ever mints, so it passes the boundary validator and is then looked up and missed.
const UNSTORED_EXECUTION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// One form the filter can name an execution in: the case it covers, and how it
/// builds that filter around an execution id.
type FilterMember = (&'static str, fn(&str) -> Value);

/// One `RuleDeclaration`. The providers come from the rule types AWS publishes for
/// stage conditions (`DeploymentWindow`, `VariableCheck`, `LambdaInvoke`,
/// `CloudWatchAlarm`); `category` and `provider` are the two members `RuleTypeId`
/// requires, and `RuleCategory` declares the single value `Rule`.
fn rule(name: &str, provider: &str) -> Value {
    json!({
        "name": name,
        "ruleTypeId": {
            "category": "Rule",
            "owner": "AWS",
            "provider": provider,
            "version": "1",
        },
    })
}

/// A one-stage declaration whose stage carries `site` (one of `StageDeclaration`'s
/// three condition members) set to `value`.
fn declaration_with(name: &str, site: &str, value: Value) -> Value {
    let mut stage = source_stage("Source");
    stage[site] = value;
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [stage],
    })
}

/// Create `pipeline`, start `executions` executions of it, and return the pipeline's
/// name with the ids those starts minted.
async fn seed(emulator: &Emulator, pipeline: Value, executions: usize) -> (String, Vec<String>) {
    let name = pipeline["name"]
        .as_str()
        .expect("the declaration names the pipeline")
        .to_string();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": pipeline}))
        .await;
    let mut ids = Vec::with_capacity(executions);
    for _ in 0..executions {
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": name}))
            .await;
        ids.push(
            started["pipelineExecutionId"]
                .as_str()
                .expect("a started execution carries its id")
                .to_string(),
        );
    }
    (name, ids)
}

/// A pipeline configured with no condition rules has no rule execution to report,
/// whatever it otherwise looks like, however many executions it has run, whatever page
/// size is asked for and whichever execution the filter selects.
///
/// The rows differ along what the rule generalises over: how many stages the pipeline
/// has, whether a stage carries a condition member at all (and at which of the three
/// sites), how many executions the store holds for it, the page size, and the form of
/// the filter.
#[tokio::test]
async fn a_pipeline_with_no_condition_rules_reports_no_rule_execution() {
    /// The filter a case sends, built from the ids its starts minted.
    type Filter = fn(&[String]) -> Option<Value>;

    // (case, declaration, executions to start, maxResults, filter)
    let cases: Vec<(&str, Value, usize, Option<i64>, Filter)> = vec![
        (
            "one stage, no condition, no execution, no filter",
            declaration("plain"),
            0,
            None,
            |_| None,
        ),
        (
            "three stages, two executions, pipelineExecutionId filter",
            json!({
                "name": "three-stages",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [
                    source_stage("Source"),
                    source_stage("Mirror"),
                    source_stage("Archive"),
                ],
            }),
            2,
            Some(1),
            |ids| Some(json!({"pipelineExecutionId": ids[0]})),
        ),
        (
            "a beforeEntry condition that declares no rules",
            declaration_with("entry-no-rules", "beforeEntry", json!({"conditions": [{}]})),
            1,
            Some(100),
            |ids| {
                Some(json!({
                    "latestInPipelineExecution": {
                        "pipelineExecutionId": ids[0],
                        "startTimeRange": "Latest",
                    }
                }))
            },
        ),
        (
            "an onFailure result with no conditions at all",
            declaration_with(
                "failure-result-only",
                "onFailure",
                json!({"result": "ROLLBACK"}),
            ),
            3,
            None,
            |ids| {
                Some(json!({
                    "latestInPipelineExecution": {
                        "pipelineExecutionId": ids[2],
                        "startTimeRange": "All",
                    }
                }))
            },
        ),
        (
            "an onSuccess condition that declares no rules",
            declaration_with(
                "success-no-rules",
                "onSuccess",
                json!({"conditions": [{"result": "FAIL"}]}),
            ),
            1,
            Some(2),
            |_| None,
        ),
    ];

    for (case, pipeline, executions, max_results, filter) in cases {
        let emulator = Emulator::new();
        let (name, ids) = seed(&emulator, pipeline, executions).await;

        let mut request = json!({"pipelineName": name});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(filter) = filter(&ids) {
            request["filter"] = filter;
        }
        let page = emulator.ok("ListRuleExecutions", &request).await;

        assert_eq!(
            page["ruleExecutionDetails"],
            json!([]),
            "case {case:?}: expected no rule execution, got {page}"
        );
        assert!(
            page["nextToken"].is_null(),
            "case {case:?}: expected no nextToken from a page that was not truncated, got {page}"
        );
    }
}

/// `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds are
/// served, and one past either bound is refused at the boundary rather than clamped.
#[tokio::test]
async fn max_results_is_accepted_inside_its_modelled_range_and_refused_outside() {
    // (case, maxResults, expected status, expected awsJson error code — "" for success)
    let cases: [(&str, i64, u16, &str); 4] = [
        ("the minimum MaxResults allows", 1, 200, ""),
        ("the maximum MaxResults allows", 100, 200, ""),
        ("one below the minimum", 0, 400, "ValidationException"),
        ("one above the maximum", 101, 400, "ValidationException"),
    ];

    let emulator = Emulator::new();
    seed(&emulator, declaration("demo"), 1).await;

    for (case, max_results, expected_status, expected_code) in cases {
        let request = json!({"pipelineName": "demo", "maxResults": max_results});
        let (status, response) = emulator.call("ListRuleExecutions", &request).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            (status, code),
            (expected_status, expected_code),
            "case {case:?}: unexpected answer {response}"
        );
        if expected_status == 200 {
            assert_eq!(
                response["ruleExecutionDetails"],
                json!([]),
                "case {case:?}: expected no rule execution, got {response}"
            );
        }
    }
}

/// This listing never truncates a page, so it has issued no cursor and every
/// `nextToken` presented to it is one it did not issue — refused with the error the
/// model declares for that, never served as a first page.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let emulator = Emulator::new();
    let (name, ids) = seed(&emulator, declaration("demo"), 1).await;

    // Inside `NextToken`'s modelled length (1..2048), so the boundary validator passes
    // each of these to the handler: arbitrary text, a cursor of either kind this
    // service's other listings issue, and both published length bounds.
    let accepted_by_the_validator: Vec<(&str, String)> = vec![
        ("not a cursor at all", "not-a-token".to_string()),
        (
            "a ListPipelines cursor naming this pipeline",
            encode_page_token(CURSOR_PIPELINE, &name),
        ),
        (
            "a ListPipelineExecutions cursor naming a stored execution",
            encode_page_token(CURSOR_EXECUTION, &ids[0]),
        ),
        ("NextToken's minimum length", "a".to_string()),
        ("NextToken's maximum length", "a".repeat(2048)),
    ];
    for (case, token) in accepted_by_the_validator {
        let (status, code) = emulator
            .expect_error(
                "ListRuleExecutions",
                &json!({"pipelineName": name, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: an unissued nextToken must be refused as the model declares"
        );
    }

    // One past either published length bound is refused at the boundary instead.
    for (case, token) in [
        ("one below NextToken's minimum length", String::new()),
        ("one above NextToken's maximum length", "a".repeat(2049)),
    ] {
        let (status, code) = emulator
            .expect_error(
                "ListRuleExecutions",
                &json!({"pipelineName": name, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a token outside the modelled length must be refused at the boundary"
        );
    }
}

/// A pipeline the store does not hold has no rule executions to list: the listing
/// reports the modelled not-found rather than an empty page.
#[tokio::test]
async fn an_unknown_pipeline_is_not_found() {
    let emulator = Emulator::new();
    seed(&emulator, declaration("demo"), 1).await;
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("gone")}))
        .await;
    emulator.ok("DeletePipeline", &json!({"name": "gone"})).await;

    for (case, pipeline_name) in [
        ("a name never created", "missing"),
        ("a pipeline that was deleted", "gone"),
        ("a stored name in different case", "DEMO"),
    ] {
        let (status, code) = emulator
            .expect_error(
                "ListRuleExecutions",
                &json!({"pipelineName": pipeline_name}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: an unknown pipeline must not be listed as empty"
        );
    }

    // The pipeline a required member names is resolved before the optional cursor is
    // judged, so an unknown pipeline is reported as such even when the request also
    // carries a token this listing never issued.
    let (status, code) = emulator
        .expect_error(
            "ListRuleExecutions",
            &json!({"pipelineName": "missing", "nextToken": "not-a-token"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNotFoundException"),
        "an unknown pipeline is resolved before the nextToken is judged"
    );
}

/// An execution id in the filter must name an execution of the pipeline being listed:
/// `PipelineExecutionNotFoundException` is documented for an execution that "cannot be
/// found, or an execution ID does not belong to the specified pipeline", and both
/// filter members that name an execution are checked the same way.
#[tokio::test]
async fn a_filter_execution_id_must_name_an_execution_of_that_pipeline() {
    let emulator = Emulator::new();
    let (name, ids) = seed(&emulator, declaration("demo"), 1).await;
    let (_, other_ids) = seed(&emulator, declaration("other"), 1).await;

    // (case, the id the filter names)
    let unusable_ids = [
        ("an id no start ever minted", UNSTORED_EXECUTION_ID.to_string()),
        ("another pipeline's execution", other_ids[0].clone()),
    ];
    // (case, the filter member that names it)
    let members: [FilterMember; 2] = [
        ("pipelineExecutionId", |id| {
            json!({"pipelineExecutionId": id})
        }),
        ("latestInPipelineExecution", |id| {
            json!({
                "latestInPipelineExecution": {
                    "pipelineExecutionId": id,
                    "startTimeRange": "Latest",
                }
            })
        }),
    ];

    for (id_case, id) in &unusable_ids {
        for (member_case, build) in &members {
            let (status, code) = emulator
                .expect_error(
                    "ListRuleExecutions",
                    &json!({"pipelineName": name, "filter": build(id)}),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "PipelineExecutionNotFoundException"),
                "case {id_case:?} via {member_case:?}: unexpected refusal"
            );
        }
    }

    // The pipeline's own execution is accepted through both members.
    for (member_case, build) in &members {
        let page = emulator
            .ok(
                "ListRuleExecutions",
                &json!({"pipelineName": name, "filter": build(&ids[0])}),
            )
            .await;
        assert_eq!(
            page["ruleExecutionDetails"],
            json!([]),
            "case {member_case:?}: this pipeline's own execution must be accepted, got {page}"
        );
    }
}

/// A pipeline that DOES configure condition rules is refused as not implemented: the
/// real service reports the executions of those rules, and nothing in this emulator
/// evaluates one, so an empty list would be a claim that none ran.
///
/// The rows span every site a stage can carry conditions at, and both ends of
/// `RuleDeclarationList`'s modelled length (1..5).
#[tokio::test]
async fn a_pipeline_that_declares_condition_rules_is_refused_as_not_implemented() {
    // (case, condition site, the value that site carries)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "one beforeEntry rule",
            "beforeEntry",
            json!({"conditions": [{"rules": [rule("Window", "DeploymentWindow")]}]}),
        ),
        (
            "one onSuccess rule",
            "onSuccess",
            json!({
                "conditions": [{
                    "result": "FAIL",
                    "rules": [rule("Check", "VariableCheck")],
                }]
            }),
        ),
        (
            "one onFailure rule",
            "onFailure",
            json!({
                "result": "ROLLBACK",
                "conditions": [{"rules": [rule("Alarm", "CloudWatchAlarm")]}],
            }),
        ),
        (
            "the five rules RuleDeclarationList allows at most",
            "beforeEntry",
            json!({
                "conditions": [{
                    "rules": [
                        rule("One", "DeploymentWindow"),
                        rule("Two", "VariableCheck"),
                        rule("Three", "CloudWatchAlarm"),
                        rule("Four", "LambdaInvoke"),
                        rule("Five", "DeploymentWindow"),
                    ],
                }]
            }),
        ),
    ];

    for (case, site, value) in cases {
        let emulator = Emulator::new();
        let (name, _) = seed(&emulator, declaration_with("ruled", site, value), 1).await;
        let (status, code) = emulator
            .expect_error("ListRuleExecutions", &json!({"pipelineName": name}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: rule executions this emulator never recorded must not be \
             reported as none"
        );

        // The refusal covers the whole call: a cursor for a listing this emulator does
        // not record is one it cannot judge, so it is not reported as malformed.
        let (status, code) = emulator
            .expect_error(
                "ListRuleExecutions",
                &json!({"pipelineName": name, "nextToken": "not-a-token"}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: the unimplemented plane is reported before a cursor is judged"
        );
    }
}

/// Listing rule executions reports the same thing twice and changes nothing a later
/// caller sees — it starts no execution and touches no stored pipeline.
#[tokio::test]
async fn listing_rule_executions_changes_nothing() {
    let emulator = Emulator::new();
    let (name, ids) = seed(&emulator, declaration("demo"), 1).await;
    let request = json!({
        "pipelineName": name,
        "filter": {"pipelineExecutionId": ids[0]},
    });

    let before = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": name}))
        .await;
    let first = emulator.ok("ListRuleExecutions", &request).await;
    let second = emulator.ok("ListRuleExecutions", &request).await;
    assert_eq!(
        first, second,
        "a read must report the same thing twice, got {first} then {second}"
    );

    let after = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": name}))
        .await;
    assert_eq!(
        before, after,
        "listing rule executions must not change the executions a later caller sees"
    );
}
