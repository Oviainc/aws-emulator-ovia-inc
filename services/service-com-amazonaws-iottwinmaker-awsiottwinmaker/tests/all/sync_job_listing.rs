// SPDX-License-Identifier: CC0-1.0

//! `ListSyncJobs` end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the `workspaceId` label,
//! the status code and the serialized body are all exercised, not just a handler
//! body.
//!
//! The suites assert RULES over tables of inputs rather than one recorded case:
//!
//! * a listing reports every sync job the named workspace HOLDS, in `syncSource`
//!   order, each summary saying about a job exactly what `GetSyncJob` says about
//!   it — and nothing about a job another workspace holds;
//! * a page sequence partitions the listing exactly once, whatever the page size
//!   and however many jobs the workspace holds;
//! * the page size is the one THIS operation documents (a default of 50 and a
//!   minimum of 0, where the other listings of this service document 25 and 1),
//!   bounded by what the service serves, with a `nextToken` exactly when rows
//!   are left over;
//! * only a token this listing minted for this workspace resumes it;
//! * listing REPORTS: beyond the lifecycle transition the clock had already made
//!   due, it changes nothing a later caller sees.
//!
//! Several of the suites need a workspace holding MORE than one sync job. The
//! create path cannot build one — a sync job is identified inside its workspace
//! by its source, and the only source the service supports is `SITEWISE` — so
//! those rows are seeded straight into the store the handlers read (the seam
//! `harness::Emulator::state` exists for), while the rule under test is about
//! the jobs the workspace holds, whatever wrote them.

use std::collections::{BTreeMap, BTreeSet};
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::list_sync_jobs::DEFAULT_PAGE_SIZE;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    ResourceRef, SyncJob, SYNC_JOB_ERROR_CODE, SYNC_JOB_ERROR_MESSAGE, SYNC_JOB_SETTLE_SPAN,
    SYNC_JOB_STATE_CREATING, SYNC_JOB_STATE_ERROR, SYNC_SOURCE_SITEWISE, MAX_PAGE_SIZE,
};

use crate::harness::{sync_job_arn, workspace_arn, Emulator, Resp};

/// The role every seeded job carries; the listing does not report it, but a
/// stored job always has one.
const SEEDED_ROLE: &str = "arn:aws:iam::000000000000:role/TwinMakerSyncRole";

/// Block until the outcome an accepted job owes has fallen due, so a case that
/// compares a listing with a single read is comparing a job the clock can no
/// longer move.
fn wait_until_settled() {
    sleep(SYNC_JOB_SETTLE_SPAN + Duration::from_millis(100));
}

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

/// Store one sync job for `sync_source` in `workspace_id`, written into the very
/// store the handlers read, exactly as `CreateSyncJob` writes one: the job
/// record under its source, and its ARN in the service's index.
///
/// Used for the sources the create path refuses (it supports one), so a suite
/// can build the multi-job workspace the pagination rules are about.
fn seed_sync_job(emulator: &Emulator, workspace_id: &str, sync_source: &str) {
    let arn = sync_job_arn(workspace_id, sync_source);
    let state = emulator.state();
    let mut state = state.lock();
    let workspace = state
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the workspace {workspace_id:?} must exist before it is seeded"));
    workspace.sync_jobs.insert(
        sync_source.to_string(),
        SyncJob::accepted(
            sync_source.to_string(),
            arn.clone(),
            workspace_id.to_string(),
            SEEDED_ROLE.to_string(),
            BTreeMap::new(),
            SystemTime::now(),
        ),
    );
    state.arn_index.insert(
        arn,
        ResourceRef::SyncJob {
            workspace_id: workspace_id.to_string(),
            sync_source: sync_source.to_string(),
        },
    );
}

/// The `syncJobSummaries` of a response: a present array, never an absent
/// member, even for a workspace holding no sync job.
fn summaries_of(case: &str, resp: &Resp) -> Vec<Value> {
    resp.member("syncJobSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: syncJobSummaries must be a present array, got {resp:?}")
        })
}

/// The `syncSource`s of a page, in the order the page served them.
fn page_sources(case: &str, resp: &Resp) -> Vec<String> {
    summaries_of(case, resp)
        .iter()
        .map(|summary| {
            summary["syncSource"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries a syncSource, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// The page size the service serves for a requested `maxResults`: this
/// operation's documented default when the caller names none, and never more
/// than the service's own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Walk every page of `ListSyncJobs` for `workspace_id` at page size
/// `max_results`, returning the sources in the order they were served (so a
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
    let mut sources = Vec::new();
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
        let resp = emulator.list_sync_jobs(workspace_id, request).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: page {pages} expected 200, got {resp:?}"
        );
        pages += 1;
        let page = page_sources(case, &resp);
        assert!(
            page.len() <= effective_page_size(max_results),
            "case {case:?}: page {pages} served {} rows, past the page size {}",
            page.len(),
            effective_page_size(max_results)
        );
        sources.extend(page);
        next_token = resp.member("nextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return (sources, pages);
        }
        assert!(
            pages < 128,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
}

/// RULE: a listing reports every sync job the named workspace holds, ordered by
/// `syncSource`, each summary carrying for that job the same `arn`,
/// `workspaceId`, `syncSource`, `status` and timestamps the single-resource read
/// reports — and nothing about a job another workspace holds.
///
/// The rows differ along what the source looks like (the supported source, which
/// is the one the create path writes, plus upper-case, lower-case, digit-leading
/// and underscore-carrying sources the modelled `SyncSource` pattern
/// `^[a-zA-Z_0-9]+$` admits) and are stored in an order that is NOT source
/// order, so the ordering assertion cannot pass by accident.
#[tokio::test]
async fn a_listing_reports_every_held_sync_job_as_the_single_read_does() {
    const WORKSPACE: &str = "listedWorkspace";
    const OTHER_WORKSPACE: &str = "neighbourWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;

    // Stored in this scrambled order; the listing must still serve them sorted.
    let seeded = ["ZETA", "a_lower", "0numeric", "MID_9"];
    for sync_source in seeded {
        seed_sync_job(&emulator, WORKSPACE, sync_source);
    }
    // The one source the create path accepts goes through the real create, so
    // the listing is proved over a job `CreateSyncJob` wrote and not only over
    // seeded ones.
    assert_eq!(
        emulator
            .create_sync_job(WORKSPACE, SYNC_SOURCE_SITEWISE, json!({"syncRole": SEEDED_ROLE}))
            .await
            .status,
        200,
        "the created sync job the listing must report"
    );
    // A job in a different workspace: the listing of one workspace must not see
    // it.
    seed_sync_job(&emulator, OTHER_WORKSPACE, "NEIGHBOUR");

    let mut expected: Vec<String> = seeded.iter().map(|s| s.to_string()).collect();
    expected.push(SYNC_SOURCE_SITEWISE.to_string());
    expected.sort();

    // Past the settle span, so the listing and the reads below are of jobs the
    // clock can no longer move between the two calls.
    wait_until_settled();

    let case = "allHeld";
    let listed = emulator
        .list_sync_jobs(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListSyncJobs, got {listed:?}"
    );
    assert_eq!(
        page_sources(case, &listed),
        expected,
        "case {case:?}: the listing must report every held job in syncSource order, got {listed:?}"
    );

    // Every summary says about a job what the single-resource read says.
    for summary in summaries_of(case, &listed) {
        let sync_source = summary["syncSource"].as_str().unwrap_or_default();
        let read = emulator.get_sync_job(Some(WORKSPACE), sync_source).await;
        assert_eq!(
            read.status, 200,
            "case {sync_source:?}: expected 200 from GetSyncJob, got {read:?}"
        );
        assert_eq!(
            summary["arn"],
            json!(sync_job_arn(WORKSPACE, sync_source)),
            "case {sync_source:?}: the summary must carry the job's ARN, got {summary:?}"
        );
        for member in [
            "arn",
            "workspaceId",
            "syncSource",
            "status",
            "creationDateTime",
            "updateDateTime",
        ] {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "case {sync_source:?}: the summary's {member} must be what GetSyncJob reports \
                 ({:?}), got {:?}",
                read.body.get(member),
                summary.get(member)
            );
        }
    }

    // The neighbour's job is the neighbour's: each listing reports its own
    // workspace and nothing else.
    let neighbour = emulator
        .list_sync_jobs(OTHER_WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        page_sources("neighbour", &neighbour),
        vec!["NEIGHBOUR".to_string()],
        "case \"neighbour\": a listing reports only the jobs ITS workspace holds, got {neighbour:?}"
    );

    // A workspace this service does not hold holds no job to list, and this
    // operation models no `ResourceNotFoundException`, so the refusal is the
    // `ValidationException` it does model.
    for case in ["absentWorkspace", "neverCreated0"] {
        let refused = emulator.list_sync_jobs(case, json!({})).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: listing an absent workspace must be refused, got {refused:?}"
        );
    }
}

/// RULE: a page sequence partitions the listing — every job the workspace holds
/// is served exactly once, in `syncSource` order, no page is larger than the
/// page size, and only the last page carries no `nextToken`.
///
/// The rows span how many jobs the workspace holds (none, one, several, and more
/// than one page at every size under test, including past this operation's
/// documented default) crossed with the page size (the documented default, one,
/// several, and the top of the published range).
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
        const WORKSPACE: &str = "pagedWorkspace";
        let emulator = emulator_with_workspaces(&[WORKSPACE]).await;
        let mut expected: Vec<String> = (0..total).map(|i| format!("SRC_{i:03}")).collect();
        for sync_source in &expected {
            seed_sync_job(&emulator, WORKSPACE, sync_source);
        }
        expected.sort();

        for max_results in page_sizes {
            let case = format!("total={total} maxResults={max_results:?}");
            let (sources, pages) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
            assert_eq!(
                sources, expected,
                "case {case:?}: the pages must serve every job exactly once in source order, got \
                 {sources:?}"
            );
            let size = effective_page_size(max_results);
            let expected_pages = total.div_ceil(size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages at page size {size}, got {pages}"
            );
            // Served exactly once is also "no duplicate across the walk".
            assert_eq!(
                sources.iter().collect::<BTreeSet<_>>().len(),
                sources.len(),
                "case {case:?}: a job must not be served twice, got {sources:?}"
            );
        }
    }
}

/// RULE: the page size is the one THIS operation documents and bounds — a
/// request naming no `maxResults` is served the documented default of 50, a
/// value inside the published range is served as asked, the documented minimum
/// of 0 is a page of no rows rather than a refusal, a value at the top of the
/// range serves the whole set, and a value past the published maximum is refused
/// — with a `nextToken` exactly when rows are left over.
///
/// The rows are the published bounds of this member (0 and 200), one past the
/// published maximum (201), values inside the range, and the request that names
/// none.
#[tokio::test]
async fn the_page_size_is_this_operations_documented_one_and_is_bounded() {
    const WORKSPACE: &str = "sizedWorkspace";
    const EMPTY_WORKSPACE: &str = "emptySizedWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, EMPTY_WORKSPACE]).await;

    // The documented numbers themselves, pinned as literals so the rows below
    // cannot drift with the implementation: this operation's `maxResults`
    // documents "The default is 50 … Minimum value of 0. Maximum value of 200."
    assert_eq!(
        DEFAULT_PAGE_SIZE, 50,
        "this operation's documented default page size is 50, got {DEFAULT_PAGE_SIZE}"
    );
    assert_eq!(
        MAX_PAGE_SIZE, 200,
        "the documented maximum page size is 200, got {MAX_PAGE_SIZE}"
    );

    // More jobs than the default page serves, so a page is truncated by the page
    // size rather than by how many jobs are held.
    let total = DEFAULT_PAGE_SIZE + 4;
    let mut all_sources: Vec<String> = (0..total).map(|i| format!("SRC_{i:03}")).collect();
    for sync_source in &all_sources {
        seed_sync_job(&emulator, WORKSPACE, sync_source);
    }
    all_sources.sort();

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
        let page = emulator.list_sync_jobs(WORKSPACE, request).await;
        assert_eq!(page.status, 200, "case {case:?}: expected 200, got {page:?}");
        let sources = page_sources(&case, &page);
        assert_eq!(
            sources.as_slice(),
            &all_sources[..expected_len],
            "case {case:?}: the page must serve the first {expected_len} jobs, got {sources:?}"
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
            .list_sync_jobs(WORKSPACE, json!({"maxResults": 1, "nextToken": token}))
            .await;
        assert_eq!(
            page_sources(&case, &resumed).as_slice(),
            &all_sources[expected_len..=expected_len],
            "case {case:?}: the token must resume where the page stopped, got {resumed:?}"
        );
    }

    // The documented minimum on a workspace holding NOTHING: an empty page and
    // no token, because there is nothing left over to resume at.
    let empty = emulator
        .list_sync_jobs(EMPTY_WORKSPACE, json!({"maxResults": 0}))
        .await;
    assert_eq!(
        (
            empty.status,
            summaries_of("emptyWorkspaceAtZero", &empty).len(),
            empty.member("nextToken").is_null(),
        ),
        (200, 0, true),
        "case \"emptyWorkspaceAtZero\": a workspace holding no job lists an empty array and no \
         token, got {empty:?}"
    );

    // One past the published maximum, where the modelled range refuses it
    // before the handler runs.
    for past_bound in [201u32, 1000] {
        let refused = emulator
            .list_sync_jobs(WORKSPACE, json!({"maxResults": past_bound}))
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
    const WORKSPACE: &str = "tokenWorkspace";
    const OTHER_WORKSPACE: &str = "otherTokenWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;
    for sync_source in ["SRC_A", "SRC_B", "SRC_C"] {
        seed_sync_job(&emulator, WORKSPACE, sync_source);
        seed_sync_job(&emulator, OTHER_WORKSPACE, sync_source);
    }

    // An absent and an empty token both mean "the first page".
    let first = emulator
        .list_sync_jobs(WORKSPACE, json!({"maxResults": 2}))
        .await;
    let from_empty = emulator
        .list_sync_jobs(WORKSPACE, json!({"maxResults": 2, "nextToken": ""}))
        .await;
    assert_eq!(
        page_sources("emptyToken", &from_empty),
        page_sources("absentToken", &first),
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
        .list_sync_jobs(OTHER_WORKSPACE, json!({"maxResults": 2}))
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
        // Decodable, but carrying no envelope: a bare source, which a caller
        // must not be able to pass as a resume key.
        ("hexWithoutEnvelope", hex::encode("SRC_C")),
        // The envelopes this service's other listings mint; a token from one
        // listing is not a token for another.
        (
            "entityListingToken",
            hex::encode(format!("iottwinmaker:entities:1:pump\n{WORKSPACE}")),
        ),
        (
            "workspaceListingToken",
            hex::encode("iottwinmaker:workspaces:1:tokenWorkspace"),
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
            hex::encode(format!("iottwinmaker:syncJobs:1:\n{WORKSPACE}")),
        ),
    ];

    for (case, token) in &rows {
        let refused = emulator
            .list_sync_jobs(WORKSPACE, json!({"maxResults": 2, "nextToken": token}))
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
        .list_sync_jobs(WORKSPACE, json!({"maxResults": 2, "nextToken": ours}))
        .await;
    assert_eq!(
        page_sources("ourToken", &resumed),
        vec!["SRC_C".to_string()],
        "case \"ourToken\": this listing's own token must resume it, got {resumed:?}"
    );
}

/// RULE: the state a listing reports for a job is the state the CLOCK owes it —
/// the non-terminal one until the outcome falls due, then the modelled failure
/// state carrying the reason — and it is the same state `GetSyncJob` reports for
/// that job; beyond materialising that owed transition, listing changes nothing
/// a later caller sees.
///
/// The rows differ in whether the workspace was LISTED during the settle span,
/// because the transition is the clock's and not a read's, and in the page size
/// the listing is driven at.
#[tokio::test]
async fn a_listing_reports_the_state_the_clock_owes_and_changes_nothing_else() {
    const LISTED_EARLY: &str = "polledListWorkspace";
    const UNLISTED: &str = "unpolledListWorkspace";
    let emulator = emulator_with_workspaces(&[LISTED_EARLY, UNLISTED]).await;
    for workspace_id in [LISTED_EARLY, UNLISTED] {
        assert_eq!(
            emulator
                .create_sync_job(
                    workspace_id,
                    SYNC_SOURCE_SITEWISE,
                    json!({"syncRole": SEEDED_ROLE}),
                )
                .await
                .status,
            200,
            "the sync job {workspace_id:?} must hold"
        );
        seed_sync_job(&emulator, workspace_id, "SECOND_SRC");
    }

    // Inside the settle span: the listing reports the non-terminal state the
    // create answered with, for both the created and the seeded job.
    let during = emulator.list_sync_jobs(LISTED_EARLY, json!({})).await;
    for summary in summaries_of("duringSettleSpan", &during) {
        assert_eq!(
            summary["status"]["state"].as_str(),
            Some(SYNC_JOB_STATE_CREATING),
            "case \"duringSettleSpan\": a listing inside the settle span reports the non-terminal \
             state, got {summary:?}"
        );
    }

    wait_until_settled();

    // Past it: both workspaces report the outcome the clock owed, whether or not
    // anybody listed them while it was pending.
    for workspace_id in [LISTED_EARLY, UNLISTED] {
        let settled = emulator
            .list_sync_jobs(workspace_id, json!({"maxResults": MAX_PAGE_SIZE}))
            .await;
        let summaries = summaries_of(workspace_id, &settled);
        assert_eq!(
            summaries.len(),
            2,
            "case {workspace_id:?}: both held jobs must be listed, got {settled:?}"
        );
        for summary in &summaries {
            assert_eq!(
                (
                    summary["status"]["state"].as_str(),
                    summary["status"]["error"]["code"].as_str(),
                    summary["status"]["error"]["message"].as_str(),
                ),
                (
                    Some(SYNC_JOB_STATE_ERROR),
                    Some(SYNC_JOB_ERROR_CODE),
                    Some(SYNC_JOB_ERROR_MESSAGE),
                ),
                "case {workspace_id:?}: the outcome has fallen due, and a failed job says why, \
                 got {summary:?}"
            );
            // The listing and the single read agree about the settled job.
            let sync_source = summary["syncSource"].as_str().unwrap_or_default();
            let read = emulator.get_sync_job(Some(workspace_id), sync_source).await;
            assert_eq!(
                summary["status"],
                read.body["status"],
                "case {sync_source:?}: the listing and GetSyncJob must report one job's status \
                 identically, got {summary:?} and {read:?}"
            );
        }
    }

    // Listing REPORTS: once the owed transition has been materialised, driving
    // the listing at any page size leaves every later read byte-for-byte as it
    // was.
    let snapshot = |resp: &Resp| resp.body.clone();
    let before_listing = snapshot(
        &emulator
            .list_sync_jobs(LISTED_EARLY, json!({"maxResults": MAX_PAGE_SIZE}))
            .await,
    );
    let mut before_reads: BTreeMap<&str, Value> = BTreeMap::new();
    for sync_source in [SYNC_SOURCE_SITEWISE, "SECOND_SRC"] {
        before_reads.insert(
            sync_source,
            snapshot(&emulator.get_sync_job(Some(LISTED_EARLY), sync_source).await),
        );
    }

    for max_results in [None, Some(1u32), Some(2), Some(MAX_PAGE_SIZE as u32)] {
        let case = format!("maxResults={max_results:?}");
        let (sources, _) = walk_pages(&emulator, &case, LISTED_EARLY, max_results).await;
        assert_eq!(
            sources.len(),
            2,
            "case {case:?}: the walk must serve every held job, got {sources:?}"
        );
        let again = snapshot(
            &emulator
                .list_sync_jobs(LISTED_EARLY, json!({"maxResults": MAX_PAGE_SIZE}))
                .await,
        );
        assert_eq!(
            again, before_listing,
            "case {case:?}: listing must not change what a later listing reports, got {again:?}"
        );
        for sync_source in [SYNC_SOURCE_SITEWISE, "SECOND_SRC"] {
            let read = snapshot(&emulator.get_sync_job(Some(LISTED_EARLY), sync_source).await);
            assert_eq!(
                read, before_reads[sync_source],
                "case {case:?}: listing must not change what GetSyncJob reports for \
                 {sync_source:?}, got {read:?}"
            );
        }
    }

    // A refused listing writes nothing either.
    let refused = emulator.list_sync_jobs("absentWorkspace", json!({})).await;
    assert!(
        refused.status >= 400,
        "a listing of an absent workspace must be refused, got {refused:?}"
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .workspaces
            .values()
            .map(|workspace| workspace.sync_jobs.len())
            .sum::<usize>(),
        4,
        "a refused listing must materialise no sync job and remove none"
    );
}
