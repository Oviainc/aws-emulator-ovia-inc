// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetRelationships`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: a `PATH_TO_ROOT` traversal reports one `HIERARCHY`
//! relationship per parent -> child edge of the stored hierarchy tree between the
//! asset named in the request and its root — the edge whose `childAssetId` is that
//! asset first, the edge whose `parentAssetId` is the root last, and nothing at all
//! for an asset that is nobody's child. The answer is computed from the association
//! index `AssociateAssets` writes and `DisassociateAssets` prunes, so it tracks the
//! store and the call itself changes nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared wire harness lives in the `asset_lifecycle` suite, and the
// hierarchy-shaped create/associate helpers in `asset_hierarchies`; this file
// reaches both rather than growing a third copy of them.
use crate::asset_hierarchies::{associate, create_asset, create_model, hierarchy_id};
use crate::asset_lifecycle::{call, error_code, hex_token, router_with_engine};

/// A UUID in the modelled `ID` form that names no stored resource.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The name every model in a test chain gives its one hierarchy.
const CHILD_HIERARCHY: &str = "children";

/// One asset model per entry of `levels`, each defining a hierarchy over the next,
/// returned ROOT FIRST.
///
/// Built deepest first, because a hierarchy definition has to name an asset model
/// that already exists.
async fn model_chain(router: &MultiServiceRouter, levels: &[&str]) -> Vec<String> {
    let mut models: Vec<String> = Vec::with_capacity(levels.len());
    for name in levels.iter().rev() {
        let body = match models.last() {
            Some(child) => json!({
                "assetModelName": name,
                "assetModelHierarchies": [
                    {"name": CHILD_HIERARCHY, "childAssetModelId": child},
                ],
            }),
            None => json!({"assetModelName": name}),
        };
        models.push(create_model(router, body).await);
    }
    models.reverse();
    models
}

/// One asset per model of `models` (root first), associated into a single chain,
/// returned root first.
///
/// Every asset also carries `assetExternalId` `<prefix>-<depth>`, so a suite can
/// address any level by either `CustomID` form.
async fn asset_chain(
    router: &MultiServiceRouter,
    models: &[String],
    prefix: &str,
) -> Vec<String> {
    let mut assets: Vec<String> = Vec::with_capacity(models.len());
    for (depth, model) in models.iter().enumerate() {
        assets.push(
            create_asset(
                router,
                json!({
                    "assetName": format!("{prefix} {depth}"),
                    "assetModelId": model,
                    "assetExternalId": format!("{prefix}-{depth}"),
                }),
            )
            .await,
        );
    }
    for depth in 0..assets.len().saturating_sub(1) {
        let hierarchy = hierarchy_id(router, &models[depth], CHILD_HIERARCHY).await;
        let (status, body) = associate(router, &assets[depth], &hierarchy, &assets[depth + 1]).await;
        assert_eq!(
            status, 200,
            "associate level {depth}: expected 200, got {status} {body}"
        );
    }
    assets
}

/// `GET /assets/{asset}/assetRelationships?traversalType=PATH_TO_ROOT{extra}`.
async fn list_relationships(
    router: &MultiServiceRouter,
    asset: &str,
    extra: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset}/assetRelationships?traversalType=PATH_TO_ROOT{extra}"),
        None,
    )
    .await
}

/// The `(parentAssetId, childAssetId)` pairs a successful answer reports, in order,
/// checking every summary is a `HIERARCHY` relationship carrying `hierarchyInfo`.
fn edges(case: &str, body: &Value) -> Vec<(String, String)> {
    body["assetRelationshipSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetRelationshipSummaries in {body}"))
        .iter()
        .map(|summary| {
            assert_eq!(
                summary["relationshipType"], "HIERARCHY",
                "case {case:?}: expected relationshipType HIERARCHY, got {summary}"
            );
            let info = &summary["hierarchyInfo"];
            let parent = info["parentAssetId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a HIERARCHY relationship needs a parentAssetId, got {summary}")
            });
            let child = info["childAssetId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a HIERARCHY relationship needs a childAssetId, got {summary}")
            });
            (parent.to_string(), child.to_string())
        })
        .collect()
}

/// The edges a `PATH_TO_ROOT` traversal from `assets[depth]` has to report, derived
/// from the chain the test associated rather than from the code under test.
fn expected_edges(assets: &[String], depth: usize) -> Vec<(String, String)> {
    (0..depth)
        .rev()
        .map(|level| (assets[level].clone(), assets[level + 1].clone()))
        .collect()
}

/// One row of the traversal table.
struct Case {
    /// What the row is, named in every assertion it makes.
    case: String,
    /// The `CustomID` label the request addresses the asset by.
    label: String,
    /// The stored asset id that label resolves to.
    resolved: String,
    /// The `(parentAssetId, childAssetId)` edges the answer has to report, in order.
    expected: Vec<(String, String)>,
}

#[tokio::test]
async fn path_to_root_reports_one_edge_per_level_up_to_the_root() {
    let (router, _engine) = router_with_engine();
    // A four-level chain, a two-level chain of its own models, and an asset that is
    // in no hierarchy at all: the rule generalises over how far an asset sits from
    // its root, so the rows below differ in that depth.
    let deep_models = model_chain(&router, &["Enterprise", "Site", "Line", "Machine"]).await;
    let deep = asset_chain(&router, &deep_models, "deep").await;
    let shallow_models = model_chain(&router, &["Plant", "Pump"]).await;
    let shallow = asset_chain(&router, &shallow_models, "shallow").await;
    let lonely = create_asset(
        &router,
        json!({"assetName": "Lonely", "assetModelId": shallow_models[1]}),
    )
    .await;

    let mut cases: Vec<Case> = Vec::new();
    for depth in 0..deep.len() {
        cases.push(Case {
            case: format!("four-level chain at depth {depth}"),
            label: deep[depth].clone(),
            resolved: deep[depth].clone(),
            expected: expected_edges(&deep, depth),
        });
    }
    // The same leaf, addressed by the other `CustomID` form the member accepts.
    cases.push(Case {
        case: "four-level leaf by externalId alias".to_string(),
        label: "externalId:deep-3".to_string(),
        resolved: deep[deep.len() - 1].clone(),
        expected: expected_edges(&deep, deep.len() - 1),
    });
    for depth in 0..shallow.len() {
        cases.push(Case {
            case: format!("two-level chain at depth {depth}"),
            label: shallow[depth].clone(),
            resolved: shallow[depth].clone(),
            expected: expected_edges(&shallow, depth),
        });
    }
    cases.push(Case {
        case: "asset in no hierarchy".to_string(),
        label: lonely.clone(),
        resolved: lonely.clone(),
        expected: Vec::new(),
    });

    for Case {
        case,
        label,
        resolved,
        expected,
    } in &cases
    {
        let (status, body) = list_relationships(&router, label, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let actual = edges(case, &body);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected the edges up to the root, got {actual:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: one page holds every edge, so no nextToken is due, got {body}"
        );
        // The invariants the traversal's documentation states, checked without the
        // table: the request's own asset is the child of the first edge, the parent of
        // the last edge is a root (it has no path of its own), and consecutive edges
        // share an asset.
        if let (Some((_, first_child)), Some((last_parent, _))) = (actual.first(), actual.last()) {
            assert_eq!(
                first_child, resolved,
                "case {case:?}: the first edge must be the asset's own, got {actual:?}"
            );
            let (status, root_body) = list_relationships(&router, last_parent, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading the last parent's own path: got {status} {root_body}"
            );
            let root_edges = edges(case, &root_body);
            assert!(
                root_edges.is_empty(),
                "case {case:?}: the last edge's parent must be the root, but it has \
                 parents of its own: {root_edges:?}"
            );
        }
        for pair in actual.windows(2) {
            assert_eq!(
                pair[0].0, pair[1].1,
                "case {case:?}: consecutive edges must share an asset, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn pages_of_the_path_concatenate_to_the_whole_path() {
    let (router, _engine) = router_with_engine();
    let models = model_chain(&router, &["Enterprise", "Site", "Line", "Machine"]).await;
    let assets = asset_chain(&router, &models, "paged").await;
    let leaf = assets[assets.len() - 1].clone();
    let whole = expected_edges(&assets, assets.len() - 1);

    // Rows: the page sizes around the path's own length (3 edges) — below it, equal
    // to it, past it, and the modelled maximum.
    for max_results in [1usize, 2, 3, 4, 250] {
        let case = format!("maxResults={max_results}");
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let extra = match token.as_deref() {
                Some(token) => format!("&maxResults={max_results}&nextToken={token}"),
                None => format!("&maxResults={max_results}"),
            };
            let (status, body) = list_relationships(&router, &leaf, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages}: expected 200, got {status} {body}"
            );
            let page = edges(&case, &body);
            assert!(
                page.len() <= max_results,
                "case {case:?}: page {pages} holds more than maxResults edges: {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging did not terminate, collected {collected:?}"
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, whole,
            "case {case:?}: the pages must concatenate to the whole path, got {collected:?}"
        );
        let expected_pages = whole.len().div_ceil(max_results).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

#[tokio::test]
async fn refuses_a_traversal_the_store_cannot_answer() {
    let (router, _engine) = router_with_engine();
    let models = model_chain(&router, &["Enterprise", "Site", "Line", "Machine"]).await;
    let assets = asset_chain(&router, &models, "refused").await;
    let leaf = assets[assets.len() - 1].clone();
    // The token form this operation issues: the hex of the child asset id the next
    // page resumes from. `hex_token` is the harness's hand-rolled encoder, kept as an
    // INDEPENDENT oracle of the handler's own.

    // Rows: one per way the request names something the store does not hold — an
    // absent asset in either `CustomID` form, and a `nextToken` that resumes from
    // nowhere on this path (undecodable, and decodable but off the path).
    let cases: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "asset id names no stored asset",
            ABSENT_ID.to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "asset alias names no stored asset",
            "externalId:no-such-asset".to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "nextToken is not a token this operation issued",
            leaf.clone(),
            "&nextToken=notatoken".to_string(),
            400,
            "InvalidRequestException",
        ),
        (
            "nextToken resumes from an asset off this path",
            leaf.clone(),
            format!("&nextToken={}", hex_token(ABSENT_ID)),
            400,
            "InvalidRequestException",
        ),
        (
            "nextToken resumes from the root, which is nobody's child",
            leaf.clone(),
            format!("&nextToken={}", hex_token(&assets[0])),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, asset, extra, expected_status, expected_code) in &cases {
        let (status, body) = list_relationships(&router, asset, extra).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_path_tracks_the_stored_associations() {
    let (router, _engine) = router_with_engine();
    let models = model_chain(&router, &["Enterprise", "Site", "Line", "Machine"]).await;
    let assets = asset_chain(&router, &models, "tracked").await;
    let leaf = assets[assets.len() - 1].clone();

    // Reading the path twice answers the same: this operation reports, it does not
    // consume or move anything.
    let (_, first) = list_relationships(&router, &leaf, "").await;
    let (_, again) = list_relationships(&router, &leaf, "").await;
    assert_eq!(
        edges("read twice", &first),
        edges("read twice", &again),
        "case {:?}: a report must not change what a later caller sees",
        "read twice"
    );

    // Rows: cutting the chain one level at a time, from the root down, leaves the
    // leaf reporting exactly the edges still stored below the cut.
    let whole = expected_edges(&assets, assets.len() - 1);
    for cut in 1..assets.len() {
        let case = format!("disassociated level {cut}");
        let hierarchy = hierarchy_id(&router, &models[cut - 1], CHILD_HIERARCHY).await;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/assets/{}/disassociate", assets[cut - 1]),
            Some(json!({ "hierarchyId": hierarchy, "childAssetId": assets[cut] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the disassociation to be accepted, got {status} {body}"
        );

        let (status, body) = list_relationships(&router, &leaf, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let actual = edges(&case, &body);
        let expected = whole[..whole.len() - cut].to_vec();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected only the edges still stored, got {actual:?}"
        );
    }
}
