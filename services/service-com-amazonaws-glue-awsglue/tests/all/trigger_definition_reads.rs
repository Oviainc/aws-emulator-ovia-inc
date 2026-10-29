// SPDX-License-Identifier: CC0-1.0

//! `GetTriggers`: the trigger DEFINITIONS this account holds, not just their names.
//!
//! RULE: `GetTriggers` "gets all the triggers associated with a job" — it reports the
//! stored `Trigger` definition of every trigger the endpoint's Region holds, the same
//! record `CreateTrigger` wrote and `GetTrigger` reads back, in name order, one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk, narrowed
//! when `DependentJobName` is sent to the triggers that can START that job — "the trigger
//! that can start this job is returned, and if there is no such trigger, all triggers are
//! returned" — and it changes nothing a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many triggers are stored, the
//! page size asked for (including the published bounds 1 and 200 of
//! `GetTriggersRequest$MaxResults` and a size past the stored count), the trigger TYPE and
//! so which definition members carry a value, which job the dependency filter names and how
//! many triggers start it, and which Region's endpoint is asked.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, ok, REGION};

/// The role ARN the jobs these triggers act on are created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-trigger-role";

/// The job every trigger in the paging suite runs. A trigger's action names a job of the
/// Region the create is served in, so each suite registers it first.
const JOB: &str = "trigger-definition-job";

/// Create the job `name` through `region`'s endpoint, so an action may name it.
async fn given_job_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, body) = call_in_region(
        router,
        region,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region} / {name}: CreateJob must answer 200, got {status} with {body}"
    );
}

/// Create the trigger `request` describes through `region`'s endpoint.
async fn given_trigger_in(router: &MultiServiceRouter, region: &str, request: &Value) {
    let (status, answer) = call_in_region(router, region, "CreateTrigger", request).await;
    assert_eq!(
        status, 200,
        "case {region} / {request}: CreateTrigger must answer 200, got {status} with {answer}"
    );
}

/// One `ON_DEMAND` trigger definition acting on the jobs `jobs` names.
fn on_demand(name: &str, jobs: &[&str]) -> Value {
    let actions: Vec<Value> = jobs.iter().map(|job| json!({ "JobName": job })).collect();
    json!({ "Name": name, "Type": "ON_DEMAND", "Actions": actions })
}

/// The `Triggers` of one answer, in the order the service returned them.
fn triggers(page: &Value) -> Vec<Value> {
    page["Triggers"]
        .as_array()
        .expect("Triggers is a list")
        .clone()
}

/// The `Name` of each definition in one answer.
fn names_of(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|trigger| {
            trigger["Name"]
                .as_str()
                .expect("Trigger.Name is a string")
                .to_string()
        })
        .collect()
}

/// Every definition a PAGED `GetTriggers` sweep of `region`'s endpoint reports for
/// `request`, following the service's own `NextToken` to exhaustion. `bound` is how many
/// triggers the Region holds, so a token that never terminated is caught instead of looping.
async fn sweep_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    request: &Value,
    bound: usize,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(cursor) = token.as_ref() {
            body["NextToken"] = json!(cursor);
        }
        let (status, answer) = call_in_region(router, region, "GetTriggers", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTriggers must answer 200, got {status} with {answer}"
        );
        let page = triggers(&answer);
        if let Some(size) = request.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                page.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {:?}",
                names_of(&page)
            );
        }
        seen.extend(page);
        token = answer
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= bound,
            "case {case:?}: the walk must terminate, got {:?} for {bound} triggers",
            names_of(&seen)
        );
    }
    seen
}

/// Every stored trigger's DEFINITION is reported, in name order, and a paged walk over the
/// service's own token reports each one exactly once — whatever the page size. Each
/// definition is the very record `GetTrigger` reads back for that name.
///
/// The rows differ along how many triggers are stored and the page size asked for, spanning
/// the published bounds of `MaxResults` and a page larger than the stored count.
#[tokio::test]
async fn every_stored_trigger_definition_is_reported_and_paged() {
    // (case, how many triggers are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no triggers at all", 0, None),
        ("one trigger, no page size", 1, None),
        // The published lower bound of `MaxResults` (`OrchestrationPageSize200`, 1..200).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(200)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        given_job_in(&router, REGION, JOB).await;
        // Created out of name order, so the read's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("trigger-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_trigger_in(&router, REGION, &on_demand(name, &[JOB])).await;
        }

        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        let reported = sweep_in(&router, REGION, case, &request, *stored).await;
        assert_eq!(
            names_of(&reported),
            expected,
            "case {case:?}: expected every stored trigger in name order, got {:?}",
            names_of(&reported)
        );

        // One record, two reads: what this call projects for a name is exactly what
        // `GetTrigger` projects for it.
        for definition in &reported {
            let name = definition["Name"].as_str().expect("a name");
            let read = ok(&router, "GetTrigger", &json!({ "Name": name })).await;
            assert_eq!(
                &read["Trigger"], definition,
                "case {case:?}: GetTrigger({name}) must report the definition GetTriggers \
                 reported, got {read}"
            );
        }

        // A reporting read changes nothing a later caller sees.
        let again = sweep_in(&router, REGION, case, &request, *stored).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same definitions, got {:?}",
            names_of(&again)
        );
    }
}

/// The definition reported for a trigger carries every member its create sent, whatever the
/// trigger's TYPE — so a `SCHEDULED` trigger reports its `Schedule` and a `CONDITIONAL` one
/// its `Predicate`, beside the actions and workflow each belongs to.
#[tokio::test]
async fn a_reported_definition_carries_the_members_its_type_requires() {
    let router = emulator();
    given_job_in(&router, REGION, "definition-job").await;
    let workflow = ok(
        &router,
        "CreateWorkflow",
        &json!({ "Name": "definition-workflow" }),
    )
    .await;
    assert_eq!(
        workflow["Name"].as_str(),
        Some("definition-workflow"),
        "the workflow the triggers belong to must be created, got {workflow}"
    );

    // (case, the create request, the members the definition must report)
    let rows: &[(&str, Value, Value)] = &[
        (
            // "True is not supported for ON_DEMAND triggers": a create never starts one.
            "an on-demand trigger",
            json!({
                "Name": "def-on-demand",
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": "definition-job" }],
                "Description": "started by a caller",
            }),
            json!({
                "Name": "def-on-demand",
                "Type": "ON_DEMAND",
                "State": "CREATED",
                "Description": "started by a caller",
                "Actions": [{ "JobName": "definition-job" }],
                "Schedule": null,
                "Predicate": null,
                "WorkflowName": null,
            }),
        ),
        (
            // "This field is required when the trigger type is SCHEDULED" (`Schedule`).
            "a scheduled trigger inside a workflow",
            json!({
                "Name": "def-scheduled",
                "Type": "SCHEDULED",
                "Schedule": "cron(0 12 * * ? *)",
                "WorkflowName": "definition-workflow",
                "Actions": [{
                    "JobName": "definition-job",
                    "Arguments": { "--mode": "full" },
                    "Timeout": 30,
                }],
            }),
            json!({
                "Name": "def-scheduled",
                "Type": "SCHEDULED",
                "State": "CREATED",
                "Schedule": "cron(0 12 * * ? *)",
                "WorkflowName": "definition-workflow",
                "Actions": [{
                    "JobName": "definition-job",
                    "Arguments": { "--mode": "full" },
                    "Timeout": 30,
                }],
                "Predicate": null,
            }),
        ),
        (
            // "...is CONDITIONAL" (`Predicate`), and a create that asks to be started
            // leaves an armed trigger in `ACTIVATED`.
            "a conditional trigger started on creation",
            json!({
                "Name": "def-conditional",
                "Type": "CONDITIONAL",
                "StartOnCreation": true,
                "Actions": [{ "JobName": "definition-job" }],
                "Predicate": {
                    "Logical": "ANY",
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": "definition-job",
                        "State": "SUCCEEDED",
                    }],
                },
            }),
            json!({
                "Name": "def-conditional",
                "Type": "CONDITIONAL",
                "State": "ACTIVATED",
                "Schedule": null,
                "Predicate": {
                    "Logical": "ANY",
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": "definition-job",
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "JobName": "definition-job" }],
            }),
        ),
    ];

    for (_, create, _) in rows {
        given_trigger_in(&router, REGION, create).await;
    }

    let reported = sweep_in(
        &router,
        REGION,
        "every type in one read",
        &json!({}),
        rows.len(),
    )
    .await;
    assert_eq!(
        names_of(&reported),
        vec![
            "def-conditional".to_string(),
            "def-on-demand".to_string(),
            "def-scheduled".to_string()
        ],
        "every trigger must be reported in name order, got {:?}",
        names_of(&reported)
    );

    for (case, create, expected) in rows {
        let name = create["Name"].as_str().expect("a name");
        let definition = reported
            .iter()
            .find(|trigger| trigger["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case:?}: {name} must be reported in {reported:?}"));
        crate::wire::assert_echoed(case, expected, definition);
    }
}

/// `DependentJobName` narrows the read to the triggers that can START the named job — the
/// triggers one of whose actions runs it — and falls back to every trigger when none can.
///
/// The rows differ along how many triggers start the named job (two, one, none), whether the
/// job exists at all, and whether the job is only WAITED ON by a predicate rather than
/// started by an action.
#[tokio::test]
async fn a_dependent_job_narrows_the_read_to_the_triggers_that_start_it() {
    let router = emulator();
    for job in ["job-alpha", "job-beta", "job-gamma", "job-unused"] {
        given_job_in(&router, REGION, job).await;
    }

    given_trigger_in(&router, REGION, &on_demand("starts-alpha", &["job-alpha"])).await;
    given_trigger_in(
        &router,
        REGION,
        &on_demand("starts-alpha-and-beta", &["job-alpha", "job-beta"]),
    )
    .await;
    given_trigger_in(&router, REGION, &on_demand("starts-beta", &["job-beta"])).await;
    // A CONDITIONAL trigger that WAITS ON job-alpha and starts job-gamma: a condition's job
    // is not a job the trigger can start, so this one answers only for job-gamma.
    given_trigger_in(
        &router,
        REGION,
        &json!({
            "Name": "waits-on-alpha",
            "Type": "CONDITIONAL",
            "Actions": [{ "JobName": "job-gamma" }],
            "Predicate": {
                "Conditions": [{
                    "LogicalOperator": "EQUALS",
                    "JobName": "job-alpha",
                    "State": "SUCCEEDED",
                }],
            },
        }),
    )
    .await;

    let everything: &[&str] = &[
        "starts-alpha",
        "starts-alpha-and-beta",
        "starts-beta",
        "waits-on-alpha",
    ];
    // (case, the DependentJobName, the names the read must report)
    let rows: &[(&str, Option<&str>, &[&str])] = &[
        ("no dependency filter", None, everything),
        (
            "a job two triggers start",
            Some("job-alpha"),
            &["starts-alpha", "starts-alpha-and-beta"],
        ),
        (
            "a job one trigger starts, named only by its action",
            Some("job-gamma"),
            &["waits-on-alpha"],
        ),
        (
            "a job two triggers start, one of them among several actions",
            Some("job-beta"),
            &["starts-alpha-and-beta", "starts-beta"],
        ),
        (
            "a stored job no trigger starts: all triggers are returned",
            Some("job-unused"),
            everything,
        ),
        (
            "a job this account does not hold: all triggers are returned",
            Some("no-such-job"),
            everything,
        ),
    ];

    for (case, dependent_job, expected) in rows {
        let mut request = json!({});
        if let Some(job) = dependent_job {
            request["DependentJobName"] = json!(job);
        }
        let reported = sweep_in(&router, REGION, case, &request, everything.len()).await;
        assert_eq!(
            names_of(&reported),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the triggers that start {dependent_job:?}, got {:?}",
            names_of(&reported)
        );

        // The same answer one definition per page: the filter survives the walk.
        let mut paged_request = request.clone();
        paged_request["MaxResults"] = json!(1);
        let paged = sweep_in(&router, REGION, case, &paged_request, everything.len()).await;
        assert_eq!(
            names_of(&paged),
            names_of(&reported),
            "case {case:?}: a paged walk must report the same set as one page, got {:?}",
            names_of(&paged)
        );
    }
}

/// A trigger is served only through the endpoint that created it, so one Region's read never
/// reports another Region's triggers.
#[tokio::test]
async fn a_read_reports_only_the_endpoints_own_triggers() {
    let router = emulator();
    // The Regions span three partitions' worth of prefixes, so nothing here depends on one
    // Region name.
    let regions = [REGION, "us-west-2", "eu-west-1", "ap-southeast-1"];

    for region in regions {
        given_job_in(&router, region, JOB).await;
        given_trigger_in(
            &router,
            region,
            &on_demand(&format!("trigger-{region}"), &[JOB]),
        )
        .await;
    }

    for region in regions {
        let reported = sweep_in(&router, region, region, &json!({}), regions.len()).await;
        assert_eq!(
            names_of(&reported),
            vec![format!("trigger-{region}")],
            "case {region:?}: an endpoint must report its own triggers and no other Region's, \
             got {:?}",
            names_of(&reported)
        );
    }
}

/// A page size outside the published range is refused by the modelled constraint rather than
/// served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_job_in(&router, REGION, JOB).await;
    given_trigger_in(&router, REGION, &on_demand("bounded-trigger", &[JOB])).await;

    // `MaxResults` is `OrchestrationPageSize200`, range 1..200: one past each published
    // bound is refused.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 201)] {
        let (status, body) = call(&router, "GetTriggers", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
