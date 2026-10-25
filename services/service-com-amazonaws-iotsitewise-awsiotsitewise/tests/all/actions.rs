// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListActions`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `ListActions` reports, from the shared store, the
//! actions recorded against the ONE target resource the request names — an asset
//! or a computation model — narrowed by the `resolveTo` filter it carries, in a
//! stable order its `nextToken` resumes; an identifier that names no stored
//! resource is a modelled refusal rather than an empty page, and no listing
//! changes what a later caller sees.
//!
//! `ExecuteAction` is the operation that records an action and is still a
//! scaffold stub (its own issue,
//! `20261024T100803Z-iotsitewise-executeaction-stub.md`), so these suites record
//! the actions directly in the shared store this handler reads — the same store a
//! handler writes through, which is what `executions` does for the executions its
//! own stub writer would record.
//!
//! The wire harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` already defines, and the computation-model create is
//! `computation_model_lifecycle`'s, re-used rather than copied.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ResolveTo, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ActionRecord};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_catalog, router_with_engine,
};
use crate::computation_model_lifecycle::{anomaly_detection, asset_model_property, id_of};

/// An id of the modelled `ID` form that no create in this suite mints, so it
/// names a resource the store does not hold.
const ABSENT_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// Which seeded resource an action is recorded against, or resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The first asset the fixture creates.
    AssetA,
    /// The second asset the fixture creates.
    AssetB,
    /// The computation model the fixture creates.
    ComputationModel,
}

impl Target {
    /// The `targetResourceType` this target is addressed by. Source:
    /// `smithy_query(query="operation", operation="ListActions")` —
    /// `targetResourceType (TargetResourceType, one of: ASSET |
    /// COMPUTATION_MODEL)`.
    fn resource_type(self) -> &'static str {
        match self {
            Target::ComputationModel => "COMPUTATION_MODEL",
            _ => "ASSET",
        }
    }
}

/// One row of the seeded action table.
#[derive(Debug, Clone)]
struct ActionRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The id the action is stored and reported under.
    id: &'static str,
    /// The action definition this action runs.
    definition: &'static str,
    /// The resource the action was taken on.
    target: Target,
    /// The detailed resource the action resolves to, when it resolves to one.
    resolve_to: Option<Target>,
    /// How many seconds before the suite's anchor the action ran.
    ran_ago: u64,
}

/// The seeded actions: one table spanning the dimensions the listing rule
/// generalises over — which resource the action targets and of which of the two
/// modelled kinds, whether it resolves to a detailed resource and which one,
/// which definition it runs, and when it ran (including two that ran in the SAME
/// instant, whose order the id has to break).
fn action_table() -> Vec<ActionRow> {
    vec![
        ActionRow {
            case: "oldest on A, resolving to nothing",
            id: "babababa-1111-4111-8111-111111111111",
            definition: "11111111-1111-4111-8111-111111111111",
            target: Target::AssetA,
            resolve_to: None,
            ran_ago: 900,
        },
        ActionRow {
            case: "A, resolving to B",
            id: "babababa-2222-4222-8222-222222222222",
            definition: "22222222-2222-4222-8222-222222222222",
            target: Target::AssetA,
            resolve_to: Some(Target::AssetB),
            ran_ago: 800,
        },
        ActionRow {
            case: "A, resolving to A itself",
            id: "babababa-3333-4333-8333-333333333333",
            definition: "11111111-1111-4111-8111-111111111111",
            target: Target::AssetA,
            resolve_to: Some(Target::AssetA),
            ran_ago: 700,
        },
        ActionRow {
            case: "A, same instant as the row below, lower id",
            id: "babababa-4444-4444-8444-444444444444",
            definition: "33333333-3333-4333-8333-333333333333",
            target: Target::AssetA,
            resolve_to: Some(Target::AssetB),
            ran_ago: 600,
        },
        ActionRow {
            case: "A, same instant as the row above, higher id",
            id: "babababa-5555-4555-8555-555555555555",
            definition: "33333333-3333-4333-8333-333333333333",
            target: Target::AssetA,
            resolve_to: None,
            ran_ago: 600,
        },
        ActionRow {
            case: "B, resolving to B",
            id: "babababa-6666-4666-8666-666666666666",
            definition: "22222222-2222-4222-8222-222222222222",
            target: Target::AssetB,
            resolve_to: Some(Target::AssetB),
            ran_ago: 500,
        },
        ActionRow {
            case: "the computation model, resolving to A",
            id: "babababa-7777-4777-8777-777777777777",
            definition: "44444444-4444-4444-8444-444444444444",
            target: Target::ComputationModel,
            resolve_to: Some(Target::AssetA),
            ran_ago: 400,
        },
        ActionRow {
            case: "the computation model, resolving to nothing",
            id: "babababa-8888-4888-8888-888888888888",
            definition: "44444444-4444-4444-8444-444444444444",
            target: Target::ComputationModel,
            resolve_to: None,
            ran_ago: 300,
        },
    ]
}

/// The router, the ids of the seeded resources and the seeded rows.
struct Fixture {
    router: MultiServiceRouter,
    asset_a: String,
    asset_b: String,
    computation_model: String,
    rows: Vec<ActionRow>,
}

impl Fixture {
    /// The stored id of a seeded target.
    fn id_of(&self, target: Target) -> &str {
        match target {
            Target::AssetA => &self.asset_a,
            Target::AssetB => &self.asset_b,
            Target::ComputationModel => &self.computation_model,
        }
    }

    /// The rows targeting `target`, in the order the listing must report them:
    /// oldest first, ties broken by action id.
    fn ordered_rows(&self, target: Target) -> Vec<ActionRow> {
        let mut rows: Vec<ActionRow> = self
            .rows
            .iter()
            .filter(|row| row.target == target)
            .cloned()
            .collect();
        // Oldest first: a bigger `ran_ago` is further in the past.
        rows.sort_by(|left, right| {
            right
                .ran_ago
                .cmp(&left.ran_ago)
                .then_with(|| left.id.cmp(right.id))
        });
        rows
    }

    /// The ids of the rows targeting `target`, in that same order.
    fn ordered_ids(&self, target: Target) -> Vec<String> {
        self.ordered_rows(target)
            .iter()
            .map(|row| row.id.to_string())
            .collect()
    }

    /// The query string that lists the actions of one seeded target.
    fn target_query(&self, target: Target) -> String {
        format!(
            "targetResourceType={}&targetResourceId={}",
            target.resource_type(),
            self.id_of(target)
        )
    }
}

/// Build the fixture: two assets and a computation model created through the API,
/// then the action table recorded in the shared store.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_a = create_asset(&router, "Boiler-A", &model).await;
    let asset_b = create_asset(&router, "Boiler-B", &model).await;

    let (status, created) = call(
        &router,
        "POST",
        "/computation-models",
        Some(json!({
            "computationModelName": "boiler-anomaly",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        })),
    )
    .await;
    assert_eq!(status, 202, "create computation model: got {status} {created}");
    let computation_model = created["computationModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create computation model: no id in {created}"))
        .to_string();

    let fixture = Fixture {
        router,
        asset_a,
        asset_b,
        computation_model,
        rows: action_table(),
    };

    let anchor = SystemTime::now();
    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for row in &fixture.rows {
            let target_id = fixture.id_of(row.target).to_string();
            let target_resource = match row.target {
                Target::ComputationModel => TargetResource {
                    asset_id: None,
                    computation_model_id: Some(target_id),
                },
                _ => TargetResource {
                    asset_id: Some(target_id),
                    computation_model_id: None,
                },
            };
            store.actions.insert(
                row.id.to_string(),
                ActionRecord {
                    action_id: row.id.to_string(),
                    action_definition_id: row.definition.to_string(),
                    target_resource,
                    resolve_to: row.resolve_to.map(|resolved| ResolveTo {
                        asset_id: fixture.id_of(resolved).to_string(),
                    }),
                    execution_time: anchor - Duration::from_secs(row.ran_ago),
                },
            );
        }
    }

    fixture
}

/// Dispatch one `ListActions` with `query` as its query string.
async fn list_actions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/actions?{query}"), None).await
}

/// The `actionId`s of a listing body, in the order it reported them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["actionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an actionSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["actionId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no actionId in {body}"))
                .to_string()
        })
        .collect()
}

/// The resume point a listing body carries, or `None` when it reports the empty
/// token the required member takes on a page with nothing left to resume from.
///
/// `None` also covers an absent member, so this oracle states "nothing left to
/// resume from" rather than one encoding of it.
fn resume_point(body: &Value) -> Option<String> {
    match body["nextToken"].as_str() {
        Some("") | None => None,
        Some(token) => Some(token.to_string()),
    }
}

/// Assert one reported summary carries exactly what its row was seeded with.
fn assert_summary(fixture: &Fixture, row: &ActionRow, summary: &Value) {
    let case = row.case;
    assert_eq!(
        summary["actionId"].as_str(),
        Some(row.id),
        "case {case:?}: unexpected actionId in {summary}"
    );
    assert_eq!(
        summary["actionDefinitionId"].as_str(),
        Some(row.definition),
        "case {case:?}: unexpected actionDefinitionId in {summary}"
    );
    let expected_target = fixture.id_of(row.target);
    let reported_target = match row.target {
        Target::ComputationModel => summary["targetResource"]["computationModelId"].as_str(),
        _ => summary["targetResource"]["assetId"].as_str(),
    };
    assert_eq!(
        reported_target,
        Some(expected_target),
        "case {case:?}: unexpected targetResource in {summary}"
    );
    // The member the target's own kind does NOT occupy stays absent, so a
    // summary names one resource rather than two.
    let other_member = match row.target {
        Target::ComputationModel => &summary["targetResource"]["assetId"],
        _ => &summary["targetResource"]["computationModelId"],
    };
    assert!(
        other_member.is_null(),
        "case {case:?}: a targetResource must name only its own kind, got {summary}"
    );
    assert_eq!(
        summary["resolveTo"]["assetId"].as_str(),
        row.resolve_to.map(|resolved| fixture.id_of(resolved)),
        "case {case:?}: unexpected resolveTo in {summary}"
    );
}

#[tokio::test]
async fn a_listing_reports_every_stored_action_of_the_target_resource_it_names() {
    let fixture = fixture().await;

    for target in [Target::AssetA, Target::AssetB, Target::ComputationModel] {
        let (status, body) = list_actions(&fixture.router, &fixture.target_query(target)).await;
        assert_eq!(
            status, 200,
            "case {target:?}: expected 200, got {status} {body}"
        );

        let expected = fixture.ordered_rows(target);
        assert_eq!(
            reported_ids(&body),
            fixture.ordered_ids(target),
            "case {target:?}: a listing must report this target's actions oldest \
             first, ties broken by id, got {body}"
        );

        let summaries = body["actionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {target:?}: no actionSummaries list in {body}"));
        for (row, summary) in expected.iter().zip(summaries) {
            assert_summary(&fixture, row, summary);
        }

        // A single page holds the whole listing, so there is nothing to resume
        // from.
        assert_eq!(
            resume_point(&body),
            None,
            "case {target:?}: a complete page must leave no resume point, got {body}"
        );
        // The member is modelled REQUIRED, so the page with nothing left to
        // resume from still carries it, as the empty token.
        assert_eq!(
            body["nextToken"],
            json!(""),
            "case {target:?}: a complete page must carry the empty token, got {body}"
        );
    }
}

#[tokio::test]
async fn a_target_resource_nothing_has_acted_on_is_an_empty_page() {
    let fixture = fixture().await;
    let model = create_model(&fixture.router, "Pump", property_catalog()).await;
    let untouched = create_asset(&fixture.router, "Pump-1", &model).await;

    let query = format!("targetResourceType=ASSET&targetResourceId={untouched}");
    let (status, body) = list_actions(&fixture.router, &query).await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    assert_eq!(
        reported_ids(&body),
        Vec::<String>::new(),
        "a stored resource nothing has acted on must report no actions, got {body}"
    );
    assert_eq!(
        resume_point(&body),
        None,
        "an empty page must leave no resume point, got {body}"
    );
}

#[tokio::test]
async fn the_resolve_to_filter_narrows_the_listing_to_its_matches() {
    let fixture = fixture().await;
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let asset_b = fixture.id_of(Target::AssetB).to_string();

    // Every form of the filter, each expected against the seeded table rather
    // than a hand-written answer: a reference to each asset, over each kind of
    // target, including one no action of the named target resolves to.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "asset A's actions resolving to A",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_a}",
                fixture.target_query(Target::AssetA)
            ),
            vec!["babababa-3333-4333-8333-333333333333".to_string()],
        ),
        (
            "asset A's actions resolving to B",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_b}",
                fixture.target_query(Target::AssetA)
            ),
            vec![
                "babababa-2222-4222-8222-222222222222".to_string(),
                "babababa-4444-4444-8444-444444444444".to_string(),
            ],
        ),
        (
            "asset B's actions resolving to A, which none does",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_a}",
                fixture.target_query(Target::AssetB)
            ),
            Vec::new(),
        ),
        (
            "the computation model's actions resolving to A",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_a}",
                fixture.target_query(Target::ComputationModel)
            ),
            vec!["babababa-7777-4777-8777-777777777777".to_string()],
        ),
        (
            "the computation model's actions resolving to B, which none does",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_b}",
                fixture.target_query(Target::ComputationModel)
            ),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let (status, body) = list_actions(&fixture.router, &query).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: unexpected matches for {query}, got {body}"
        );
    }
}

#[tokio::test]
async fn paging_a_listing_walks_the_whole_order_once_and_resumes_where_it_stopped() {
    let fixture = fixture().await;
    let order = fixture.ordered_ids(Target::AssetA);

    // Every page size from one row to one more than the listing holds, plus the
    // published upper bound of `maxResults` (range 1..250).
    for page_size in [1usize, 2, 3, order.len(), order.len() + 1, 250] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!(
                "{}&maxResults={page_size}",
                fixture.target_query(Target::AssetA)
            );
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list_actions(&fixture.router, &query).await;
            assert_eq!(
                status, 200,
                "case {page_size}: expected 200, got {status} {body}"
            );
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page must hold at most maxResults rows, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= order.len() + 1,
                "case {page_size}: paging must terminate, walked {walked:?}"
            );

            match resume_point(&body) {
                Some(next) => {
                    // The token names the first row of the next page, hex-encoded
                    // — checked against the suite's own encoder, not the
                    // handler's.
                    assert_eq!(
                        next,
                        hex_token(&order[walked.len()]),
                        "case {page_size}: unexpected resume point, got {body}"
                    );
                    token = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case {page_size}: paging must walk the whole order exactly once"
        );
        let unique: HashSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {page_size}: paging must report no row twice, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_or_filter_that_names_nothing_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let before = list_actions(&fixture.router, &fixture.target_query(Target::AssetA)).await;

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a target asset no store holds",
            format!("targetResourceType=ASSET&targetResourceId={ABSENT_ID}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a target computation model no store holds",
            format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={ABSENT_ID}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an asset id addressed as a computation model",
            format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={asset_a}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resolveTo asset no store holds",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={ABSENT_ID}",
                fixture.target_query(Target::AssetA)
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resolveTo type with no id",
            format!(
                "{}&resolveToResourceType=ASSET",
                fixture.target_query(Target::AssetA)
            ),
            400,
            "InvalidRequestException",
        ),
        (
            "a resolveTo id with no type",
            format!(
                "{}&resolveToResourceId={asset_a}",
                fixture.target_query(Target::AssetA)
            ),
            400,
            "InvalidRequestException",
        ),
        (
            "a nextToken this operation never issued",
            format!("{}&nextToken=zzzz", fixture.target_query(Target::AssetA)),
            400,
            "InvalidRequestException",
        ),
        (
            "a nextToken whose resume point is not in this enumeration",
            format!(
                "{}&nextToken={}",
                fixture.target_query(Target::AssetA),
                hex_token(ABSENT_ID)
            ),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, query, status_wanted, code_wanted) in cases {
        let (status, body) = list_actions(&fixture.router, &query).await;
        assert_eq!(
            (status, error_code(&body)),
            (status_wanted, code_wanted),
            "case {case:?}: unexpected refusal for {query}, got {body}"
        );
    }

    // A listing reports; none of the calls above — refused or served — may change
    // what the next caller sees.
    let after = list_actions(&fixture.router, &fixture.target_query(Target::AssetA)).await;
    assert_eq!(
        after, before,
        "a listing must not change what a later listing reports"
    );
}
