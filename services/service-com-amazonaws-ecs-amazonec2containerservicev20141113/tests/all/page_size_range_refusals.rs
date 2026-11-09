// SPDX-License-Identifier: CC0-1.0

//! Every paging operation's `maxResults`, driven end to end over the shared wire harness
//! in [`crate::ecs_wire`].
//!
//! The rule under test: a `maxResults` outside the inclusive page-size range its own
//! member documentation states is refused with `InvalidParameterException` and the
//! sentence its own operation words for an out-of-range page size, with that operation's
//! own documented bounds and nothing taken from the request — the daemon-plane listings
//! state the whole range at EITHER end of it ("MaxResults must be a positive number
//! between <min>-<max>."), while the long-standing listings name the smallest page they
//! take below the range ("Max results can not be lesser than <min>") and state the range
//! above it. A `maxResults` inside the range, each published bound included, is accepted
//! and paged with.
//!
//! The refusal is a property of the RANGE and of the sentence that operation words, not of
//! how far past a bound the value is, so the tables below vary the END crossed (below the
//! minimum, above the maximum), the DISTANCE past it (one step, far), the RANGE (the
//! 1..=100 most lists document and the 1..=10 the two narrow pages do) and the OPERATION.
//!
//! The ranges come from the members' own docs
//! (`smithy_query(query="doc",shape="<Request>",member="maxResults")`):
//!
//! * `ListClusters`, `ListTasks`, `ListServices`, `ListServicesByNamespace`,
//!   `ListTaskDefinitions`, `ListTaskDefinitionFamilies`, `ListContainerInstances`,
//!   `ListAttributes`, `ListServiceDeployments`, `ListDaemons`,
//!   `ListDaemonTaskDefinitions` — "This value can be between 1 and 100."
//! * `ListAccountSettings` — "This value can be between 1 and 10."
//!
//! `DescribeCapacityProviders` documents the same 1..=10 at its own `maxResults`, but that
//! range names the page it SERVES rather than the page a request may ask for: a bigger ask
//! is served that page instead of refused ([`crate::capacity_providers`] pins both ends of
//! it), so only the end below the range is a refusal here.
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a refusal or a page that is not computed from persisted state cannot
//! pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, service, string};

/// The family the daemon revisions this suite pages over are registered under.
const DAEMON_FAMILY: &str = "pagedaemon";

/// The image those revisions' container starts from; irrelevant to every rule here, so
/// one value.
const IMAGE: &str = "public.ecr.aws/aws-observability/aws-otel-collector:latest";

/// How many daemon revisions the positive control pages over.
const REVISIONS: usize = 3;

/// The sentence a daemon-plane page size outside `min..=max` earns, as Amazon ECS words
/// it: fixed text around the two documented bounds, naming neither the value the request
/// carried nor the member's spelling on the wire.
fn range_sentence(min: i32, max: i32) -> String {
    format!("MaxResults must be a positive number between {min}-{max}.")
}

/// The sentence a long-standing listing words for a page size below `min`: the member as
/// that refusal names it and the one bound the value crossed.
fn below_minimum_sentence(min: i32) -> String {
    format!("Max results can not be lesser than {min}")
}

/// A router holding [`REVISIONS`] daemon task-definition revisions, so a page size inside
/// the range has something to page over and a refusal has something to leave untouched.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    for revision in 1..=REVISIONS {
        ok(
            &router,
            "RegisterDaemonTaskDefinition",
            json!({
                "family": DAEMON_FAMILY,
                "containerDefinitions": [
                    { "name": format!("agent{revision}"), "image": IMAGE, "memory": 128 },
                ],
            }),
            "fixture",
        )
        .await;
    }
    router
}

/// The ARNs `ListDaemonTaskDefinitions` reports for `request`, and the token it hands
/// back.
async fn daemon_page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListDaemonTaskDefinitions", request, case).await;
    let arns = list(&body, "daemonTaskDefinitions", case)
        .iter()
        .map(|summary| string(summary, "arn", case))
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (arns, next_token)
}

/// Drive `rows` of (case, operation, request, expected message) and assert each is refused
/// with that message under `InvalidParameterException`.
async fn assert_refusals(router: &MultiServiceRouter, rows: Vec<(&str, &str, Value, String)>) {
    for (case, operation, request, expected) in rows {
        let (code, message) = refusal(router, operation, request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", expected.as_str()),
            "case {case:?}: expected {operation} to state its page-size rule, got {code} \
             with {message:?}"
        );
    }
}

#[tokio::test]
async fn a_daemon_plane_page_size_outside_its_range_states_that_range_at_both_ends() {
    let router = fixture().await;

    // One row per (operation, end crossed, distance past it): the sentence is the same at
    // either end of the range and however far past it the value is, so nothing in it can
    // come from the value.
    let expected = range_sentence(1, 100);
    assert_refusals(
        &router,
        vec![
            (
                "a daemon task-definition page one below the range",
                "ListDaemonTaskDefinitions",
                json!({ "maxResults": 0 }),
                expected.clone(),
            ),
            (
                "a daemon task-definition page far below the range",
                "ListDaemonTaskDefinitions",
                json!({ "maxResults": -100 }),
                expected.clone(),
            ),
            (
                "a daemon task-definition page at the floor of the member's type",
                "ListDaemonTaskDefinitions",
                json!({ "maxResults": i32::MIN }),
                expected.clone(),
            ),
            (
                "a daemon task-definition page one above the range",
                "ListDaemonTaskDefinitions",
                json!({ "maxResults": 101 }),
                expected.clone(),
            ),
            (
                "a daemon task-definition page far above the range",
                "ListDaemonTaskDefinitions",
                json!({ "maxResults": i32::MAX }),
                expected.clone(),
            ),
            (
                "a daemon task-definition page refused beside every other filter",
                "ListDaemonTaskDefinitions",
                json!({
                    "family": DAEMON_FAMILY,
                    "status": "ALL",
                    "sort": "DESC",
                    "maxResults": -1,
                }),
                expected.clone(),
            ),
            (
                "a daemon page one below the range",
                "ListDaemons",
                json!({ "maxResults": 0 }),
                expected.clone(),
            ),
            (
                "a daemon page far above the range",
                "ListDaemons",
                json!({ "maxResults": 5_000 }),
                expected.clone(),
            ),
        ],
    )
    .await;

    // Each published bound is INSIDE the range, so each is accepted rather than refused —
    // the sentence is earned by crossing a bound, not by naming one.
    for (case, max_results) in [("the smallest daemon page", 1), ("the largest daemon page", 100)] {
        let (arns, _) = daemon_page(
            &router,
            json!({ "family": DAEMON_FAMILY, "maxResults": max_results }),
            case,
        )
        .await;
        assert!(
            !arns.is_empty(),
            "case {case:?}: a page size at a published bound must be paged with, got {arns:?}"
        );
    }

    // The positive control: the refusals changed nothing, and a page size inside the
    // range still cuts the stored listing into pages of that many entries whose token
    // walks every revision exactly once.
    let case = "a walk of one-revision pages";
    let (whole, token) = daemon_page(&router, json!({ "family": DAEMON_FAMILY }), case).await;
    assert_eq!(
        (whole.len(), token.clone()),
        (REVISIONS, None),
        "case {case:?}: the refusals must leave every registered revision listed, got \
         {whole:?} with {token:?}"
    );

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..REVISIONS {
        let mut request = json!({ "family": DAEMON_FAMILY, "maxResults": 1 });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (arns, next) = daemon_page(&router, request, case).await;
        assert_eq!(
            arns.len(),
            1,
            "case {case:?}: a page may not exceed maxResults, got {arns:?}"
        );
        walked.extend(arns);
        token = next;
    }
    assert_eq!(
        (walked.clone(), token.clone()),
        (whole, None),
        "case {case:?}: the walk must report every revision exactly once and then stop, \
         got {walked:?} with {token:?}"
    );
}

#[tokio::test]
async fn a_long_standing_page_size_names_the_bound_it_crossed_below_and_the_range_above() {
    let router = service();

    // The listings that predate the daemon plane keep their own two sentences, each with
    // that operation's own documented bounds. `request` carries that operation's REQUIRED
    // members (and nothing else), because the page-size rule is a property of the member,
    // not of what the listing would have reported.
    //
    // `ListAccountSettings$maxResults` is the one member modelled with a default of `0`,
    // so zero there is a page size the request never named rather than one below the
    // range; its below-the-range row is negative.
    assert_refusals(
        &router,
        vec![
            (
                "a cluster page one below the range",
                "ListClusters",
                json!({ "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
            (
                "a cluster page one above the range",
                "ListClusters",
                json!({ "maxResults": 101 }),
                range_sentence(1, 100),
            ),
            (
                "a task page far below the range",
                "ListTasks",
                json!({ "maxResults": -37 }),
                below_minimum_sentence(1),
            ),
            (
                "a task page far above the range",
                "ListTasks",
                json!({ "maxResults": 5_000 }),
                range_sentence(1, 100),
            ),
            (
                "a service page one below the range",
                "ListServices",
                json!({ "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
            (
                "a namespace's service page far below the range",
                "ListServicesByNamespace",
                json!({ "namespace": "pagedspace", "maxResults": -1 }),
                below_minimum_sentence(1),
            ),
            (
                "a task-definition page one below the range",
                "ListTaskDefinitions",
                json!({ "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
            (
                "a family page at the floor of the member's type",
                "ListTaskDefinitionFamilies",
                json!({ "maxResults": i32::MIN }),
                below_minimum_sentence(1),
            ),
            (
                "a container-instance page one below the range",
                "ListContainerInstances",
                json!({ "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
            (
                "an attribute page one below the range",
                "ListAttributes",
                json!({ "targetType": "container-instance", "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
            (
                "a service-deployment page one above the range",
                "ListServiceDeployments",
                json!({ "service": "pagedservice", "maxResults": 101 }),
                range_sentence(1, 100),
            ),
            (
                "an account-setting page below its narrower range",
                "ListAccountSettings",
                json!({ "maxResults": -1 }),
                below_minimum_sentence(1),
            ),
            (
                "an account-setting page one above its narrower range",
                "ListAccountSettings",
                json!({ "maxResults": 11 }),
                range_sentence(1, 10),
            ),
            (
                "a capacity-provider page one below its narrower range",
                "DescribeCapacityProviders",
                json!({ "maxResults": 0 }),
                below_minimum_sentence(1),
            ),
        ],
    )
    .await;

    // And each published bound of those ranges is inside them, so it is answered rather
    // than refused.
    for (case, operation, request, min, max) in [
        (
            "the smallest cluster page",
            "ListClusters",
            json!({ "maxResults": 1 }),
            1,
            100,
        ),
        (
            "the largest cluster page",
            "ListClusters",
            json!({ "maxResults": 100 }),
            1,
            100,
        ),
        (
            "the smallest task-definition page",
            "ListTaskDefinitions",
            json!({ "maxResults": 1 }),
            1,
            100,
        ),
        (
            "the largest family page",
            "ListTaskDefinitionFamilies",
            json!({ "maxResults": 100 }),
            1,
            100,
        ),
        (
            "the largest account-setting page",
            "ListAccountSettings",
            json!({ "maxResults": 10 }),
            1,
            10,
        ),
        (
            "the smallest capacity-provider page",
            "DescribeCapacityProviders",
            json!({ "maxResults": 1 }),
            1,
            10,
        ),
        (
            "a capacity-provider page above the page this read serves",
            "DescribeCapacityProviders",
            json!({ "maxResults": 1_000 }),
            1,
            10,
        ),
    ] {
        let body = ok(&router, operation, request, case).await;
        let object = body.as_object().unwrap_or_else(|| {
            panic!("case {case:?}: expected {operation} to answer an object, got {body}")
        });
        // A page inside the range is PAGED with, so whatever list the answer carries holds
        // at most the page that operation serves — which is also the end of the range a
        // request may ask beyond.
        let served = object
            .values()
            .filter_map(|value| value.as_array())
            .map(Vec::len)
            .max()
            .unwrap_or_default();
        assert!(
            served <= max,
            "case {case:?}: expected {operation} to answer a page inside {min}..={max}, got \
             {served} entries in {body}"
        );
    }
}
