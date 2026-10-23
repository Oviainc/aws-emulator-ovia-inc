// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for sharing a workload: `CreateWorkloadShare` and the
//! `ListWorkloadShares` that reads the share back.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. A share GRANTS a permission on a workload to a principal that does not have it, so
//!    a request whose `SharedWith` names the principal that is doing the sharing (the
//!    workload's owning account, which every request in this emulator is attributed to)
//!    is refused `400 ValidationException` and records nothing; any OTHER principal is
//!    accepted and gets a share.
//! 2. An accepted share is persisted: it answers a minted `ShareId` matching
//!    `^[0-9a-f]{32}$`, unique across shares and not derived from the request, and it
//!    reads back through `ListWorkloadShares` carrying the principal and permission it
//!    was created with and the `PENDING` status a share waits on its invitation in.
//! 3. A share belongs to (workload, principal): the same principal cannot hold two
//!    shares of one workload (`409 ConflictException`), and an `@idempotencyToken` replay
//!    returns the original share rather than minting a second.
//! 4. `ListWorkloadShares` reports: it narrows by `SharedWithPrefix` and `Status`, pages
//!    over every share exactly once at any `MaxResults`, and changes nothing a later
//!    caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The `PermissionType` members the model publishes —
/// `smithy_query(query="shape", shape="PermissionType")` lists exactly these two.
const PERMISSION_TYPES: [&str; 2] = ["CONTRIBUTOR", "READONLY"];

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

/// Principals a workload can be shared WITH: "The Amazon Web Services account ID,
/// organization ID, or organizational unit (OU) ID with which the workload … is shared"
/// (`SharedWith` doc, `@length(12, 2048)` and no pattern), so the rows span all three
/// kinds and never just the account id the issue recorded.
const OTHER_PRINCIPALS: [(&str, &str); 5] = [
    ("another-account", "111122223333"),
    ("a-third-account", "444455556666"),
    ("an-account-of-nines", "999999999999"),
    ("an-organization", "o-abcd1234efgh"),
    ("an-organizational-unit", "ou-abcd-1a2b3c4d"),
];

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

fn share_request(shared_with: &str, permission_type: &str, token: &str) -> Value {
    json!({
        "SharedWith": shared_with,
        "PermissionType": permission_type,
        "ClientRequestToken": token,
    })
}

/// The `ShareId` a successful `CreateWorkloadShare` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
fn created_share_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkloadShare 200, got {answer:?}"
    );
    let id = member(answer, "ShareId").to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}

/// The `WorkloadShareSummaries` of a successful `ListWorkloadShares`.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListWorkloadShares 200, got {answer:?}"
    );
    answer
        .body
        .get("WorkloadShareSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListWorkloadShares must report WorkloadShareSummaries, got {answer:?}")
        })
}

/// Rule 1: `SharedWith` naming the sharer's own principal is a 400 `ValidationException`
/// that records no share, while the same request shape with any other principal is
/// accepted. The rows differ along the permission asked for and along which other
/// principal is offered the share, and the self-share row is derived from the emulator's
/// own account contract rather than a literal account id.
#[tokio::test]
async fn sharing_with_the_sharers_own_principal_is_refused_and_records_nothing() {
    let client = Client::new();
    let own = engine::emulator_account_id();

    for (index, permission_type) in PERMISSION_TYPES.iter().enumerate() {
        let case = format!("self-share-{permission_type}");
        let workload_id = created_id(
            &client
                .create(minimal_create(
                    &format!("self-share-host-{index}"),
                    &format!("tok-self-host-{index}"),
                ))
                .await,
            &case,
        );

        // The same token twice: a refused request records nothing, not even its token,
        // so the second attempt is refused exactly as the first.
        for attempt in 1..=2 {
            let refused = client
                .create_share(
                    &workload_id,
                    share_request(own, permission_type, &format!("tok-self-{index}")),
                )
                .await;
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
            // request, so it states that relationship — and names no account id, not even
            // the caller's own.
            assert_eq!(
                member(&refused, "Message"),
                "[Validation] The shared with principal cannot be the same as the sharer's \
                 principal.",
                "case {case:?}/attempt-{attempt}: expected the self-share refusal sentence, \
                 got {refused:?}"
            );
            assert!(
                !refused.body.to_string().contains(own),
                "case {case:?}/attempt-{attempt}: the refusal must disclose no account id, \
                 got {refused:?}"
            );

            let listed = client.list_shares(&workload_id, "").await;
            assert!(
                summaries(&listed, &case).is_empty(),
                "case {case:?}/attempt-{attempt}: a refused share must leave the workload \
                 with no shares, got {listed:?}"
            );
        }

        // The positive control: the same request shape with a DIFFERENT principal is
        // accepted on the same workload, so the refusal above is the principal and not a
        // broken operation.
        for (principal_case, shared_with) in OTHER_PRINCIPALS {
            let case = format!("{case}/{principal_case}");
            let created = client
                .create_share(
                    &workload_id,
                    share_request(
                        shared_with,
                        permission_type,
                        &format!("tok-ok-{index}-{principal_case}"),
                    ),
                )
                .await;
            let share_id = created_share_id(&created, &case);
            assert_eq!(
                member(&created, "WorkloadId"),
                workload_id,
                "case {case:?}: the output must name the workload shared, got {created:?}"
            );

            let listed = client.list_shares(&workload_id, "").await;
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
                "case {case:?}: the share must report the principal it was created with, \
                 got {summary:?}"
            );
            assert_eq!(
                summary.get("PermissionType").and_then(Value::as_str),
                Some(*permission_type),
                "case {case:?}: the share must report the permission it granted, got {summary:?}"
            );
            assert_eq!(
                summary.get("Status").and_then(Value::as_str),
                Some("PENDING"),
                "case {case:?}: a new share waits on its invitation in PENDING, got {summary:?}"
            );
        }
    }
}

/// Rule 2: a `ShareId` is minted, not derived from the request — sharing two workloads
/// with the same principal, and one workload with several principals, yields ids that
/// are all distinct and all match the modelled pattern.
#[tokio::test]
async fn share_ids_are_minted_and_unique_across_workloads_and_principals() {
    let client = Client::new();
    let mut seen: Vec<String> = Vec::new();

    for host in 0..2 {
        let case = format!("host-{host}");
        let workload_id = created_id(
            &client
                .create(minimal_create(
                    &format!("mint-host-{host}"),
                    &format!("tok-mint-host-{host}"),
                ))
                .await,
            &case,
        );
        for (principal_case, shared_with) in OTHER_PRINCIPALS {
            let case = format!("{case}/{principal_case}");
            let created = client
                .create_share(
                    &workload_id,
                    // The SAME parameters on both workloads, bar the path id: an id that
                    // was a function of the request would repeat here.
                    share_request(shared_with, "READONLY", &format!("tok-mint-{host}-{principal_case}")),
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

/// Rule 3: one principal holds one share of one workload. A second share request for a
/// principal already holding one is a 409 naming the share in the way; an idempotency
/// token replayed with the same parameters returns the original share, and replayed with
/// different parameters is a 409.
#[tokio::test]
async fn a_share_belongs_to_one_workload_and_principal() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("share-identity-host", "tok-identity"))
            .await,
        "host",
    );

    for (principal_case, shared_with) in OTHER_PRINCIPALS {
        let token = format!("tok-identity-{principal_case}");
        let created = client
            .create_share(&workload_id, share_request(shared_with, "READONLY", &token))
            .await;
        let share_id = created_share_id(&created, principal_case);

        // Same token, same parameters: a replay of the original request.
        let replay = client
            .create_share(&workload_id, share_request(shared_with, "READONLY", &token))
            .await;
        assert_eq!(
            member(&replay, "ShareId"),
            share_id,
            "case {principal_case:?}: an idempotent replay must return the original ShareId, \
             got {replay:?}"
        );

        // Same token, different parameters: not the same request.
        let reused = client
            .create_share(
                &workload_id,
                share_request(shared_with, "CONTRIBUTOR", &token),
            )
            .await;
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
        // exists, whatever permission the second request asks for.
        for permission_type in PERMISSION_TYPES {
            let case = format!("{principal_case}/{permission_type}");
            let duplicate = client
                .create_share(
                    &workload_id,
                    share_request(
                        shared_with,
                        permission_type,
                        &format!("tok-dup-{principal_case}-{permission_type}"),
                    ),
                )
                .await;
            assert_eq!(
                duplicate.status, 409,
                "case {case:?}: a second share for one principal must be a 409, got {duplicate:?}"
            );
            assert_eq!(
                duplicate.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {duplicate:?}"
            );
            assert_eq!(
                member(&duplicate, "ResourceId"),
                share_id,
                "case {case:?}: the 409 must name the share in the way, got {duplicate:?}"
            );
        }

        let listed = client.list_shares(&workload_id, "").await;
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

/// Rule 4: `ListWorkloadShares` narrows by `SharedWithPrefix` and by `Status` over every
/// published `ShareStatus`, and reports the same thing twice in a row — a read changes
/// nothing a later caller sees.
#[tokio::test]
async fn listing_shares_filters_and_never_changes_what_a_later_caller_sees() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("share-filter-host", "tok-filter"))
            .await,
        "host",
    );
    for (principal_case, shared_with) in OTHER_PRINCIPALS {
        let created = client
            .create_share(
                &workload_id,
                share_request(shared_with, "READONLY", &format!("tok-filter-{principal_case}")),
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
        let listed = client
            .list_shares(&workload_id, &format!("SharedWithPrefix={prefix}"))
            .await;
        let found = summaries(&listed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} share(s) for prefix {prefix:?}, got {listed:?}"
        );
        for summary in &found {
            let shared_with = summary.get("SharedWith").and_then(Value::as_str).unwrap_or("");
            assert!(
                shared_with.starts_with(prefix),
                "case {case:?}: {shared_with:?} does not start with {prefix:?}, got {listed:?}"
            );
        }
    }

    // Every new share is PENDING, so that status selects all of them and each of the
    // seven other published statuses selects none.
    for status in SHARE_STATUSES {
        let listed = client
            .list_shares(&workload_id, &format!("Status={status}"))
            .await;
        let expected = if status == "PENDING" {
            OTHER_PRINCIPALS.len()
        } else {
            0
        };
        assert_eq!(
            summaries(&listed, status).len(),
            expected,
            "case {status:?}: expected {expected} share(s) with that status, got {listed:?}"
        );
    }

    // A status outside the published set is a bad request, not an empty page.
    let bogus = client.list_shares(&workload_id, "Status=NOT_A_STATUS").await;
    assert_eq!(
        bogus.status, 400,
        "case \"unknown-status\": expected HTTP 400, got {bogus:?}"
    );
    assert_eq!(
        bogus.error_code, "ValidationException",
        "case \"unknown-status\": expected ValidationException, got {bogus:?}"
    );

    // Reading does not mutate: two identical reads report the same shares.
    let first = client.list_shares(&workload_id, "").await;
    let again = client.list_shares(&workload_id, "").await;
    assert_eq!(
        summaries(&first, "repeat-read"),
        summaries(&again, "repeat-read"),
        "case \"repeat-read\": a read must not change what the next caller sees, got \
         {first:?} then {again:?}"
    );
}

/// Rule 4, paging: at any `MaxResults` the pages enumerate every share exactly once, and
/// the last page carries no `NextToken`. Rows are the published `@range(1, 50)` minimum,
/// values between it and the share count, the count itself and the published maximum.
#[tokio::test]
async fn paging_enumerates_every_share_exactly_once_at_any_page_size() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("share-paging-host", "tok-paging"))
            .await,
        "host",
    );

    const SHARE_COUNT: usize = 7;
    let mut all: Vec<String> = Vec::new();
    for index in 0..SHARE_COUNT {
        let created = client
            .create_share(
                &workload_id,
                share_request(
                    &format!("{:012}", 100_000_000_000u64 + index as u64),
                    PERMISSION_TYPES[index % PERMISSION_TYPES.len()],
                    &format!("tok-paging-{index}"),
                ),
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
            let listed = client.list_shares(&workload_id, &query).await;
            let page = summaries(&listed, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} share(s), got {listed:?}"
            );
            assert_eq!(
                member(&listed, "WorkloadId"),
                workload_id,
                "case {case:?}: every page must name the workload, got {listed:?}"
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
        ("a-workloads-cursor", "7b226e223a2261222c2269223a2262227d"),
    ] {
        let listed = client
            .list_shares(&workload_id, &format!("NextToken={token}"))
            .await;
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

/// The refusals both operations owe a request that cannot be served: an unknown
/// `PermissionType`, a `SharedWith` outside the published `@length(12, 2048)` bound, and
/// a `WorkloadId` that resolves to nothing. A share also cannot outlive its workload.
#[tokio::test]
async fn share_requests_that_cannot_be_served_are_refused_with_the_modelled_error() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("share-refusal-host", "tok-refusal"))
            .await,
        "host",
    );

    // Rows outside the published `PermissionType` enum, including a lowercase spelling of
    // a published member and the empty string.
    for (case, permission_type) in [
        ("unknown-member", "FULL_ACCESS"),
        ("lowercase-member", "readonly"),
        ("empty", ""),
        ("member-of-another-enum", "PENDING"),
    ] {
        let refused = client
            .create_share(
                &workload_id,
                share_request("111122223333", permission_type, &format!("tok-pt-{case}")),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for PermissionType {permission_type:?}, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }

    // `SharedWith` is @length(min: 12, max: 2048): one below each published bound is
    // refused, each bound itself is accepted.
    for (case, shared_with) in [
        ("one-below-the-minimum", "1".repeat(11)),
        ("empty", String::new()),
        ("one-above-the-maximum", "1".repeat(2049)),
    ] {
        let refused = client
            .create_share(
                &workload_id,
                share_request(&shared_with, "READONLY", &format!("tok-sw-{case}")),
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
        let accepted = client
            .create_share(
                &workload_id,
                share_request(&shared_with, "READONLY", &format!("tok-sw-ok-{case}")),
            )
            .await;
        created_share_id(&accepted, case);
    }

    // A `WorkloadId` naming no workload is the lookup's 404 on both operations.
    for (case, absent) in [
        ("well-formed-absent", ABSENT_HEX_ID.to_string()),
        ("not-hex-at-all", "not-a-workload-id".to_string()),
        ("one-character-short", "0".repeat(31)),
    ] {
        for (operation, answer) in [
            (
                "CreateWorkloadShare",
                client
                    .create_share(
                        &absent,
                        share_request("111122223333", "READONLY", &format!("tok-absent-{case}")),
                    )
                    .await,
            ),
            (
                "ListWorkloadShares",
                client.list_shares(&absent, "").await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                absent,
                "case {case:?}/{operation}: ResourceId must be the id as sent, got {answer:?}"
            );
        }
    }

    // Deleting the workload takes its shares with it.
    let deleted = client.delete(&workload_id, "tok-refusal-delete").await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete\": DeleteWorkload must succeed, got {deleted:?}"
    );
    let orphans = client.list_shares(&workload_id, "").await;
    assert_eq!(
        orphans.status, 404,
        "case \"delete\": the shares of a deleted workload must not resolve, got {orphans:?}"
    );
}

/// Rule 4, the published bounds of the list's OWN members: `MaxResults` is
/// `@range(min: 1, max: 50)` and `SharedWithPrefix` `@length(max: 100)`
/// (`smithy_query(query="operation", operation="ListWorkloadShares")`), so each bound is
/// served and one step outside it is a 400 `ValidationException` — a page size outside
/// the published range is refused, never quietly clamped into it.
#[tokio::test]
async fn the_lists_own_members_are_served_on_each_bound_and_refused_outside_it() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("share-bounds-host", "tok-bounds"))
            .await,
        "host",
    );
    // Two shares, so the minimum page size serves strictly fewer than all of them and
    // the bound is load-bearing rather than trivially satisfied.
    for (index, shared_with) in ["111122223333", "o-abcde12345"].iter().enumerate() {
        let created = client
            .create_share(
                &workload_id,
                share_request(shared_with, "READONLY", &format!("tok-bounds-{index}")),
            )
            .await;
        created_share_id(&created, shared_with);
    }

    let longest_prefix = "1".repeat(100);
    let overlong_prefix = "1".repeat(101);

    // (case, query, shares the page must carry, whether more are owed after it)
    for (case, query, expected, more_owed) in [
        ("max-results-at-the-minimum", "MaxResults=1".to_string(), 1, true),
        ("max-results-at-the-maximum", "MaxResults=50".to_string(), 2, false),
        // No principal of this workload is a hundred characters long, so the prefix at
        // its published bound selects an empty page rather than failing.
        (
            "prefix-at-the-maximum-length",
            format!("SharedWithPrefix={longest_prefix}"),
            0,
            false,
        ),
    ] {
        let listed = client.list_shares(&workload_id, &query).await;
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
            listed.body.get("NextToken").and_then(Value::as_str).is_some(),
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
        let refused = client.list_shares(&workload_id, &query).await;
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
