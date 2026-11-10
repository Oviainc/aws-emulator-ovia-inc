// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` means for `CreateDashboard`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rule these rows pin: a `clientToken` suppresses re-execution only for a
//! REPEAT of the request it was spent on, and a repeat is recognised by the whole
//! request — not by the token alone. A request differing in any member it carries
//! is a different call, so this create carries it out and answers with the
//! identifiers of the dashboard IT minted: the answer carries nothing of the
//! earlier call but that call's dashboard, so answering from it would hand the
//! caller a dashboard it never asked for and mint nothing for the request it
//! actually sent.
//!
//! The rows span the member the second request differs in — the required name,
//! the optional description the first call did not carry, the definition literal,
//! the project the dashboard is created in, and the tags map — and each row also
//! pins the repeat (replayed, writing nothing) and the second call's own repeat,
//! so the token's record is shown to have MOVED to the call it was last spent on.
//!
//! The HTTP harness (`call`, `router_with_engine`), the store counters (`Kind`,
//! `held`), the token helpers (`with_token`, `token_for`), the project seeding
//! (`seed_projects`), the read (`describe_dashboard`) and the definition builder
//! (`definition_of`) are the ones the sibling suites define; all are reused rather
//! than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, router_with_engine};
use crate::client_token_idempotency::with_token;
use crate::client_token_replay::{held, token_for, Kind};
use crate::dashboard_lifecycle::{definition_of, describe_dashboard, seed_projects};

/// Where every dashboard request goes, as `OPERATIONS.md` reports the routes:
/// `POST /dashboards`, `GET /dashboards/{dashboardId}`,
/// `GET /dashboards?projectId=...`.
const DASHBOARDS: &str = "/dashboards";

/// One row: a create served under a token, and a DIFFERENT create sent under that
/// same, now spent, token.
struct Row {
    case: &'static str,
    /// The request the row's token is spent on.
    first: Value,
    /// The same create with ONE member different, which is a different call.
    second: Value,
    /// What a read of the dashboard the FIRST call minted reports — before the
    /// differing request, and unchanged after it.
    first_reads: Vec<(&'static str, Value)>,
    /// What a read of the dashboard the SECOND request minted reports: what THAT
    /// request asked for.
    second_reads: Vec<(&'static str, Value)>,
    /// The tags `ListTagsForResource` must report for the second dashboard, for
    /// the row whose requests differ only in their tags.
    second_tags: Option<Value>,
}

/// The id a create answered with.
fn minted(case: &str, body: &Value) -> String {
    body["dashboardId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no dashboardId in {body}"))
        .to_string()
}

/// Assert a read of the dashboard `id` reports each of `members`.
async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    step: &str,
    id: &str,
    members: &[(&'static str, Value)],
) {
    let (status, read) = describe_dashboard(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?} {step}: the read of {id} got {status} {read}"
    );
    for (member, value) in members {
        assert_eq!(
            &read[*member], value,
            "case {case:?} {step}: expected {member} {value}, got {read}"
        );
    }
}

/// The ids `ListDashboards` reports for `project_id`.
async fn listed(router: &MultiServiceRouter, case: &str, project_id: &str) -> Vec<String> {
    let uri = format!("{DASHBOARDS}?projectId={project_id}&maxResults=250");
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListDashboards on {project_id} got {status} {body}"
    );
    body["dashboardSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no dashboardSummaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The rows, with the two projects the differing-project row needs. The first and
/// second definitions differ in size as well as in content, because the member is
/// a literal of up to 204800 characters rather than a short name.
fn rows(projects: &[String]) -> Vec<Row> {
    let home = projects[0].clone();
    let elsewhere = projects[1].clone();
    let small = definition_of(64);
    let large = definition_of(4096);

    vec![
        Row {
            case: "CreateDashboard differing in the name",
            first: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenFirstDashboard",
                "dashboardDefinition": small.clone(),
            }),
            second: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenSecondDashboard",
                "dashboardDefinition": small.clone(),
            }),
            first_reads: vec![("dashboardName", json!("SpentTokenFirstDashboard"))],
            second_reads: vec![("dashboardName", json!("SpentTokenSecondDashboard"))],
            second_tags: None,
        },
        Row {
            case: "CreateDashboard differing in the description",
            first: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenDescribedDashboard",
                "dashboardDefinition": small.clone(),
            }),
            second: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenDescribedDashboard",
                "dashboardDescription": "a description the first call did not carry",
                "dashboardDefinition": small.clone(),
            }),
            first_reads: vec![
                // The first call carried no description, so the record holds none.
                ("dashboardDescription", json!(null)),
            ],
            second_reads: vec![(
                "dashboardDescription",
                json!("a description the first call did not carry"),
            )],
            second_tags: None,
        },
        Row {
            case: "CreateDashboard differing in the definition",
            first: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenRedefinedDashboard",
                "dashboardDefinition": small.clone(),
            }),
            second: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenRedefinedDashboard",
                "dashboardDefinition": large.clone(),
            }),
            first_reads: vec![("dashboardDefinition", json!(small.clone()))],
            second_reads: vec![("dashboardDefinition", json!(large))],
            second_tags: None,
        },
        Row {
            case: "CreateDashboard differing in the project",
            first: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenMovedDashboard",
                "dashboardDefinition": small.clone(),
            }),
            second: json!({
                "projectId": elsewhere.clone(),
                "dashboardName": "SpentTokenMovedDashboard",
                "dashboardDefinition": small.clone(),
            }),
            first_reads: vec![("projectId", json!(home.clone()))],
            second_reads: vec![("projectId", json!(elsewhere))],
            second_tags: None,
        },
        Row {
            case: "CreateDashboard differing only in the tags",
            first: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenTaggedDashboard",
                "dashboardDefinition": small.clone(),
            }),
            second: json!({
                "projectId": home.clone(),
                "dashboardName": "SpentTokenTaggedDashboard",
                "dashboardDefinition": small,
                "tags": { "stage": "test" },
            }),
            first_reads: vec![("dashboardName", json!("SpentTokenTaggedDashboard"))],
            second_reads: vec![("dashboardName", json!("SpentTokenTaggedDashboard"))],
            second_tags: Some(json!({ "stage": "test" })),
        },
    ]
}

#[tokio::test]
async fn a_create_differing_from_the_call_its_token_names_mints_a_dashboard_of_its_own() {
    let (router, engine) = router_with_engine();
    let projects = seed_projects(&router, 2).await;

    for row in rows(&projects) {
        let Row {
            case,
            first,
            second,
            first_reads,
            second_reads,
            second_tags,
        } = row;
        let token = token_for(case);

        let before = held(&engine, Kind::Dashboard);
        let (status, served) =
            call(&router, "POST", DASHBOARDS, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the first create got {status} {served}"
        );
        let after_first = held(&engine, Kind::Dashboard);
        assert_eq!(
            after_first,
            before + 1,
            "case {case:?}: the create must mint exactly one dashboard, {before} -> {after_first}"
        );
        let first_id = minted(case, &served);
        assert_reads(
            &router,
            case,
            "after the first create",
            &first_id,
            &first_reads,
        )
        .await;

        // The RETRY: the SAME request under the same token answers from the call it
        // repeats and mints nothing.
        let (status, replayed) =
            call(&router, "POST", DASHBOARDS, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a retry got {status} {replayed}"
        );
        assert_eq!(
            replayed, served,
            "case {case:?}: a retry must replay the first answer, got {replayed} after {served}"
        );
        let after_retry = held(&engine, Kind::Dashboard);
        assert_eq!(
            after_retry, after_first,
            "case {case:?}: a retry must mint nothing, {after_first} -> {after_retry}"
        );

        // The DIFFERING request under that same, spent, token: a different call,
        // carried out as the create it is.
        let (status, answered) =
            call(&router, "POST", DASHBOARDS, Some(with_token(&second, &token))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a create differing from the call its token names must be served, \
             got {status} {answered}"
        );
        let after_second = held(&engine, Kind::Dashboard);
        assert_eq!(
            after_second,
            after_first + 1,
            "case {case:?}: that create must mint one dashboard of its own, {after_first} -> \
             {after_second}"
        );
        for member in ["dashboardId", "dashboardArn"] {
            assert_ne!(
                answered[member], served[member],
                "case {case:?}: the differing create must answer with its OWN {member}, got \
                 {answered} after {served}"
            );
        }

        // The dashboard it minted holds what THIS request asked for, and the first
        // one is untouched by it.
        let second_id = minted(case, &answered);
        assert_reads(
            &router,
            case,
            "reading the dashboard the differing create minted",
            &second_id,
            &second_reads,
        )
        .await;
        assert_reads(
            &router,
            case,
            "reading the first dashboard again",
            &first_id,
            &first_reads,
        )
        .await;

        // And it is listed in the project it was created in, so the dashboard the
        // caller asked for exists where that caller would look for it.
        let project = second["projectId"].as_str().unwrap_or_default();
        let ids = listed(&router, case, project).await;
        assert!(
            ids.contains(&second_id),
            "case {case:?}: the dashboard the differing create minted must be listed in \
             {project}, got {ids:?}"
        );

        // The token's record moved to the call it was last spent on: a repeat of
        // THAT call replays its answer and mints nothing.
        let (status, again) =
            call(&router, "POST", DASHBOARDS, Some(with_token(&second, &token))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a retry of the differing create got {status} {again}"
        );
        assert_eq!(
            again, answered,
            "case {case:?}: that retry must replay the answer of the call it repeats, got \
             {again} after {answered}"
        );
        let after_second_retry = held(&engine, Kind::Dashboard);
        assert_eq!(
            after_second_retry, after_second,
            "case {case:?}: that retry must mint nothing, {after_second} -> {after_second_retry}"
        );

        if let Some(expected) = second_tags {
            let arn = answered["dashboardArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no dashboardArn in {answered}"));
            let (status, tags) =
                call(&router, "GET", &format!("/tags?resourceArn={arn}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource got {status} {tags}"
            );
            assert_eq!(
                tags["tags"], expected,
                "case {case:?}: expected the tags {expected} this create carried, got {tags}"
            );
        }
    }
}
