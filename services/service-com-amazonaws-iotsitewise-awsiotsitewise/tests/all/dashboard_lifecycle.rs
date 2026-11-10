// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise Monitor dashboard slice
//! (`CreateDashboard` -> `DescribeDashboard` -> `UpdateDashboard`), driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created dashboard is persisted and reads back exactly as it was created —
//!   its project, its name, its description and its definition literal — with the
//!   ARN form `CreateDashboard` documents, and a read never changes what the next
//!   caller sees;
//! * an update REPLACES the members a caller owns (the name and the definition it
//!   must carry, the description it may) and leaves the dashboard's identity and
//!   provenance alone: same id, same ARN, same project, same tags, same creation
//!   date, with the last-update date moved to the instant of the write; an
//!   omitted description clears the stored one, because the request states the
//!   dashboard's new state rather than patching selected members;
//! * an id that resolves to no stored dashboard is a modelled 404 on the read and
//!   on the write alike, and a create naming a project this store does not hold is
//!   the same refusal, rather than a fabricated answer;
//! * a member outside its modelled bounds is refused with a client error and
//!   stores nothing, while the same member AT its bound is accepted;
//! * the store is bounded: with the dashboard map at its ceiling a create is the
//!   modelled quota refusal, and it is accepted again as soon as there is room;
//! * `clientToken` makes the create idempotent: a RETRY — the same request under
//!   the token of a create this store already served — answers from the dashboard
//!   THAT create minted and writes nothing, while a create carrying no token, a
//!   token no create has spent, or a request differing from the call its token was
//!   spent on mints a dashboard of its own.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    dashboard_arn, service_state, DashboardRecord, MAX_DASHBOARDS,
};

use super::asset_lifecycle::{
    call, error_code, is_modelled_id, router, router_with_engine, REGION,
};

/// Where every dashboard request goes, as `OPERATIONS.md` reports the routes:
/// `POST /dashboards`, `GET|PUT /dashboards/{dashboardId}`.
const DASHBOARDS: &str = "/dashboards";

/// The published maximum of `dashboardName`. Source: `smithy_query(query="operation",
/// operation="CreateDashboard")` — `dashboardName` targets `Name`, len 1..256.
const NAME_MAX: usize = 256;

/// The published maximum of `dashboardDescription`. Source: the same call —
/// `dashboardDescription` targets `Description`, len 1..2048.
const DESCRIPTION_MAX: usize = 2048;

/// The published maximum of `dashboardDefinition`. Source: the same call —
/// `dashboardDefinition` targets `DashboardDefinition`, len 0..204800, and its
/// `^.+$` pattern is what makes the empty literal a refusal despite the 0 floor.
const DEFINITION_MAX: usize = 204_800;

/// The floor of `clientToken`'s modelled length. Source:
/// `smithy_query(query="operation", operation="CreateDashboard")` — `clientToken`
/// targets `ClientToken`, len 36..64, pattern `^\S{36,64}$`.
const CLIENT_TOKEN_MIN: usize = 36;

/// A `clientToken` built from `seed`: a non-space string padded out to
/// [`CLIENT_TOKEN_MIN`], so a row states a token without restating the bound.
fn client_token(seed: usize) -> String {
    let token = format!("retry-token-{seed}-");
    format!(
        "{token}{}",
        "0".repeat(CLIENT_TOKEN_MIN.saturating_sub(token.chars().count()))
    )
}

/// How many dashboards the shared store holds — what proves a replay wrote
/// nothing.
fn stored_dashboards(engine: &std::sync::Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the IoT SiteWise store")
        .lock()
        .dashboards
        .len()
}

/// A modelled `ID` built from `seed`: a lowercase hyphenated UUID that is not the
/// all-zero one the pattern refuses. Used for the ids expected to resolve to
/// nothing — a dashboard no create minted and a project no create minted.
pub(crate) fn modelled_id(seed: u32) -> String {
    format!("{seed:08x}-1c2d-4e5f-8a9b-0c1d2e3f4a5b")
}

/// A dashboard definition literal of `length` characters: a JSON object whose one
/// property is padded out, so the row spans a size without leaving the form a
/// definition takes.
pub(crate) fn definition_of(length: usize) -> String {
    let envelope = r#"{"widgets":[{"title":""}]}"#;
    let padding = length.saturating_sub(envelope.len());
    format!(
        r#"{{"widgets":[{{"title":"{}"}}]}}"#,
        "w".repeat(padding)
    )
}

/// One row of the dashboard table.
#[derive(Debug, Clone)]
struct DashboardCase {
    label: &'static str,
    project_id: String,
    name: String,
    description: Option<String>,
    definition: String,
    /// The `tags` the create carries, if any.
    tags: Option<Value>,
    /// The `clientToken` the create carries, if any.
    client_token: Option<String>,
}

/// The rows, differing along what the rule generalises over: the name's
/// characters and length, whether the optional description is carried at all and
/// how long it is, the size and shape of the definition literal, the project the
/// dashboard belongs to, and whether the create carries tags and a client token.
/// The length rows sit exactly AT the published bounds above.
///
/// `projects` are the stored projects the rows are spread across, so the table
/// also spans "two dashboards of one project" and "dashboards of different
/// projects".
fn dashboard_cases(projects: &[String]) -> Vec<DashboardCase> {
    let project = |at: usize| projects[at % projects.len()].clone();
    vec![
        DashboardCase {
            label: "minimal",
            project_id: project(0),
            name: "ops-overview".to_string(),
            description: None,
            definition: r#"{"widgets":[]}"#.to_string(),
            tags: None,
            client_token: None,
        },
        DashboardCase {
            label: "described-and-tagged",
            project_id: project(1),
            name: "line 3 / kiln temperatures".to_string(),
            description: Some("Temperatures across the kiln line".to_string()),
            definition: definition_of(512),
            tags: Some(json!({ "stage": "test", "owner": "plant-ops" })),
            client_token: Some(format!("client-token-{:0>36}", 2)),
        },
        DashboardCase {
            label: "unicode-name-and-description",
            project_id: project(2),
            name: "Übersicht — 工場 ①".to_string(),
            description: Some("Vue d'ensemble de l'usine — 概要".to_string()),
            definition: definition_of(2_048),
            tags: Some(json!({ "équipe": "maintenance" })),
            client_token: None,
        },
        DashboardCase {
            label: "name-at-its-maximum",
            project_id: project(0),
            name: "n".repeat(NAME_MAX),
            description: None,
            definition: definition_of(64),
            tags: None,
            client_token: Some(format!("client-token-{:0>36}", 4)),
        },
        DashboardCase {
            label: "description-at-its-maximum",
            project_id: project(3),
            name: "single-widget".to_string(),
            description: Some("d".repeat(DESCRIPTION_MAX)),
            definition: definition_of(8_192),
            tags: Some(json!({ "slot": "5" })),
            client_token: None,
        },
        DashboardCase {
            label: "definition-at-its-maximum",
            project_id: project(4),
            name: "every-asset".to_string(),
            description: Some("x".to_string()),
            definition: definition_of(DEFINITION_MAX),
            tags: None,
            client_token: None,
        },
    ]
}

/// The create body one row sends.
fn create_body(case: &DashboardCase) -> Value {
    let mut body = Map::new();
    body.insert(
        "projectId".to_string(),
        Value::String(case.project_id.clone()),
    );
    body.insert(
        "dashboardName".to_string(),
        Value::String(case.name.clone()),
    );
    body.insert(
        "dashboardDefinition".to_string(),
        Value::String(case.definition.clone()),
    );
    if let Some(description) = case.description.clone() {
        body.insert(
            "dashboardDescription".to_string(),
            Value::String(description),
        );
    }
    if let Some(tags) = case.tags.clone() {
        body.insert("tags".to_string(), tags);
    }
    if let Some(token) = case.client_token.clone() {
        body.insert("clientToken".to_string(), Value::String(token));
    }
    Value::Object(body)
}

/// The update body that rewrites a dashboard into `case`: every member
/// `UpdateDashboard` carries, and the description only when the row has one.
fn update_body(case: &DashboardCase) -> Value {
    let mut body = Map::new();
    body.insert(
        "dashboardName".to_string(),
        Value::String(case.name.clone()),
    );
    body.insert(
        "dashboardDefinition".to_string(),
        Value::String(case.definition.clone()),
    );
    if let Some(description) = case.description.clone() {
        body.insert(
            "dashboardDescription".to_string(),
            Value::String(description),
        );
    }
    Value::Object(body)
}

/// `CreateDashboard` with `body`.
async fn create_dashboard(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", DASHBOARDS, Some(body)).await
}

/// `DescribeDashboard` on `id`. Reached by the sibling reporting suite too,
/// rather than letting it grow a second copy of this request.
pub(crate) async fn describe_dashboard(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{DASHBOARDS}/{id}"), None).await
}

/// `UpdateDashboard` on `id` with `body`.
async fn update_dashboard(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("{DASHBOARDS}/{id}"), Some(body)).await
}

/// `ListTagsForResource` on `arn`, the reader the create's `tags` are proved
/// through.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// Where the portal and the projects a dashboard lives in are created, as
/// `OPERATIONS.md` reports the routes: `POST /portals`, `POST /projects`.
const PORTALS: &str = "/portals";
/// See [`PORTALS`].
const PROJECTS: &str = "/projects";

/// Create a portal and `count` projects in it, and return the project ids: a
/// dashboard is created IN a project, so these are the containers the rows name.
///
/// `pub(crate)` so the sibling dashboard suites reach it rather than growing a
/// second copy of this seeding.
pub(crate) async fn seed_projects(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let (status, portal) = call(
        router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": "portal-for-dashboards",
            "portalContactEmail": "admin@example.com",
            "roleArn": "arn:aws:iam::123456789012:role/service-role-monitor",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreatePortal expected 202, got {status} {portal}"
    );
    let portal_id = portal["portalId"].as_str().unwrap_or_default().to_string();

    let mut projects = Vec::new();
    for at in 0..count {
        let (status, project) = call(
            router,
            "POST",
            PROJECTS,
            Some(json!({
                "portalId": portal_id,
                "projectName": format!("project-for-dashboards-{at}"),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "CreateProject expected 201, got {status} {project}"
        );
        projects.push(project["projectId"].as_str().unwrap_or_default().to_string());
    }
    projects
}

/// Create the dashboard `case` describes and return `(id, arn)`.
async fn seed(router: &MultiServiceRouter, case: &DashboardCase) -> (String, String) {
    let (status, created) = create_dashboard(router, create_body(case)).await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateDashboard expected 201, got {status} {created}",
        case.label
    );
    (
        created["dashboardId"].as_str().unwrap_or_default().to_string(),
        created["dashboardArn"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test]
async fn created_dashboard_reads_back_through_describe_for_every_input_shape() {
    let router = router();
    let projects = seed_projects(&router, 3).await;
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for case in dashboard_cases(&projects) {
        let body = create_body(&case);
        let (status, created) = create_dashboard(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {status} {created}",
            case.label
        );

        let id = created["dashboardId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: dashboardId must be a modelled UUID, got {created}",
            case.label
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: dashboardId {id} was already minted for another dashboard",
            case.label
        );
        let arn = created["dashboardArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:dashboard/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {:?}: unexpected dashboardArn in {created}",
            case.label
        );

        let (status, described) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeDashboard expected 200, got {status}",
            case.label
        );
        assert_eq!(
            described["dashboardId"].as_str().unwrap_or_default(),
            id,
            "case {:?}: the read reported another id in {described}",
            case.label
        );
        assert_eq!(
            described["dashboardArn"].as_str().unwrap_or_default(),
            arn,
            "case {:?}: the read reported another ARN in {described}",
            case.label
        );
        for member in [
            "projectId",
            "dashboardName",
            "dashboardDefinition",
            "dashboardDescription",
        ] {
            assert_eq!(
                &described[member], &body[member],
                "case {:?}: {member} must read back as it was sent, got {:?}",
                case.label, described[member]
            );
        }

        let creation = described["dashboardCreationDate"]
            .as_f64()
            .unwrap_or_default();
        let last_update = described["dashboardLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            creation > 0.0,
            "case {:?}: dashboardCreationDate must be a real instant, got {creation}",
            case.label
        );
        assert_eq!(
            creation, last_update,
            "case {:?}: an untouched dashboard's lastUpdateDate is its creationDate, got \
             {last_update}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let (status, again) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second read expected 200, got {status}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: DescribeDashboard answered differently on the second read",
            case.label
        );

        // The tags the create carried are persisted on the dashboard, and a
        // dashboard created without tags carries an empty set rather than another
        // dashboard's.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: unexpected tag set in {listed}",
            case.label
        );
    }
}

#[tokio::test]
async fn update_replaces_every_caller_owned_member_and_keeps_the_dashboard_s_identity() {
    let router = router();
    let projects = seed_projects(&router, 3).await;
    let cases = dashboard_cases(&projects);

    for (at, case) in cases.iter().enumerate() {
        let (id, arn) = seed(&router, case).await;
        let (status, before) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read before the update expected 200, got {status}",
            case.label
        );

        // The update states the dashboard's NEW name, description and definition:
        // the next row's, so every write differs from what it replaces along each
        // member at once — including the rows that carry no description, which is
        // what proves an omitted description clears the stored one.
        let next = &cases[(at + 1) % cases.len()];
        let update = update_body(next);
        // Far enough apart that the two instants cannot render as one timestamp.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, updated) = update_dashboard(&router, &id, update.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateDashboard expected 200, got {status} {updated}",
            case.label
        );

        let (status, after) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read after the update expected 200, got {status}",
            case.label
        );
        for member in ["dashboardName", "dashboardDefinition"] {
            assert_eq!(
                &after[member], &update[member],
                "case {:?}: {member} must read back as the update stated it, got {:?}",
                case.label, after[member]
            );
        }
        match next.description.as_deref() {
            Some(description) => assert_eq!(
                after["dashboardDescription"].as_str(),
                Some(description),
                "case {:?}: the update's description must read back, got {:?}",
                case.label,
                after["dashboardDescription"]
            ),
            None => assert!(
                after.get("dashboardDescription").is_none(),
                "case {:?}: an update carrying no description must clear the stored one, got {:?}",
                case.label,
                after["dashboardDescription"]
            ),
        }
        for member in ["dashboardId", "dashboardArn", "projectId", "dashboardCreationDate"] {
            assert_eq!(
                &after[member], &before[member],
                "case {:?}: the update must leave {member} alone, got {:?}",
                case.label, after[member]
            );
        }
        let before_update = before["dashboardLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        let after_update = after["dashboardLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            after_update > before_update,
            "case {:?}: the update must move lastUpdateDate past {before_update}, got \
             {after_update}",
            case.label
        );

        // The tags the create carried are the create's, not the update's to
        // change: the update carries no `tags` member at all.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: the update must leave the tag set alone, got {listed}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_a_modelled_not_found() {
    let router = router();
    let projects = seed_projects(&router, 2).await;
    let cases = dashboard_cases(&projects);
    let case = &cases[0];
    let (stored_id, _) = seed(&router, case).await;
    let (_, stored) = describe_dashboard(&router, &stored_id).await;

    for unknown in [modelled_id(0x90), modelled_id(0x91)] {
        let expected = format!("Dashboard with ID {unknown} not found");

        let (status, refused) = describe_dashboard(&router, &unknown).await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: DescribeDashboard expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {unknown:?}: unexpected refusal message in {refused}"
        );

        let (status, refused) =
            update_dashboard(&router, &unknown, update_body(&cases[1])).await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: UpdateDashboard expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {unknown:?}: unexpected refusal message in {refused}"
        );
    }

    // A dashboard is created IN a project, so a `projectId` that resolves to no
    // stored project is the same modelled 404, naming the project the request
    // supplied.
    for unknown_project in [modelled_id(0xa0), modelled_id(0xa1)] {
        let mut body = create_body(case);
        body["projectId"] = Value::String(unknown_project.clone());
        let (status, refused) = create_dashboard(&router, body).await;
        assert_eq!(
            status, 404,
            "case {unknown_project:?}: CreateDashboard expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {unknown_project:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Project with ID {unknown_project} not found"),
            "case {unknown_project:?}: unexpected refusal message in {refused}"
        );
    }

    // A refusal stores nothing and changes nothing: the seeded dashboard is
    // exactly as it was.
    let (status, again) = describe_dashboard(&router, &stored_id).await;
    assert_eq!(status, 200, "the seeded dashboard expected 200, got {status}");
    assert_eq!(
        again, stored,
        "a refused read or write must leave the stored dashboard alone"
    );
}

/// One row of the bounds table: the member, a value AT its published bound and a
/// value one character PAST it.
fn bound_cases() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "dashboardName",
            "n".repeat(NAME_MAX),
            "n".repeat(NAME_MAX + 1),
        ),
        (
            "dashboardDescription",
            "d".repeat(DESCRIPTION_MAX),
            "d".repeat(DESCRIPTION_MAX + 1),
        ),
        (
            "dashboardDefinition",
            definition_of(DEFINITION_MAX),
            definition_of(DEFINITION_MAX + 1),
        ),
    ]
}

#[tokio::test]
async fn a_member_past_its_modelled_bound_is_refused_and_stores_nothing() {
    let router = router();
    let projects = seed_projects(&router, 2).await;
    let cases = dashboard_cases(&projects);
    let base = &cases[1];
    let (stored_id, _) = seed(&router, base).await;
    let (_, stored) = describe_dashboard(&router, &stored_id).await;

    for (at, (member, at_bound, past_bound)) in bound_cases().into_iter().enumerate() {
        // AT the bound the member is accepted, on the create and on the update.
        // Under a token of its own, so the row is a create rather than a retry of
        // the seeded one, whose token `base` already spent.
        let mut accepted = create_body(base);
        accepted[member] = Value::String(at_bound.clone());
        accepted["clientToken"] = Value::String(client_token(0xb0 + at));
        let (status, created) = create_dashboard(&router, accepted).await;
        assert_eq!(
            status, 201,
            "case {member:?} at its bound: CreateDashboard expected 201, got {status} {created}"
        );

        // One character PAST it the request is refused, with a client error and
        // without storing anything.
        let mut refused_body = create_body(base);
        refused_body[member] = Value::String(past_bound.clone());
        let (status, refused) = create_dashboard(&router, refused_body).await;
        assert_eq!(
            status, 400,
            "case {member:?} past its bound: CreateDashboard expected 400, got {status}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "InvalidRequestException",
            "case {member:?} past its bound: CreateDashboard refused with {code:?}, expected \
             the client-input error this operation models"
        );
        let mut refused_update = update_body(base);
        refused_update[member] = Value::String(past_bound);
        let (status, refused) = update_dashboard(&router, &stored_id, refused_update).await;
        assert_eq!(
            status, 400,
            "case {member:?} past its bound: UpdateDashboard expected 400, got {status}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "InvalidRequestException",
            "case {member:?} past its bound: UpdateDashboard refused with {code:?}, expected \
             the client-input error this operation models"
        );

        let (status, again) = describe_dashboard(&router, &stored_id).await;
        assert_eq!(
            status, 200,
            "case {member:?}: the seeded dashboard expected 200, got {status}"
        );
        assert_eq!(
            again, stored,
            "case {member:?}: a refused write must leave the stored dashboard alone"
        );
    }
}

/// A seeded dashboard record, as the ceiling suite fills the map with: everything
/// a create would have written, for the id `at` names.
fn seeded_record(at: usize) -> DashboardRecord {
    let id = format!("{at:08x}-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let now = SystemTime::now();
    DashboardRecord {
        arn: dashboard_arn(REGION, &id),
        id,
        project_id: modelled_id(0x5eed),
        name: format!("seeded-{at}"),
        description: None,
        definition: r#"{"widgets":[]}"#.to_string(),
        creation_date: now,
        last_update_date: now,
        client_token: None,
        tags: HashMap::new(),
    }
}

#[tokio::test]
async fn a_create_past_the_dashboard_ceiling_is_the_modelled_quota_refusal() {
    let (router, engine) = router_with_engine();
    let projects = seed_projects(&router, 1).await;
    let cases = dashboard_cases(&projects);
    let case = &cases[0];

    // The map is filled to its ceiling through the shared store rather than with
    // `MAX_DASHBOARDS` HTTP creates: the bound is what is under test, not the
    // thousand requests it would take to reach it over the wire.
    {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        for at in 0..MAX_DASHBOARDS {
            let record = seeded_record(at);
            store.dashboards.insert(record.id.clone(), record);
        }
        assert_eq!(
            store.dashboards.len(),
            MAX_DASHBOARDS,
            "the seeded ids must be distinct, the store holds {}",
            store.dashboards.len()
        );
    }

    let (status, refused) = create_dashboard(&router, create_body(case)).await;
    assert_eq!(
        status, 410,
        "a create at the ceiling expected 410, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a create at the ceiling: unexpected error code in {refused}"
    );
    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .dashboards
        .len();
    assert_eq!(
        held, MAX_DASHBOARDS,
        "a refused create must store nothing, the store holds {held}"
    );

    // With room again the same create is accepted, so the refusal is the bound
    // and not a dead end.
    let freed = seeded_record(0).id;
    service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .dashboards
        .remove(&freed);
    let (status, created) = create_dashboard(&router, create_body(case)).await;
    assert_eq!(
        status, 201,
        "a create with room again expected 201, got {status} {created}"
    );
    let id = created["dashboardId"].as_str().unwrap_or_default();
    assert!(
        is_modelled_id(id),
        "a create with room again must mint a modelled id, got {created}"
    );
    let (status, described) = describe_dashboard(&router, id).await;
    assert_eq!(
        status, 200,
        "the dashboard created with room again expected 200, got {status} {described}"
    );
}

#[tokio::test]
async fn a_retry_under_a_spent_client_token_replays_the_create_and_writes_nothing() {
    let (router, engine) = router_with_engine();
    let projects = seed_projects(&router, 3).await;

    // Every row of the table, each under a token of its own: what the retry is
    // recognised by does not depend on what else the create carried.
    for (at, case) in dashboard_cases(&projects).iter().enumerate() {
        let mut body = create_body(case);
        body["clientToken"] = Value::String(client_token(at));

        let (status, created) = create_dashboard(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateDashboard expected 201, got {status} {created}",
            case.label
        );
        let id = created["dashboardId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let held = stored_dashboards(&engine);
        let (status, described) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeDashboard expected 200, got {status} {described}",
            case.label
        );

        // The SAME request again, under the SAME token: the answer the first
        // call gave, and nothing new in the store.
        let (status, replayed) = create_dashboard(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: a retry expected 201, got {status} {replayed}",
            case.label
        );
        assert_eq!(
            replayed, created,
            "case {:?}: a retry under a spent clientToken must replay the first answer, got \
             {replayed}",
            case.label
        );
        assert_eq!(
            stored_dashboards(&engine),
            held,
            "case {:?}: a replayed create must mint no second dashboard, the store holds {}",
            case.label,
            stored_dashboards(&engine)
        );

        // A request differing from the call the token was spent on repeats no
        // call: it is served as the create it is, mints a dashboard of its own,
        // and leaves the first one exactly as the first call left it.
        let mut differing = body.clone();
        differing["dashboardName"] = Value::String(format!("rewritten-{at}"));
        differing["dashboardDefinition"] = Value::String(definition_of(128));
        let (status, answered) = create_dashboard(&router, differing).await;
        assert_eq!(
            status, 201,
            "case {:?}: a differing create under the spent token expected 201, got {status} \
             {answered}",
            case.label
        );
        assert_ne!(
            answered["dashboardId"], created["dashboardId"],
            "case {:?}: a differing create must answer with its OWN dashboardId, got {answered} \
             after {created}",
            case.label
        );
        assert_eq!(
            stored_dashboards(&engine),
            held + 1,
            "case {:?}: a differing create must mint one dashboard of its own, the store holds {}",
            case.label,
            stored_dashboards(&engine)
        );
        let minted_id = answered["dashboardId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, minted) = describe_dashboard(&router, &minted_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the dashboard the differing create minted expected 200, got {status} \
             {minted}",
            case.label
        );
        assert_eq!(
            minted["dashboardName"],
            Value::String(format!("rewritten-{at}")),
            "case {:?}: that dashboard must hold what THIS request asked for, got {minted}",
            case.label
        );
        let (status, again) = describe_dashboard(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeDashboard expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: a differing create must leave the first dashboard alone",
            case.label
        );
    }
}

#[tokio::test]
async fn a_create_with_no_token_or_an_unspent_one_mints_a_dashboard_of_its_own() {
    let (router, engine) = router_with_engine();
    let projects = seed_projects(&router, 3).await;

    for (at, case) in dashboard_cases(&projects).iter().enumerate() {
        let mut minted = BTreeSet::new();
        let held = stored_dashboards(&engine);

        // No token at all: nothing recognises the call as a retry, so the same
        // request twice over is two dashboards.
        let mut untokened = create_body(case);
        untokened
            .as_object_mut()
            .expect("the create body is a JSON object")
            .remove("clientToken");
        // Two tokens no create has spent: likewise two dashboards, so it is the
        // token's having been SPENT that replays, not its being carried.
        let mut first_token = create_body(case);
        first_token["clientToken"] = Value::String(client_token(0x100 + at));
        let mut second_token = create_body(case);
        second_token["clientToken"] = Value::String(client_token(0x200 + at));

        for (label, body) in [
            ("no token, first call", untokened.clone()),
            ("no token, second call", untokened),
            ("an unspent token", first_token),
            ("another unspent token", second_token),
        ] {
            let (status, created) = create_dashboard(&router, body).await;
            assert_eq!(
                status, 201,
                "case {:?} / {label}: CreateDashboard expected 201, got {status} {created}",
                case.label
            );
            let id = created["dashboardId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_modelled_id(&id),
                "case {:?} / {label}: expected a modelled id, got {created}",
                case.label
            );
            let (status, described) = describe_dashboard(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {:?} / {label}: DescribeDashboard expected 200, got {status} {described}",
                case.label
            );
            assert!(
                minted.insert(id.clone()),
                "case {:?} / {label}: {id} was already minted for another create",
                case.label
            );
        }

        assert_eq!(
            minted.len(),
            4,
            "case {:?}: expected four distinct dashboards, got {minted:?}",
            case.label
        );
        assert_eq!(
            stored_dashboards(&engine),
            held + 4,
            "case {:?}: each of the four creates must store a dashboard, the store holds {}",
            case.label,
            stored_dashboards(&engine)
        );
    }
}
