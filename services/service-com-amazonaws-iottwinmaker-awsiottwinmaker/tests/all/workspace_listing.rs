// SPDX-License-Identifier: CC0-1.0

//! `ListWorkspaces` end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the status code and the
//! serialized body are all exercised, not just a handler body.
//!
//! The tests assert RULES over tables of inputs rather than one recorded case:
//!
//! * a listing reports every STORED workspace of the account, in `workspaceId`
//!   order, each summary carrying the members `GetWorkspace` reports for that
//!   same workspace — so a create is visible in the listing and a delete is not;
//! * a page sequence partitions the listing exactly once, whatever the page size
//!   and however many workspaces are stored;
//! * the page size is the documented one and is bounded, and a `maxResults` or a
//!   `nextToken` outside what the service accepts is refused with the modelled
//!   `ValidationException`;
//! * listing is a READ: it changes nothing a later caller sees.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MAX_WORKSPACES,
};

use crate::harness::{workspace_arn, Emulator, Resp};

/// The `workspaceSummaries` of a `ListWorkspaces` response: a present array,
/// never an absent member, even for an account holding no workspaces.
fn summaries_of(case: &str, resp: &Resp) -> Vec<Value> {
    resp.member("workspaceSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: workspaceSummaries must be a present array, got {resp:?}")
        })
}

/// The `workspaceId`s of a page, in the order the page served them.
fn page_ids(case: &str, resp: &Resp) -> Vec<String> {
    summaries_of(case, resp)
        .iter()
        .map(|summary| {
            summary["workspaceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries a workspaceId, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// The page size the service serves for a requested `maxResults`: the documented
/// default of 25 when the caller names none, and never more than the service's
/// own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Walk every page of `ListWorkspaces` at page size `max_results`, returning the
/// ids in the order they were served (so a duplicate is visible) and the number
/// of pages.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
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
        let resp = emulator.list_workspaces(request).await;
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
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
}

/// Create `workspace_id` with `body`, asserting the create succeeded.
async fn create(emulator: &Emulator, workspace_id: &str, body: Value) {
    let created = emulator
        .call("POST", &format!("/workspaces/{workspace_id}"), Some(body))
        .await;
    assert_eq!(
        created.status, 200,
        "creating {workspace_id:?} must succeed, got {created:?}"
    );
}

/// RULE: a listing reports every stored workspace of the account, ordered by
/// `workspaceId`, each summary carrying the same `arn`, `description` and
/// timestamps the single-resource read reports for that workspace — and nothing
/// for a workspace that is not stored.
///
/// The rows differ along what the create persisted (no description, the empty
/// description, a plain one, a multi-byte one, an `s3Location` and a `role` the
/// summary does not carry) and are created in an order that is NOT id order, so
/// the ordering assertion cannot pass by accident.
#[tokio::test]
async fn a_listing_reports_every_stored_workspace_as_the_single_read_does() {
    let emulator = Emulator::new();

    // (workspaceId, the create's body) — created in this scrambled order.
    let rows: Vec<(&str, Value)> = vec![
        ("mid_plant", json!({"description": "the plant floor"})),
        ("zz_last", json!({})),
        ("aa_first", json!({"description": ""})),
        (
            "with_role",
            json!({
                "description": "Kühlpumpe — 冷却ポンプ",
                "s3Location": "arn:aws:s3:::twinmaker-bucket",
                "role": "arn:aws:iam::000000000000:role/TwinMakerRole",
            }),
        ),
        ("n0_middle", json!({"description": "x"})),
    ];
    for (workspace_id, body) in &rows {
        create(&emulator, workspace_id, body.clone()).await;
    }

    let mut expected_ids: Vec<String> = rows
        .iter()
        .map(|(workspace_id, _)| workspace_id.to_string())
        .collect();
    expected_ids.sort();

    let case = "allStored";
    let listed = emulator
        .list_workspaces(json!({"maxResults": MAX_WORKSPACES}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListWorkspaces, got {listed:?}"
    );
    assert_eq!(
        page_ids(case, &listed),
        expected_ids,
        "case {case:?}: the listing must report every stored workspace in workspaceId order, got \
         {listed:?}"
    );

    // Every summary says about a workspace what the single-resource read says.
    for summary in summaries_of(case, &listed) {
        let workspace_id = summary["workspaceId"].as_str().unwrap_or_default();
        let read = emulator.get_workspace(workspace_id).await;
        assert_eq!(
            read.status, 200,
            "case {workspace_id:?}: expected 200 from GetWorkspace, got {read:?}"
        );
        assert_eq!(
            summary["arn"],
            json!(workspace_arn(workspace_id)),
            "case {workspace_id:?}: the summary must carry the minted ARN, got {summary:?}"
        );
        for member in ["arn", "description", "creationDateTime", "updateDateTime"] {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "case {workspace_id:?}: the summary's {member} must be what GetWorkspace reports \
                 ({:?}), got {:?}",
                read.body.get(member),
                summary.get(member)
            );
        }
        // Nothing has linked a service into any workspace, so the optional
        // member is omitted rather than served as an empty list — the same
        // choice `GetWorkspace` makes.
        assert!(
            summary.get("linkedServices").is_none_or(Value::is_null),
            "case {workspace_id:?}: linkedServices must be omitted while nothing is linked, got \
             {summary:?}"
        );
    }

    // A workspace that is no longer stored is no longer listed, and the rest of
    // the listing is untouched.
    assert_eq!(
        emulator.delete_workspace("mid_plant").await.status,
        200,
        "deleting the workspace under test must succeed"
    );
    expected_ids.retain(|workspace_id| workspace_id != "mid_plant");
    let after_delete = emulator
        .list_workspaces(json!({"maxResults": MAX_WORKSPACES}))
        .await;
    assert_eq!(
        page_ids("afterDelete", &after_delete),
        expected_ids,
        "case \"afterDelete\": a deleted workspace must not be listed, got {after_delete:?}"
    );
}

/// RULE: a page sequence partitions the listing — every stored workspace is
/// served exactly once, in `workspaceId` order, no page is larger than the page
/// size, and only the last page carries no `nextToken`.
///
/// The rows span how many workspaces are stored (none, one, several, more than
/// one page at every size under test) crossed with the page size (the documented
/// default, one, several, the whole set, and more than the account can hold).
#[tokio::test]
async fn page_sequences_partition_the_listing_at_every_page_size() {
    // Fewer than the emulator's own ceiling, and more than the documented
    // default page size, so the default is exercised against a truncated page.
    let totals = [0usize, 1, 2, 7, DEFAULT_PAGE_SIZE + 5];
    let page_sizes: [Option<u32>; 6] = [
        None,
        Some(1),
        Some(2),
        Some(5),
        Some(DEFAULT_PAGE_SIZE as u32),
        Some(MAX_WORKSPACES as u32),
    ];

    for total in totals {
        assert!(
            total <= MAX_WORKSPACES,
            "the table must stay inside the emulator's ceiling of {MAX_WORKSPACES} workspaces"
        );
        let emulator = Emulator::new();
        let mut expected_ids: Vec<String> = (0..total).map(|i| format!("ws{i:03}")).collect();
        for workspace_id in &expected_ids {
            create(&emulator, workspace_id, json!({})).await;
        }
        expected_ids.sort();

        for max_results in page_sizes {
            let case = format!("total={total} maxResults={max_results:?}");
            let (ids, pages) = walk_pages(&emulator, &case, max_results).await;
            assert_eq!(
                ids, expected_ids,
                "case {case:?}: the pages must serve every workspace exactly once in id order, \
                 got {ids:?}"
            );
            let size = effective_page_size(max_results);
            let expected_pages = total.div_ceil(size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages at page size {size}, got {pages}"
            );
        }
    }
}

/// RULE: the page size is the one the service documents and bounds — a request
/// naming no `maxResults` is served the documented default of 25, a value inside
/// the published range is served as asked, a value above what the service serves
/// is capped, and a value outside the member's published range is refused — with
/// a `nextToken` exactly when rows are left over.
///
/// The rows are the published bounds of `maxResults` (1 and 200), one below the
/// documented minimum (0), one past the published maximum (201), values inside
/// the range, and the request that names none.
#[tokio::test]
async fn the_page_size_is_the_documented_one_and_is_bounded() {
    let emulator = Emulator::new();

    // The documented numbers themselves, pinned as literals so the rows below
    // cannot drift with the implementation: the `maxResults` member documents
    // "The default is 25" and the model's own range tops out at 200.
    assert_eq!(
        DEFAULT_PAGE_SIZE, 25,
        "the documented default page size is 25, got {DEFAULT_PAGE_SIZE}"
    );
    assert_eq!(
        MAX_PAGE_SIZE, 200,
        "the documented maximum page size is 200, got {MAX_PAGE_SIZE}"
    );

    // More workspaces than the default page serves, so a page is truncated by
    // the page size rather than by the stored count. The account cannot hold
    // `MAX_PAGE_SIZE` workspaces (the emulator's ceiling is lower), so the cap
    // at `MAX_PAGE_SIZE` shows up as "a page size at or above the top of the
    // range serves the whole set".
    let total = DEFAULT_PAGE_SIZE + 5;
    let mut all_ids: Vec<String> = (0..total).map(|i| format!("ws{i:03}")).collect();
    for workspace_id in &all_ids {
        create(&emulator, workspace_id, json!({})).await;
    }
    all_ids.sort();

    // (what the caller asks for, how many summaries the service serves)
    let rows: Vec<(Option<u32>, usize)> = vec![
        (None, DEFAULT_PAGE_SIZE),
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
        let page = emulator.list_workspaces(request).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200, got {page:?}"
        );
        let ids = page_ids(&case, &page);
        assert_eq!(
            ids.as_slice(),
            &all_ids[..expected_len],
            "case {case:?}: the page must serve the first {expected_len} workspaces, got {ids:?}"
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
            .list_workspaces(json!({"maxResults": 1, "nextToken": token}))
            .await;
        assert_eq!(
            page_ids(&case, &resumed).as_slice(),
            &all_ids[expected_len..=expected_len],
            "case {case:?}: the token must resume where the page stopped, got {resumed:?}"
        );
    }

    // Below the documented minimum of 1, and one past the published top of the
    // range (where the modelled constraint refuses it before the handler runs).
    for past_bound in [0u32, 201] {
        let refused = emulator
            .list_workspaces(json!({"maxResults": past_bound}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case Some({past_bound}): a maxResults outside the documented range must be refused, \
             got {refused:?}"
        );
    }
}

/// RULE: a `nextToken` is only resumable as the token this listing minted — an
/// absent or empty one starts at the beginning, and anything else is refused
/// with the modelled `ValidationException` rather than silently resumed.
///
/// The rows differ along how the token is wrong: not a token at all, one mangled
/// in transit, one carrying no envelope, ones minted by this service's OTHER
/// listings, and one whose envelope is right but whose resume key is empty.
#[tokio::test]
async fn only_a_token_this_listing_minted_resumes_it() {
    let emulator = Emulator::new();
    for workspace_id in ["ws_a", "ws_b", "ws_c"] {
        create(&emulator, workspace_id, json!({})).await;
    }

    // An absent and an empty token both mean "the first page".
    let first = emulator.list_workspaces(json!({"maxResults": 2})).await;
    let from_empty = emulator
        .list_workspaces(json!({"maxResults": 2, "nextToken": ""}))
        .await;
    assert_eq!(
        page_ids("emptyToken", &from_empty),
        page_ids("absentToken", &first),
        "case \"emptyToken\": an empty nextToken starts the listing, got {from_empty:?}"
    );

    let rows: Vec<(&str, String)> = vec![
        ("notATokenAtAll", "not-a-token".to_string()),
        // One of this listing's own tokens, mangled in transit so it is no
        // longer decodable at all.
        ("mangledToken", {
            let token = first
                .member("nextToken")
                .as_str()
                .expect("the first page leaves rows over, so it answers a token")
                .to_string();
            token[..token.len() - 1].to_string()
        }),
        // Decodable, but carrying no envelope: a bare workspace id, which a
        // caller must not be able to pass as a resume key.
        ("hexWithoutEnvelope", hex::encode("ws_a")),
        // The envelopes this service's other listings mint; a token from one
        // listing is not a token for another.
        (
            "entityListingToken",
            hex::encode("iottwinmaker:entities:1:pump\nws_a"),
        ),
        (
            "tagListingToken",
            hex::encode(format!(
                "iottwinmaker:tags:1:env\n{}",
                workspace_arn("ws_a")
            )),
        ),
        // Our own envelope, with no resume key inside it.
        (
            "emptyResumeKey",
            hex::encode("iottwinmaker:workspaces:1:"),
        ),
    ];

    for (case, token) in &rows {
        let refused = emulator
            .list_workspaces(json!({"maxResults": 2, "nextToken": token}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token this listing did not mint must be refused, got {refused:?}"
        );
    }
}

/// RULE: listing is a READ — it changes nothing a later caller sees, whatever
/// page size or resume point it is driven at.
///
/// The rows are the page sizes a caller can drive the listing at; after every
/// one, the whole listing and each workspace's single-resource read must be
/// byte-for-byte what they were before.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let workspace_ids = ["ws_a", "ws_b", "ws_c", "ws_d"];
    for workspace_id in workspace_ids {
        create(&emulator, workspace_id, json!({"description": workspace_id})).await;
    }

    // An account holding no workspaces answers an EMPTY list and no token,
    // never an absent member — checked on its own emulator, so it is a listing
    // of an empty account rather than of a drained one.
    let empty = Emulator::new();
    let empty_listing = empty.list_workspaces(json!({})).await;
    assert_eq!(
        (
            empty_listing.status,
            summaries_of("emptyAccount", &empty_listing).len()
        ),
        (200, 0),
        "case \"emptyAccount\": an account with no workspaces lists an empty array, got \
         {empty_listing:?}"
    );
    assert!(
        empty_listing.member("nextToken").is_null(),
        "case \"emptyAccount\": an empty listing carries no token, got {empty_listing:?}"
    );

    let snapshot = |resp: &Resp| resp.body.clone();
    let before_listing = snapshot(&emulator.list_workspaces(json!({"maxResults": 50})).await);
    let mut before_reads: BTreeMap<&str, Value> = BTreeMap::new();
    for workspace_id in workspace_ids {
        before_reads.insert(
            workspace_id,
            snapshot(&emulator.get_workspace(workspace_id).await),
        );
    }

    for max_results in [None, Some(1u32), Some(2), Some(50)] {
        let case = format!("maxResults={max_results:?}");
        let (ids, _) = walk_pages(&emulator, &case, max_results).await;
        assert_eq!(
            ids.len(),
            workspace_ids.len(),
            "case {case:?}: the walk must serve every workspace, got {ids:?}"
        );

        let again = snapshot(&emulator.list_workspaces(json!({"maxResults": 50})).await);
        assert_eq!(
            again, before_listing,
            "case {case:?}: listing must not change what a later listing reports, got {again:?}"
        );
        for workspace_id in workspace_ids {
            let read = snapshot(&emulator.get_workspace(workspace_id).await);
            assert_eq!(
                read,
                before_reads[workspace_id],
                "case {case:?}: listing must not change what GetWorkspace reports for \
                 {workspace_id:?}, got {read:?}"
            );
        }
    }
}
