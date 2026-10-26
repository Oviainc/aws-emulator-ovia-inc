// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListProjectAssets` as the PAGINATED reader of one IoT
//! SiteWise Monitor project's asset membership, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule this suite pins: the listing answers from the membership stored on the
//! project the path names — never from the assets the service holds — in one stable
//! order, cut into pages of at most the size in force (the caller's `maxResults`,
//! else the default this operation documents for a caller that names none), and
//! issues a `nextToken` naming the asset the next page starts at exactly when an
//! asset follows the page it answered.
//!
//! The sibling `project_assets` suite walks the association side and the published
//! bounds of `maxResults`, but over a membership smaller than the documented
//! default, where a service that applies that default and one that pages not at all
//! answer alike. The rows here differ along the size of the membership — empty, one,
//! on either side of the documented default, and well past twice it — and along the
//! page size each listing asks for, naming none among them, so the default page size
//! is observed rather than assumed; and along which project of several holds which
//! assets out of one shared pool, so the scoping of the listing to the project in
//! the path is observed too.
//!
//! The HTTP harness (`call`, `hex_token`, `router_with_engine`) is the asset suite's
//! and the project helpers (`create_project`, `create_assets`, `associate`,
//! `list_project_assets`, `stored_membership`) are the project-assets suite's; this
//! suite reuses all of them rather than growing a second copy.

use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, hex_token, router_with_engine};
use super::project_assets::{
    associate, create_assets, create_project, list_project_assets, stored_membership,
};

/// The page size `ListProjectAssets` documents for a caller that names no
/// `maxResults` (smithy_query query="operation" operation="ListProjectAssets":
/// `maxResults` (MaxResults, range 1..250) — "Default: 50").
const DEFAULT_MAX_RESULTS: usize = 50;

/// The published bounds of `maxResults`, from the same source as the default
/// above: `@range(min: 1, max: 250)`.
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 250;

/// The greatest number of ids one `BatchAssociateProjectAssets` request may carry
/// (smithy_query query="operation" operation="BatchAssociateProjectAssets":
/// `assetIds` (IDs, len 1..100)), so a larger membership is built from several
/// requests.
const MAX_IDS_PER_REQUEST: usize = 100;

/// `GET /projects/{projectId}/assets`, as the model binds the operation.
fn project_assets_uri(project_id: &str) -> String {
    format!("/projects/{project_id}/assets")
}

/// Read ONE page of a project's assets and answer `(the page, its nextToken)`.
async fn read_page(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut uri = project_assets_uri(project_id);
    let mut separator = '?';
    if let Some(size) = max_results {
        uri.push_str(&format!("{separator}maxResults={size}"));
        separator = '&';
    }
    if let Some(token) = next_token {
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
    let token = answered["nextToken"].as_str().map(ToString::to_string);
    (page, token)
}

/// Build a project holding `count` freshly created assets, and answer its id
/// beside that membership in the order a listing must report it: ascending by id.
async fn membership(
    router: &MultiServiceRouter,
    case: &str,
    count: usize,
) -> (String, Vec<String>) {
    let project_id = create_project(router, case, &format!("{case}-project")).await;
    let asset_ids = create_assets(router, case, count).await;
    for chunk in asset_ids.chunks(MAX_IDS_PER_REQUEST) {
        associate(router, case, &project_id, chunk, None).await;
    }
    let mut expected = asset_ids;
    expected.sort();
    (project_id, expected)
}

/// The membership sizes the rows span: empty, one, one under the documented
/// default, exactly it, one past it, and well past twice it.
fn membership_sizes() -> Vec<usize> {
    vec![
        0,
        1,
        DEFAULT_MAX_RESULTS - 1,
        DEFAULT_MAX_RESULTS,
        DEFAULT_MAX_RESULTS + 1,
        2 * DEFAULT_MAX_RESULTS + 3,
    ]
}

/// The page sizes each membership is read at: naming none (so the documented
/// default is in force), both published bounds of `maxResults`, a size that
/// divides none of the memberships above evenly, and the default named outright.
fn page_sizes() -> Vec<Option<usize>> {
    vec![
        None,
        Some(MAX_RESULTS_MIN),
        Some(7),
        Some(DEFAULT_MAX_RESULTS),
        Some(MAX_RESULTS_MAX),
    ]
}

#[tokio::test]
async fn the_listing_cuts_the_stored_membership_into_pages_of_the_size_in_force() {
    for size in membership_sizes() {
        let case = format!("membership-of-{size}");
        let (router, engine) = router_with_engine();
        let (project_id, expected) = membership(&router, &case, size).await;
        assert_eq!(
            stored_membership(&engine, &project_id),
            expected,
            "case {case:?}: the membership this suite reads back must be the one on \
             the project's record"
        );

        for asked in page_sizes() {
            let label = format!("{case}-maxResults-{asked:?}");
            // The size in force: what the caller asked for, else the documented
            // default.
            let in_force = asked.unwrap_or(DEFAULT_MAX_RESULTS);
            let end = in_force.min(expected.len());

            let (first, token) = read_page(&router, &label, &project_id, asked, None).await;
            assert_eq!(
                first,
                expected[..end],
                "case {label:?}: the first page holds the first {end} assets of the \
                 enumeration, got {first:?}"
            );
            // The token names the asset the next page starts at, and comes back
            // only when an asset follows this page — checked against an INDEPENDENT
            // encoder, not the handler's own.
            let expected_token = expected.get(end).map(|id| hex_token(id.as_str()));
            assert_eq!(
                token, expected_token,
                "case {label:?}: expected nextToken {expected_token:?}, got {token:?}"
            );

            // Following the token to the end reports every asset of the membership
            // once, in that same order.
            let walked = list_project_assets(&router, &label, &project_id, asked).await;
            assert_eq!(
                walked, expected,
                "case {label:?}: paging to the end reports the whole membership once, \
                 got {walked:?}"
            );
        }

        // Every listing above was a read: none of them changed what the next caller
        // sees.
        assert_eq!(
            stored_membership(&engine, &project_id),
            expected,
            "case {case:?}: the listings above wrote nothing to the store"
        );
    }
}

#[tokio::test]
async fn each_project_lists_only_the_assets_its_own_membership_holds() {
    let case = "scoping";
    let (router, engine) = router_with_engine();
    // One pool of assets the service holds, shared by every project below, so only
    // the membership — never the pool — can tell the listings apart.
    let pool = create_assets(&router, case, 5).await;

    // The rows: which assets of the pool each project holds, by index in it. One
    // project holds none of them, so a listing reporting the service's assets
    // rather than the project's would answer five where it must answer none.
    let rows: Vec<(&str, Vec<usize>)> = vec![
        ("holds-none-of-the-pool", Vec::new()),
        ("holds-one-of-the-pool", vec![2]),
        ("holds-three-of-the-pool-named-out-of-order", vec![4, 0, 1]),
        ("holds-the-whole-pool", (0..pool.len()).collect()),
    ];

    let mut projects: Vec<(String, String, Vec<String>)> = Vec::new();
    for (label, held) in &rows {
        let project_id = create_project(&router, label, &format!("{label}-project")).await;
        let named: Vec<String> = held.iter().map(|at| pool[*at].clone()).collect();
        if !named.is_empty() {
            associate(&router, label, &project_id, &named, None).await;
        }
        let mut expected = named;
        expected.sort();
        projects.push(((*label).to_string(), project_id, expected));
    }

    for (label, project_id, expected) in &projects {
        // At the documented default page size, and one asset at a time, so the
        // scoping holds however the enumeration is cut.
        for asked in [None, Some(MAX_RESULTS_MIN)] {
            let listed = list_project_assets(&router, label, project_id, asked).await;
            assert_eq!(
                &listed, expected,
                "case {label:?} (maxResults={asked:?}): a project lists only its own \
                 assets, expected {expected:?}, got {listed:?}"
            );
        }
        assert_eq!(
            &stored_membership(&engine, project_id),
            expected,
            "case {label:?}: the listings answer the membership on this project's own \
             record"
        );
    }
}
