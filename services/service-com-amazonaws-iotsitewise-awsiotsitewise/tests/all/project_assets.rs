// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the assets an IoT SiteWise Monitor project holds
//! (`BatchAssociateProjectAssets` -> `ListProjectAssets`), driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * associating assets with a project adds them to that project's membership, a
//!   SET, so an id repeated inside one request and an asset already in the project
//!   each count once and a request repeated verbatim leaves the membership
//!   unchanged;
//! * the listing answers from exactly that stored membership, in one stable order,
//!   one `maxResults`-sized page at a time (default 50), issuing a `nextToken` only
//!   when an asset follows the page it answered;
//! * a request naming a resource the service does not hold — the project, or any
//!   one of the assets — is refused in full, in the one wording every not-found
//!   refusal of this service carries, and associates nothing;
//! * an asset the service no longer holds is in no project's membership;
//! * a read never changes what the next caller sees.
//!
//! The rows differ along what those rules generalise over: how many assets one
//! request names (one, several, the modelled maximum of 100), how many requests the
//! membership is built from, whether the ids repeat within or across requests, the
//! order the ids arrive in, whether a `clientToken` is carried, and the page size a
//! listing asks for — at each published bound of `maxResults` and one past each.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `router_with_engine`,
//! `create_model`, `create_asset`, `hex_token`) is the one the asset suite already
//! has, `error_message` is the refusal suite's and the portal a project lives in is
//! created through the access-policy suite's helper; this suite reuses all of them
//! rather than growing a second copy.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use super::access_policy_lifecycle::create_portal;
use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, is_modelled_id, router_with_engine,
};
use super::hierarchies_etags_and_refusals::error_message;

/// Where every project request goes, as `OPERATIONS.md` reports the routes:
/// `POST /projects`, `POST /projects/{projectId}/assets/associate`,
/// `GET /projects/{projectId}/assets`.
const PROJECTS: &str = "/projects";

/// The page size `ListProjectAssets` documents for a caller that names none
/// (smithy_query query="operation" operation="ListProjectAssets": `maxResults`
/// (MaxResults, range 1..250) — "Default: 50").
const DEFAULT_MAX_RESULTS: usize = 50;

/// The published bounds of `maxResults`, from the same source as the default
/// above: `@range(min: 1, max: 250)`.
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 250;

/// The greatest number of ids one association request may carry
/// (smithy_query query="shape" shape="IDs": `@length(min: 1, max: 100)`).
const MAX_IDS_PER_REQUEST: usize = 100;

/// A UUID of the modelled `ID` form that no create in this suite ever mints, used
/// where a request has to name a resource the service does not hold.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One row of the association table.
#[derive(Debug)]
struct AssociateCase {
    label: &'static str,
    /// How many assets the row creates.
    assets: usize,
    /// The association requests the row sends, each naming assets by their index
    /// in the created list. Repeats — inside one request and across two — are the
    /// point of several rows.
    requests: Vec<Vec<usize>>,
    /// The `clientToken` each request carries, if any — the SEED of it: a token
    /// is spent by one request, so the loop below builds a distinct token per
    /// request from this seed. A token targets
    /// `ClientToken` (len 36..64, pattern `^\S{36,64}$`), per smithy_query
    /// query="operation" operation="BatchAssociateProjectAssets".
    client_token: Option<&'static str>,
}

/// The rows.
fn associate_cases() -> Vec<AssociateCase> {
    vec![
        AssociateCase {
            label: "one-asset-in-one-request",
            assets: 1,
            requests: vec![vec![0]],
            client_token: None,
        },
        AssociateCase {
            label: "three-assets-in-one-request",
            assets: 3,
            requests: vec![vec![0, 1, 2]],
            client_token: None,
        },
        AssociateCase {
            label: "one-id-repeated-inside-one-request",
            assets: 2,
            requests: vec![vec![1, 0, 1]],
            client_token: None,
        },
        AssociateCase {
            label: "two-requests-naming-one-asset-in-common",
            assets: 3,
            requests: vec![vec![0, 1], vec![1, 2]],
            client_token: None,
        },
        AssociateCase {
            label: "the-same-request-sent-twice",
            assets: 2,
            requests: vec![vec![0, 1], vec![0, 1]],
            client_token: None,
        },
        AssociateCase {
            label: "ids-arriving-in-descending-order",
            assets: 4,
            requests: vec![vec![3, 2, 1, 0]],
            client_token: None,
        },
        AssociateCase {
            label: "one-asset-per-request-each-carrying-a-client-token-of-its-own",
            assets: 3,
            requests: vec![vec![0], vec![1], vec![2]],
            client_token: Some("3f1c6a9e-8b2d-4e5f-9a1b-7c6d5e4f3a2b-token"),
        },
        AssociateCase {
            label: "a-request-at-the-modelled-maximum-of-one-hundred-ids",
            assets: MAX_IDS_PER_REQUEST,
            requests: vec![(0..MAX_IDS_PER_REQUEST).collect()],
            client_token: None,
        },
    ]
}

/// `POST /projects/{projectId}/assets/associate`.
pub(crate) fn associate_uri(project_id: &str) -> String {
    format!("{PROJECTS}/{project_id}/assets/associate")
}

/// `GET /projects/{projectId}/assets`.
fn project_assets_uri(project_id: &str) -> String {
    format!("{PROJECTS}/{project_id}/assets")
}

/// The body one association request carries.
pub(crate) fn associate_body(asset_ids: &[String], client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert(
        "assetIds".to_string(),
        Value::Array(
            asset_ids
                .iter()
                .map(|id| Value::String(id.clone()))
                .collect(),
        ),
    );
    if let Some(token) = client_token {
        body.insert("clientToken".to_string(), Value::String(token.to_string()));
    }
    Value::Object(body)
}

/// Create a project in a fresh portal and answer its id.
pub(crate) async fn create_project(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let portal_id = create_portal(router, &format!("{name}-portal")).await;
    let (status, created) = call(
        router,
        "POST",
        PROJECTS,
        Some(json!({ "portalId": portal_id, "projectName": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateProject expected 201, got {status} {created}"
    );
    let project_id = created["projectId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&project_id),
        "case {case:?}: projectId must be a modelled UUID, got {created}"
    );
    project_id
}

/// Create `count` assets of one fresh asset model and answer their ids.
pub(crate) async fn create_assets(
    router: &MultiServiceRouter,
    case: &str,
    count: usize,
) -> Vec<String> {
    let model_id = create_model(router, &format!("{case}-model"), json!([])).await;
    let mut ids = Vec::with_capacity(count);
    for at in 0..count {
        ids.push(create_asset(router, &format!("{case}-asset-{at}"), &model_id).await);
    }
    ids
}

/// Send one association request and assert it was served: HTTP 200 with an EMPTY
/// per-asset error list, which is what "a list of associated error information, if
/// any" states when every asset was associated.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    asset_ids: &[String],
    client_token: Option<&str>,
) {
    let (status, answered) = call(
        router,
        "POST",
        &associate_uri(project_id),
        Some(associate_body(asset_ids, client_token)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchAssociateProjectAssets expected 200, got {status} {answered}"
    );
    let errors = answered["errors"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected an errors list, got {answered}")
    });
    assert!(
        errors.is_empty(),
        "case {case:?}: expected no per-asset errors, got {answered}"
    );
}

/// Every asset of `project_id`, read through `ListProjectAssets` and following the
/// `nextToken` to the end. `max_results` is the page size each request asks for, or
/// `None` to leave the documented default to the service.
///
/// Asserts the paging contract on the way: no page is longer than the size in
/// force, and a `nextToken` comes back exactly when more assets follow.
pub(crate) async fn list_project_assets(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let page_size = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut uri = project_assets_uri(project_id);
        let mut separator = '?';
        if let Some(size) = max_results {
            uri.push_str(&format!("{separator}maxResults={size}"));
            separator = '&';
        }
        if let Some(token) = next_token.as_deref() {
            uri.push_str(&format!("{separator}nextToken={token}"));
        }
        let (status, answered) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListProjectAssets {uri} expected 200, got {status} {answered}"
        );
        let page: Vec<String> = answered["assetIds"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an assetIds list, got {answered}"))
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} assets, got {} in {answered}",
            page.len()
        );
        collected.extend(page);
        next_token = answered["nextToken"].as_str().map(ToString::to_string);
        if let Some(token) = next_token.as_deref() {
            assert!(
                !token.is_empty(),
                "case {case:?}: a nextToken is never empty, got {answered}"
            );
        } else {
            break;
        }
    }
    collected
}

/// The membership stored on the project's own record — the only way to tell the
/// listing answers from the store rather than from the request that wrote it.
pub(crate) fn stored_membership(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    project_id: &str,
) -> Vec<String> {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let store = state.lock();
    store
        .projects
        .get(project_id)
        .map(|project| project.asset_ids.iter().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn associating_assets_adds_them_to_the_project_membership_the_listing_reads_back() {
    for case in associate_cases() {
        let label = case.label;
        let (router, engine) = router_with_engine();
        let project_id = create_project(&router, label, &format!("{label}-project")).await;
        let asset_ids = create_assets(&router, label, case.assets).await;

        // Nothing is associated with a freshly created project.
        let empty = list_project_assets(&router, label, &project_id, None).await;
        assert!(
            empty.is_empty(),
            "case {label:?}: a new project holds no assets, got {empty:?}"
        );

        let mut expected: BTreeSet<String> = BTreeSet::new();
        for (at, request) in case.requests.iter().enumerate() {
            let named: Vec<String> = request.iter().map(|at| asset_ids[*at].clone()).collect();
            // A token is spent by ONE request: `clientToken` makes the call
            // idempotent, so each of a row's requests carries a token of its own,
            // built from the row's seed and the request's place in it.
            let token = case
                .client_token
                .map(|token| format!("{at}-{token}"));
            associate(&router, label, &project_id, &named, token.as_deref()).await;
            expected.extend(named);
        }
        let expected: Vec<String> = expected.into_iter().collect();

        let listed = list_project_assets(&router, label, &project_id, None).await;
        assert_eq!(
            listed, expected,
            "case {label:?}: the listing reports the stored membership, got {listed:?}"
        );
        assert_eq!(
            stored_membership(&engine, &project_id),
            expected,
            "case {label:?}: the membership is persisted on the project's record"
        );

        // A read changes nothing a later caller sees.
        let again = list_project_assets(&router, label, &project_id, None).await;
        assert_eq!(
            again, expected,
            "case {label:?}: a second listing answers the same, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_the_membership_at_every_published_bound_of_max_results() {
    let case = "paging";
    let (router, _engine) = router_with_engine();
    let project_id = create_project(&router, case, "paging-project").await;
    // More assets than the smallest page size and fewer than the largest, so the
    // rows below cover a page smaller than the membership, a page exactly its size
    // and a page larger than it.
    let asset_ids = create_assets(&router, case, 6).await;
    associate(&router, case, &project_id, &asset_ids, None).await;

    let mut expected: Vec<String> = asset_ids.clone();
    expected.sort();

    // The page sizes: both published bounds, one under the membership, its exact
    // size, one over it, and the documented default (asked for by naming nothing).
    let page_sizes = [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(5),
        Some(expected.len()),
        Some(expected.len() + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in page_sizes {
        let label = format!("maxResults={page_size:?}");
        let listed = list_project_assets(&router, &label, &project_id, page_size).await;
        assert_eq!(
            listed, expected,
            "case {label:?}: paging through the membership reports every asset once, got {listed:?}"
        );
    }

    // One page, read on its own: the first `maxResults` assets of the enumeration,
    // and a `nextToken` that names the asset the next page starts at — checked
    // against an INDEPENDENT oracle rather than against the handler's own encoder.
    let (status, first) = call(
        &router,
        "GET",
        &format!("{}?maxResults=2", project_assets_uri(&project_id)),
        None,
    )
    .await;
    assert_eq!(status, 200, "one page: got {status} {first}");
    assert_eq!(
        first["assetIds"],
        json!([expected[0], expected[1]]),
        "one page: the first two assets of the enumeration, got {first}"
    );
    assert_eq!(
        first["nextToken"].as_str().unwrap_or_default(),
        hex_token(&expected[2]),
        "one page: the token names the asset the next page starts at, got {first}"
    );

    // The last page carries no token: "or null if there are no additional results".
    let (status, last) = call(
        &router,
        "GET",
        &format!(
            "{}?maxResults=2&nextToken={}",
            project_assets_uri(&project_id),
            hex_token(&expected[4])
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "last page: got {status} {last}");
    assert_eq!(
        last["assetIds"],
        json!([expected[4], expected[5]]),
        "last page: the final two assets, got {last}"
    );
    assert!(
        last["nextToken"].is_null(),
        "last page: no token follows the last page, got {last}"
    );
}

#[tokio::test]
async fn a_request_naming_a_resource_this_service_does_not_hold_is_refused_in_full() {
    let case = "refusals";
    let (router, engine) = router_with_engine();
    let project_id = create_project(&router, case, "refusal-project").await;
    let asset_ids = create_assets(&router, case, 3).await;
    // One asset is associated first, so every refusal below can be shown to have
    // left the membership exactly as it was.
    associate(&router, case, &project_id, &asset_ids[..1], None).await;
    let settled: Vec<String> = vec![asset_ids[0].clone()];

    // A project this service holds none of: the association names no project to
    // record anything on.
    let (status, answered) = call(
        &router,
        "POST",
        &associate_uri(ABSENT_ID),
        Some(associate_body(&asset_ids, None)),
    )
    .await;
    assert_eq!(
        status, 404,
        "absent project: expected 404, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "absent project: the modelled 404 code, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Project with ID {ABSENT_ID} not found"),
        "absent project: the one not-found wording, naming the id the request sent"
    );

    // One absent asset among assets that do exist: the whole request is refused,
    // so the two that exist are NOT associated either.
    let mixed = vec![
        asset_ids[1].clone(),
        ABSENT_ID.to_string(),
        asset_ids[2].clone(),
    ];
    let (status, answered) = call(
        &router,
        "POST",
        &associate_uri(&project_id),
        Some(associate_body(&mixed, None)),
    )
    .await;
    assert_eq!(
        status, 404,
        "absent asset: expected 404, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "absent asset: the modelled 404 code, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Asset with ID {ABSENT_ID} not found"),
        "absent asset: the one not-found wording, naming the id the request sent"
    );
    assert_eq!(
        list_project_assets(&router, "absent asset", &project_id, None).await,
        settled,
        "absent asset: a refused request associates nothing"
    );
    assert_eq!(
        stored_membership(&engine, &project_id),
        settled,
        "absent asset: a refused request writes nothing to the store"
    );

    // A listing of a project this service holds none of. `ListProjectAssets`
    // declares no `ResourceNotFoundException`, so the absence is stated on the
    // client-input error it does declare — never as an empty page, which would
    // report absence as success.
    let (status, answered) = call(&router, "GET", &project_assets_uri(ABSENT_ID), None).await;
    assert_eq!(
        status, 400,
        "listing an absent project: expected 400, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidRequestException",
        "listing an absent project: the modelled client-input code, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Project with ID {ABSENT_ID} not found"),
        "listing an absent project: the one not-found wording"
    );

    // A `nextToken` naming a resume point this enumeration does not hold.
    let (status, answered) = call(
        &router,
        "GET",
        &format!(
            "{}?nextToken={}",
            project_assets_uri(&project_id),
            hex_token(ABSENT_ID)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "a token this operation never issued: expected 400, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidRequestException",
        "a token this operation never issued: the modelled code, got {answered}"
    );

    // The modelled bounds of the two list members, refused before any handler runs:
    // `assetIds` carries @length(min: 1, max: 100) and `maxResults` @range(1, 250).
    let past_the_batch_maximum: Vec<String> = (0..=MAX_IDS_PER_REQUEST)
        .map(|at| format!("{:08x}-0000-4000-8000-000000000000", at + 1))
        .collect();
    let bound_cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "no assetIds at all, one under @length min 1",
            "POST",
            associate_uri(&project_id),
            Some(associate_body(&[], None)),
        ),
        (
            "101 assetIds, one past @length max 100",
            "POST",
            associate_uri(&project_id),
            Some(associate_body(&past_the_batch_maximum, None)),
        ),
        (
            "maxResults under @range min 1",
            "GET",
            format!("{}?maxResults=0", project_assets_uri(&project_id)),
            None,
        ),
        (
            "maxResults past @range max 250",
            "GET",
            format!(
                "{}?maxResults={}",
                project_assets_uri(&project_id),
                MAX_RESULTS_MAX + 1
            ),
            None,
        ),
    ];
    for (label, method, uri, body) in &bound_cases {
        let (status, answered) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a modelled-rule violation is a 400, got {status} {answered}"
        );
    }
    assert_eq!(
        stored_membership(&engine, &project_id),
        settled,
        "every refusal above left the stored membership exactly as it was"
    );
}

#[tokio::test]
async fn an_asset_this_service_no_longer_holds_is_in_no_project() {
    let case = "deleted-asset";
    let (router, engine) = router_with_engine();
    let project_id = create_project(&router, case, "deleted-asset-project").await;
    let asset_ids = create_assets(&router, case, 3).await;
    associate(&router, case, &project_id, &asset_ids, None).await;

    let doomed = asset_ids[1].clone();
    let (status, deleted) = call(&router, "DELETE", &format!("/assets/{doomed}"), None).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteAsset expected 202, got {status} {deleted}"
    );

    let mut expected: Vec<String> = asset_ids
        .iter()
        .filter(|id| **id != doomed)
        .cloned()
        .collect();
    expected.sort();

    let listed = list_project_assets(&router, case, &project_id, None).await;
    assert_eq!(
        listed, expected,
        "case {case:?}: the deleted asset is in no project, got {listed:?}"
    );
    assert_eq!(
        stored_membership(&engine, &project_id),
        expected,
        "case {case:?}: the membership on the record dropped the deleted asset"
    );
}
