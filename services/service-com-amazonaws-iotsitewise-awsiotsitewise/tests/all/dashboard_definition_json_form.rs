// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the documented FORM of a dashboard's definition, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with, so
//! every assertion is about the wire behaviour a real client sees.
//!
//! The rule this suite pins: `dashboardDefinition` is "The dashboard definition
//! specified in a JSON literal" (`smithy_query(query="doc",
//! shape="CreateDashboardRequest", member="dashboardDefinition")`), and the
//! service holds it verbatim and reports it back verbatim, so a body that parses
//! as no JSON document is refused with the client-input error both write paths
//! model — `InvalidRequestException` (400) — and nothing is stored or
//! overwritten, while every JSON document, of every form and up to the member's
//! published length, is accepted and reads back exactly as it was written.
//!
//! The rows span what a JSON document IS and IS NOT rather than the one body the
//! report was filed with, and each is driven through BOTH write paths
//! (`CreateDashboard`, `UpdateDashboard`) and read back through
//! `DescribeDashboard`. `dashboard_lifecycle` pins the round trip, the modelled
//! bounds, the ceiling and the create's `clientToken` idempotency; what is pinned
//! HERE is the member's own documented form. The HTTP harness and the dashboard
//! request helpers are the ones those suites already have.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::dashboard_lifecycle::{definition_of, describe_dashboard, seed_projects};

/// Where every dashboard request goes, as `OPERATIONS.md` reports the routes:
/// `POST /dashboards`, `GET|PUT /dashboards/{dashboardId}`.
const DASHBOARDS: &str = "/dashboards";

/// The published maximum of `dashboardDefinition`. Source:
/// `smithy_query(query="shape", shape="DashboardDefinition")` — len 0..204800,
/// pattern `^.+$`.
const DEFINITION_MAX: usize = 204_800;

/// One row of the definition-form table: the body sent as the dashboard's
/// definition and whether the documented form admits it.
struct DefinitionRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    definition: String,
    /// Whether the definition is a JSON document, and so whether the write is
    /// served or refused.
    accepted: bool,
}

/// The rows, spanning what the documented form admits and what it does not:
/// every JSON form on the accepted side — object, array, the scalars, nesting,
/// non-ASCII text, leading and trailing whitespace, and a document AT the
/// member's published length — and truncated, doubled, trailing-garbage,
/// JavaScript-but-not-JSON and plainly non-JSON bodies on the refused one.
fn definition_rows() -> Vec<DefinitionRow> {
    let row = |case: &'static str, definition: String, accepted: bool| DefinitionRow {
        case,
        definition,
        accepted,
    };
    vec![
        row("an empty object", "{}".to_string(), true),
        row(
            "a widget object",
            json!({ "widgets": [{ "title": "pump pressure" }] }).to_string(),
            true,
        ),
        row(
            "a deeply nested object",
            json!({ "a": { "b": { "c": [1, 2, { "d": true }] } } }).to_string(),
            true,
        ),
        row("an array", "[]".to_string(), true),
        row("a string literal", "\"a definition\"".to_string(), true),
        row("a number literal", "7".to_string(), true),
        row("a boolean literal", "true".to_string(), true),
        row("the null literal", "null".to_string(), true),
        row(
            "an object padded with whitespace",
            "  { \"widgets\": [] }  ".to_string(),
            true,
        ),
        row(
            "an object of non-ASCII text",
            json!({ "title": "Kühlung — 熱交換器" }).to_string(),
            true,
        ),
        row(
            "an object AT the published length",
            definition_of(DEFINITION_MAX),
            true,
        ),
        row("prose", "not a json literal".to_string(), false),
        row("a bare word", "widgets".to_string(), false),
        row("a truncated object", "{\"widgets\":".to_string(), false),
        row("an unopened brace", "}".to_string(), false),
        row("two documents", "{} {}".to_string(), false),
        row("trailing garbage", "{} and then some".to_string(), false),
        row(
            "single-quoted keys, which JSON does not admit",
            "{'widgets': []}".to_string(),
            false,
        ),
        row(
            "a trailing comma, which JSON does not admit",
            "{\"widgets\": [],}".to_string(),
            false,
        ),
        row(
            "an unquoted key, which JSON does not admit",
            "{widgets: []}".to_string(),
            false,
        ),
        row("xml", "<dashboard><widget/></dashboard>".to_string(), false),
    ]
}

/// The create body for `project` carrying `definition`. No `clientToken`, so
/// every row mints a dashboard of its own rather than replaying another's.
fn create_body(project_id: &str, definition: &str) -> Value {
    json!({
        "projectId": project_id,
        "dashboardName": "definition-form",
        "dashboardDefinition": definition,
    })
}

/// `CreateDashboard` with `body`.
async fn create_dashboard(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", DASHBOARDS, Some(body)).await
}

/// `UpdateDashboard` on `id`, restating the dashboard with `definition`.
async fn update_dashboard(
    router: &MultiServiceRouter,
    id: &str,
    definition: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("{DASHBOARDS}/{id}"),
        Some(json!({
            "dashboardName": "definition-form",
            "dashboardDefinition": definition,
        })),
    )
    .await
}

/// How many dashboards the shared store holds — what proves a refused write
/// stored nothing.
fn stored_dashboards(engine: &std::sync::Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the IoT SiteWise store")
        .lock()
        .dashboards
        .len()
}

#[tokio::test]
async fn a_create_whose_definition_is_not_a_json_literal_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();
    let project = seed_projects(&router, 1).await.remove(0);

    for row in definition_rows() {
        let case = row.case;
        let before = stored_dashboards(&engine);
        let (status, answered) =
            create_dashboard(&router, create_body(&project, &row.definition)).await;

        if row.accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a JSON definition expected 201, got {status} {answered}"
            );
            let id = answered["dashboardId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, described) = describe_dashboard(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created dashboard expected 200, got {status} {described}"
            );
            assert_eq!(
                described["dashboardDefinition"].as_str(),
                Some(row.definition.as_str()),
                "case {case:?}: the read must answer with the definition written, got {}",
                described["dashboardDefinition"]
            );
            assert_eq!(
                stored_dashboards(&engine),
                before + 1,
                "case {case:?}: an accepted create stores one dashboard"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a definition that is no JSON document expected 400, \
                 got {status} {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: refused with {code:?}, expected the client-input error \
                 this operation models, in {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("dashboardDefinition"),
                "case {case:?}: the refusal must name the member it is about, got {message:?}"
            );
            assert_eq!(
                stored_dashboards(&engine),
                before,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn an_update_whose_definition_is_not_a_json_literal_leaves_the_stored_dashboard_alone() {
    let (router, engine) = router_with_engine();
    let project = seed_projects(&router, 1).await.remove(0);
    let (status, created) = create_dashboard(
        &router,
        create_body(&project, &json!({ "widgets": [] }).to_string()),
    )
    .await;
    assert_eq!(
        status, 201,
        "the seeded dashboard: CreateDashboard expected 201, got {status} {created}"
    );
    let id = created["dashboardId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = stored_dashboards(&engine);

    for row in definition_rows() {
        let case = row.case;
        let (_, stored) = describe_dashboard(&router, &id).await;
        let (status, answered) = update_dashboard(&router, &id, &row.definition).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a JSON definition expected 200, got {status} {answered}"
            );
            let (status, described) = describe_dashboard(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the updated dashboard expected 200, got {status} {described}"
            );
            assert_eq!(
                described["dashboardDefinition"].as_str(),
                Some(row.definition.as_str()),
                "case {case:?}: the read must answer with the definition written, got {}",
                described["dashboardDefinition"]
            );
            // The write replaced the definition and nothing else about which
            // dashboard this is.
            for member in ["dashboardId", "dashboardArn", "projectId"] {
                assert_eq!(
                    described[member], stored[member],
                    "case {case:?}: an update must leave {member} alone, got {described}"
                );
            }
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a definition that is no JSON document expected 400, \
                 got {status} {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: refused with {code:?}, expected the client-input error \
                 this operation models, in {answered}"
            );
            let (status, again) = describe_dashboard(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the stored dashboard expected 200, got {status} {again}"
            );
            assert_eq!(
                again, stored,
                "case {case:?}: a refused update must leave the stored dashboard alone"
            );
        }
        assert_eq!(
            stored_dashboards(&engine),
            before,
            "case {case:?}: an update never adds or removes a dashboard"
        );
    }
}
