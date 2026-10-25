// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for sharing a Well-Architected Tool profile:
//! `CreateProfileShare` and the `ListProfileShares` that reads the share back, plus the
//! share invitation the share raises, which `ListShareInvitations` reports.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. A share GRANTS access to a profile to a principal that does not have it, so a
//!    request whose `SharedWith` names the principal doing the sharing (the profile's
//!    owning account, which every request in this emulator is attributed to) is refused
//!    `400 ValidationException` and records nothing; any OTHER principal is accepted and
//!    gets a share. The rows cross the three kinds of principal `SharedWith` publishes.
//! 2. An accepted share is persisted: it answers a minted `ShareId` matching
//!    `^[0-9a-f]{32}$`, unique across profiles and principals and not derived from the
//!    request, and it reads back through `ListProfileShares` carrying the principal it was
//!    created with and the `PENDING` status a share waits on its invitation in.
//! 3. A share belongs to (profile, principal): the same principal cannot hold two shares
//!    of one profile (`409 ConflictException`), an `@idempotencyToken` replay returns the
//!    original share rather than minting a second, and the same token with different
//!    parameters is a 409.
//! 4. `ListProfileShares` reports: it narrows by `SharedWithPrefix` and `Status`, pages
//!    over every share exactly once at any `MaxResults`, serves each published bound of
//!    its own members and refuses one step outside it, and changes nothing a later caller
//!    sees. A `ProfileArn` that resolves to nothing is a 404 on both operations.
//! 5. The invitation a profile share raises is a `PROFILE` invitation: it carries the
//!    shared profile's ARN and name and none of the members belonging to an invitation of
//!    another resource kind, and it narrows by `ShareResourceType` and `ProfileNamePrefix`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the profile builders are the shared
//! `super::workload_harness`; the invitation readers are the ones `super::share_invitations`
//! already spells.
//!
//! The share a RULE-5 invitation stands for is seeded through `record_profile_share` (see
//! its doc), for the reason `Client::record_workload_share` gives: an invitation is offered
//! to the principal a profile was shared WITH, and a share whose recipient is the caller
//! has no HTTP path in a single-account emulator.

use serde_json::{json, Value};

use super::share_invitations::{invitations, member as summary_member};
use super::workload_harness::{created_profile, encode_path_segment, member, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// A `ProfileArn` of the modelled form that names no stored profile — the Region this
/// harness signs in and the emulator's own account, so only the resource part is absent.
fn absent_profile_arn() -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:profile/0000000000000000000000000000000f",
        engine::emulator_account_id()
    )
}

/// The `ShareStatus` members the model publishes —
/// `smithy_query(query="shape", shape="ShareStatus")`.
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
/// answer — "Shared access to a workload is not removed until the workload invitation is
/// deleted" (the create docs) and `PENDING` is the member for exactly that
/// (`smithy_query(query="shape", shape="ShareStatus")`). Spelled as the literal the model
/// publishes rather than read back out of the code under test.
const NEW_SHARE_STATUS: &str = "PENDING";

/// Principals a profile can be shared WITH: "The Amazon Web Services account ID,
/// organization ID, or organizational unit (OU) ID with which the workload, lens, profile,
/// or review template is shared" (`SharedWith` doc, `@length(12, 2048)` and no pattern), so
/// the rows span all three kinds rather than only an account id.
const OTHER_PRINCIPALS: [(&str, &str); 5] = [
    ("another-account", "111122223333"),
    ("a-third-account", "444455556666"),
    ("an-account-of-nines", "999999999999"),
    ("an-organization", "o-abcd1234efgh"),
    ("an-organizational-unit", "ou-abcd-1a2b3c4d"),
];

/// `POST /profiles/{ProfileArn}/shares`, the wire form of `CreateProfileShare`. The ARN is
/// percent-encoded because it is a non-greedy `@httpLabel` carrying `:` and `/` inside ONE
/// path segment, exactly as an SDK sends it.
async fn create_share(client: &Client, profile_arn: &str, shared_with: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/profiles/{}/shares", encode_path_segment(profile_arn)),
            Some(json!({ "SharedWith": shared_with, "ClientRequestToken": token })),
        )
        .await
}

/// `GET /profiles/{ProfileArn}/shares`, the wire form of `ListProfileShares`, with every
/// filter and paging member bound to the query string. `query` is passed as sent, empty for
/// a request that names none.
async fn list_shares(client: &Client, profile_arn: &str, query: &str) -> Answer {
    let encoded = encode_path_segment(profile_arn);
    let uri = if query.is_empty() {
        format!("/profiles/{encoded}/shares")
    } else {
        format!("/profiles/{encoded}/shares?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `ShareId` a successful `CreateProfileShare` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
fn created_share_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateProfileShare 200, got {answer:?}"
    );
    let id = member(answer, "ShareId").to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}

/// The `ProfileShareSummaries` of a successful `ListProfileShares`.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListProfileShares 200, got {answer:?}"
    );
    answer
        .body
        .get("ProfileShareSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListProfileShares must report ProfileShareSummaries, got {answer:?}")
        })
}

/// Record a share of the profile `profile_arn` with `shared_with`, in the `ShareStatus`
/// `status`, straight through the shared state, and answer its
/// `(ShareId, ShareInvitationId)`.
///
/// The profile counterpart of `Client::record_workload_share`, for the same reason: a share
/// invitation is offered to the principal a profile was shared WITH, so a test of the
/// invitation reads needs a share whose recipient is the CALLER — and that share has no HTTP
/// path here, because `CreateProfileShare` refuses a `SharedWith` naming the principal doing
/// the sharing and this emulator attributes every request to its one account. The same seed
/// is what puts a share in a status other than the one a new share carries: only answering
/// its invitation moves it, and the invitation of a share the caller created is not the
/// caller's to answer. The seed records exactly what `CreateProfileShare` records (rules 1
/// and 2 assert that), and the READ under test still crosses the HTTP boundary.
fn record_profile_share(
    client: &Client,
    profile_arn: &str,
    shared_with: &str,
    status: &str,
) -> (String, String) {
    let state_handle = client.store();
    let mut store = state_handle.lock();
    let shared_by = store
        .profiles
        .get(profile_arn)
        .map(|profile| profile.owner.clone())
        .expect("the profile under test must be stored");
    let share_id = store.mint_share_id();
    let share_invitation_id = store.mint_share_invitation_id();
    store
        .profile_shares
        .entry(profile_arn.to_string())
        .or_default()
        .insert(
            share_id.clone(),
            state::ProfileShareRecord {
                share_id: share_id.clone(),
                share_invitation_id: share_invitation_id.clone(),
                profile_arn: profile_arn.to_string(),
                shared_by,
                shared_with: shared_with.to_string(),
                status: status.to_string(),
                status_message: None,
            },
        );
    (share_id, share_invitation_id)
}

/// Every share the store holds for `profile_arn`, in `ShareId` order — read to prove a
/// REFUSED request recorded nothing.
fn stored_share_count(client: &Client, profile_arn: &str) -> usize {
    let state_handle = client.store();
    let store = state_handle.lock();
    store.profile_shares_of(profile_arn).len()
}

/// Rules 1 and 2: `SharedWith` naming the sharer's own principal is a 400
/// `ValidationException` that records no share, while the same request shape with any other
/// principal is accepted, minted an id and read back through `ListProfileShares`. The rows
/// cross every kind of principal and two profiles.
#[tokio::test]
async fn sharing_with_the_sharers_own_principal_is_refused_and_records_nothing() {
    let client = Client::new();
    let own = engine::emulator_account_id();

    for index in 0..2 {
        let case = format!("self-share-{index}");
        let (profile_arn, _) = created_profile(&client, &format!("self-share-host-{index}")).await;

        // The same token twice: a refused request records nothing, not even its token, so
        // the second attempt is refused exactly as the first.
        for attempt in 1..=2 {
            let refused =
                create_share(&client, &profile_arn, own, &format!("tok-self-{index}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}/attempt-{attempt}: expected HTTP 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_code, "ValidationException",
                "case {case:?}/attempt-{attempt}: expected ValidationException, got {refused:?}"
            );
            assert!(
                refused
                    .body
                    .get("Fields")
                    .and_then(Value::as_array)
                    .is_some_and(|fields| fields
                        .iter()
                        .any(|field| field.get("Name").and_then(Value::as_str)
                            == Some("SharedWith"))),
                "case {case:?}/attempt-{attempt}: the refusal must name SharedWith, got {refused:?}"
            );
            // The refusal is about the RELATIONSHIP between the two principals of the
            // request, so it names no account id, not even the caller's own.
            assert!(
                !refused.body.to_string().contains(own),
                "case {case:?}/attempt-{attempt}: the refusal must disclose no account id, \
                 got {refused:?}"
            );
            assert!(
                summaries(&list_shares(&client, &profile_arn, "").await, &case).is_empty(),
                "case {case:?}/attempt-{attempt}: a refused share must leave the profile with \
                 no shares"
            );
        }

        // The positive control: the same request shape with a DIFFERENT principal is
        // accepted on the same profile, so the refusal above is the principal and not a
        // broken operation.
        for (principal_case, shared_with) in OTHER_PRINCIPALS {
            let case = format!("{case}/{principal_case}");
            let created = create_share(
                &client,
                &profile_arn,
                shared_with,
                &format!("tok-ok-{index}-{principal_case}"),
            )
            .await;
            let share_id = created_share_id(&created, &case);
            assert_eq!(
                member(&created, "ProfileArn"),
                profile_arn,
                "case {case:?}: the output must name the profile shared, got {created:?}"
            );

            let listed = list_shares(&client, &profile_arn, "").await;
            let reported = summaries(&listed, &case);
            let found: Vec<&Value> = reported
                .iter()
                .filter(|summary| {
                    summary.get("ShareId").and_then(Value::as_str) == Some(share_id.as_str())
                })
                .collect();
            assert_eq!(
                found.len(),
                1,
                "case {case:?}: the created share must appear exactly once, got {listed:?}"
            );
            let summary = found[0];
            assert_eq!(
                summary.get("SharedWith").and_then(Value::as_str),
                Some(shared_with),
                "case {case:?}: the share must report the principal it was created with, got \
                 {summary:?}"
            );
            assert_eq!(
                summary.get("Status").and_then(Value::as_str),
                Some(NEW_SHARE_STATUS),
                "case {case:?}: a new share waits on its invitation in PENDING, got {summary:?}"
            );
            // `ProfileShareSummary` declares no permission member, so a profile share
            // reports none.
            assert_eq!(
                summary.get("PermissionType"),
                None,
                "case {case:?}: a profile share grants no permission, got {summary:?}"
            );
        }
    }
}

/// Rule 2: a `ShareId` is minted, not derived from the request — sharing two profiles with
/// the same principals yields ids that are all distinct and all match the modelled pattern.
#[tokio::test]
async fn share_ids_are_minted_and_unique_across_profiles_and_principals() {
    let client = Client::new();
    let mut seen: Vec<String> = Vec::new();

    for host in 0..2 {
        let (profile_arn, _) = created_profile(&client, &format!("mint-host-{host}")).await;
        for (principal_case, shared_with) in OTHER_PRINCIPALS {
            let case = format!("host-{host}/{principal_case}");
            let created = create_share(
                &client,
                &profile_arn,
                shared_with,
                // The SAME parameters on both profiles, bar the path ARN: an id that was a
                // function of the request would repeat here.
                &format!("tok-mint-{host}-{principal_case}"),
            )
            .await;
            let share_id = created_share_id(&created, &case);
            assert!(
                !seen.contains(&share_id),
                "case {case:?}: ShareId {share_id:?} was already minted for another share, \
                 got {seen:?}"
            );
            seen.push(share_id);
        }
    }

    assert_eq!(
        seen.len(),
        2 * OTHER_PRINCIPALS.len(),
        "every share must have been minted an id, got {seen:?}"
    );
}

/// Rule 3: one principal holds one share of one profile. A second share request for a
/// principal already holding one is a 409 naming the share in the way; an idempotency token
/// replayed with the same parameters returns the original share, and replayed with different
/// parameters is a 409.
#[tokio::test]
async fn a_share_belongs_to_one_profile_and_principal() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "share-identity-host").await;

    for (principal_case, shared_with) in OTHER_PRINCIPALS {
        let token = format!("tok-identity-{principal_case}");
        let created = create_share(&client, &profile_arn, shared_with, &token).await;
        let share_id = created_share_id(&created, principal_case);

        // Same token, same parameters: a replay of the original request.
        let replay = create_share(&client, &profile_arn, shared_with, &token).await;
        assert_eq!(
            member(&replay, "ShareId"),
            share_id,
            "case {principal_case:?}: an idempotent replay must return the original ShareId, \
             got {replay:?}"
        );

        // Same token, different parameters: not the same request.
        let reused = create_share(&client, &profile_arn, "777788889999", &token).await;
        assert_eq!(
            reused.status, 409,
            "case {principal_case:?}: a token reused with different parameters must be a 409, \
             got {reused:?}"
        );
        assert_eq!(
            reused.error_code, "ConflictException",
            "case {principal_case:?}: expected ConflictException, got {reused:?}"
        );

        // A fresh token for a principal that already holds a share: the grant already
        // exists.
        let duplicate = create_share(
            &client,
            &profile_arn,
            shared_with,
            &format!("tok-dup-{principal_case}"),
        )
        .await;
        assert_eq!(
            duplicate.status, 409,
            "case {principal_case:?}: a second share for one principal must be a 409, got \
             {duplicate:?}"
        );
        assert_eq!(
            member(&duplicate, "ResourceId"),
            share_id,
            "case {principal_case:?}: the 409 must name the share in the way, got {duplicate:?}"
        );

        let listed = list_shares(&client, &profile_arn, "").await;
        let for_principal = summaries(&listed, principal_case)
            .into_iter()
            .filter(|summary| {
                summary.get("SharedWith").and_then(Value::as_str) == Some(shared_with)
            })
            .count();
        assert_eq!(
            for_principal, 1,
            "case {principal_case:?}: the principal must hold exactly one share, got {listed:?}"
        );
    }
}

/// Rule 4: `ListProfileShares` narrows by `SharedWithPrefix` and by `Status` over every
/// published `ShareStatus`, and reports the same thing twice in a row — a read changes
/// nothing a later caller sees.
#[tokio::test]
async fn listing_shares_filters_and_never_changes_what_a_later_caller_sees() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "share-filter-host").await;
    for (principal_case, shared_with) in OTHER_PRINCIPALS {
        let created = create_share(
            &client,
            &profile_arn,
            shared_with,
            &format!("tok-filter-{principal_case}"),
        )
        .await;
        created_share_id(&created, principal_case);
    }

    // Rows differ along the prefix's length and along which kind of principal it selects.
    for (case, prefix, expected) in [
        ("every-principal", "", OTHER_PRINCIPALS.len()),
        ("accounts-starting-1", "1", 1),
        ("accounts-starting-4", "4", 1),
        ("organizations", "o-", 1),
        ("organizational-units", "ou-", 1),
        // `o` prefixes both the organization and the OU id.
        ("everything-starting-o", "o", 2),
        ("one-exact-principal", "111122223333", 1),
        ("no-principal", "zzz", 0),
    ] {
        let listed = list_shares(&client, &profile_arn, &format!("SharedWithPrefix={prefix}")).await;
        let found = summaries(&listed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} share(s) for prefix {prefix:?}, got {listed:?}"
        );
        for summary in &found {
            let shared_with = summary
                .get("SharedWith")
                .and_then(Value::as_str)
                .unwrap_or("");
            assert!(
                shared_with.starts_with(prefix),
                "case {case:?}: {shared_with:?} does not start with {prefix:?}, got {listed:?}"
            );
        }
    }

    // Every share created over the wire is PENDING, and one share is seeded in each of the
    // seven other published statuses — so every published member of the filter selects a
    // non-empty, KNOWN set, and a filter hardcoded to one status could not pass. A status
    // other than the one a new share carries has no HTTP path here (only answering the
    // invitation moves it, and this caller created these shares), so those rows are seeded.
    let mut seeded: Vec<(&str, String)> = Vec::new();
    for status in SHARE_STATUSES.iter().filter(|status| **status != NEW_SHARE_STATUS) {
        let (share_id, _) = record_profile_share(
            &client,
            &profile_arn,
            &format!("5555{:08}", seeded.len()),
            status,
        );
        seeded.push((status, share_id));
    }

    for status in SHARE_STATUSES {
        let listed = list_shares(&client, &profile_arn, &format!("Status={status}")).await;
        let found = summaries(&listed, status);
        if status == NEW_SHARE_STATUS {
            assert_eq!(
                found.len(),
                OTHER_PRINCIPALS.len(),
                "case {status:?}: expected every share created over the wire, got {listed:?}"
            );
        } else {
            let expected = seeded
                .iter()
                .find(|(seeded_status, _)| *seeded_status == status)
                .map(|(_, share_id)| share_id.as_str())
                .unwrap_or_default();
            assert_eq!(
                found
                    .iter()
                    .map(|summary| summary.get("ShareId").and_then(Value::as_str).unwrap_or(""))
                    .collect::<Vec<&str>>(),
                vec![expected],
                "case {status:?}: expected exactly the share seeded in that status, got {listed:?}"
            );
        }
        for summary in &found {
            assert_eq!(
                summary.get("Status").and_then(Value::as_str),
                Some(status),
                "case {status:?}: a filtered page carries only that status, got {summary:?}"
            );
        }
    }

    // A status outside the published set is a bad request, not an empty page.
    let bogus = list_shares(&client, &profile_arn, "Status=NOT_A_STATUS").await;
    assert_eq!(
        bogus.status, 400,
        "case \"unknown-status\": expected HTTP 400, got {bogus:?}"
    );
    assert_eq!(
        bogus.error_code, "ValidationException",
        "case \"unknown-status\": expected ValidationException, got {bogus:?}"
    );

    // Reading does not mutate: two identical reads report the same shares.
    let first = list_shares(&client, &profile_arn, "").await;
    let again = list_shares(&client, &profile_arn, "").await;
    assert_eq!(
        summaries(&first, "repeat-read"),
        summaries(&again, "repeat-read"),
        "case \"repeat-read\": a read must not change what the next caller sees, got {first:?} \
         then {again:?}"
    );
}

/// Rule 4, paging: at any `MaxResults` the pages enumerate every share exactly once, and the
/// last page carries no `NextToken`. Rows are the published `@range(1, 50)` minimum, values
/// between it and the share count, the count itself and the published maximum.
#[tokio::test]
async fn paging_enumerates_every_share_exactly_once_at_any_page_size() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "share-paging-host").await;

    const SHARE_COUNT: usize = 7;
    let mut all: Vec<String> = Vec::new();
    for index in 0..SHARE_COUNT {
        let created = create_share(
            &client,
            &profile_arn,
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
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= SHARE_COUNT + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            let query = match &next_token {
                Some(token) => format!("MaxResults={page_size}&NextToken={token}"),
                None => format!("MaxResults={page_size}"),
            };
            let listed = list_shares(&client, &profile_arn, &query).await;
            let page = summaries(&listed, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} share(s), got {listed:?}"
            );
            for summary in page {
                walked.push(
                    summary
                        .get("ShareId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = listed
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
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
        let listed = list_shares(&client, &profile_arn, &format!("NextToken={token}")).await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: expected HTTP 400 for a foreign NextToken, got {listed:?}"
        );
        assert_eq!(
            listed.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {listed:?}"
        );
    }
}

/// Rule 4, the published bounds of the list's OWN members: `MaxResults` is
/// `@range(min: 1, max: 50)` and `SharedWithPrefix` `@length(max: 100)`
/// (`smithy_query(query="operation", operation="ListProfileShares")`), so each bound is
/// served and one step outside it is a 400 `ValidationException` — a page size outside the
/// published range is refused, never quietly clamped into it. `SharedWith` is
/// `@length(12, 2048)`, so each of its bounds is accepted by the create and one step outside
/// refused.
#[tokio::test]
async fn the_published_bounds_are_served_and_one_step_outside_them_is_refused() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "share-bounds-host").await;
    // Two shares, so the minimum page size serves strictly fewer than all of them and the
    // bound is load-bearing rather than trivially satisfied.
    for (index, shared_with) in ["111122223333", "o-abcde12345"].iter().enumerate() {
        let created = create_share(
            &client,
            &profile_arn,
            shared_with,
            &format!("tok-bounds-{index}"),
        )
        .await;
        created_share_id(&created, shared_with);
    }

    let longest_prefix = "1".repeat(100);
    let overlong_prefix = "1".repeat(101);

    // (case, query, shares the page must carry, whether more are owed after it)
    for (case, query, expected, more_owed) in [
        (
            "max-results-at-the-minimum",
            "MaxResults=1".to_string(),
            1,
            true,
        ),
        (
            "max-results-at-the-maximum",
            "MaxResults=50".to_string(),
            2,
            false,
        ),
        // No principal of this profile is a hundred characters long, so the prefix at its
        // published bound selects an empty page rather than failing.
        (
            "prefix-at-the-maximum-length",
            format!("SharedWithPrefix={longest_prefix}"),
            0,
            false,
        ),
    ] {
        let listed = list_shares(&client, &profile_arn, &query).await;
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
            listed
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .is_some(),
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
        let refused = list_shares(&client, &profile_arn, &query).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 outside the published bound, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }

    // `SharedWith` is @length(min: 12, max: 2048): one outside each published bound is
    // refused, each bound itself is accepted.
    for (case, shared_with) in [
        ("one-below-the-minimum", "1".repeat(11)),
        ("empty", String::new()),
        ("one-above-the-maximum", "1".repeat(2049)),
    ] {
        let refused = create_share(
            &client,
            &profile_arn,
            &shared_with,
            &format!("tok-sw-{case}"),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a SharedWith of {} character(s), got {refused:?}",
            shared_with.len()
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }
    for (case, shared_with) in [
        ("at-the-minimum", "2".repeat(12)),
        ("at-the-maximum", "3".repeat(2048)),
    ] {
        let accepted = create_share(
            &client,
            &profile_arn,
            &shared_with,
            &format!("tok-sw-ok-{case}"),
        )
        .await;
        created_share_id(&accepted, case);
    }
}

/// Rule 4: a `ProfileArn` that resolves to no stored profile is the lookup's 404 on both
/// operations, and a refused create records nothing. The rows differ along how the ARN
/// misses: a well-formed ARN of an absent profile, and one the modelled pattern refuses
/// outright (which the boundary validator turns into a 400 before the lookup runs).
#[tokio::test]
async fn an_arn_that_resolves_to_no_profile_is_refused_on_both_operations() {
    let client = Client::new();
    let absent = absent_profile_arn();

    for (operation, answer) in [
        (
            "CreateProfileShare",
            create_share(&client, &absent, "111122223333", "tok-absent").await,
        ),
        (
            "ListProfileShares",
            list_shares(&client, &absent, "").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case \"well-formed-absent\"/{operation}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case \"well-formed-absent\"/{operation}: expected ResourceNotFoundException, got \
             {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            absent,
            "case \"well-formed-absent\"/{operation}: ResourceId must be the ARN as sent, got \
             {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            state::PROFILE_RESOURCE_TYPE,
            "case \"well-formed-absent\"/{operation}: the missing resource is a profile, got \
             {answer:?}"
        );
    }
    assert_eq!(
        stored_share_count(&client, &absent),
        0,
        "case \"well-formed-absent\": a refused share must record nothing"
    );

    // An ARN the modelled `ProfileArn` pattern refuses never reaches the lookup: it is a
    // 400 from the boundary validator on both operations.
    for (case, malformed) in [
        ("not-an-arn", "not-a-profile-arn".to_string()),
        (
            "another-resource-kind",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:workload/0000000000000000000000000000000f",
                engine::emulator_account_id()
            ),
        ),
    ] {
        for (operation, answer) in [
            (
                "CreateProfileShare",
                create_share(&client, &malformed, "111122223333", &format!("tok-{case}")).await,
            ),
            (
                "ListProfileShares",
                list_shares(&client, &malformed, "").await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}/{operation}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// Rule 5: the invitation a profile share raises is a `PROFILE` invitation naming the shared
/// profile, it narrows by `ShareResourceType` and `ProfileNamePrefix`, and reading it mutates
/// nothing. The profile rows differ along the profile's name, so the prefixes select
/// different counts.
#[tokio::test]
async fn a_profile_share_raises_a_profile_invitation_for_the_principal_it_was_offered_to() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    // Names chosen so the prefix rows below select different counts.
    const NAMES: [&str; 3] = ["Shared-alpha", "Shared-beta", "Other-profile"];
    let mut profiles: Vec<(&str, String)> = Vec::new();
    for name in NAMES {
        let (arn, _) = created_profile(&client, name).await;
        profiles.push((name, arn));
    }

    // One invitation per profile, addressed to the caller, plus one addressed elsewhere —
    // which is none of this caller's business and must not be reported.
    let mut offered: Vec<(&str, String, String)> = Vec::new();
    for (name, arn) in &profiles {
        let (share_id, invitation_id) = record_profile_share(&client, arn, caller, NEW_SHARE_STATUS);
        offered.push((name, share_id, invitation_id));
    }
    let elsewhere =
        create_share(&client, &profiles[0].1, "111122223333", "tok-elsewhere").await;
    created_share_id(&elsewhere, "offered-elsewhere");

    let listed = client
        .list_share_invitations("ShareResourceType=PROFILE")
        .await;
    let reported = invitations(&listed, "profile-invitations");
    assert_eq!(
        reported.len(),
        profiles.len(),
        "case \"profile-invitations\": expected one invitation per shared profile, got {listed:?}"
    );

    for (index, (name, share_id, invitation_id)) in offered.iter().enumerate() {
        let found = reported
            .iter()
            .find(|invitation| summary_member(invitation, "ShareInvitationId") == invitation_id)
            .unwrap_or_else(|| {
                panic!("case {name:?}: the pending share must be reported, got {listed:?}")
            });
        assert_eq!(
            summary_member(found, "ShareResourceType"),
            "PROFILE",
            "case {name:?}: a share of a profile is a PROFILE invitation, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "ProfileArn"),
            profiles[index].1,
            "case {name:?}: the invitation must name the shared profile, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "ProfileName"),
            *name,
            "case {name:?}: the invitation must carry the profile's name, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "SharedWith"),
            caller,
            "case {name:?}: the invitation is the one offered to the caller, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "SharedBy"),
            caller,
            "case {name:?}: the invitation must name the account that shared the profile, got \
             {found:?}"
        );
        assert_ne!(
            summary_member(found, "ShareInvitationId"),
            share_id.as_str(),
            "case {name:?}: the invitation is its own resource, so its id is not the ShareId, \
             got {found:?}"
        );
        // A profile share grants no permission, and the members of another resource kind
        // belong to that kind.
        for absent in [
            "PermissionType",
            "WorkloadId",
            "WorkloadName",
            "LensName",
            "LensArn",
            "TemplateName",
            "TemplateArn",
        ] {
            assert_eq!(
                found.get(absent),
                None,
                "case {name:?}: a profile invitation carries no {absent}, got {found:?}"
            );
        }
    }

    // `ProfileNamePrefix` narrows by the profile's NAME; the rows differ along the prefix's
    // length and along how many of the shared profiles it selects.
    for (case, prefix, expected) in [
        ("every-profile", "", profiles.len()),
        ("one-letter", "S", 2),
        ("one-profile", "Shared-a", 1),
        ("another-profile", "Other", 1),
        ("exact-name", "Shared-beta", 1),
        ("no-profile", "zzz", 0),
    ] {
        let narrowed = client
            .list_share_invitations(&format!("ProfileNamePrefix={prefix}"))
            .await;
        let found = invitations(&narrowed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} invitation(s) for prefix {prefix:?}, got \
             {narrowed:?}"
        );
        for invitation in &found {
            let name = summary_member(invitation, "ProfileName");
            assert!(
                name.starts_with(prefix),
                "case {case:?}: {name:?} does not start with {prefix:?}, got {narrowed:?}"
            );
        }
    }

    // A workload prefix selects invitations of that kind, of which there are none here.
    let crossed = client
        .list_share_invitations("WorkloadNamePrefix=Shared-")
        .await;
    assert_eq!(
        invitations(&crossed, "a-workload-prefix").len(),
        0,
        "case \"a-workload-prefix\": a profile invitation carries no workload name, got {crossed:?}"
    );

    // Reading does not mutate: a second read reports the same invitations, and the shares
    // they stand for still wait on them.
    let again = client
        .list_share_invitations("ShareResourceType=PROFILE")
        .await;
    assert_eq!(
        invitations(&again, "repeat-read"),
        reported,
        "case \"repeat-read\": a read must not change what the next caller sees, got {listed:?} \
         then {again:?}"
    );
    for (name, arn) in &profiles {
        let listed = list_shares(&client, arn, "").await;
        for summary in summaries(&listed, name) {
            assert_eq!(
                summary.get("Status").and_then(Value::as_str),
                Some(NEW_SHARE_STATUS),
                "case {name:?}: reading the invitations must not move a share's status, got \
                 {summary:?}"
            );
        }
    }
}
