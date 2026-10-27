// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk every paginated listing of this service shares, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rule this suite pins — for a TABLE of listings rather than for one of them:
//! a paginated listing enumerates its entries in one stable order, cut into pages
//! of at most `maxResults` entries (the page size the operation documents when the
//! caller sends none), issues a `nextToken` only while another entry follows,
//! resumes exactly where the previous page stopped, and refuses a token it never
//! issued with the modelled `InvalidRequestException` rather than answering an
//! empty page.
//!
//! The rows span the listing shapes this service has, so the one walk is exercised
//! through every form it is reached in: an account-wide listing and ones scoped by
//! a path label or a query member; entries keyed by a minted id and by a
//! caller-given name; a listing of bare strings and ones of summaries; a composite
//! resume key; and the two operations whose `maxResults` the model makes required
//! with a default of its own (`ListPipelines`, `ListTasks`), which used to read the
//! page size with no default at all.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `router`) and the creates
//! are the ones the sibling suites already have; this suite reuses them rather than
//! growing a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::access_policy_lifecycle::create_portal;
use crate::application_lifecycle::create_workspace;
use crate::asset_lifecycle::{call, create_asset, error_code, hex_token, router};
use crate::gateway_lifecycle::{greengrass, greengrass_v2, siemens_ie, GATEWAYS};
use crate::pipeline_lifecycle::{node, pipelines_uri};

/// How many entries every listing of the fixture holds, so one table of page sizes
/// says the same thing about each row.
const ENTRIES: usize = 7;

/// The page size this service's listings serve when the caller sends no
/// `maxResults`, which is also the default the models that declare one declare
/// ("Default: 50"). The fixture is shorter than it, so an unpaged call answers in
/// ONE page.
const DEFAULT_MAX_RESULTS: usize = 50;

/// The published maximum of `maxResults` on these listings (`@range(min: 1, max:
/// 250)`; `OPERATIONS.md`, and `smithy_query(query="operation",
/// operation="ListPortals")`).
const MAX_RESULTS_MAX: usize = 250;

/// One listing under test.
struct Listing {
    /// What a failed assertion names this row by: the operation.
    case: &'static str,
    /// The request URI, carrying whatever non-paging members the operation
    /// requires.
    uri: String,
    /// The output member the entries arrive in.
    member: &'static str,
    /// The member of an entry that identifies it, or `None` when the entries are
    /// bare strings (`ListProjectAssets` reports `assetIds`).
    key: Option<&'static str>,
}

/// The URI of one page of `listing`, with the paging members the row asks for
/// appended to the listing's own query.
fn page_uri(listing: &Listing, page_size: Option<usize>, token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = page_size {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        query.push(format!("nextToken={token}"));
    }
    if query.is_empty() {
        return listing.uri.clone();
    }
    let separator = if listing.uri.contains('?') { '&' } else { '?' };
    format!("{}{separator}{}", listing.uri, query.join("&"))
}

/// The entry keys one answer reports, in the order it reported them.
fn keys(listing: &Listing, body: &Value) -> Vec<String> {
    let entries = body[listing.member].as_array().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected a {} list, got {body}",
            listing.case, listing.member
        )
    });
    entries
        .iter()
        .map(|entry| {
            let key = match listing.key {
                Some(member) => entry[member].as_str(),
                None => entry.as_str(),
            };
            key.unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected an entry key in {entry}, got {body}",
                    listing.case
                )
            })
            .to_string()
        })
        .collect()
}

/// One page of `listing`, asserting it was served.
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: Option<usize>,
    token: Option<&str>,
) -> Value {
    let uri = page_uri(listing, page_size, token);
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: {uri} expected 200, got {status} {body}",
        listing.case
    );
    body
}

/// Walk the whole enumeration of `listing` at `page_size` (or with no `maxResults`
/// at all when it is `None`): the keys in the order the pages reported them, and
/// how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: Option<usize>,
) -> (Vec<String>, usize) {
    let case = format!("{} at {page_size:?}", listing.case);
    let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let body = page(router, listing, page_size, token.as_deref()).await;
        let reported = keys(listing, &body);
        pages += 1;
        assert!(
            reported.len() <= limit,
            "case {case:?}: a page holds at most {limit} entries, got {} in {body}",
            reported.len()
        );
        seen.extend(reported.iter().cloned());

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // A token is issued only while an entry follows the page, and a page
            // another entry follows is a FULL page.
            Some(_) => assert_eq!(
                reported.len(),
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

/// Seven asset-model property definitions: one row per `PropertyType` member a
/// definition may carry (attribute | measurement | transform) across the
/// `PropertyDataType` values a model property can hold, so neither the asset's
/// properties nor the model's are seven copies of one shape. Source:
/// `smithy_query(query="shape", shape="PropertyType,PropertyDataType")` —
/// `PropertyDataType` is STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO |
/// ANNOTATION | JSON, of which the last three are not model-property types.
fn property_definitions() -> Value {
    json!([
        {"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}},
        {"name": "Location", "dataType": "STRING", "type": {"attribute": {"defaultValue": "bay-1"}}},
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
        {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
        {
            "name": "AlarmState", "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE",
            "type": {"measurement": {}},
        },
        {
            "name": "Fahrenheit", "dataType": "DOUBLE", "unit": "Fahrenheit",
            "type": {"transform": {
                "expression": "celsius * 9 / 5 + 32",
                "variables": [{"name": "celsius", "value": {"propertyName": "Temperature"}}],
            }},
        },
    ])
}

/// Create one asset model and return its id. The first row carries the whole
/// property catalog, which is what the two property listings enumerate.
async fn create_model(router: &MultiServiceRouter, name: &str, properties: Value) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({ "assetModelName": name, "assetModelProperties": properties })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModel({name}): expected 202, got {status} {body}"
    );
    body["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {body}"))
        .to_string()
}

/// Create one project in `portal_id` and return its id.
async fn create_project(router: &MultiServiceRouter, portal_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/projects",
        Some(json!({ "portalId": portal_id, "projectName": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateProject({name}): expected 201, got {status} {body}"
    );
    body["projectId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateProject({name}): no projectId in {body}"))
        .to_string()
}

/// Create one dashboard in `project_id`.
async fn create_dashboard(router: &MultiServiceRouter, project_id: &str, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/dashboards",
        Some(json!({
            "projectId": project_id,
            "dashboardName": name,
            "dashboardDefinition": json!({ "widgets": [] }).to_string(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateDashboard({name}): expected 201, got {status} {body}"
    );
}

/// A `taskConfiguration` carrying only the members `ContainerTaskConfiguration`
/// requires (`smithy_query(query="shape", shape="ContainerTaskConfiguration")`).
fn container_task(image: &str) -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": image,
            "taskExecutionRole": "arn:aws:iam::123456789012:role/SiteWiseTaskRole",
            "processingType": "GENERIC_COMPUTE_PROCESSING",
            "processingUnit": "UNITS_2",
        }
    })
}

/// Create one task in `workspace`.
async fn create_task(router: &MultiServiceRouter, workspace: &str, name: &str, image: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/tasks"),
        Some(json!({ "taskName": name, "taskConfiguration": container_task(image) })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTask({name}): expected 200, got {status} {body}"
    );
}

/// Create one pipeline in `workspace`.
async fn create_pipeline(router: &MultiServiceRouter, workspace: &str, name: &str, task: &str) {
    let (status, body) = call(
        router,
        "POST",
        &pipelines_uri(workspace),
        Some(json!({ "pipelineName": name, "computations": [node("only", task, &[])] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePipeline({name}): expected 200, got {status} {body}"
    );
}

/// Create one gateway, the platform differing row by row so the enumeration is not
/// seven copies of one gateway.
async fn create_gateway(router: &MultiServiceRouter, name: &str, platform: Value) {
    let (status, body) = call(
        router,
        "POST",
        GATEWAYS,
        Some(json!({ "gatewayName": name, "gatewayPlatform": platform })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateGateway({name}): expected 201, got {status} {body}"
    );
}

/// The whole fixture, and the table of listings it makes enumerable: [`ENTRIES`]
/// entries in each.
async fn fixture(router: &MultiServiceRouter) -> Vec<Listing> {
    let workspace = "page-walk";
    create_workspace(router, workspace).await;

    // Monitor: one portal holding the projects, one project holding the dashboards.
    let mut portals: Vec<String> = Vec::new();
    for at in 0..ENTRIES {
        portals.push(create_portal(router, &format!("page-walk-portal-{at}")).await);
    }
    let mut projects: Vec<String> = Vec::new();
    for at in 0..ENTRIES {
        projects.push(create_project(router, &portals[0], &format!("project-{at}")).await);
    }
    for at in 0..ENTRIES {
        create_dashboard(router, &projects[0], &format!("dashboard-{at}")).await;
    }

    // The gateway platforms the model declares, cycled so neighbouring rows differ.
    // Each carries a device name of its own: a platform device is registered to at
    // most one gateway.
    for at in 0..ENTRIES {
        let platform = match at % 3 {
            0 => greengrass_v2(&format!("page-walk-thing-{at}"), None),
            1 => greengrass(&format!("page-walk-group-{at}")),
            _ => siemens_ie(&format!("page-walk-siemens-{at}")),
        };
        create_gateway(router, &format!("gateway-{at}"), platform).await;
    }

    // The asset side: the first model carries the property catalog, and every asset
    // is created from it, so both property listings and the asset listing have
    // exactly [`ENTRIES`] entries.
    let mut models: Vec<String> = Vec::new();
    for at in 0..ENTRIES {
        let properties = if at == 0 {
            property_definitions()
        } else {
            json!([{"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}}])
        };
        models.push(create_model(router, &format!("model-{at}"), properties).await);
    }
    let mut assets: Vec<String> = Vec::new();
    for at in 0..ENTRIES {
        assets.push(create_asset(router, &format!("asset-{at}"), &models[0]).await);
    }

    // The project's membership: every asset, so `ListProjectAssets` enumerates
    // [`ENTRIES`] bare ids.
    let (status, associated) = call(
        router,
        "POST",
        &format!("/projects/{}/assets/associate", projects[0]),
        Some(json!({ "assetIds": assets })),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateProjectAssets: expected 200, got {status} {associated}"
    );

    // The workspace side: the two listings whose `maxResults` the model makes
    // required with a default of its own.
    for at in 0..ENTRIES {
        create_pipeline(router, workspace, &format!("pipeline-{at}"), "clean").await;
        create_task(
            router,
            workspace,
            &format!("task-{at}"),
            "123456789012.dkr.ecr.us-west-2.amazonaws.com/analytics:v1",
        )
        .await;
    }

    vec![
        Listing {
            case: "ListPortals",
            uri: "/portals".to_string(),
            member: "portalSummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListGateways",
            uri: GATEWAYS.to_string(),
            member: "gatewaySummaries",
            key: Some("gatewayId"),
        },
        Listing {
            case: "ListProjects",
            uri: format!("/projects?portalId={}", portals[0]),
            member: "projectSummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListDashboards",
            uri: format!("/dashboards?projectId={}", projects[0]),
            member: "dashboardSummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListProjectAssets",
            uri: format!("/projects/{}/assets", projects[0]),
            member: "assetIds",
            key: None,
        },
        Listing {
            case: "ListAssetModels",
            uri: "/asset-models".to_string(),
            member: "assetModelSummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListAssets",
            uri: format!("/assets?assetModelId={}", models[0]),
            member: "assetSummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListAssetModelProperties",
            uri: format!("/asset-models/{}/properties", models[0]),
            member: "assetModelPropertySummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListAssetProperties",
            uri: format!("/assets/{}/properties", assets[0]),
            member: "assetPropertySummaries",
            key: Some("id"),
        },
        Listing {
            case: "ListPipelines",
            uri: pipelines_uri(workspace),
            member: "pipelineSummaries",
            key: Some("pipelineName"),
        },
        Listing {
            case: "ListTasks",
            uri: format!("/workspaces/{workspace}/tasks"),
            member: "taskSummaries",
            key: Some("taskName"),
        },
    ]
}

/// The whole enumeration of `listing` in one page: the order every paged walk of it
/// has to reproduce.
async fn baseline(router: &MultiServiceRouter, listing: &Listing) -> Vec<String> {
    let (reference, pages) = walk(router, listing, Some(MAX_RESULTS_MAX)).await;
    assert_eq!(
        pages, 1,
        "case {:?}: the whole enumeration fits one page of {MAX_RESULTS_MAX}, took {pages}",
        listing.case
    );
    assert_eq!(
        reference.len(),
        ENTRIES,
        "case {:?}: expected {ENTRIES} entries, got {reference:?}",
        listing.case
    );
    let mut unique = reference.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ENTRIES,
        "case {:?}: every entry is reported once, got {reference:?}",
        listing.case
    );
    reference
}

#[tokio::test]
async fn every_listing_pages_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    let listings = fixture(&router).await;

    for listing in &listings {
        let reference = baseline(&router, listing).await;

        // Rows differ in page size: the published minimum, sizes that divide the
        // enumeration and sizes that do not, exactly it, one past it, the published
        // maximum, and no `maxResults` at all (the documented default of 50).
        let page_sizes = [
            Some(1_usize),
            Some(2),
            Some(3),
            Some(ENTRIES),
            Some(ENTRIES + 1),
            Some(MAX_RESULTS_MAX),
            None,
        ];
        for page_size in page_sizes {
            let case = format!("{} at {page_size:?}", listing.case);
            let (seen, pages) = walk(&router, listing, page_size).await;
            assert_eq!(
                seen, reference,
                "case {case:?}: the pages joined must be the whole enumeration once, in one \
                 order, got {seen:?}"
            );
            let expected_pages = ENTRIES.div_ceil(page_size.unwrap_or(DEFAULT_MAX_RESULTS));
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages over {ENTRIES} entries, \
                 walked {pages}"
            );
        }
    }
}

#[tokio::test]
async fn the_token_a_page_issues_is_opaque_hex_and_resumes_at_the_next_entry() {
    let router = router();
    let listings = fixture(&router).await;

    for listing in &listings {
        let reference = baseline(&router, listing).await;

        let first = page(&router, listing, Some(1), None).await;
        let token = first["nextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: a first page of one of {ENTRIES} entries carries a token, got {first}",
                listing.case
            )
        });
        // The form every listing of this service issues a token in: the hex of the
        // entry's resume key, which is inside the `^[A-Za-z0-9+/=]+$` the paging
        // members model.
        assert!(
            !token.is_empty()
                && token.len() % 2 == 0
                && token.chars().all(|ch| ch.is_ascii_hexdigit()),
            "case {:?}: expected an even-length hex token, got {token:?}",
            listing.case
        );

        // It resumes at the entry AFTER the page it was issued with, and the rest of
        // the enumeration follows from there in the one order.
        let resumed = page(&router, listing, Some(MAX_RESULTS_MAX), Some(token)).await;
        assert_eq!(
            keys(listing, &resumed),
            reference[1..].to_vec(),
            "case {:?}: the token must resume at the second entry, got {resumed}",
            listing.case
        );
    }
}

#[tokio::test]
async fn a_token_a_listing_never_issued_is_refused_by_every_listing() {
    let router = router();
    let listings = fixture(&router).await;

    // Rows differ in HOW the token is wrong: not hex at all, an odd-length hex run,
    // the hex of a well-formed id no entry carries, and the hex of text that is no
    // key of any enumeration.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        ("an odd-length hex run", "abc".to_string()),
        (
            "the hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("the hex of text that is no key", hex_token("no-such-entry")),
    ];
    for listing in &listings {
        let reference = baseline(&router, listing).await;
        for (how, token) in &forged {
            let uri = page_uri(listing, None, Some(token));
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {:?} ({how}): a forged nextToken must be refused, got {status} {body}",
                listing.case
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {:?} ({how}): unexpected error code in {body}",
                listing.case
            );
        }

        // The refusals changed nothing a later caller sees, and neither did the
        // reads: a listing REPORTS.
        let (again, _) = walk(&router, listing, Some(MAX_RESULTS_MAX)).await;
        assert_eq!(
            again, reference,
            "case {:?}: the refusals must leave the enumeration as it was, got {again:?}",
            listing.case
        );
    }
}
