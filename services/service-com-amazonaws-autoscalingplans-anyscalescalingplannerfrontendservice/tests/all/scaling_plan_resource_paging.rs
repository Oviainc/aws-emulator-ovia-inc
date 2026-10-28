// SPDX-License-Identifier: CC0-1.0

//! Paging over the scalable resources a scaling plan covers, ACROSS the writes that
//! rewrite the plan's scaling instructions while a walk is in flight.
//!
//! The rule pinned here: `DescribeScalingPlanResources` reports the resources a plan
//! covers in the order of the identity that addresses each of them — the
//! `(ServiceNamespace, ResourceId, ScalableDimension)` triple of its scaling instruction,
//! since "Each scaling instruction applies to one resource"
//! (`smithy_query(query="shape",shape="ScalingInstruction")`) — and resumes a walk
//! strictly AFTER the identity of the resource the previous page stopped at. So a page
//! boundary keeps its meaning however the instruction list is rewritten between pages:
//! an `UpdateScalingPlan` that drops, adds or reorders instructions can neither skip a
//! resource the plan still covers nor report one twice.
//!
//! Rows differ along what that rule generalises over: the ORDER the instructions were
//! written in, the NUMBER of resources the plan covers, the PAGE SIZE the walk uses, and
//! WHAT the mid-walk write does to the list — drops the resource the cursor names, drops
//! one the walk already passed or has not reached yet, adds one sorting before or after
//! the cursor, reverses the list, or replaces every instruction at once.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`] and the catalog of scalable resources from
//! [`crate::scaling_plan_resource_policies`]; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{
    instruction, resource_identity, stack_source, Emulator, ResourceIdentity,
};
use crate::scaling_plan_resource_policies::{resource_cases, ResourceCase};

/// The plan every row walks.
const PLAN: &str = "my-plan";

/// The `ServiceNamespace` of the resources the walk rows cover, and the one
/// `ScalableDimension` it owns that they are scaled on
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension")`).
const NAMESPACE: &str = "autoscaling";
const DIMENSION: &str = "autoscaling:autoScalingGroup:DesiredCapacity";

/// The documented maximum `MaxResults`: "The value must be between 1 and 50"
/// (`smithy_query(query="operation",operation="DescribeScalingPlanResources")`).
const MAX_RESULTS_MAX: usize = 50;

/// The resource id of the Auto Scaling group named by `letter`.
///
/// Every walk row's resources share the prefix an
/// `autoscaling:autoScalingGroup:DesiredCapacity` resource id carries
/// (`autoScalingGroup/my-asg`, from the worked examples in
/// `smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`) and differ
/// only in the trailing letter, so the identity order of a row's resources is the
/// alphabetical order of its letters — which every row asserts of the service before it
/// uses it to compute what the walk owes.
fn asg_id(letter: char) -> String {
    format!("autoScalingGroup/asg-{letter}")
}

/// One scaling instruction covering the Auto Scaling group named by `letter`.
fn asg_instruction(letter: char) -> Value {
    instruction(
        NAMESPACE,
        &asg_id(letter),
        DIMENSION,
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )
}

/// The identity the resource of [`asg_instruction`] is addressed by.
fn asg_identity(letter: char) -> ResourceIdentity {
    (NAMESPACE.to_string(), asg_id(letter), DIMENSION.to_string())
}

/// The identities of `letters` in the order a read reports them: the order of the
/// identity that addresses each resource.
fn ordered(letters: &[char]) -> Vec<ResourceIdentity> {
    let mut letters: Vec<char> = letters.to_vec();
    letters.sort_unstable();
    letters.into_iter().map(asg_identity).collect()
}

/// Create [`PLAN`] covering `instructions` and let the clock apply it, so the plan is
/// `Active` and every resource it covers is reported.
async fn create_covering(emulator: &Emulator, instructions: Vec<Value>) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// Put exactly the instructions for `letters` in force, in the order given, and let the
/// clock apply the update.
async fn update_to(emulator: &Emulator, letters: &[char]) {
    let instructions: Vec<Value> = letters.iter().copied().map(asg_instruction).collect();
    emulator
        .ok(
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ScalingPlanVersion": 1,
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// One page of [`PLAN`]'s resources: the identities it reports and the token it hands
/// back. `max_results` absent asks for the documented default page.
async fn page(
    emulator: &Emulator,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (Vec<ResourceIdentity>, Option<String>) {
    let mut request = json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 });
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let body = emulator.ok("DescribeScalingPlanResources", request).await;
    let reported = body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no ScalingPlanResources in {body:?}"));
    let next_token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported.iter().map(resource_identity).collect(), next_token)
}

/// Every identity the pages from `token` on report, at `max_results` per page.
///
/// Bounded by `pages`: a cursor that stands still fails the case instead of looping
/// forever.
async fn walk(
    emulator: &Emulator,
    case: &str,
    max_results: usize,
    token: Option<String>,
    pages: usize,
) -> Vec<ResourceIdentity> {
    let mut seen: Vec<ResourceIdentity> = Vec::new();
    let mut token = token;
    let mut index = 0usize;
    while let Some(current) = token {
        assert!(
            index < pages,
            "case {case:?}: the walk must end within {pages} pages, got {seen:?}"
        );
        let (identities, next_token) = page(emulator, Some(max_results), Some(&current)).await;
        assert!(
            identities.len() <= max_results,
            "case {case:?} page {index}: MaxResults {max_results} must cap the page, got \
             {identities:?}"
        );
        // One cursor answers one page: resuming a walk reads state rather than consuming
        // the token it resumes from.
        let (again, again_token) = page(emulator, Some(max_results), Some(&current)).await;
        assert_eq!(
            (&again, &again_token),
            (&identities, &next_token),
            "case {case:?} page {index}: the same cursor must answer the same page, got \
             {again:?} with {again_token:?}"
        );
        seen.extend(identities);
        token = next_token;
        index += 1;
    }
    seen
}

/// One row: the resources the plan is created with (in the order the request writes
/// them), the page size the walk uses, and what an `UpdateScalingPlan` puts in force
/// after the first page.
struct WalkCase {
    label: &'static str,
    created: &'static [char],
    page_size: usize,
    /// The letters the mid-walk update carries, in the REQUEST's order, or `None` for a
    /// walk nothing writes to.
    updated: Option<&'static [char]>,
}

/// The table the walk test spans.
fn walk_cases() -> Vec<WalkCase> {
    vec![
        WalkCase {
            label: "nothing is written between the pages",
            created: &['c', 'a', 'b', 'd'],
            page_size: 1,
            updated: None,
        },
        WalkCase {
            label: "two resources per page over an odd count, nothing written",
            created: &['e', 'c', 'a', 'b', 'd'],
            page_size: 2,
            updated: None,
        },
        WalkCase {
            label: "the largest page covers the whole plan at once",
            created: &['b', 'a', 'c'],
            page_size: MAX_RESULTS_MAX,
            updated: None,
        },
        WalkCase {
            label: "the resource the cursor names is dropped",
            created: &['a', 'b', 'c'],
            page_size: 1,
            updated: Some(&['b', 'c']),
        },
        WalkCase {
            label: "a resource the walk already passed is dropped",
            created: &['a', 'b', 'c', 'd'],
            page_size: 2,
            updated: Some(&['a', 'c', 'd']),
        },
        WalkCase {
            label: "a resource the walk has not reached is dropped",
            created: &['a', 'b', 'c', 'd'],
            page_size: 1,
            updated: Some(&['a', 'b', 'd']),
        },
        WalkCase {
            label: "a resource sorting before the cursor is added",
            created: &['b', 'c', 'd'],
            page_size: 1,
            updated: Some(&['a', 'b', 'c', 'd']),
        },
        WalkCase {
            label: "a resource sorting after the cursor is added",
            created: &['a', 'b'],
            page_size: 1,
            updated: Some(&['a', 'b', 'c']),
        },
        WalkCase {
            label: "the whole instruction list is reversed",
            created: &['a', 'b', 'c', 'd'],
            page_size: 2,
            updated: Some(&['d', 'c', 'b', 'a']),
        },
        WalkCase {
            label: "every instruction is replaced by resources sorting after the cursor",
            created: &['b', 'c'],
            page_size: 1,
            updated: Some(&['e', 'f']),
        },
        WalkCase {
            label: "every instruction is replaced by resources sorting before the cursor",
            created: &['e', 'f'],
            page_size: 1,
            updated: Some(&['a', 'b']),
        },
        WalkCase {
            label: "one resource is dropped and another added at once",
            created: &['a', 'b', 'c'],
            page_size: 1,
            updated: Some(&['b', 'd']),
        },
    ]
}

#[tokio::test]
async fn a_walk_resumes_after_the_resource_the_previous_page_stopped_at() {
    for case in walk_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let instructions: Vec<Value> = case.created.iter().copied().map(asg_instruction).collect();
        create_covering(&emulator, instructions).await;

        // One unpaged read first: the resources come back in the order of the identity
        // that addresses each of them, not in the order the instructions were written.
        let (unpaged, unpaged_token) = page(&emulator, None, None).await;
        assert_eq!(
            unpaged,
            ordered(case.created),
            "case {label:?}: an unpaged read must report every covered resource once, in \
             the order of the identity that addresses it, got {unpaged:?}"
        );
        assert!(
            unpaged_token.is_none(),
            "case {label:?}: a page that covered the whole plan continues nowhere, got \
             {unpaged_token:?}"
        );

        // The first page of the walk, and the cursor it hands back.
        let (first, token) = page(&emulator, Some(case.page_size), None).await;
        let expected_first: Vec<ResourceIdentity> = ordered(case.created)
            .into_iter()
            .take(case.page_size)
            .collect();
        assert_eq!(
            first, expected_first,
            "case {label:?}: the first page must report the first {} resources in identity \
             order, got {first:?}",
            case.page_size
        );
        assert_eq!(
            token.is_some(),
            case.created.len() > case.page_size,
            "case {label:?}: a token is handed back exactly while resources remain, got \
             {token:?}"
        );

        // The write that moves every later instruction's POSITION in the stored list.
        if let Some(updated) = case.updated {
            update_to(&emulator, updated).await;
            let (in_force, _) = page(&emulator, None, None).await;
            assert_eq!(
                in_force,
                ordered(updated),
                "case {label:?}: the update must leave exactly its own resources covered, \
                 got {in_force:?}"
            );
        }

        // Resuming reports every resource still covered whose identity sorts after the
        // one the first page stopped at — each exactly once, in that order — and nothing
        // the first page already reported.
        let covered = case.updated.unwrap_or(case.created);
        let stopped_at = first.last().cloned();
        let expected_rest: Vec<ResourceIdentity> = ordered(covered)
            .into_iter()
            .filter(|identity| {
                stopped_at
                    .as_ref()
                    .is_some_and(|stopped_at| identity > stopped_at)
            })
            .collect();
        let rest = walk(
            &emulator,
            label,
            case.page_size,
            token,
            covered.len() + case.created.len() + 1,
        )
        .await;
        assert_eq!(
            rest, expected_rest,
            "case {label:?}: the resumed walk must report exactly the still-covered \
             resources after {stopped_at:?}, got {rest:?}"
        );

        let whole: Vec<ResourceIdentity> = first.into_iter().chain(rest).collect();
        let distinct: BTreeSet<&ResourceIdentity> = whole.iter().collect();
        assert_eq!(
            distinct.len(),
            whole.len(),
            "case {label:?}: no resource may be reported twice over one walk, got {whole:?}"
        );
    }
}

#[tokio::test]
async fn every_page_size_reassembles_the_whole_coverage_exactly_once() {
    let cases = resource_cases();
    let emulator = Emulator::new();
    create_covering(
        &emulator,
        cases.iter().map(ResourceCase::instruction).collect(),
    )
    .await;

    // The whole coverage, unpaged: one resource per instruction, each addressed by its
    // own identity, and nothing continues after it.
    let (unpaged, unpaged_token) = page(&emulator, None, None).await;
    assert!(
        unpaged_token.is_none(),
        "the default page covers the plan, so it continues nowhere, got {unpaged_token:?}"
    );
    let expected: BTreeSet<ResourceIdentity> = cases
        .iter()
        .map(|case| {
            (
                case.namespace.to_string(),
                case.resource_id.to_string(),
                case.dimension.to_string(),
            )
        })
        .collect();
    let reported: BTreeSet<ResourceIdentity> = unpaged.iter().cloned().collect();
    assert_eq!(
        reported, expected,
        "every instruction's resource must be reported exactly once, got {unpaged:?}"
    );
    assert_eq!(
        unpaged.len(),
        expected.len(),
        "no resource may be reported twice in one page, got {unpaged:?}"
    );

    // Whatever the page size, the pages reassemble that one read: the walk is a walk
    // over the same ordering, cut in different places.
    for page_size in [1, 2, 3, cases.len() - 1, cases.len(), MAX_RESULTS_MAX] {
        let (first, token) = page(&emulator, Some(page_size), None).await;
        assert!(
            first.len() <= page_size,
            "case MaxResults={page_size}: the page must be capped, got {first:?}"
        );
        let case = format!("MaxResults={page_size}");
        let rest = walk(&emulator, &case, page_size, token, cases.len() + 1).await;
        let walked: Vec<ResourceIdentity> = first.into_iter().chain(rest).collect();
        assert_eq!(
            walked, unpaged,
            "case {case}: the pages must reassemble the unpaged read exactly, got {walked:?}"
        );
    }
}
