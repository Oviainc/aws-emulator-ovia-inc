// SPDX-License-Identifier: CC0-1.0

//! `ListAssociatedAssets`, end to end over the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule these suites pin: `ListAssociatedAssets` reports, from the hierarchy
//! associations the store holds, every asset associated with the queried asset in
//! the requested traversal direction — the assets associated UNDER it for `CHILD`,
//! the asset it is associated under for `PARENT` — restricted to one hierarchy when
//! the request names one, in a stable order, paged by `maxResults` with a resumable
//! `nextToken`; a label that resolves to no stored asset, or to a hierarchy the
//! asset does not declare, is a modelled 404, and the read leaves the store exactly
//! as it found it.
//!
//! The associations themselves are made through `AssociateAssets` over the wire
//! (`tests/all/asset_hierarchies.rs` owns that operation's own cover), so what this
//! suite reads back is what the real writer persisted, not hand-written state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

// `tests/all/asset_lifecycle.rs` is this crate's test harness: the request walk and
// the error-code reader live there, so this suite reaches them rather than growing a
// second copy. The create/describe/associate walk this suite needs is already
// written for `AssociateAssets`'s own cover, so it comes from there.
use crate::asset_hierarchies::{associate, create_asset, create_model, hierarchy_id};
use crate::asset_lifecycle::{call, error_code, hex_token, router_with_engine};

/// A snapshot of the whole association map, so a read can be shown to leave it as
/// it found it.
fn association_map(
    engine: &Arc<engine::InMemoryEngine>,
) -> std::collections::HashMap<(String, String), Vec<String>> {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let guard = state.lock();
    guard.asset_hierarchy_children.clone()
}

/// `GET /assets/{assetId}/hierarchies` with `query` appended.
async fn list_associated(
    router: &MultiServiceRouter,
    asset_label: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/assets/{asset_label}/hierarchies")
    } else {
        format!("/assets/{asset_label}/hierarchies?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// The `id` of every summary in a `ListAssociatedAssets` answer, in answer order.
fn summary_ids(body: &Value) -> Vec<String> {
    body["assetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetSummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no id in {body}"))
                .to_string()
        })
        .collect()
}

/// A fixture: a parent asset of a model with two hierarchies, three children, and
/// the associations `left -> [one, two]`, `right -> [three]`, each recorded by a
/// real `AssociateAssets` call.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    parent: String,
    left: String,
    right: String,
    children: Vec<String>,
}

impl Fixture {
    async fn build() -> Self {
        let (router, engine) = router_with_engine();
        let child_model = create_model(
            &router,
            json!({
                "assetModelName": "Turbine",
                "assetModelExternalId": "turbine_model",
            }),
        )
        .await;
        let parent_model = create_model(
            &router,
            json!({
                "assetModelName": "WindFarm",
                "assetModelHierarchies": [
                    {"name": "left_wing", "externalId": "left_wing", "childAssetModelId": &child_model},
                    {"name": "right_wing", "childAssetModelId": "externalId:turbine_model"},
                ],
            }),
        )
        .await;
        let left = hierarchy_id(&router, &parent_model, "left_wing").await;
        let right = hierarchy_id(&router, &parent_model, "right_wing").await;

        let parent = create_asset(
            &router,
            json!({
                "assetName": "Farm1",
                "assetModelId": &parent_model,
                "assetExternalId": "farm_one",
            }),
        )
        .await;
        let mut children = Vec::new();
        for name in ["Turbine1", "Turbine2", "Turbine3"] {
            children.push(
                create_asset(
                    &router,
                    json!({
                        "assetName": name,
                        "assetModelId": &child_model,
                        "assetExternalId": name.to_lowercase(),
                    }),
                )
                .await,
            );
        }

        // Two children under the first hierarchy, one under the second — recorded
        // the way a client records them, through `AssociateAssets`.
        for (hierarchy, child) in [
            (&left, &children[0]),
            (&left, &children[1]),
            (&right, &children[2]),
        ] {
            let (status, body) = associate(&router, &parent, hierarchy, child).await;
            assert_eq!(
                status, 200,
                "associate {child} under {hierarchy}: got {status} {body}"
            );
        }

        Self {
            router,
            engine,
            parent,
            left,
            right,
            children,
        }
    }
}

#[tokio::test]
async fn each_traversal_direction_reports_the_stored_associations() {
    let fixture = Fixture::build().await;
    let (one, two, three) = (
        fixture.children[0].clone(),
        fixture.children[1].clone(),
        fixture.children[2].clone(),
    );

    // Rows span the two directions the model declares, the filtered and unfiltered
    // hierarchy forms, and an asset at each end of an association: the expectation
    // is derived from the seeded map, never from one recorded request.
    let cases: [(&str, &str, &str, Vec<String>); 8] = [
        (
            "children across every hierarchy",
            &fixture.parent,
            "",
            vec![one.clone(), two.clone(), three.clone()],
        ),
        (
            "children of the first hierarchy only",
            &fixture.parent,
            &format!("hierarchyId={}", fixture.left),
            vec![one.clone(), two.clone()],
        ),
        (
            "children of the second hierarchy only",
            &fixture.parent,
            &format!("hierarchyId={}", fixture.right),
            vec![three.clone()],
        ),
        (
            "CHILD stated explicitly",
            &fixture.parent,
            "traversalDirection=CHILD",
            vec![one.clone(), two.clone(), three.clone()],
        ),
        (
            "the parent of an associated child",
            &one,
            "traversalDirection=PARENT",
            vec![fixture.parent.clone()],
        ),
        (
            "the parent through the associating hierarchy",
            &three,
            &format!("traversalDirection=PARENT&hierarchyId={}", fixture.right),
            vec![fixture.parent.clone()],
        ),
        (
            "the parent through a hierarchy that does not hold the child",
            &three,
            &format!("traversalDirection=PARENT&hierarchyId={}", fixture.left),
            Vec::new(),
        ),
        (
            "a root asset has no parent",
            &fixture.parent,
            "traversalDirection=PARENT",
            Vec::new(),
        ),
    ];

    for (case, asset, query, expected) in cases {
        let (status, body) = list_associated(&fixture.router, asset, query).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let ids = summary_ids(&body);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a complete page must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn every_page_size_walks_each_associated_asset_exactly_once() {
    let fixture = Fixture::build().await;
    // One more child than any page size below, so the last row still pages.
    let extra = create_asset(
        &fixture.router,
        json!({"assetName": "Turbine4", "assetModelId": "externalId:turbine_model"}),
    )
    .await;
    let (status, body) = associate(&fixture.router, &fixture.parent, &fixture.left, &extra).await;
    assert_eq!(status, 200, "associate the fourth child: got {status} {body}");
    let expected = vec![
        fixture.children[0].clone(),
        fixture.children[1].clone(),
        fixture.children[2].clone(),
        extra.clone(),
    ];

    // Rows are page sizes: each published bound of the modelled `maxResults` range
    // (1..250, from smithy_query on ListAssociatedAssets.maxResults), the sizes
    // either side of the result count, and the absent form that takes the default.
    let cases: [(&str, Option<i32>); 6] = [
        ("the range minimum", Some(1)),
        ("a page smaller than the result set", Some(3)),
        ("a page exactly the result set", Some(4)),
        ("a page larger than the result set", Some(5)),
        ("the range maximum", Some(250)),
        ("no maxResults at all", None),
    ];

    for (case, max_results) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // Bounded so a token that never clears cannot hang the suite.
        for _page in 0..expected.len() + 1 {
            let mut query = match max_results {
                Some(size) => format!("maxResults={size}"),
                None => String::new(),
            };
            if let Some(resume) = token.as_deref() {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={resume}"));
            }
            let (status, body) = list_associated(&fixture.router, &fixture.parent, &query).await;
            assert_eq!(status, 200, "case {case:?}: got {status} {body}");
            let page = summary_ids(&body);
            if let Some(size) = max_results {
                assert!(
                    page.len() <= size as usize,
                    "case {case:?}: page of {} exceeds maxResults {size}: {page:?}",
                    page.len()
                );
            }
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            // The token is the resume point of the next page: the hex of the id that
            // follows the ones already seen.
            match token.as_deref() {
                Some(issued) => {
                    // A token past the end would be a defect in its own right, so it
                    // is reported as one rather than panicking on the lookup below.
                    assert!(
                        seen.len() < expected.len(),
                        "case {case:?}: a token was issued after every asset was \
                         reported: {seen:?}"
                    );
                    assert_eq!(
                        issued,
                        hex_token(&expected[seen.len()]),
                        "case {case:?}: unexpected resume point after {seen:?}"
                    );
                }
                None => break,
            }
        }
        assert!(token.is_none(), "case {case:?}: pagination never finished");
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn external_id_labels_name_the_same_associations_as_raw_ids() {
    let fixture = Fixture::build().await;
    let expected = vec![fixture.children[0].clone(), fixture.children[1].clone()];

    // Each row is one label form a `CustomID` member accepts, on the asset and on
    // the hierarchy; both must name the same association.
    let cases: [(&str, String, String); 3] = [
        (
            "raw asset id and raw hierarchy id",
            fixture.parent.clone(),
            fixture.left.clone(),
        ),
        (
            "externalId asset label",
            "externalId:farm_one".to_string(),
            fixture.left.clone(),
        ),
        (
            "externalId hierarchy label",
            fixture.parent.clone(),
            "externalId:left_wing".to_string(),
        ),
    ];

    for (case, asset, hierarchy) in cases {
        let (status, body) =
            list_associated(&fixture.router, &asset, &format!("hierarchyId={hierarchy}")).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let ids = summary_ids(&body);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

/// One row of the refusal table: a request that is wrong in exactly one dimension,
/// the modelled answer it must get, and the requested identifier the message is
/// built from (`None` where no identifier is echoed).
struct Refusal {
    case: &'static str,
    asset: String,
    query: String,
    status: u16,
    /// The wire error code, where this operation's own modelled error set fixes it.
    /// `None` for a request the shared validation layer refuses BEFORE the handler
    /// runs: the status is the requirement there, and the envelope that layer uses
    /// is the subject of its own open issue
    /// (`20261022T070208Z-iotsitewise-iot-sitewise-control-plane-6-divergences-…`,
    /// cause 5), so pinning it here would pin a defect.
    code: Option<&'static str>,
    names: Option<String>,
}

#[tokio::test]
async fn unresolvable_labels_and_tokens_are_modelled_refusals() {
    let fixture = Fixture::build().await;
    // A hierarchy some OTHER model declares: it resolves, but the queried asset does
    // not declare it, so it names no hierarchy OF THIS ASSET.
    let other_model = create_model(
        &fixture.router,
        json!({
            "assetModelName": "SolarFarm",
            "assetModelHierarchies": [
                {"name": "panels", "childAssetModelId": "externalId:turbine_model"},
            ],
        }),
    )
    .await;
    let foreign = hierarchy_id(&fixture.router, &other_model, "panels").await;
    // A well-formed id that names nothing, and a hex token for an asset that is not
    // in this enumeration.
    let absent = "11111111-2222-3333-4444-555555555555";

    // Rows span each dimension a request can be wrong in: the asset label, the
    // hierarchy label (unknown, and known-but-not-this-asset's) and the token. The
    // last column is the identifier the refusal message is built from — the rule for
    // these messages, which AWS derives from the request.
    let cases: [Refusal; 8] = [
        Refusal {
            case: "asset id that names nothing",
            asset: absent.to_string(),
            query: String::new(),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some(absent.to_string()),
        },
        Refusal {
            case: "asset externalId that names nothing",
            asset: "externalId:no_such_asset".to_string(),
            query: String::new(),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some("externalId:no_such_asset".to_string()),
        },
        Refusal {
            case: "hierarchy id no model declares",
            asset: fixture.parent.clone(),
            query: format!("hierarchyId={absent}"),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some(absent.to_string()),
        },
        Refusal {
            case: "hierarchy externalId no model declares",
            asset: fixture.parent.clone(),
            query: "hierarchyId=externalId:no_such_hierarchy".to_string(),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some("externalId:no_such_hierarchy".to_string()),
        },
        Refusal {
            case: "hierarchy another model declares",
            asset: fixture.parent.clone(),
            query: format!("hierarchyId={foreign}"),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some(foreign.clone()),
        },
        Refusal {
            case: "nextToken this operation never issued",
            asset: fixture.parent.clone(),
            query: format!("nextToken={}", hex_token(absent)),
            status: 400,
            code: Some("InvalidRequestException"),
            names: None,
        },
        Refusal {
            // Upward, the hierarchy belongs to the parent, so it is resolved against
            // the hierarchies of the assets that hold the queried asset as a child;
            // a label none of them declares is still a 404 on that path.
            case: "hierarchy nothing on the upward side declares",
            asset: fixture.children[0].clone(),
            query: format!("traversalDirection=PARENT&hierarchyId={absent}"),
            status: 404,
            code: Some("ResourceNotFoundException"),
            names: Some(absent.to_string()),
        },
        Refusal {
            // `TraversalDirection` declares exactly CHILD and PARENT (smithy_query on
            // ListAssociatedAssets.traversalDirection), so a third value is a request
            // error (400), never a silently-defaulted direction. The shared validation
            // layer refuses it before this handler runs, so the handler's own
            // `InvalidRequestException` branch is the belt-and-braces path.
            case: "traversalDirection outside the modelled enum",
            asset: fixture.parent.clone(),
            query: "traversalDirection=SIBLING".to_string(),
            status: 400,
            code: None,
            names: Some("SIBLING".to_string()),
        },
    ];

    for row in cases {
        let Refusal {
            case,
            asset,
            query,
            status: status_expected,
            code,
            names,
        } = row;
        let (status, body) = list_associated(&fixture.router, &asset, &query).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected {status_expected}, got {status} {body}"
        );
        if let Some(code) = code {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected {code}, got {body}"
            );
        }
        if let Some(identifier) = names {
            let message = body["message"].as_str().unwrap_or("");
            assert!(
                message.contains(&identifier),
                "case {case:?}: the refusal must name {identifier:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_summary_agrees_with_describe_asset_and_the_read_changes_nothing() {
    let fixture = Fixture::build().await;

    let (status, first) = list_associated(&fixture.router, &fixture.parent, "").await;
    assert_eq!(status, 200, "first read: got {status} {first}");
    // The member-by-member comparison below is only evidence if it actually runs:
    // pin the count the seeded associations imply so an empty answer cannot pass
    // this suite vacuously.
    assert_eq!(
        summary_ids(&first).len(),
        fixture.children.len(),
        "the fixture associates {} children, got {first}",
        fixture.children.len()
    );

    for summary in first["assetSummaries"].as_array().expect("summaries") {
        let id = summary["id"].as_str().expect("an id");
        let (status, described) = call(&fixture.router, "GET", &format!("/assets/{id}"), None).await;
        assert_eq!(status, 200, "DescribeAsset {id}: got {status} {described}");
        // Each row is one member the two views of the SAME stored asset share; a
        // summary built from anything but that record would disagree on one.
        let shared: [(&str, &str); 6] = [
            ("id", "assetId"),
            ("arn", "assetArn"),
            ("name", "assetName"),
            ("assetModelId", "assetModelId"),
            ("creationDate", "assetCreationDate"),
            ("hierarchies", "assetHierarchies"),
        ];
        for (summary_member, describe_member) in shared {
            assert_eq!(
                summary.get(summary_member),
                described.get(describe_member),
                "case {summary_member:?}: the summary and DescribeAsset disagree for {id}: \
                 {summary} vs {described}"
            );
        }
        let state = summary["status"]["state"].as_str().unwrap_or("");
        assert!(
            ["CREATING", "ACTIVE"].contains(&state),
            "case {id:?}: a summary must report a modelled state, got {state:?}"
        );
    }

    // A reporting operation must not change what a later caller sees: neither the
    // association map it reads nor the answer it gives.
    let associations = association_map(&fixture.engine);
    let (status, second) = list_associated(&fixture.router, &fixture.parent, "").await;
    assert_eq!(status, 200, "second read: got {status} {second}");
    assert_eq!(
        summary_ids(&first),
        summary_ids(&second),
        "two reads of the same store must answer the same assets"
    );
    let after = association_map(&fixture.engine);
    assert_eq!(
        associations, after,
        "listing associated assets must leave the association map untouched"
    );
}
