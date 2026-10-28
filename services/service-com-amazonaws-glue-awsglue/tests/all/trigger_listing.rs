// SPDX-License-Identifier: CC0-1.0

//! `ListTriggers`: the names of the trigger resources this account holds.
//!
//! RULE: `ListTriggers` "retrieves the names of all trigger resources in this Amazon Web
//! Services account, or the resources with the specified tag" — so it reports the name of
//! every trigger the endpoint's Region holds, from the same records `CreateTrigger` wrote
//! and `GetTrigger` reads back, in name order, one `MaxResults`-sized page at a time with a
//! `NextToken` that resumes the walk, narrowed to the triggers carrying every tag a `Tags`
//! filter names, and — when `DependentJobName` is sent — to the triggers that can START
//! that job, "and if there is no such trigger, all triggers are returned".
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many triggers are stored, the
//! page size asked for (including the published bounds 1 and 200 of
//! `ListTriggersRequest$MaxResults` and a size past the stored count), which tags the
//! filter names, which job the dependency filter names and how many triggers start it, and
//! which Region's endpoint is asked.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, ok, REGION};

/// The role ARN the jobs these triggers act on are created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The job every trigger in the paging and tagging suites runs. A trigger's action names a
/// job of the Region the create is served in, so each suite registers it first.
const JOB: &str = "trigger-listing-job";

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

/// Create one `ON_DEMAND` trigger through `region`'s endpoint, acting on the jobs `jobs`
/// names and carrying the tags `tags` names (`Value::Null` for none).
async fn given_trigger_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    jobs: &[&str],
    tags: Value,
) {
    let actions: Vec<Value> = jobs.iter().map(|job| json!({ "JobName": job })).collect();
    let mut body = json!({
        "Name": name,
        // "True is not supported for ON_DEMAND triggers": the type a create never starts,
        // so no row depends on a trigger's state.
        "Type": "ON_DEMAND",
        "Actions": actions,
    });
    if !tags.is_null() {
        body["Tags"] = tags;
    }
    let (status, answer) = call_in_region(router, region, "CreateTrigger", &body).await;
    assert_eq!(
        status, 200,
        "case {region} / {name}: CreateTrigger must answer 200, got {status} with {answer}"
    );
}

/// The `TriggerNames` of one answer, in the order the service returned them.
fn trigger_names(page: &Value) -> Vec<String> {
    page["TriggerNames"]
        .as_array()
        .expect("TriggerNames is a list")
        .iter()
        .map(|name| {
            name.as_str()
                .expect("a trigger name is a string")
                .to_string()
        })
        .collect()
}

/// Every name a PAGED `ListTriggers` sweep of `region`'s endpoint reports for `request`,
/// following the service's own `NextToken` to exhaustion. `bound` is how many triggers the
/// Region holds, so a token that never terminated is caught instead of looping.
async fn sweep_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    request: &Value,
    bound: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(cursor) = token.as_ref() {
            body["NextToken"] = json!(cursor);
        }
        let (status, answer) = call_in_region(router, region, "ListTriggers", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTriggers must answer 200, got {status} with {answer}"
        );
        let names = trigger_names(&answer);
        if let Some(size) = request.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                names.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {names:?}"
            );
        }
        seen.extend(names);
        token = answer
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= bound,
            "case {case:?}: the walk must terminate, got {seen:?} for {bound} triggers"
        );
    }
    seen
}

/// Every stored trigger is listed, in name order, and a paged walk over the service's own
/// token reports each name exactly once — whatever the page size.
///
/// The rows differ along how many triggers are stored and the page size asked for, spanning
/// the published bounds of `MaxResults` and a page larger than the stored count.
#[tokio::test]
async fn every_stored_trigger_is_listed_and_paged() {
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
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("trigger-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_trigger_in(&router, REGION, name, &[JOB], Value::Null).await;
        }

        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        let listed = sweep_in(&router, REGION, case, &request, *stored).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored trigger name in name order, got {listed:?}"
        );

        // A name the listing reports is a trigger `GetTrigger` resolves: one record, two
        // reads.
        for name in &listed {
            let read = ok(&router, "GetTrigger", &json!({ "Name": name })).await;
            assert_eq!(
                read["Trigger"]["Name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: GetTrigger({name}) must report the trigger the listing \
                 named, got {read}"
            );
        }
    }
}

/// A `Tags` filter narrows the listing to the triggers carrying EVERY pair it names.
#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_triggers_carrying_every_pair() {
    let router = emulator();
    given_job_in(&router, REGION, JOB).await;
    given_trigger_in(
        &router,
        REGION,
        "tagged-both",
        &[JOB],
        json!({"team": "data", "env": "prod"}),
    )
    .await;
    given_trigger_in(&router, REGION, "tagged-team", &[JOB], json!({"team": "data"})).await;
    given_trigger_in(&router, REGION, "tagged-other", &[JOB], json!({"team": "ops"})).await;
    given_trigger_in(&router, REGION, "untagged", &[JOB], Value::Null).await;

    let everything: &[&str] = &["tagged-both", "tagged-other", "tagged-team", "untagged"];
    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        ("no filter: every trigger", Value::Null, everything),
        ("an empty filter narrows nothing", json!({}), everything),
        (
            "one pair",
            json!({"team": "data"}),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({"team": "data", "env": "prod"}),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({"team": "finance"}), &[]),
        ("a key nothing carries", json!({"owner": "data"}), &[]),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let listed = sweep_in(&router, REGION, case, &request, everything.len()).await;
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the triggers carrying every pair of {filter}, got \
             {listed:?}"
        );
    }
}

/// `DependentJobName` narrows the listing to the triggers that can START the named job —
/// the triggers one of whose actions runs it — and falls back to the whole listing when no
/// trigger can.
///
/// The rows differ along how many triggers start the named job (two, one, none), whether
/// the job exists at all, whether the job is only WAITED ON by a predicate rather than
/// started by an action, and whether a tag filter is combined with the dependency.
#[tokio::test]
async fn a_dependent_job_narrows_the_listing_to_the_triggers_that_start_it() {
    let router = emulator();
    for job in ["job-alpha", "job-beta", "job-gamma", "job-unused"] {
        given_job_in(&router, REGION, job).await;
    }

    given_trigger_in(
        &router,
        REGION,
        "starts-alpha",
        &["job-alpha"],
        json!({"env": "prod"}),
    )
    .await;
    given_trigger_in(
        &router,
        REGION,
        "starts-alpha-and-beta",
        &["job-alpha", "job-beta"],
        json!({"env": "dev"}),
    )
    .await;
    given_trigger_in(&router, REGION, "starts-beta", &["job-beta"], Value::Null).await;
    // A CONDITIONAL trigger that WAITS ON job-alpha and starts job-gamma: a condition's
    // job is not a job the trigger can start, so this one answers only for job-gamma.
    let (status, answer) = call(
        &router,
        "CreateTrigger",
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
    assert_eq!(
        status, 200,
        "the conditional trigger must be created, got {status} with {answer}"
    );

    let everything: &[&str] = &[
        "starts-alpha",
        "starts-alpha-and-beta",
        "starts-beta",
        "waits-on-alpha",
    ];
    // (case, the DependentJobName, the Tags filter, the names it must report)
    let rows: &[(&str, Option<&str>, Value, &[&str])] = &[
        ("no dependency filter", None, Value::Null, everything),
        (
            "a job two triggers start",
            Some("job-alpha"),
            Value::Null,
            &["starts-alpha", "starts-alpha-and-beta"],
        ),
        (
            "a job one trigger starts, named only by its action",
            Some("job-gamma"),
            Value::Null,
            &["waits-on-alpha"],
        ),
        (
            "a job two triggers start, one of them among several actions",
            Some("job-beta"),
            Value::Null,
            &["starts-alpha-and-beta", "starts-beta"],
        ),
        (
            "a stored job no trigger starts: all triggers are returned",
            Some("job-unused"),
            Value::Null,
            everything,
        ),
        (
            "a job this account does not hold: all triggers are returned",
            Some("no-such-job"),
            Value::Null,
            everything,
        ),
        (
            "a dependency and a tag filter together",
            Some("job-alpha"),
            json!({"env": "prod"}),
            &["starts-alpha"],
        ),
        (
            // `Tags` decides WHICH triggers the listing is over, so the fallback of
            // "all triggers are returned" is over that narrowed set too.
            "a tag filter plus a job nothing in it starts",
            Some("job-beta"),
            json!({"env": "prod"}),
            &["starts-alpha"],
        ),
    ];

    for (case, dependent_job, filter, expected) in rows {
        let mut request = json!({});
        if let Some(job) = dependent_job {
            request["DependentJobName"] = json!(job);
        }
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let listed = sweep_in(&router, REGION, case, &request, everything.len()).await;
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the triggers that start {dependent_job:?} under \
             {filter}, got {listed:?}"
        );
    }

    // The same answer one page at a time: the dependency filter survives the walk.
    let paged = sweep_in(
        &router,
        REGION,
        "a job two triggers start, one name per page",
        &json!({ "DependentJobName": "job-alpha", "MaxResults": 1 }),
        everything.len(),
    )
    .await;
    assert_eq!(
        paged,
        vec![
            "starts-alpha".to_string(),
            "starts-alpha-and-beta".to_string()
        ],
        "a paged walk must report the same set as one page, got {paged:?}"
    );
}

/// A trigger is served only through the endpoint that created it, so one Region's listing
/// never names another Region's triggers.
#[tokio::test]
async fn a_listing_names_only_the_endpoints_own_triggers() {
    let router = emulator();
    // The Regions span three partitions' worth of prefixes, so nothing here depends on one
    // Region name.
    let regions = [REGION, "us-west-2", "eu-west-1", "ap-southeast-1"];

    for region in regions {
        given_job_in(&router, region, JOB).await;
        given_trigger_in(&router, region, &format!("trigger-{region}"), &[JOB], Value::Null).await;
    }

    for region in regions {
        let listed = sweep_in(&router, region, region, &json!({}), regions.len()).await;
        assert_eq!(
            listed,
            vec![format!("trigger-{region}")],
            "case {region:?}: an endpoint must name its own triggers and no other Region's, \
             got {listed:?}"
        );
    }
}

/// A page size outside the published range is refused by the modelled constraint rather
/// than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_job_in(&router, REGION, JOB).await;
    given_trigger_in(&router, REGION, "bounded-trigger", &[JOB], Value::Null).await;

    // `MaxResults` is `OrchestrationPageSize200`, range 1..200: one past each published
    // bound is refused.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 201)] {
        let (status, body) = call(&router, "ListTriggers", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
