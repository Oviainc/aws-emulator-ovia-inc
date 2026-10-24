// SPDX-License-Identifier: CC0-1.0

//! `ListScenes` end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the `workspaceId` label,
//! the status code and the serialized body are all exercised, not just a handler
//! body.
//!
//! The suites assert RULES over tables of inputs rather than one recorded case:
//!
//! * a listing reports every scene the named workspace HOLDS, in `sceneId`
//!   order, each summary saying about a scene exactly what `GetScene` says about
//!   it — and nothing about a scene another workspace holds;
//! * a page sequence partitions the listing exactly once, whatever the page size
//!   and however many scenes the workspace holds;
//! * the page size stays inside what the model publishes for `maxResults`
//!   (0..200), with this service's default when a request names none, and a
//!   `nextToken` exactly when rows are left over;
//! * only a token this listing minted for this workspace resumes it;
//! * listing REPORTS: it changes nothing a later caller sees.
//!
//! Every scene under test is written by the real `CreateScene` path, so the
//! listing is proved over records the create persisted rather than over seeded
//! ones.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

use crate::harness::{scene_arn, workspace_arn, Emulator, Resp};

/// The members a `SceneSummary` carries that `GetSceneResponse` carries too —
/// the ones a summary and the single-resource read must agree about. Taken from
/// the model: `smithy_query(query="shape",shape="SceneSummary")` declares
/// `sceneId`, `contentLocation`, `arn`, `creationDateTime` and `updateDateTime`
/// required and `description` optional.
const SHARED_MEMBERS: [&str; 6] = [
    "sceneId",
    "contentLocation",
    "arn",
    "creationDateTime",
    "updateDateTime",
    "description",
];

/// An emulator holding the workspaces `workspace_ids` names.
async fn emulator_with_workspaces(workspace_ids: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for workspace_id in workspace_ids {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// Create one scene through the real `CreateScene` path, failing the case loudly
/// if the create the suite depends on did not land.
async fn create_scene(emulator: &Emulator, workspace_id: &str, scene_id: &str, request: Value) {
    let mut body = request;
    body["sceneId"] = json!(scene_id);
    body["contentLocation"] = body
        .get("contentLocation")
        .cloned()
        .unwrap_or_else(|| json!(format!("s3://bucket/{scene_id}.json")));
    let created = emulator.create_scene(workspace_id, body).await;
    assert_eq!(
        created.status, 200,
        "case {scene_id:?}: the scene the listing must report has to be created, got {created:?}"
    );
}

/// The `sceneSummaries` of a response: a present array, never an absent member,
/// even for a workspace holding no scene.
fn summaries_of(case: &str, resp: &Resp) -> Vec<Value> {
    resp.member("sceneSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: sceneSummaries must be a present array, got {resp:?}")
        })
}

/// The `sceneId`s of a page, in the order the page served them.
fn page_ids(case: &str, resp: &Resp) -> Vec<String> {
    summaries_of(case, resp)
        .iter()
        .map(|summary| {
            summary["sceneId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries a sceneId, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// The page size the service serves for a requested `maxResults`: this service's
/// default when the caller names none (the member's documentation publishes
/// none), and never more than the service's own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Walk every page of `ListScenes` for `workspace_id` at page size
/// `max_results`, returning the ids in the order they were served (so a
/// duplicate is visible) and the number of pages.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    workspace_id: &str,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
    assert_ne!(
        max_results,
        Some(0),
        "case {case:?}: a page size of zero never advances, so a walk cannot be driven at it"
    );
    let mut ids = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let resp = emulator.list_scenes(workspace_id, request).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: page {pages} expected 200, got {resp:?}"
        );
        pages += 1;
        let page = page_ids(case, &resp);
        assert!(
            page.len() <= effective_page_size(max_results),
            "case {case:?}: page {pages} served {} rows, past the page size {}",
            page.len(),
            effective_page_size(max_results)
        );
        ids.extend(page);
        next_token = resp.member("nextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return (ids, pages);
        }
        assert!(
            pages < 128,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
}

/// The table the reporting rows are built from: `(sceneId, the create's body)`.
///
/// The rows differ along what the rule generalises over — the shape of the
/// `sceneId` (upper case, lower case, digit-leading, underscore-carrying), which
/// optional members the create carried (a description, an empty description, no
/// description at all, capabilities, scene metadata, tags) and the form of the
/// `contentLocation` (the modelled `S3Url` pattern `^[sS]3://[A-Za-z0-9._/-]+$`
/// admits either case of the scheme and a nested key) — never along a mere
/// rename. They are created in an order that is NOT `sceneId` order, so the
/// ordering assertion cannot pass by accident.
fn reporting_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ZETA",
            json!({
                "contentLocation": "S3://bucket/nested/path/zeta.json",
                "description": "the last scene by id",
            }),
        ),
        ("a_lower9", json!({"description": ""})),
        ("0numeric", json!({"capabilities": ["zeta", "MATTERPORT"]})),
        (
            "MID_9",
            json!({
                "contentLocation": "s3://bucket/mid.name_v1.json",
                "sceneMetadata": {"floor": "3", "building_id": "B-12"},
                "tags": {"team": "plant"},
            }),
        ),
        ("bare", json!({})),
    ]
}

/// RULE: a listing reports every scene the named workspace holds, ordered by
/// `sceneId`, each summary carrying for that scene the same `sceneId`,
/// `contentLocation`, `arn`, `description` and timestamps the single-resource
/// read reports — and nothing about a scene another workspace holds; a workspace
/// the service does not hold is refused with the modelled `ValidationException`.
#[tokio::test]
async fn a_listing_reports_every_held_scene_as_the_single_read_does() {
    const WORKSPACE: &str = "listedScenesWorkspace";
    const OTHER_WORKSPACE: &str = "neighbourScenesWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;

    for (scene_id, request) in reporting_rows() {
        create_scene(&emulator, WORKSPACE, scene_id, request).await;
    }
    // A scene in a different workspace: the listing of one workspace must not
    // see it.
    create_scene(&emulator, OTHER_WORKSPACE, "neighbourScene", json!({})).await;

    let mut expected: Vec<String> = reporting_rows()
        .iter()
        .map(|(scene_id, _)| scene_id.to_string())
        .collect();
    expected.sort();

    let case = "allHeld";
    let listed = emulator
        .list_scenes(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListScenes, got {listed:?}"
    );
    assert_eq!(
        page_ids(case, &listed),
        expected,
        "case {case:?}: the listing must report every held scene in sceneId order, got {listed:?}"
    );

    // Every summary says about a scene what the single-resource read says.
    for summary in summaries_of(case, &listed) {
        let scene_id = summary["sceneId"].as_str().unwrap_or_default().to_string();
        let read = emulator.get_scene(WORKSPACE, &scene_id).await;
        assert_eq!(
            read.status, 200,
            "case {scene_id:?}: expected 200 from GetScene, got {read:?}"
        );
        assert_eq!(
            summary["arn"],
            json!(scene_arn(WORKSPACE, &scene_id)),
            "case {scene_id:?}: the summary must carry the scene's ARN, got {summary:?}"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "case {scene_id:?}: the summary's {member} must be what GetScene reports ({:?}), \
                 got {:?}",
                read.body.get(member),
                summary.get(member)
            );
        }
    }

    // The neighbour's scene is the neighbour's: each listing reports its own
    // workspace and nothing else.
    let neighbour = emulator
        .list_scenes(OTHER_WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        page_ids("neighbour", &neighbour),
        vec!["neighbourScene".to_string()],
        "case \"neighbour\": a listing reports only the scenes ITS workspace holds, got \
         {neighbour:?}"
    );

    // A workspace this service does not hold holds no scene to list, and this
    // operation models no `ResourceNotFoundException`, so the refusal is the
    // `ValidationException` it does model.
    for case in ["absentWorkspace", "neverCreated0"] {
        let refused = emulator.list_scenes(case, json!({})).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: listing an absent workspace must be refused, got {refused:?}"
        );
    }
}

/// RULE: a page sequence partitions the listing — every scene the workspace
/// holds is served exactly once, in `sceneId` order, no page is larger than the
/// page size, and only the last page carries no `nextToken`.
///
/// The rows span how many scenes the workspace holds (none, one, several, and
/// more than one page at every size under test, including past the default)
/// crossed with the page size (the default, one, several, and the top of the
/// published range).
#[tokio::test]
async fn page_sequences_partition_the_listing_at_every_page_size() {
    let totals = [0usize, 1, 2, 7, DEFAULT_PAGE_SIZE + 3];
    let page_sizes: [Option<u32>; 6] = [
        None,
        Some(1),
        Some(2),
        Some(5),
        Some(DEFAULT_PAGE_SIZE as u32),
        Some(MAX_PAGE_SIZE as u32),
    ];

    for total in totals {
        const WORKSPACE: &str = "pagedScenesWorkspace";
        let emulator = emulator_with_workspaces(&[WORKSPACE]).await;
        let mut expected: Vec<String> = (0..total).map(|i| format!("scene_{i:03}")).collect();
        for scene_id in &expected {
            create_scene(&emulator, WORKSPACE, scene_id, json!({})).await;
        }
        expected.sort();

        for max_results in page_sizes {
            let case = format!("total={total} maxResults={max_results:?}");
            let (ids, pages) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
            assert_eq!(
                ids, expected,
                "case {case:?}: the pages must serve every scene exactly once in id order, got \
                 {ids:?}"
            );
            let size = effective_page_size(max_results);
            let expected_pages = total.div_ceil(size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages at page size {size}, got {pages}"
            );
            // Served exactly once is also "no duplicate across the walk".
            assert_eq!(
                ids.iter().collect::<BTreeSet<_>>().len(),
                ids.len(),
                "case {case:?}: a scene must not be served twice, got {ids:?}"
            );
        }
    }
}

/// RULE: the page size stays inside what the model publishes for `maxResults` —
/// a request naming none is served this service's default, a value inside the
/// published range (0..200) is served as asked, the published minimum of 0 is a
/// page of no rows rather than a refusal, a value at the top of the range serves
/// the whole set, and a value past the published maximum is refused — with a
/// `nextToken` exactly when rows are left over.
///
/// The rows are the published bounds of this member (0 and 200), one past the
/// published maximum (201), values inside the range, and the request that names
/// none.
#[tokio::test]
async fn the_page_size_stays_inside_the_published_range() {
    const WORKSPACE: &str = "sizedScenesWorkspace";
    const EMPTY_WORKSPACE: &str = "emptySizedScenesWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, EMPTY_WORKSPACE]).await;

    // The published number itself, pinned as a literal so the rows below cannot
    // drift with the implementation: this member's modelled range is 0..200
    // (`smithy_query(query="shape",shape="MaxResults")`), and its documentation
    // publishes no default and no minimum of its own.
    assert_eq!(
        MAX_PAGE_SIZE, 200,
        "the published maximum page size is 200, got {MAX_PAGE_SIZE}"
    );

    // More scenes than the default page serves, so a page is truncated by the
    // page size rather than by how many scenes are held.
    let total = DEFAULT_PAGE_SIZE + 4;
    let mut all_ids: Vec<String> = (0..total).map(|i| format!("scene_{i:03}")).collect();
    for scene_id in &all_ids {
        create_scene(&emulator, WORKSPACE, scene_id, json!({})).await;
    }
    all_ids.sort();

    // (what the caller asks for, how many summaries the service serves)
    let rows: Vec<(Option<u32>, usize)> = vec![
        (None, DEFAULT_PAGE_SIZE),
        (Some(0), 0),
        (Some(1), 1),
        (Some(7), 7),
        (Some(DEFAULT_PAGE_SIZE as u32), DEFAULT_PAGE_SIZE),
        (Some(MAX_PAGE_SIZE as u32), total),
    ];

    for (max_results, expected_len) in rows {
        let case = format!("maxResults={max_results:?}");
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        let page = emulator.list_scenes(WORKSPACE, request).await;
        assert_eq!(page.status, 200, "case {case:?}: expected 200, got {page:?}");
        let ids = page_ids(&case, &page);
        assert_eq!(
            ids.as_slice(),
            &all_ids[..expected_len],
            "case {case:?}: the page must serve the first {expected_len} scenes, got {ids:?}"
        );
        let token = page.member("nextToken").as_str().map(str::to_string);
        if expected_len == total {
            assert_eq!(
                token, None,
                "case {case:?}: a page holding the whole listing carries no token, got {page:?}"
            );
            continue;
        }
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a listing with rows left over must answer a token, got {page:?}")
        });
        let resumed = emulator
            .list_scenes(WORKSPACE, json!({"maxResults": 1, "nextToken": token}))
            .await;
        assert_eq!(
            page_ids(&case, &resumed).as_slice(),
            &all_ids[expected_len..=expected_len],
            "case {case:?}: the token must resume where the page stopped, got {resumed:?}"
        );
    }

    // The published minimum on a workspace holding NOTHING: an empty page and no
    // token, because there is nothing left over to resume at.
    let empty = emulator
        .list_scenes(EMPTY_WORKSPACE, json!({"maxResults": 0}))
        .await;
    assert_eq!(
        (
            empty.status,
            summaries_of("emptyWorkspaceAtZero", &empty).len(),
            empty.member("nextToken").is_null(),
        ),
        (200, 0, true),
        "case \"emptyWorkspaceAtZero\": a workspace holding no scene lists an empty array and no \
         token, got {empty:?}"
    );

    // One past the published maximum, where the modelled range refuses it before
    // the handler runs.
    for past_bound in [201u32, 1000] {
        let refused = emulator
            .list_scenes(WORKSPACE, json!({"maxResults": past_bound}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case Some({past_bound}): a maxResults past the published range must be refused, got \
             {refused:?}"
        );
    }
}

/// RULE: a `nextToken` is only resumable as the token this listing minted for
/// this workspace — an absent or empty one starts at the beginning, and anything
/// else is refused with the modelled `ValidationException` rather than silently
/// resumed.
///
/// The rows differ along how the token is wrong: not a token at all, one mangled
/// in transit, one carrying no envelope, ones minted by this service's OTHER
/// listings, one this listing minted for a DIFFERENT workspace, and one whose
/// envelope is right but whose resume key is empty.
#[tokio::test]
async fn only_a_token_this_listing_minted_for_this_workspace_resumes_it() {
    const WORKSPACE: &str = "tokenScenesWorkspace";
    const OTHER_WORKSPACE: &str = "otherTokenScenesWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;
    for scene_id in ["scene_a", "scene_b", "scene_c"] {
        create_scene(&emulator, WORKSPACE, scene_id, json!({})).await;
        create_scene(&emulator, OTHER_WORKSPACE, scene_id, json!({})).await;
    }

    // An absent and an empty token both mean "the first page".
    let first = emulator.list_scenes(WORKSPACE, json!({"maxResults": 2})).await;
    let from_empty = emulator
        .list_scenes(WORKSPACE, json!({"maxResults": 2, "nextToken": ""}))
        .await;
    assert_eq!(
        page_ids("emptyToken", &from_empty),
        page_ids("absentToken", &first),
        "case \"emptyToken\": an empty nextToken starts the listing, got {from_empty:?}"
    );

    let ours = first
        .member("nextToken")
        .as_str()
        .expect("the first page leaves rows over, so it answers a token")
        .to_string();
    // The same listing over the OTHER workspace mints a token scoped to that
    // workspace; it must not resume this one.
    let other_workspaces_token = emulator
        .list_scenes(OTHER_WORKSPACE, json!({"maxResults": 2}))
        .await
        .member("nextToken")
        .as_str()
        .expect("the other workspace's first page leaves rows over too")
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("notATokenAtAll", "not-a-token".to_string()),
        // One of this listing's own tokens, mangled in transit so it is no
        // longer decodable at all.
        ("mangledToken", ours[..ours.len() - 1].to_string()),
        // Decodable, but carrying no envelope: a bare id, which a caller must
        // not be able to pass as a resume key.
        ("hexWithoutEnvelope", hex::encode("scene_c")),
        // The envelopes this service's other listings mint; a token from one
        // listing is not a token for another.
        (
            "entityListingToken",
            hex::encode(format!("iottwinmaker:entities:1:pump\n{WORKSPACE}")),
        ),
        (
            "syncJobListingToken",
            hex::encode(format!("iottwinmaker:syncJobs:1:SITEWISE\n{WORKSPACE}")),
        ),
        (
            "workspaceListingToken",
            hex::encode(format!("iottwinmaker:workspaces:1:{WORKSPACE}")),
        ),
        (
            "tagListingToken",
            hex::encode(format!(
                "iottwinmaker:tags:1:env\n{}",
                workspace_arn(WORKSPACE)
            )),
        ),
        // This listing's token, minted for another workspace.
        ("anotherWorkspacesToken", other_workspaces_token),
        // Our own envelope, with no resume key inside it.
        (
            "emptyResumeKey",
            hex::encode(format!("iottwinmaker:scenes:1:\n{WORKSPACE}")),
        ),
    ];

    for (case, token) in &rows {
        let refused = emulator
            .list_scenes(WORKSPACE, json!({"maxResults": 2, "nextToken": token}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token this listing did not mint for this workspace must be refused, \
             got {refused:?}"
        );
    }

    // Our own token still resumes, so what refused the rows above is the token
    // and nothing else.
    let resumed = emulator
        .list_scenes(WORKSPACE, json!({"maxResults": 2, "nextToken": ours}))
        .await;
    assert_eq!(
        page_ids("ourToken", &resumed),
        vec!["scene_c".to_string()],
        "case \"ourToken\": this listing's own token must resume it, got {resumed:?}"
    );
}

/// RULE: listing REPORTS — driving it at any page size, with any token, leaves
/// every later read and every later listing exactly as it was, and a refused
/// listing writes nothing either.
#[tokio::test]
async fn listing_scenes_changes_nothing_a_later_caller_sees() {
    const WORKSPACE: &str = "reportingScenesWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE]).await;
    let scene_ids = ["scene_one", "scene_two", "scene_three"];
    for scene_id in scene_ids {
        create_scene(&emulator, WORKSPACE, scene_id, json!({"description": scene_id})).await;
    }

    let snapshot = |resp: &Resp| resp.body.clone();
    let before_listing = snapshot(
        &emulator
            .list_scenes(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
            .await,
    );
    let mut before_reads: BTreeMap<&str, Value> = BTreeMap::new();
    for scene_id in scene_ids {
        before_reads.insert(
            scene_id,
            snapshot(&emulator.get_scene(WORKSPACE, scene_id).await),
        );
    }

    for max_results in [None, Some(1u32), Some(2), Some(MAX_PAGE_SIZE as u32)] {
        let case = format!("maxResults={max_results:?}");
        let (ids, _) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
        assert_eq!(
            ids.len(),
            scene_ids.len(),
            "case {case:?}: the walk must serve every held scene, got {ids:?}"
        );
        let again = snapshot(
            &emulator
                .list_scenes(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
                .await,
        );
        assert_eq!(
            again, before_listing,
            "case {case:?}: listing must not change what a later listing reports, got {again:?}"
        );
        for scene_id in scene_ids {
            let read = snapshot(&emulator.get_scene(WORKSPACE, scene_id).await);
            assert_eq!(
                read, before_reads[scene_id],
                "case {case:?}: listing must not change what GetScene reports for {scene_id:?}, \
                 got {read:?}"
            );
        }
    }

    // A refused listing writes nothing either: neither the absent workspace nor
    // a scene appears, and nothing held is removed.
    for request in [json!({}), json!({"nextToken": "not-a-token"})] {
        let refused = emulator.list_scenes("absentScenesWorkspace", request.clone()).await;
        assert!(
            refused.status >= 400,
            "case {request:?}: a listing of an absent workspace must be refused, got {refused:?}"
        );
    }
    let state = emulator.state();
    let state = state.lock();
    assert_eq!(
        (
            state.workspaces.len(),
            state
                .workspaces
                .values()
                .map(|workspace| workspace.scenes.len())
                .sum::<usize>(),
        ),
        (1, scene_ids.len()),
        "a refused listing must materialise no workspace and no scene, and remove none"
    );
}
