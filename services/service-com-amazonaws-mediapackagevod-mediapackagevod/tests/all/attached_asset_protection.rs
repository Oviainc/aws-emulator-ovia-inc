// SPDX-License-Identifier: CC0-1.0

//! `DeletePackagingGroup` and the ASSET side of its referential-integrity guard.
//!
//! Every test drives real HTTP through the router the production binary builds, so what
//! is asserted is the wire behaviour a `mediapackage-vod` client sees, and each test
//! owns its own engine. The request harness is the shared one
//! ([`super::packaging_lifecycle::call`] and friends).
//!
//! The RULE under test, over a TABLE of inputs rather than one recorded call: a
//! control-plane resource that other resources still reference cannot be deleted —
//! while ANY stored child names the addressed group in its `PackagingGroupId`, the
//! delete is refused with the modelled `UnprocessableEntityException` (HTTP 422,
//! `errorKind client`) naming the dependency class, the refused call changes nothing,
//! and the identical delete succeeds once the children are gone. Assets and packaging
//! configurations are INDEPENDENT classes: either alone refuses the delete.
//!
//! The configuration side of the same rule, and the 404 for an id no group is stored
//! under, are in `tests/all/packaging_group_deletion.rs`.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::packaging_lifecycle::{call, create_asset, create_config, create_group, router, REGION};
// The status + `x-amzn-errortype` + body harness this crate's suites share, rather than
// a third copy of it.
use super::resource_tag_listing::{call as answer, Answer};

/// The recorded success status of this service's deletes (`DeletePackagingGroup`,
/// `DeleteAsset` and `DeletePackagingConfiguration` all answer 204 No Content — see
/// `src/ops/delete_packaging_group.rs`).
const DELETE_SUCCESS: u16 = 204;

/// The wire error code of the dependency refusal and its modelled `httpError`
/// (`smithy_query(query="operation",operation="DeletePackagingGroup")`:
/// `UnprocessableEntityException`, `errorKind client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// The wire error code of a missing addressed resource and its modelled `httpError`
/// (same query: `NotFoundException`, `errorKind client`, `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// The message a delete refused for still-attached assets carries.
const ATTACHED_ASSETS_MESSAGE: &str = "id cannot be deleted because it has attached assets";

/// `DeletePackagingGroup`'s whole answer for `id`.
async fn delete_group(router: &MultiServiceRouter, id: &str) -> Answer {
    answer(
        router,
        REGION,
        "DELETE",
        &format!("/packaging_groups/{id}"),
        None,
    )
    .await
}

/// `DescribePackagingGroup`'s whole answer for `id`.
async fn describe_group(router: &MultiServiceRouter, id: &str) -> Answer {
    answer(
        router,
        REGION,
        "GET",
        &format!("/packaging_groups/{id}"),
        None,
    )
    .await
}

/// One row of the attached-asset table.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The ids of the assets created IN the group under test.
    assets: Vec<&'static str>,
    /// Whether the group ALSO holds a packaging configuration, to show the asset
    /// precondition is independent of the configuration one.
    with_config: bool,
}

/// The group every row populates and then tries to delete.
const GROUP: &str = "sim2real-mpvod-held";

/// The rows span the attached-asset COUNT (one, two, five), the FORM of the asset id
/// (short, dotted, underscored, upper-cased, and the long UUID-suffixed form the
/// recorded fixtures generate) and whether a packaging configuration is attached as
/// well — because the rule is about whether ANY asset still names the group, not about
/// any of those.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one asset, no configuration",
            assets: vec!["sim2real-mpvod-asset-7f1c2d90-4b55-4e2f-9b0a-1f4d8c6e5a21"],
            with_config: false,
        },
        Row {
            case: "two assets, no configuration",
            assets: vec!["a", "asset.two"],
            with_config: false,
        },
        Row {
            case: "five assets of differing id form, no configuration",
            assets: vec![
                "a-1",
                "asset_2",
                "ASSET-3",
                "asset.4",
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            ],
            with_config: false,
        },
        Row {
            case: "one asset and one configuration",
            assets: vec!["asset-with-config"],
            with_config: true,
        },
    ]
}

/// Populate a fresh router with `row`'s group, assets and optional configuration.
async fn populate(row: &Row) -> MultiServiceRouter {
    let router = router();
    create_group(&router, GROUP, Some(json!({ "suite": "attached-assets" }))).await;
    if row.with_config {
        create_config(
            &router,
            "cfg",
            GROUP,
            json!({ "hlsPackage": { "hlsManifests": [{}] } }),
        )
        .await;
    }
    for id in &row.assets {
        create_asset(&router, id, GROUP).await;
    }
    router
}

#[tokio::test]
async fn a_group_an_asset_is_attached_to_cannot_be_deleted() {
    for row in rows() {
        let router = populate(&row).await;
        let case = row.case;
        let (_, before) = call(&router, "GET", &format!("/packaging_groups/{GROUP}"), None).await;

        // Twice: a refusal that had a partial effect would answer differently the
        // second time.
        for attempt in 1..=2 {
            let refused = delete_group(&router, GROUP).await;
            assert_eq!(
                (refused.status, refused.errortype.as_str()),
                (UNPROCESSABLE.1, UNPROCESSABLE.0),
                "case {case:?} attempt {attempt}: DeletePackagingGroup with {} asset(s) \
                 attached: expected HTTP {} / {}, got HTTP {} / {:?} body {}",
                row.assets.len(),
                UNPROCESSABLE.1,
                UNPROCESSABLE.0,
                refused.status,
                refused.errortype,
                refused.body
            );
            // The attached-asset class is reported whether or not a configuration is
            // attached too, so the asset precondition is not shadowed by the other one.
            assert_eq!(
                refused.body["message"].as_str(),
                Some(ATTACHED_ASSETS_MESSAGE),
                "case {case:?} attempt {attempt}: expected the attached-assets message, \
                 got {}",
                refused.body
            );
            assert_eq!(
                refused.body["errors"],
                json!({ "id": ["cannot be deleted because it has attached assets"] }),
                "case {case:?} attempt {attempt}: expected the violation attributed to \
                 `id`, got {}",
                refused.body
            );
        }

        // The refusal had NO side effect: the group reads back exactly as before and
        // every asset still names it.
        let (status, after) = call(&router, "GET", &format!("/packaging_groups/{GROUP}"), None).await;
        assert_eq!(
            (status, &after),
            (200, &before),
            "case {case:?}: a refused delete must leave the group exactly as it was, \
             before {before} after {after}"
        );
        for id in &row.assets {
            let (status, asset) = call(&router, "GET", &format!("/assets/{id}"), None).await;
            assert_eq!(
                (status, asset["packagingGroupId"].as_str()),
                (200, Some(GROUP)),
                "case {case:?}: DescribeAsset {id:?} after the refused delete must still \
                 report the group, got HTTP {status} body {asset}"
            );
        }

        // The positive control: with the assets (and any configuration) deleted through
        // their own operations, the identical delete succeeds and the group is gone.
        for id in &row.assets {
            let (status, body) = call(&router, "DELETE", &format!("/assets/{id}"), None).await;
            assert_eq!(
                status, DELETE_SUCCESS,
                "case {case:?}: DeleteAsset {id:?}: expected {DELETE_SUCCESS}, got {status} \
                 body {body}"
            );
        }
        if row.with_config {
            let (status, body) =
                call(&router, "DELETE", "/packaging_configurations/cfg", None).await;
            assert_eq!(
                status, DELETE_SUCCESS,
                "case {case:?}: DeletePackagingConfiguration cfg: expected \
                 {DELETE_SUCCESS}, got {status} body {body}"
            );
        }
        let accepted = delete_group(&router, GROUP).await;
        assert_eq!(
            accepted.status, DELETE_SUCCESS,
            "case {case:?}: with no child left, DeletePackagingGroup: expected \
             {DELETE_SUCCESS}, got {} body {}",
            accepted.status, accepted.body
        );
        let gone = describe_group(&router, GROUP).await;
        assert_eq!(
            (gone.status, gone.errortype.as_str()),
            (NOT_FOUND.1, NOT_FOUND.0),
            "case {case:?}: DescribePackagingGroup after the accepted delete: expected \
             HTTP {} / {}, got HTTP {} / {:?} body {}",
            NOT_FOUND.1,
            NOT_FOUND.0,
            gone.status,
            gone.errortype,
            gone.body
        );
        assert_eq!(
            gone.body["message"].as_str(),
            Some(format!("PackagingGroup with id={GROUP} not found").as_str()),
            "case {case:?}: expected the not-found message naming the requested id, got {}",
            gone.body
        );
    }
}

#[tokio::test]
async fn only_assets_of_the_addressed_group_protect_it() {
    // (case, the ids of assets created in ANOTHER group — the group under test holds
    // none of its own)
    let cases: [(&str, &[&str]); 3] = [
        ("one asset elsewhere", &["elsewhere-1"]),
        ("three assets elsewhere", &["a", "b", "c"]),
        (
            "an asset elsewhere whose id starts with this group's id",
            &["sim2real-mpvod-held-asset"],
        ),
    ];

    for (case, elsewhere) in cases {
        let router = router();
        create_group(&router, GROUP, None).await;
        create_group(&router, "other-group", None).await;
        for id in elsewhere {
            create_asset(&router, id, "other-group").await;
        }

        let accepted = delete_group(&router, GROUP).await;
        assert_eq!(
            accepted.status, DELETE_SUCCESS,
            "case {case:?}: a group no asset of its own names must delete: expected \
             {DELETE_SUCCESS}, got {} body {}",
            accepted.status, accepted.body
        );
        // The other group's assets are untouched, and that group is still protected.
        for id in elsewhere {
            let (status, asset) = call(&router, "GET", &format!("/assets/{id}"), None).await;
            assert_eq!(
                (status, asset["packagingGroupId"].as_str()),
                (200, Some("other-group")),
                "case {case:?}: the other group's asset {id:?} must be untouched, got \
                 HTTP {status} body {asset}"
            );
        }
        let other = delete_group(&router, "other-group").await;
        assert_eq!(
            (other.status, other.errortype.as_str()),
            (UNPROCESSABLE.1, UNPROCESSABLE.0),
            "case {case:?}: the group those assets DO name must be protected: expected \
             HTTP {} / {}, got HTTP {} / {:?} body {}",
            UNPROCESSABLE.1,
            UNPROCESSABLE.0,
            other.status,
            other.errortype,
            other.body
        );
    }
}
