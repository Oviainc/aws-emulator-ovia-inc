// SPDX-License-Identifier: CC0-1.0

//! The ONE paged-walk rule both of this service's paginated reads page by, driven
//! end-to-end through each of them.
//!
//! The rule pinned here: a page reports the next entries of an ordered collection
//! starting STRICTLY AFTER the key the previous page stopped at, is capped at the
//! effective `MaxResults`, and hands back a `NextToken` exactly while entries remain —
//! so the pages of a walk reassemble the unpaged read exactly once, in order, whatever
//! the page size. `DescribeScalingPlans` walks the stored plans by plan name and
//! `DescribeScalingPlanResources` walks one plan's covered resources by the
//! `(ServiceNamespace, ResourceId, ScalableDimension)` identity that addresses each
//! (`smithy_query(query="operation",operation="DescribeScalingPlans,DescribeScalingPlanResources")`
//! — both carry `MaxResults`/`NextToken` and bind `InvalidNextTokenException`), and both
//! reach that rule through the single owner of it, `crate::state::take_page`.
//!
//! Rows differ along what the rule generalises over: the NUMBER of entries walked, the
//! PAGE SIZE (one, several, every entry at once, and the documented maximum), and
//! whether the collection is shorter than, equal to or longer than a page. Each row
//! drives both operations, so one shared walk is exercised from both of its callers.
//!
//! The token namespace is per operation, so the last row also checks that a token one
//! read minted is not a cursor the other accepts.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::MAX_RESULTS_MAX;

use crate::scaling_plan_lifecycle::{
    instruction, resource_identity, stack_source, Emulator, ResourceIdentity,
};

/// The `ServiceNamespace` and the one `ScalableDimension` it owns that the walked
/// resources are scaled on
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension")`).
const NAMESPACE: &str = "autoscaling";
const DIMENSION: &str = "autoscaling:autoScalingGroup:DesiredCapacity";

/// The name of the `n`th plan a row creates. Zero-padded, so the plan names sort in the
/// order they are created in and a row can state the page it expects.
fn plan_name(n: usize) -> String {
    format!("plan-{n:02}")
}

/// The resource id of the `n`th Auto Scaling group a plan covers, carrying the prefix an
/// `autoscaling:autoScalingGroup:DesiredCapacity` resource id has (from the worked
/// examples in `smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`)
/// and zero-padded for the same reason as [`plan_name`].
fn asg_id(plan: usize, n: usize) -> String {
    format!("autoScalingGroup/asg-{plan:02}-{n:02}")
}

/// One scaling instruction covering the `n`th Auto Scaling group of plan `plan`.
fn asg_instruction(plan: usize, n: usize) -> Value {
    instruction(
        NAMESPACE,
        &asg_id(plan, n),
        DIMENSION,
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )
}

/// One page of the plans: the names it reports and the token it hands back.
async fn plans_page(
    emulator: &Emulator,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let body = emulator
        .ok("DescribeScalingPlans", request(json!({}), max_results, token))
        .await;
    let reported = body["ScalingPlans"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no ScalingPlans in {body:?}"));
    let names = reported
        .iter()
        .map(|plan| plan["ScalingPlanName"].as_str().unwrap_or_default().to_string())
        .collect();
    (names, next_token(&body))
}

/// One page of `plan`'s covered resources: the identities it reports and the token it
/// hands back.
async fn resources_page(
    emulator: &Emulator,
    plan: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (Vec<ResourceIdentity>, Option<String>) {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            request(
                json!({ "ScalingPlanName": plan, "ScalingPlanVersion": 1 }),
                max_results,
                token,
            ),
        )
        .await;
    let reported = body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no ScalingPlanResources in {body:?}"));
    (
        reported.iter().map(resource_identity).collect(),
        next_token(&body),
    )
}

/// `base` with the paging members the row asks for. `max_results` absent asks for the
/// documented default page.
fn request(mut base: Value, max_results: Option<usize>, token: Option<&str>) -> Value {
    if let Some(max_results) = max_results {
        base["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        base["NextToken"] = json!(token);
    }
    base
}

/// The `NextToken` of a response, absent where the page continues nowhere.
fn next_token(body: &Value) -> Option<String> {
    body.get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row: how many plans it creates, how many resources each plan covers, and the page
/// size both walks use.
struct PageCase {
    label: &'static str,
    plans: usize,
    resources: usize,
    page_size: usize,
}

/// The table both walks span.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            label: "one entry, one per page: the collection is exactly one page",
            plans: 1,
            resources: 1,
            page_size: 1,
        },
        PageCase {
            label: "one entry per page over several entries",
            plans: 4,
            resources: 3,
            page_size: 1,
        },
        PageCase {
            label: "two per page over an odd count, so the last page is short",
            plans: 5,
            resources: 5,
            page_size: 2,
        },
        PageCase {
            label: "the page is exactly the collection",
            plans: 3,
            resources: 3,
            page_size: 3,
        },
        PageCase {
            label: "the page is wider than the collection",
            plans: 2,
            resources: 2,
            page_size: 7,
        },
        PageCase {
            label: "the documented maximum page covers everything at once",
            plans: 6,
            resources: 4,
            page_size: MAX_RESULTS_MAX as usize,
        },
        PageCase {
            label: "three per page over a count one short of two full pages",
            plans: 5,
            resources: 7,
            page_size: 3,
        },
    ]
}

/// Create `plans` plans, each covering `resources` Auto Scaling groups, and let the clock
/// apply them so every plan is active and every resource it covers is reported.
async fn create_plans(emulator: &Emulator, plans: usize, resources: usize) {
    for plan in 0..plans {
        let instructions: Vec<Value> = (0..resources).map(|n| asg_instruction(plan, n)).collect();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": plan_name(plan),
                    "ApplicationSource": stack_source(plan),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
    }
    emulator.make_all_due();
}

/// Walk one paginated read page by page from the first page on, and return the entries it
/// reported in order.
///
/// `fetch` reads one page; `pages` bounds the walk, so a cursor that stands still fails
/// the case instead of looping forever. Each page is asserted against the rule: capped at
/// the page size, non-empty while it continues, and continuing exactly while entries
/// remain.
async fn walk<T, F, Fut>(case: &str, operation: &str, total: usize, page_size: usize, fetch: F) -> Vec<T>
where
    T: std::fmt::Debug,
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = (Vec<T>, Option<String>)>,
{
    let mut seen: Vec<T> = Vec::new();
    let mut token: Option<String> = None;
    let pages = total / page_size.max(1) + 2;
    for index in 0..pages {
        let (entries, next) = fetch(token.clone()).await;
        assert!(
            entries.len() <= page_size,
            "case {case:?} {operation} page {index}: MaxResults {page_size} must cap the \
             page, got {entries:?}"
        );
        seen.extend(entries);
        let remaining = total.saturating_sub(seen.len());
        assert_eq!(
            next.is_some(),
            remaining > 0,
            "case {case:?} {operation} page {index}: a token is handed back exactly while \
             entries remain ({remaining} left), got {next:?}"
        );
        token = next;
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case:?} {operation}: the walk must end within {pages} pages, got {seen:?}");
}

#[tokio::test]
async fn both_paginated_reads_walk_their_collection_exactly_once_whatever_the_page_size() {
    for case in page_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        create_plans(&emulator, case.plans, case.resources).await;

        // The plans, unpaged: every created plan once, in plan-name order.
        let (unpaged_plans, unpaged_plans_token) = plans_page(&emulator, None, None).await;
        let expected_plans: Vec<String> = (0..case.plans).map(plan_name).collect();
        assert_eq!(
            unpaged_plans, expected_plans,
            "case {label:?}: an unpaged read must report every plan once in name order, got \
             {unpaged_plans:?}"
        );
        assert!(
            unpaged_plans_token.is_none(),
            "case {label:?}: a page that covered every plan continues nowhere, got \
             {unpaged_plans_token:?}"
        );

        // The walk over the plans, at this row's page size, reassembles that read.
        let walked_plans = walk(label, "DescribeScalingPlans", case.plans, case.page_size, |token| {
            let emulator = &emulator;
            async move { plans_page(emulator, Some(case.page_size), token.as_deref()).await }
        })
        .await;
        assert_eq!(
            walked_plans, unpaged_plans,
            "case {label:?}: the pages of the plan walk must reassemble the unpaged read \
             exactly, got {walked_plans:?}"
        );

        // The same rule over the resources of a plan: the walk reassembles the unpaged
        // read of the same collection, cut in different places.
        let plan = plan_name(case.plans - 1);
        let (unpaged_resources, unpaged_resources_token) =
            resources_page(&emulator, &plan, None, None).await;
        let expected_resources: Vec<ResourceIdentity> = (0..case.resources)
            .map(|n| {
                (
                    NAMESPACE.to_string(),
                    asg_id(case.plans - 1, n),
                    DIMENSION.to_string(),
                )
            })
            .collect();
        assert_eq!(
            unpaged_resources, expected_resources,
            "case {label:?}: an unpaged read must report every covered resource once in \
             identity order, got {unpaged_resources:?}"
        );
        assert!(
            unpaged_resources_token.is_none(),
            "case {label:?}: a page that covered every resource continues nowhere, got \
             {unpaged_resources_token:?}"
        );

        let walked_resources = walk(
            label,
            "DescribeScalingPlanResources",
            case.resources,
            case.page_size,
            |token| {
                let (emulator, plan) = (&emulator, plan.as_str());
                async move {
                    resources_page(emulator, plan, Some(case.page_size), token.as_deref()).await
                }
            },
        )
        .await;
        assert_eq!(
            walked_resources, unpaged_resources,
            "case {label:?}: the pages of the resource walk must reassemble the unpaged read \
             exactly, got {walked_resources:?}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_strictly_after_the_entry_its_cursor_names() {
    let emulator = Emulator::new();
    create_plans(&emulator, 4, 4).await;
    let plan = plan_name(0);

    // Each read's first page of one, and the entry it stopped at.
    let (plans_first, plans_token) = plans_page(&emulator, Some(1), None).await;
    let (resources_first, resources_token) = resources_page(&emulator, &plan, Some(1), None).await;
    let plans_token = plans_token.expect("plans remain after a page of one");
    let resources_token = resources_token.expect("resources remain after a page of one");

    // Resuming reports the entries after the one the page stopped at — never that one
    // again — and resuming TWICE from one cursor answers the same page, since a cursor is
    // a position in state rather than something a read consumes.
    for (operation, first, token) in [
        ("DescribeScalingPlans", plans_first.clone(), plans_token.clone()),
        (
            "DescribeScalingPlanResources",
            resources_first
                .iter()
                .map(|identity| identity.1.clone())
                .collect(),
            resources_token.clone(),
        ),
    ] {
        let stopped_at = first.last().cloned().expect("a non-empty first page");
        let rest: Vec<String> = if operation == "DescribeScalingPlans" {
            plans_page(&emulator, Some(MAX_RESULTS_MAX as usize), Some(&token))
                .await
                .0
        } else {
            resources_page(
                &emulator,
                &plan,
                Some(MAX_RESULTS_MAX as usize),
                Some(&token),
            )
            .await
            .0
            .iter()
            .map(|identity| identity.1.clone())
            .collect()
        };
        assert!(
            rest.iter().all(|entry| *entry > stopped_at),
            "case {operation}: the resumed page must hold only entries after \
             {stopped_at:?}, got {rest:?}"
        );
        assert_eq!(
            rest.len(),
            3,
            "case {operation}: three of the four entries remain after a page of one, got \
             {rest:?}"
        );
    }

    // The token namespace is per operation, so neither read accepts the other's cursor.
    let (status, code) = emulator
        .err(
            "DescribeScalingPlans",
            json!({ "NextToken": resources_token }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidNextTokenException"),
        "case DescribeScalingPlans: a cursor the resource read minted is not valid here, got \
         {status} {code}"
    );
    let (status, code) = emulator
        .err(
            "DescribeScalingPlanResources",
            json!({
                "ScalingPlanName": plan,
                "ScalingPlanVersion": 1,
                "NextToken": plans_token,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidNextTokenException"),
        "case DescribeScalingPlanResources: a cursor the plan read minted is not valid here, \
         got {status} {code}"
    );
}
