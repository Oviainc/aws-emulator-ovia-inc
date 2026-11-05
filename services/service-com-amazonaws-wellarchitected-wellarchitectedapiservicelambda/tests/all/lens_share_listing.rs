// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLensShares`: the read that reports the shares recorded
//! against one lens.
//!
//! The rule pinned here, over TABLES of inputs rather than one recorded request: a paged
//! list of the shares of a resource REPORTS the shares the store holds against the
//! resource its path names, in `ShareId` order, narrowed by every filter the request
//! carries and cut into pages of at most `MaxResults`, and it writes nothing. Applied to
//! the lens shares `CreateLensShare` records, that is:
//!
//! 1. Every share recorded against a lens reads back through the list, exactly once,
//!    carrying the `ShareId` the create minted, the principal it was offered to and the
//!    status the share stands in — and no share of ANOTHER lens. Rows cross several shapes
//!    of lens and all three kinds of principal `SharedWith` publishes.
//! 2. The list narrows by `SharedWithPrefix` and by `Status` over every published
//!    `ShareStatus` member, and two identical reads report the same page: a read changes
//!    nothing a later caller sees.
//! 3. At any `MaxResults` the pages enumerate every share exactly once and the last page
//!    carries no `NextToken`; a `NextToken` this service did not issue is a 400 rather than
//!    an empty page.
//! 4. Each published bound of the list's own members is served and one step outside it is a
//!    400 `ValidationException` — a page size outside the published range is refused, never
//!    quietly clamped into it.
//! 5. The lens is resolved by what the `LensAlias` NAMES, not by its spelling: an ARN and
//!    that ARN with a version pinned after it reach the same shares, a lens that exists and
//!    holds no shares reports an empty page (one Amazon Web Services publishes, which
//!    "cannot be shared", and a custom lens no version has been published from), and an
//!    alias naming no lens at all is a 404 naming the LENS that was missing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the lens document
//! and import helpers are `super::custom_lens_import`'s, and the shares this suite reads
//! are granted through `super::lens_shares`'s own `CreateLensShare` helpers — one spelling
//! of each request for both suites.
//!
//! A share in a status other than the one a new share carries is seeded straight through
//! the store, for the reason `super::profile_shares` gives: only answering the share's
//! invitation moves its status, and the invitation of a share this caller created is
//! offered to another principal, so those rows have no HTTP path in a single-account
//! emulator.

use serde_json::Value;

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::lens_shares::{
    create_share, created_share_id, publish_lenses, stored_shares, PRINCIPALS,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The `ShareStatus` members the model publishes —
/// `smithy_query(query="shape", shape="ShareStatus")`. Spelled out here rather than read
/// out of the code under test.
const SHARE_STATUSES: [&str; 8] = [
    "ACCEPTED",
    "ASSOCIATED",
    "ASSOCIATING",
    "EXPIRED",
    "FAILED",
    "PENDING",
    "REJECTED",
    "REVOKED",
];

/// The `ShareStatus` a share carries while it waits on the invitation its recipient has to
/// answer, the `PENDING` member of that same enum.
const NEW_SHARE_STATUS: &str = "PENDING";

/// `GET /lenses/{LensAlias}/shares`, the wire form of `ListLensShares`, with every filter
/// and paging member bound to the query string. The alias is percent-encoded because a
/// custom lens is named by its ARN, which carries `:` and `/` inside ONE path segment.
/// `query` is passed as sent, empty for a request that names none. `pub(crate)` because
/// `super::bare_lens_name_resolution` lists a lens's shares through this one spelling of the
/// request.
pub(crate) async fn list_shares(client: &Client, alias: &str, query: &str) -> Answer {
    let encoded = encode_path_segment(alias);
    let uri = if query.is_empty() {
        format!("/lenses/{encoded}/shares")
    } else {
        format!("/lenses/{encoded}/shares?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `LensShareSummaries` of a successful `ListLensShares`.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensShares 200, got {answer:?}"
    );
    answer
        .body
        .get("LensShareSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListLensShares must report LensShareSummaries, got {answer:?}")
        })
}

/// One member of one summary, or `""` when the summary does not carry it.
fn summary_member<'a>(summary: &'a Value, name: &str) -> &'a str {
    summary.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The `ShareId`s a page reports, in the order it reported them.
fn page_share_ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|summary| summary_member(summary, "ShareId").to_string())
        .collect()
}

/// The `NextToken` an answer carries, if it carries one.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A published lens per case, named after the case so the rows never share a lens. The
/// documents differ in shape as well as in name, because the lens is the resource the
/// shares belong to.
async fn published_lenses(client: &Client, cases: &[&'static str]) -> Vec<String> {
    let documents: Vec<(&'static str, Value)> = cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            (
                *case,
                lens_document(
                    &format!("Listed-{case}-lens"),
                    None,
                    &[("governance", 1, 2), ("security", index + 1, 2)],
                ),
            )
        })
        .collect();
    publish_lenses(client, &documents)
        .await
        .into_iter()
        .map(|(_, arn)| arn)
        .collect()
}

/// Record `status` as where the share `share_id` of the lens `alias` stands, straight
/// through the shared state (see this suite's module doc for why those rows are seeded).
fn record_share_status(client: &Client, alias: &str, share_id: &str, status: &str) {
    let state_handle = client.store();
    let mut store = state_handle.lock();
    let share = store
        .lens_shares
        .get_mut(state::lens_alias(alias))
        .and_then(|shares| shares.get_mut(share_id))
        .expect("the share whose status is seeded must be stored");
    share.status = status.to_string();
}

/// Rule 1: every share recorded against a lens reads back through the list exactly once,
/// with the members a lens share summary carries, and no share of another lens.
#[tokio::test]
async fn every_recorded_share_reads_back_through_the_list_in_share_id_order() {
    let client = Client::new();
    let lenses = published_lenses(&client, &["minimal", "described", "broad"]).await;

    // (lens case, lens ARN) -> the shares granted on it, as (principal, ShareId).
    let mut granted: Vec<(String, Vec<(&str, String)>)> = Vec::new();
    for (index, arn) in lenses.iter().enumerate() {
        let mut shares: Vec<(&str, String)> = Vec::new();
        for (principal_case, shared_with) in PRINCIPALS {
            let case = format!("lens-{index}/{principal_case}");
            let created =
                create_share(&client, arn, shared_with, &format!("tok-{index}-{principal_case}"))
                    .await;
            shares.push((shared_with, created_share_id(&created, &case)));
        }
        granted.push((arn.clone(), shares));
    }

    for (index, (arn, shares)) in granted.iter().enumerate() {
        let case = format!("lens-{index}");
        let listed = list_shares(&client, arn, "").await;
        let page = summaries(&listed, &case);
        assert_eq!(
            page.len(),
            shares.len(),
            "case {case:?}: expected {} share(s) of this lens, got {listed:?}",
            shares.len()
        );

        // The list reports the shares the store holds for THIS lens, in `ShareId` order.
        let mut expected: Vec<String> = shares
            .iter()
            .map(|(_, share_id)| share_id.clone())
            .collect();
        expected.sort();
        assert_eq!(
            page_share_ids(&page),
            expected,
            "case {case:?}: the list must report every share of the lens in ShareId order, \
             got {listed:?}"
        );
        assert_eq!(
            page_share_ids(&page),
            stored_shares(&client, arn)
                .into_iter()
                .map(|share| share.share_id)
                .collect::<Vec<String>>(),
            "case {case:?}: the list must report what the store holds, got {listed:?}"
        );

        for (shared_with, share_id) in shares {
            let summary = page
                .iter()
                .find(|summary| summary_member(summary, "ShareId") == share_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: share {share_id:?} must be reported, got {listed:?}")
                });
            assert_eq!(
                summary_member(summary, "SharedWith"),
                *shared_with,
                "case {case:?}: the summary must name the principal the share was offered \
                 to, got {summary:?}"
            );
            assert_eq!(
                summary_member(summary, "Status"),
                NEW_SHARE_STATUS,
                "case {case:?}: a new share waits on its invitation, got {summary:?}"
            );
            // `StatusMessage` is optional and "compliments the Status field": a share
            // simply waiting on its invitation has nothing to add.
            assert!(
                summary.get("StatusMessage").is_none(),
                "case {case:?}: a pending share carries no StatusMessage, got {summary:?}"
            );
        }

        // A share belongs to the lens it shares: no OTHER lens's share is reported here.
        for (other_index, (other_arn, other_shares)) in granted.iter().enumerate() {
            if other_arn == arn {
                continue;
            }
            for (_, other_id) in other_shares {
                assert!(
                    !page_share_ids(&page).contains(other_id),
                    "case {case:?}: share {other_id:?} of lens {other_index} must not be \
                     reported here, got {listed:?}"
                );
            }
        }
    }
}

/// Rule 2: the list narrows by `SharedWithPrefix` and by `Status`, and reports the same
/// thing twice in a row.
#[tokio::test]
async fn listing_narrows_by_prefix_and_status_and_never_changes_what_a_later_caller_sees() {
    let client = Client::new();
    let arn = published_lenses(&client, &["filtered"]).await.remove(0);
    for (principal_case, shared_with) in PRINCIPALS {
        let created =
            create_share(&client, &arn, shared_with, &format!("tok-filter-{principal_case}")).await;
        created_share_id(&created, principal_case);
    }

    // Rows differ along the prefix's length and along which kind of principal it selects.
    for (case, prefix, expected) in [
        ("every-principal", "", PRINCIPALS.len()),
        ("accounts-starting-1", "1", 1),
        ("accounts-starting-4", "4", 1),
        ("organizations", "o-", 1),
        ("organizational-units", "ou-", 1),
        // `o` prefixes both the organization and the OU id.
        ("everything-starting-o", "o", 2),
        ("one-exact-principal", "111122223333", 1),
        ("no-principal", "zzz", 0),
    ] {
        let listed = list_shares(&client, &arn, &format!("SharedWithPrefix={prefix}")).await;
        let found = summaries(&listed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} share(s) for prefix {prefix:?}, got {listed:?}"
        );
        for summary in &found {
            let shared_with = summary_member(summary, "SharedWith");
            assert!(
                shared_with.starts_with(prefix),
                "case {case:?}: {shared_with:?} does not start with {prefix:?}, got {listed:?}"
            );
        }
    }

    // Every share granted over the wire is PENDING; one further share is seeded in each of
    // the seven other published statuses, so every published member of the filter selects a
    // non-empty, KNOWN set and a filter hardcoded to one status could not pass.
    let mut seeded: Vec<(&str, String)> = Vec::new();
    for status in SHARE_STATUSES
        .iter()
        .filter(|status| **status != NEW_SHARE_STATUS)
    {
        let shared_with = format!("5555{:08}", seeded.len());
        let created = create_share(
            &client,
            &arn,
            &shared_with,
            &format!("tok-status-{}", seeded.len()),
        )
        .await;
        let share_id = created_share_id(&created, status);
        record_share_status(&client, &arn, &share_id, status);
        seeded.push((status, share_id));
    }

    for status in SHARE_STATUSES {
        let listed = list_shares(&client, &arn, &format!("Status={status}")).await;
        let found = summaries(&listed, status);
        if status == NEW_SHARE_STATUS {
            assert_eq!(
                found.len(),
                PRINCIPALS.len(),
                "case {status:?}: expected every share still waiting on its invitation, got \
                 {listed:?}"
            );
        } else {
            let expected = seeded
                .iter()
                .find(|(seeded_status, _)| *seeded_status == status)
                .map(|(_, share_id)| share_id.clone())
                .unwrap_or_default();
            assert_eq!(
                page_share_ids(&found),
                vec![expected],
                "case {status:?}: expected exactly the share seeded in that status, got \
                 {listed:?}"
            );
        }
        for summary in &found {
            assert_eq!(
                summary_member(summary, "Status"),
                status,
                "case {status:?}: a filtered page carries only that status, got {summary:?}"
            );
        }
    }

    // A status outside the published set is a bad request, not an empty page.
    let bogus = list_shares(&client, &arn, "Status=NOT_A_STATUS").await;
    assert_eq!(
        bogus.status, 400,
        "case \"unknown-status\": expected HTTP 400, got {bogus:?}"
    );
    assert_eq!(
        bogus.error_code, "ValidationException",
        "case \"unknown-status\": expected ValidationException, got {bogus:?}"
    );

    // Reading does not mutate: two identical reads report the same shares.
    let first = list_shares(&client, &arn, "").await;
    let again = list_shares(&client, &arn, "").await;
    assert_eq!(
        summaries(&first, "repeat-read"),
        summaries(&again, "repeat-read"),
        "case \"repeat-read\": a read must not change what the next caller sees, got \
         {first:?} then {again:?}"
    );
}

/// Rule 3: at any `MaxResults` the pages enumerate every share exactly once, and a token
/// this service never issued is refused. Rows are the published `@range(1, 50)` minimum,
/// values between it and the share count, the count itself and the published maximum.
#[tokio::test]
async fn paging_enumerates_every_share_exactly_once_at_any_page_size() {
    let client = Client::new();
    let arn = published_lenses(&client, &["paged"]).await.remove(0);

    const SHARE_COUNT: usize = 7;
    let mut all: Vec<String> = Vec::new();
    for index in 0..SHARE_COUNT {
        let created = create_share(
            &client,
            &arn,
            &format!("{:012}", 100_000_000_000u64 + index as u64),
            &format!("tok-paging-{index}"),
        )
        .await;
        all.push(created_share_id(&created, &format!("share-{index}")));
    }
    all.sort();

    for page_size in [1usize, 2, 3, SHARE_COUNT, 50] {
        let case = format!("max-results-{page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= SHARE_COUNT + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            let query = match &token {
                Some(token) => format!("MaxResults={page_size}&NextToken={token}"),
                None => format!("MaxResults={page_size}"),
            };
            let listed = list_shares(&client, &arn, &query).await;
            let page = summaries(&listed, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} share(s), got {listed:?}"
            );
            walked.extend(page_share_ids(&page));
            token = next_token(&listed);
            if token.is_none() {
                break;
            }
        }
        let mut sorted = walked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            walked.len(),
            "case {case:?}: no share may be reported twice, got {walked:?}"
        );
        assert_eq!(
            sorted, all,
            "case {case:?}: paging must enumerate every share, got {walked:?}"
        );
    }

    // A `NextToken` this service never issued is a bad request, not an empty page.
    for (case, token) in [
        ("not-hex", "notatoken"),
        ("hex-but-not-a-cursor", "6162636465"),
        ("another-lists-cursor", "7b226e223a2261222c2269223a2262227d"),
    ] {
        let refused = list_shares(&client, &arn, &format!("NextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a foreign NextToken, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }
}

/// Rule 4: `MaxResults` is `@range(min: 1, max: 50)` and `SharedWithPrefix`
/// `@length(max: 100)` (`smithy_query(query="operation", operation="ListLensShares")`), so
/// each published bound is served and one step outside it is a 400 `ValidationException`.
#[tokio::test]
async fn the_published_bounds_are_served_and_one_step_outside_them_is_refused() {
    let client = Client::new();
    let arn = published_lenses(&client, &["bounded"]).await.remove(0);
    // Two shares, so the minimum page size serves strictly fewer than all of them and the
    // bound is load-bearing rather than trivially satisfied.
    for (index, shared_with) in ["111122223333", "o-abcde12345"].iter().enumerate() {
        let created =
            create_share(&client, &arn, shared_with, &format!("tok-bounds-{index}")).await;
        created_share_id(&created, shared_with);
    }

    let longest_prefix = "1".repeat(100);
    let overlong_prefix = "1".repeat(101);

    // (case, query, shares the page must carry, whether more are owed after it)
    for (case, query, expected, more_owed) in [
        ("max-results-at-the-minimum", "MaxResults=1".to_string(), 1, true),
        ("max-results-at-the-maximum", "MaxResults=50".to_string(), 2, false),
        // No principal of this lens is a hundred characters long, so the prefix at its
        // published bound selects an empty page rather than failing.
        (
            "prefix-at-the-maximum-length",
            format!("SharedWithPrefix={longest_prefix}"),
            0,
            false,
        ),
    ] {
        let listed = list_shares(&client, &arn, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a value on the published bound must be served, got {listed:?}"
        );
        assert_eq!(
            summaries(&listed, case).len(),
            expected,
            "case {case:?}: expected {expected} share(s) on this page, got {listed:?}"
        );
        assert_eq!(
            next_token(&listed).is_some(),
            more_owed,
            "case {case:?}: NextToken must be present only while shares remain, got {listed:?}"
        );
    }

    for (case, query) in [
        ("max-results-below-the-range", "MaxResults=0".to_string()),
        ("max-results-above-the-range", "MaxResults=51".to_string()),
        (
            "prefix-past-the-maximum-length",
            format!("SharedWithPrefix={overlong_prefix}"),
        ),
    ] {
        let refused = list_shares(&client, &arn, &query).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 outside the published bound, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }
}

/// Rule 5: the lens is resolved by what the `LensAlias` names. A lens that exists and holds
/// no shares reports an empty page; an alias naming no lens at all is a 404 that NAMES the
/// lens as the missing resource.
#[tokio::test]
async fn the_lens_is_resolved_by_what_the_alias_names() {
    let client = Client::new();
    let arn = published_lenses(&client, &["resolved"]).await.remove(0);
    let created = create_share(&client, &arn, "111122223333", "tok-resolution").await;
    let share_id = created_share_id(&created, "resolution");

    // "For custom lenses, this is the lens ARN" and a version-pinned ARN names the same
    // lens, so both spellings reach the share the lens holds.
    for (case, alias) in [
        ("the-lens-arn", arn.clone()),
        ("a-version-pinned-lens-arn", format!("{arn}/1.0")),
    ] {
        let listed = list_shares(&client, &alias, "").await;
        assert_eq!(
            page_share_ids(&summaries(&listed, case)),
            vec![share_id.clone()],
            "case {case:?}: the spelling must reach the lens's own shares, got {listed:?}"
        );
    }

    // A lens Amazon Web Services publishes EXISTS and holds no shares: "Lenses provided by
    // Amazon Web Services (Amazon Web Services Official Content) cannot be shared"
    // (`CreateLensShare` doc). `AWS_OFFICIAL_LENS_ALIASES` is the catalog a bare alias
    // resolves against, and an official lens is also named by an account-less ARN.
    let official_alias = state::AWS_OFFICIAL_LENS_ALIASES
        .iter()
        .find(|alias| **alias != state::FRAMEWORK_LENS_ALIAS)
        .expect("the official lens catalog carries more than the framework lens");
    // An imported lens no version has been published from is stored and holds no shares
    // either: it "cannot be applied to workloads or shared with other Amazon Web Services
    // accounts until it's published with CreateLensVersion" (`ImportLens` doc).
    let draft = import(
        &client,
        &lens_document("Listed-draft-lens", None, &[("governance", 1, 2)]).to_string(),
        None,
        "import-draft",
        None,
    )
    .await;
    let draft_arn = imported_arn(&draft, "draft");
    wait_for_the_import_to_land();

    for (case, alias) in [
        ("an-official-lens-alias", (*official_alias).to_string()),
        ("the-framework-lens-alias", state::FRAMEWORK_LENS_ALIAS.to_string()),
        (
            "an-official-lens-arn",
            state::official_lens_arn(state::FRAMEWORK_LENS_ALIAS),
        ),
        ("an-unpublished-custom-lens", draft_arn.clone()),
    ] {
        let listed = list_shares(&client, &alias, "").await;
        assert_eq!(
            summaries(&listed, case),
            Vec::<Value>::new(),
            "case {case:?}: a lens that holds no shares must report an empty page, got \
             {listed:?}"
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {case:?}: an empty page owes no NextToken, got {listed:?}"
        );
    }

    // An alias that resolves to no lens at all is a 404 naming the LENS, by the alias as
    // sent: a bare name outside the official catalog, and a well-formed custom lens ARN
    // nothing is stored under.
    for (case, alias) in [
        ("an-alias-naming-no-lens", "notalens".to_string()),
        ("an-unknown-custom-lens-arn", ABSENT_LENS_ARN.to_string()),
    ] {
        let refused = list_shares(&client, &alias, "").await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused
                .body
                .get("ResourceType")
                .and_then(Value::as_str)
                .unwrap_or(""),
            state::LENS_RESOURCE_TYPE,
            "case {case:?}: the 404 must name the LENS as missing, got {refused:?}"
        );
        assert_eq!(
            refused
                .body
                .get("ResourceId")
                .and_then(Value::as_str)
                .unwrap_or(""),
            alias,
            "case {case:?}: the 404 must name the alias as sent, got {refused:?}"
        );
    }

    // Every refusal and every empty page above left the lens's own share standing.
    let listed = list_shares(&client, &arn, "").await;
    assert_eq!(
        page_share_ids(&summaries(&listed, "unchanged")),
        vec![share_id],
        "case \"unchanged\": the reads must leave the lens's shares as they were, got \
         {listed:?}"
    );
}
