// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListComputationModelResolveToResources`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the listing of a computation model reports one summary per DISTINCT resource
//!   the actions executed on THAT model resolved to, as the shared store holds
//!   them: a resource acted on repeatedly is reported once, an executed action
//!   that resolved to nothing contributes none, and a resource another model's —
//!   or an asset's — executed actions resolved to is not reported here;
//! * the resolution is reported however the clock has fallen on the run itself, so
//!   an action still running has already resolved to its resource;
//! * the enumeration is ordered by the resolved resource's own id and paged by the
//!   one page walk this service's listings share;
//! * a `computationModelId` no create made is a modelled refusal, as is a
//!   `nextToken` this listing never issued;
//! * an action executed through the API shows up in that model's listing, and in
//!   no other model's;
//! * listing changes nothing a later caller sees.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `hex_token`, the
//! asset creates) is the one `asset_lifecycle` already defines, and the
//! computation-model create is the one `computation_model_execution_summary`
//! already has, re-used rather than copied.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    ActionPayload, ResolveTo, TargetResource,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ActionRecord, ExecutionRecord, ACTION_EXECUTION_DELAY, ACTIVATION_DELAY,
    EXECUTION_STATE_COMPLETED,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_catalog, router_with_engine,
};
use crate::computation_model_execution_summary::create_computation_model;
use crate::computation_model_lifecycle::id_of;
use crate::hierarchies_etags_and_refusals::error_message;

/// An id of the modelled `ID` form that no create in this suite mints, so it names
/// a resource the store does not hold.
const ABSENT_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// How many assets the fixture creates, so a row can name one of its own.
const ASSETS: usize = 8;

/// How many DISTINCT resources the busy model's executed actions resolved to —
/// what the paging table walks.
const ENTRIES: usize = 6;

/// The page size this service's listings serve when the caller sends no
/// `maxResults`. The fixture is shorter than it, so an unpaged call answers in ONE
/// page.
const DEFAULT_MAX_RESULTS: usize = 50;

/// The published maximum of `maxResults` on this listing (`@range(min: 1, max:
/// 250)`; `smithy_query(query="operation",
/// operation="ListComputationModelResolveToResources")`).
const MAX_RESULTS_MAX: usize = 250;

/// Which computation model of the fixture a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Model {
    /// The model most of the seeded executes were taken on.
    Busy,
    /// A second model, so one model's listing cannot report another's resolved
    /// resources.
    Other,
    /// A model nothing was executed on.
    Idle,
}

/// What an action was executed on: a computation model, or an asset — the other
/// kind of target `TargetResource` carries, which no model's listing may report.
#[derive(Debug, Clone, Copy)]
enum Target {
    Model(Model),
    /// The fixture asset at this index.
    Asset(usize),
}

/// One row of the seeded table: one action executed on a target resource.
#[derive(Debug, Clone)]
struct ExecuteRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The resource the action was executed on.
    target: Target,
    /// The fixture asset the action resolved to, when it resolved to one.
    resolve_to: Option<usize>,
    /// How many seconds from the fixture's anchor the run is due to end: negative
    /// for a run already over, positive for one still in flight.
    settles_in: i64,
}

/// The seeded executes: one table spanning the dimensions the rule generalises
/// over — which resource the action was executed on (two computation models, and
/// an asset), whether it resolved to a resource and which one, whether two
/// executes resolved to the SAME resource, and whether the run is over or still in
/// flight.
fn execute_table() -> Vec<ExecuteRow> {
    vec![
        ExecuteRow {
            case: "the busy model, resolving to asset 0, run over",
            target: Target::Model(Model::Busy),
            resolve_to: Some(0),
            settles_in: -600,
        },
        ExecuteRow {
            case: "the busy model, a second execute resolving to asset 0",
            target: Target::Model(Model::Busy),
            resolve_to: Some(0),
            settles_in: -500,
        },
        ExecuteRow {
            case: "the busy model, resolving to asset 1, still in flight",
            target: Target::Model(Model::Busy),
            resolve_to: Some(1),
            settles_in: 600,
        },
        ExecuteRow {
            case: "the busy model, resolving to asset 2, run over",
            target: Target::Model(Model::Busy),
            resolve_to: Some(2),
            settles_in: -400,
        },
        ExecuteRow {
            case: "the busy model, resolving to asset 3, still in flight",
            target: Target::Model(Model::Busy),
            resolve_to: Some(3),
            settles_in: 700,
        },
        ExecuteRow {
            case: "the busy model, resolving to asset 4, run over",
            target: Target::Model(Model::Busy),
            resolve_to: Some(4),
            settles_in: -300,
        },
        ExecuteRow {
            case: "the busy model, resolving to asset 5, run over",
            target: Target::Model(Model::Busy),
            resolve_to: Some(5),
            settles_in: -200,
        },
        ExecuteRow {
            case: "the busy model, resolving to nothing",
            target: Target::Model(Model::Busy),
            resolve_to: None,
            settles_in: -100,
        },
        ExecuteRow {
            case: "the other model, resolving to asset 6",
            target: Target::Model(Model::Other),
            resolve_to: Some(6),
            settles_in: -150,
        },
        ExecuteRow {
            case: "an execute on an ASSET, resolving to asset 7",
            target: Target::Asset(7),
            resolve_to: Some(7),
            settles_in: -50,
        },
    ]
}

/// The router, the assets a resolved resource can name, and the computation models
/// the seeded executes were taken on.
struct Fixture {
    router: MultiServiceRouter,
    assets: Vec<String>,
    busy: String,
    other: String,
    idle: String,
    rows: Vec<ExecuteRow>,
}

impl Fixture {
    /// The id of one of the fixture's computation models.
    fn model_id(&self, model: Model) -> &str {
        match model {
            Model::Busy => &self.busy,
            Model::Other => &self.other,
            Model::Idle => &self.idle,
        }
    }

    /// The `TargetResource` a row's execute was recorded against.
    fn target_resource(&self, target: Target) -> TargetResource {
        match target {
            Target::Model(model) => TargetResource {
                asset_id: None,
                computation_model_id: Some(self.model_id(model).to_string()),
            },
            Target::Asset(at) => TargetResource {
                asset_id: Some(self.assets[at].clone()),
                computation_model_id: None,
            },
        }
    }

    /// The distinct resolved resources the table implies for `model`, in the id
    /// order the listing enumerates them by.
    fn expected(&self, model: Model) -> Vec<String> {
        let mut ids: Vec<String> = self
            .rows
            .iter()
            .filter(|row| matches!(row.target, Target::Model(named) if named == model))
            .filter_map(|row| row.resolve_to.map(|at| self.assets[at].clone()))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// A modelled `ID` for the `at`-th seeded record of kind `kind`, so a row
/// addresses records of its own without minting any.
fn seeded_id(kind: u16, at: usize) -> String {
    format!("{:08x}-{kind:04x}-4000-8000-{:012x}", at + 1, at + 1)
}

/// Build the fixture: the assets and computation models through the API, then the
/// execute table recorded in the shared store.
///
/// Executing an action leaves TWO records of it — the action taken on the target
/// resource and the run of that action — so each row is seeded as both, exactly as
/// `ExecuteAction` writes them. The table is seeded directly because it spans runs
/// still in flight and an action executed on an asset, neither of which the API can
/// record: an asset publishes no action definition, and a run's end is owed from
/// the instant the execute was accepted.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let mut assets = Vec::with_capacity(ASSETS);
    for at in 0..ASSETS {
        assets.push(create_asset(&router, &format!("Boiler-{at}"), &asset_model).await);
    }
    let busy = create_computation_model(&router, "boiler-anomaly").await;
    let other = create_computation_model(&router, "pump-anomaly").await;
    let idle = create_computation_model(&router, "idle-anomaly").await;

    let fixture = Fixture {
        router,
        assets,
        busy,
        other,
        idle,
        rows: execute_table(),
    };

    let anchor = SystemTime::now();
    seed(&engine, &fixture, anchor);
    fixture
}

/// Record the fixture's table in the shared store, each row as the action and the
/// run an execute leaves behind.
fn seed(engine: &Arc<engine::InMemoryEngine>, fixture: &Fixture, anchor: SystemTime) {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let mut store = state.lock();
    for (at, row) in fixture.rows.iter().enumerate() {
        let target_resource = fixture.target_resource(row.target);
        let resolve_to = row.resolve_to.map(|asset| ResolveTo {
            asset_id: fixture.assets[asset].clone(),
        });
        let action_id = seeded_id(0x2222, at);
        let execution_id = seeded_id(0x1111, at);
        store.actions.insert(
            action_id.clone(),
            ActionRecord {
                action_id,
                action_definition_id: id_of('4'),
                target_resource: target_resource.clone(),
                action_payload: ActionPayload {
                    string_value: "{}".to_string(),
                },
                resolve_to: resolve_to.clone(),
                execution_time: anchor - Duration::from_secs(900),
                client_token: None,
            },
        );
        store.executions.insert(
            execution_id.clone(),
            ExecutionRecord {
                execution_id,
                action_type: Some("Amazon Web Services/ANOMALY_DETECTION_TRAINING".to_string()),
                target_resource,
                target_resource_version: "1".to_string(),
                resolve_to,
                execution_start_time: anchor - Duration::from_secs(900),
                settles_at: shifted(anchor, row.settles_in),
                outcome: EXECUTION_STATE_COMPLETED.to_string(),
                execution_entity_version: None,
            },
        );
    }
}

/// `anchor` moved by `seconds`, which may be negative.
fn shifted(anchor: SystemTime, seconds: i64) -> SystemTime {
    if seconds < 0 {
        anchor - Duration::from_secs(seconds.unsigned_abs())
    } else {
        anchor + Duration::from_secs(seconds as u64)
    }
}

/// `GET /computation-models/{id}/resolve-to-resources`, with the paging members
/// the call asks for appended.
async fn list(
    router: &MultiServiceRouter,
    computation_model_id: &str,
    page_size: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = page_size {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        query.push(format!("nextToken={token}"));
    }
    let mut uri = format!("/computation-models/{computation_model_id}/resolve-to-resources");
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(&query.join("&"));
    }
    call(router, "GET", &uri, None).await
}

/// The resolved resource ids one answer reports, in the order it reported them.
fn reported(case: &str, body: &Value) -> Vec<String> {
    body["computationModelResolveToResourceSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary list, got {body}")
        })
        .iter()
        .map(|summary| {
            summary["resolveTo"]["assetId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a resolved resource in {summary}, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// Walk the whole enumeration of `model_id` at `page_size`: the ids in the order
/// the pages reported them, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    page_size: Option<usize>,
) -> (Vec<String>, usize) {
    let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let (status, body) = list(router, model_id, page_size, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = reported(case, &body);
        pages += 1;
        assert!(
            page.len() <= limit,
            "case {case:?}: a page holds at most {limit} entries, got {} in {body}",
            page.len()
        );
        seen.extend(page.iter().cloned());

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // A token is issued only while an entry follows the page, and a page
            // another entry follows is a FULL page.
            Some(_) => assert_eq!(
                page.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
        assert!(
            pages <= ENTRIES + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (seen, pages)
}

#[tokio::test]
async fn each_distinct_resource_an_executed_action_resolved_to_is_listed_once() {
    let fixture = fixture().await;

    for model in [Model::Busy, Model::Other, Model::Idle] {
        let case = format!("{model:?}");
        let model_id = fixture.model_id(model).to_string();
        let (status, body) = list(&fixture.router, &model_id, Some(MAX_RESULTS_MAX), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let expected = fixture.expected(model);
        let got = reported(&case, &body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the distinct resolved resources {expected:?} the store \
             holds, in id order, got {got:?}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an enumeration that fits one page leaves no token, got {body}"
        );
    }

    // The busy model's executes span the dimensions the table names, so its
    // enumeration is the one the paging suite below walks.
    let busy = fixture.expected(Model::Busy);
    assert_eq!(
        busy.len(),
        ENTRIES,
        "the fixture must imply {ENTRIES} distinct resolved resources, got {busy:?}"
    );

    // Row by row: a resource an executed action resolved to is reported by the
    // listing of the computation model that action was executed ON, and an action
    // executed on an ASSET is reported by no model's listing at all.
    let answers: Vec<(Model, Vec<String>)> = {
        let mut answers = Vec::new();
        for model in [Model::Busy, Model::Other, Model::Idle] {
            let case = format!("{model:?}");
            let (_, body) = list(&fixture.router, fixture.model_id(model), Some(MAX_RESULTS_MAX), None).await;
            answers.push((model, reported(&case, &body)));
        }
        answers
    };
    for row in &fixture.rows {
        let case = row.case;
        let Some(at) = row.resolve_to else {
            // An executed action that resolved to nothing contributes no summary,
            // which the whole-enumeration equality above already pins.
            continue;
        };
        let asset = fixture.assets[at].clone();
        let executed_on = match row.target {
            Target::Model(model) => Some(model),
            Target::Asset(_) => None,
        };
        for (model, listed) in &answers {
            let owed = executed_on == Some(*model);
            assert_eq!(
                listed.contains(&asset),
                owed,
                "case {case:?}: the listing of {model:?} must {} {asset}, got {listed:?}",
                if owed { "report" } else { "not report" }
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_one_stable_enumeration_whatever_the_page_size() {
    let fixture = fixture().await;
    let model_id = fixture.model_id(Model::Busy).to_string();
    let reference = fixture.expected(Model::Busy);

    // Rows differ in page size: the published minimum, sizes that divide the
    // enumeration and sizes that do not, exactly it, one past it, the published
    // maximum, and no `maxResults` at all.
    for page_size in [
        Some(1_usize),
        Some(2),
        Some(4),
        Some(ENTRIES),
        Some(ENTRIES + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let case = format!("the busy model at {page_size:?}");
        let (seen, pages) = walk(&fixture.router, &case, &model_id, page_size).await;
        assert_eq!(
            seen, reference,
            "case {case:?}: the pages joined must be the whole enumeration once, in one order, \
             got {seen:?}"
        );
        let expected_pages = ENTRIES.div_ceil(page_size.unwrap_or(DEFAULT_MAX_RESULTS));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages over {ENTRIES} entries, walked {pages}"
        );
    }

    // The token a page issues is the opaque hex this service's listings share, and
    // it resumes at the entry AFTER the page it came with.
    let (status, first) = list(&fixture.router, &model_id, Some(1), None).await;
    assert_eq!(status, 200, "the first page: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one of {ENTRIES} entries carries a token, got {first}"))
        .to_string();
    assert!(
        !token.is_empty() && token.len() % 2 == 0 && token.chars().all(|ch| ch.is_ascii_hexdigit()),
        "expected an even-length hex token, got {token:?}"
    );
    let (status, resumed) = list(&fixture.router, &model_id, Some(MAX_RESULTS_MAX), Some(&token)).await;
    assert_eq!(status, 200, "the resumed page: got {status} {resumed}");
    assert_eq!(
        reported("the resumed page", &resumed),
        reference[1..].to_vec(),
        "the token must resume at the second entry, got {resumed}"
    );
}

/// One row of the table of refusals: what the request gets wrong, and the answer
/// the rule owes it.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    /// Which model the request names, or `None` for an id no create made.
    model: Option<Model>,
    /// The `nextToken` the request carries, when it carries one.
    token: Option<String>,
    status: u16,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_listing_of_what_the_store_does_not_hold_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let reference = fixture.expected(Model::Busy);
    // The one wording every paginated listing of this service refuses an unusable
    // token with.
    let bad_token = "The nextToken is not a token this operation issued.".to_string();

    // Rows differ in WHAT the request gets wrong: the model it names, and — for a
    // token — how the token is wrong (not hex at all, an odd-length hex run, the hex
    // of a resolved resource this enumeration does not hold, and the hex of text
    // that is no key of any enumeration).
    let cases = vec![
        RefusalCase {
            case: "a computationModelId no create made",
            model: None,
            token: None,
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Computation Model with ID {ABSENT_ID} not found"),
        },
        RefusalCase {
            case: "a computationModelId no create made, with a token",
            model: None,
            token: Some(hex_token(&reference[0])),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Computation Model with ID {ABSENT_ID} not found"),
        },
        RefusalCase {
            case: "a token that is not hex at all",
            model: Some(Model::Busy),
            token: Some("zzzz".to_string()),
            status: 400,
            code: "InvalidRequestException",
            message: bad_token.clone(),
        },
        RefusalCase {
            case: "a token that is an odd-length hex run",
            model: Some(Model::Busy),
            token: Some("abc".to_string()),
            status: 400,
            code: "InvalidRequestException",
            message: bad_token.clone(),
        },
        RefusalCase {
            case: "the hex of a resource this model's actions did not resolve to",
            model: Some(Model::Busy),
            token: Some(hex_token(&fixture.assets[6])),
            status: 400,
            code: "InvalidRequestException",
            message: bad_token.clone(),
        },
        RefusalCase {
            case: "the hex of text that is no key of any enumeration",
            model: Some(Model::Busy),
            token: Some(hex_token("no-such-entry")),
            status: 400,
            code: "InvalidRequestException",
            message: bad_token.clone(),
        },
        RefusalCase {
            case: "a token on a model nothing was executed on",
            model: Some(Model::Idle),
            token: Some(hex_token(&reference[0])),
            status: 400,
            code: "InvalidRequestException",
            message: bad_token,
        },
    ];

    for refusal in &cases {
        let case = refusal.case;
        let model_id = refusal
            .model
            .map(|model| fixture.model_id(model).to_string())
            .unwrap_or_else(|| ABSENT_ID.to_string());
        let (status, body) = list(
            &fixture.router,
            &model_id,
            None,
            refusal.token.as_deref(),
        )
        .await;
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected {}, got {status} {body}",
            refusal.status
        );
        assert!(
            error_code(&body).ends_with(refusal.code),
            "case {case:?}: expected {}, got {body}",
            refusal.code
        );
        assert_eq!(
            error_message(&body),
            refusal.message.as_str(),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }

    // The refusals changed nothing a later caller sees.
    let (again, _) = walk(
        &fixture.router,
        "after the refusals",
        fixture.model_id(Model::Busy),
        Some(MAX_RESULTS_MAX),
    )
    .await;
    assert_eq!(
        again, reference,
        "the refusals must leave the enumeration as it was, got {again:?}"
    );
}

#[tokio::test]
async fn an_action_executed_through_the_api_shows_in_that_models_resolved_resources() {
    let fixture = fixture().await;
    let model_id = create_computation_model(&fixture.router, "fresh-anomaly").await;

    // A model nothing has been executed on resolves to nothing.
    let (status, body) = list(&fixture.router, &model_id, None, None).await;
    assert_eq!(status, 200, "a fresh model: got {status} {body}");
    assert!(
        reported("a fresh model", &body).is_empty(),
        "a fresh model: expected no resolved resources, got {body}"
    );

    // An action is executed only on a settled model, so wait for the transition the
    // create owes, then take the actions the model's computation type publishes.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let (status, described) = call(
        &fixture.router,
        "GET",
        &format!("/computation-models/{model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {model_id}: got {status} {described}");
    let definition = described["actionDefinitions"]
        .as_array()
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition["actionDefinitionId"].as_str())
        .unwrap_or_else(|| panic!("no actionDefinitions in {described}"))
        .to_string();

    // Four executes: two resolving to ONE asset, one resolving to another, and one
    // resolving to nothing — so the listing has to report the two distinct resources
    // and nothing else.
    let first = fixture.assets[0].clone();
    let second = fixture.assets[1].clone();
    for (at, resolve_to) in [
        (0, Some(first.clone())),
        (1, Some(first.clone())),
        (2, Some(second.clone())),
        (3, None),
    ] {
        let mut request = json!({
            "targetResource": { "computationModelId": model_id },
            "actionDefinitionId": definition,
            "actionPayload": { "stringValue": "{}" },
        });
        if let Some(asset_id) = resolve_to {
            request["resolveTo"] = json!({ "assetId": asset_id });
        }
        let (status, taken) = call(&fixture.router, "POST", "/actions", Some(request)).await;
        assert_eq!(status, 202, "execute action {at}: got {status} {taken}");
    }

    let mut expected = vec![first, second];
    expected.sort();

    // Straight after the executes, while the runs are still in flight, the
    // resolutions are already reported: the action resolved to the resource when it
    // was executed.
    let (status, body) = list(&fixture.router, &model_id, None, None).await;
    assert_eq!(status, 200, "after the executes: got {status} {body}");
    assert_eq!(
        reported("after the executes", &body),
        expected,
        "after the executes: expected the two distinct resources {expected:?}, got {body}"
    );

    // And once the runs are due to have ended, the same two.
    tokio::time::sleep(ACTION_EXECUTION_DELAY + Duration::from_millis(250)).await;
    let (status, body) = list(&fixture.router, &model_id, None, None).await;
    assert_eq!(status, 200, "after the runs ended: got {status} {body}");
    assert_eq!(
        reported("after the runs ended", &body),
        expected,
        "after the runs ended: expected the same two resources {expected:?}, got {body}"
    );

    // One model's executes are not another's: the model the seeded table calls idle
    // still resolves to nothing.
    let (status, body) = list(&fixture.router, fixture.model_id(Model::Idle), None, None).await;
    assert_eq!(status, 200, "the idle model: got {status} {body}");
    assert!(
        reported("the idle model", &body).is_empty(),
        "the idle model: another model's resolved resources must not be reported, got {body}"
    );
}

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    let model_id = fixture.model_id(Model::Busy).to_string();
    let executions_query =
        format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={model_id}");

    let (_, before) = call(
        &fixture.router,
        "GET",
        &format!("/executions?{executions_query}"),
        None,
    )
    .await;

    let (status, first) = list(&fixture.router, &model_id, None, None).await;
    assert_eq!(status, 200, "the first read: got {status} {first}");
    let (status, second) = list(&fixture.router, &model_id, None, None).await;
    assert_eq!(status, 200, "the second read: got {status} {second}");
    assert_eq!(
        reported("the first read", &first),
        reported("the second read", &second),
        "two reads of one listing must agree, got {first} then {second}"
    );

    let (_, after) = call(
        &fixture.router,
        "GET",
        &format!("/executions?{executions_query}"),
        None,
    )
    .await;
    assert_eq!(
        before["executionSummaries"], after["executionSummaries"],
        "listing the resolved resources must leave the executions it read untouched, got \
         {before} then {after}"
    );
}
