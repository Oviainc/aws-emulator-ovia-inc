// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE page-size rule this service's paginated share reads
//! share: `ListWorkloadShares`, `ListShareInvitations`, `ListProfileShares` and
//! `ListTemplateShares` all derive their page size from the single helper in
//! `crate::state`, so a page holds the `MaxResults` the request named and, when the
//! request names none, the ceiling the modelled member publishes.
//!
//! The rule pinned here, over a TABLE of page sizes crossed with every one of those four
//! operations:
//!
//! 1. A request that carries `MaxResults` gets a page of that many items while items
//!    remain, and a request that omits it gets a page of the published ceiling — the same
//!    number for all four reads, because one `@range(min: 1, max: 50)` bounds each of
//!    their `MaxResults` members.
//! 2. However the page size is reached, the pages still enumerate every stored item
//!    exactly once and only the last page carries no `NextToken`.
//!
//! Each collection holds more items than the ceiling, so the row that omits `MaxResults`
//! distinguishes the published ceiling from any other default.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The resource builders and the share-recording seeds are the ones the
//! `super::workload_shares`, `super::share_invitations`, `super::profile_shares` and
//! `super::template_shares` suites already spell; the `MaxResults` bounds those suites
//! pin for one operation each are untouched.

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, created_profile, member, minimal_create, review_template, Answer, Client,
};

/// The page size every one of these reads serves when a request names none, and the most
/// it serves at all: the maximum of the `MaxResults` member each of them publishes,
/// `@range(min: 1, max: 50)` — `smithy_query(query="operation",
/// operation="ListWorkloadShares,ListShareInvitations,ListProfileShares,ListTemplateShares")`
/// reports that same range for all four.
const PUBLISHED_MAX_RESULTS: usize = 50;

/// How many items each collection under test holds: more than the ceiling, so the page a
/// request that omits `MaxResults` gets is a FULL page and leaves items behind.
const ITEM_COUNT: usize = PUBLISHED_MAX_RESULTS + 2;

/// A principal to share with, distinct per index: "The Amazon Web Services account ID …
/// with which the workload, lens, profile, or review template is shared" (`SharedWith`
/// doc), so a 12-digit account id.
fn principal(index: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + index as u64)
}

/// One paginated read under test, with whatever its page is listed from.
enum Read {
    WorkloadShares(String),
    ShareInvitations,
    ProfileShares(String),
    TemplateShares(String),
}

impl Read {
    /// The list member this read reports its page in.
    fn page_member(&self) -> &'static str {
        match self {
            Read::WorkloadShares(_) => "WorkloadShareSummaries",
            Read::ShareInvitations => "ShareInvitationSummaries",
            Read::ProfileShares(_) => "ProfileShareSummaries",
            Read::TemplateShares(_) => "TemplateShareSummaries",
        }
    }

    /// The member that identifies one reported item, which is also what this read orders
    /// and resumes its pages by.
    fn id_member(&self) -> &'static str {
        match self {
            Read::ShareInvitations => "ShareInvitationId",
            _ => "ShareId",
        }
    }

    /// The read itself, with `query` sent as given (empty for a request naming nothing).
    async fn list(&self, client: &Client, query: &str) -> Answer {
        match self {
            Read::WorkloadShares(workload_id) => client.list_shares(workload_id, query).await,
            Read::ShareInvitations => client.list_share_invitations(query).await,
            Read::ProfileShares(profile_arn) => {
                super::profile_shares::list_shares(client, profile_arn, query).await
            }
            Read::TemplateShares(template_arn) => {
                super::template_shares::list_shares(client, template_arn, query).await
            }
        }
    }
}

/// The id a successful create answered with, checked against the modelled
/// `^[0-9a-f]{32}$` share-id pattern.
fn created_share_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the create to answer 200, got {answer:?}"
    );
    let id = member(answer, "ShareId").to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}

/// One page: the ids it reported, in the order it reported them, and the `NextToken` it
/// answered with.
async fn page_of(
    client: &Client,
    read: &Read,
    query: &str,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let answer = read.list(client, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {answer:?}"
    );
    let page = answer
        .body
        .get(read.page_member())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must report {}, got {answer:?}",
                read.page_member()
            )
        });
    let ids = page
        .iter()
        .map(|item| {
            item.get(read.id_member())
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let next_token = answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next_token)
}

/// A workload with [`ITEM_COUNT`] shares of it, each offered over HTTP to a principal of
/// its own.
async fn workload_shares(client: &Client) -> (Read, Vec<String>) {
    let workload_id = created_id(
        &client
            .create(minimal_create("page-size-share-host", "tok-page-size-host"))
            .await,
        "workload-share-host",
    );
    let mut ids = Vec::new();
    for index in 0..ITEM_COUNT {
        let created = client
            .create_share(
                &workload_id,
                json!({
                    "SharedWith": principal(index),
                    "PermissionType": "READONLY",
                    "ClientRequestToken": format!("tok-workload-share-{index}"),
                }),
            )
            .await;
        ids.push(created_share_id(&created, &format!("workload-share-{index}")));
    }
    (Read::WorkloadShares(workload_id), ids)
}

/// [`ITEM_COUNT`] invitations addressed to the caller: one workload each, shared with the
/// calling principal and still waiting to be answered. The shares are recorded straight
/// through the shared state for the reason `Client::record_workload_share` gives — a
/// share whose recipient is the caller has no HTTP path in a single-account emulator —
/// and the READ under test still crosses the HTTP boundary.
fn share_invitations(client: &Client) -> (Read, Vec<String>) {
    client.seed_workloads(ITEM_COUNT);
    let hosts: Vec<String> = {
        let handle = client.store();
        let store = handle.lock();
        store.workloads.keys().cloned().collect()
    };
    assert_eq!(
        hosts.len(),
        ITEM_COUNT,
        "case \"share-invitations\": expected {ITEM_COUNT} host workload(s), got {}",
        hosts.len()
    );
    let caller = engine::emulator_account_id();
    let ids = hosts
        .iter()
        .map(|workload_id| {
            client
                .record_workload_share(workload_id, caller, "READONLY", "PENDING")
                .1
        })
        .collect();
    (Read::ShareInvitations, ids)
}

/// A profile with [`ITEM_COUNT`] shares of it, each offered over HTTP to a principal of
/// its own.
async fn profile_shares(client: &Client) -> (Read, Vec<String>) {
    let (profile_arn, _) = created_profile(client, "page-size-profile").await;
    let mut ids = Vec::new();
    for index in 0..ITEM_COUNT {
        let created = super::profile_shares::create_share(
            client,
            &profile_arn,
            &principal(index),
            &format!("tok-profile-share-{index}"),
        )
        .await;
        ids.push(created_share_id(&created, &format!("profile-share-{index}")));
    }
    (Read::ProfileShares(profile_arn), ids)
}

/// A review template with [`ITEM_COUNT`] shares of it, each offered over HTTP to a
/// principal of its own.
async fn template_shares(client: &Client) -> (Read, Vec<String>) {
    let template_arn = review_template(client, "page-size-template", &["wellarchitected"]).await;
    let mut ids = Vec::new();
    for index in 0..ITEM_COUNT {
        let created = super::template_shares::create_share(
            client,
            &template_arn,
            &principal(index),
            &format!("tok-template-share-{index}"),
        )
        .await;
        ids.push(created_share_id(
            &created,
            &format!("template-share-{index}"),
        ));
    }
    (Read::TemplateShares(template_arn), ids)
}

/// Rules 1 and 2, over every read that shares the page-size rule crossed with a table of
/// page sizes: the published minimum, values between it and the stored count, the
/// published maximum, and the request that names no `MaxResults` at all — which must get
/// a page of the published maximum rather than every stored item or a default of its own.
#[tokio::test]
async fn a_page_holds_the_requested_max_results_or_the_published_ceiling() {
    let workload_client = Client::new();
    let invitation_client = Client::new();
    let profile_client = Client::new();
    let template_client = Client::new();

    let (workload_read, workload_ids) = workload_shares(&workload_client).await;
    let (invitation_read, invitation_ids) = share_invitations(&invitation_client);
    let (profile_read, profile_ids) = profile_shares(&profile_client).await;
    let (template_read, template_ids) = template_shares(&template_client).await;

    let reads = [
        ("ListWorkloadShares", &workload_client, workload_read, workload_ids),
        (
            "ListShareInvitations",
            &invitation_client,
            invitation_read,
            invitation_ids,
        ),
        ("ListProfileShares", &profile_client, profile_read, profile_ids),
        (
            "ListTemplateShares",
            &template_client,
            template_read,
            template_ids,
        ),
    ];

    for (operation, client, read, stored) in reads {
        let mut expected = stored.clone();
        expected.sort();
        assert_eq!(
            expected.len(),
            ITEM_COUNT,
            "case {operation:?}: expected {ITEM_COUNT} stored item(s), got {expected:?}"
        );

        for requested in [None, Some(1usize), Some(2), Some(7), Some(PUBLISHED_MAX_RESULTS)] {
            let case = format!("{operation}/max-results-{requested:?}");
            let page_size = requested.unwrap_or(PUBLISHED_MAX_RESULTS);

            let mut walked: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut first_page = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= ITEM_COUNT + 1,
                    "case {case:?}: paging must terminate, walked {walked:?}"
                );
                let query = match (requested, &next_token) {
                    (Some(size), Some(token)) => format!("MaxResults={size}&NextToken={token}"),
                    (Some(size), None) => format!("MaxResults={size}"),
                    (None, Some(token)) => format!("NextToken={token}"),
                    (None, None) => String::new(),
                };
                let (ids, token) = page_of(client, &read, &query, &case).await;
                let remaining = ITEM_COUNT - walked.len();
                // Rule 1: a page holds the requested size while items remain, and the
                // remainder on the last page.
                assert_eq!(
                    ids.len(),
                    page_size.min(remaining),
                    "case {case:?}: page {pages} must hold {} item(s), got {ids:?}",
                    page_size.min(remaining)
                );
                first_page.get_or_insert(ids.len());
                walked.extend(ids);
                // Rule 2: only the last page carries no token.
                assert_eq!(
                    token.is_some(),
                    walked.len() < ITEM_COUNT,
                    "case {case:?}: after {} of {ITEM_COUNT} item(s) the NextToken was {token:?}",
                    walked.len()
                );
                next_token = token;
                if next_token.is_none() {
                    break;
                }
            }

            assert_eq!(
                first_page,
                Some(page_size.min(ITEM_COUNT)),
                "case {case:?}: the first page must hold {} item(s), got {first_page:?}",
                page_size.min(ITEM_COUNT)
            );
            let mut sorted = walked.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                walked.len(),
                "case {case:?}: no item may be reported twice, got {walked:?}"
            );
            assert_eq!(
                sorted, expected,
                "case {case:?}: paging must enumerate every stored item, got {walked:?}"
            );
        }
    }
}
