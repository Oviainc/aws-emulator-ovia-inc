// SPDX-License-Identifier: CC0-1.0

//! `CreateLegalHold`, `ListLegalHolds` and `GetLegalHold`, driven as real HTTP
//! through the router.
//!
//! The rules under test:
//!
//! * a legal hold is a STORED resource: the create mints its id and ARN and
//!   records the title, description and recovery-point selection it was given,
//!   and `ListLegalHolds` and `GetLegalHold` report every stored hold exactly
//!   once, member for member with the answer its create gave;
//! * a hold is returned in a non-terminal status, so the `CREATING` ->
//!   `ACTIVE` transition its clock owes is observable and nothing else about
//!   the hold changes with it;
//! * a listing is a page of `MaxResults` holds plus an opaque cursor to the
//!   next one, and walking the cursors visits every stored hold once;
//! * a create that repeats an `IdempotencyToken` answers with the hold that
//!   token already made, so no second hold is listed;
//! * a request outside what AWS accepts — an absent required member (including
//!   the recovery-point selection, which AWS requires though the model does
//!   not), a selection bound in the future, a resource identifier that is not
//!   an ARN, a page size outside the modelled range, a cursor from
//!   no result set, an id the account holds nothing under — is refused with the
//!   modelled exception for that condition and stores nothing.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::vault_and_backup_job::{call, error_code, router, string_at, ACCOUNT, REGION};

/// Longer than a legal hold's settle span
/// (`crate::state::LEGAL_HOLD_SETTLE_AFTER`), so a read after this wait observes
/// the activation the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`, for the reason given on
/// `vault_and_backup_job::PAST_SETTLE`: the workspace's `tokio` dependency does
/// not enable the `time` feature, and the handlers read the wall clock anyway.
const PAST_LEGAL_HOLD_SETTLE: Duration = Duration::from_millis(3_400);

/// The statuses the model lists for `LegalHoldStatus`
/// (`smithy_query` shape `LegalHoldStatus`: `CREATING | ACTIVE | CANCELING |
/// CANCELED`) that a hold nobody has cancelled passes through.
const STATUS_CREATING: &str = "CREATING";
const STATUS_ACTIVE: &str = "ACTIVE";

/// The ends of the modelled `MaxResults` range (`smithy_query` shape
/// `MaxResults`: `@range` 1..1000).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// The sentence AWS states for a legal-hold create that selects no recovery
/// points, which this service words once (`crate::state`'s
/// `MESSAGE_MISSING_RESOURCE_SELECTION`).
const MISSING_RESOURCE_SELECTION: &str = "Missing Resource Selection.";

/// The sentence AWS states for a legal hold id the account does not hold, which
/// this service words once (`crate::state`'s `MESSAGE_LEGAL_HOLD_NOT_FOUND`).
const LEGAL_HOLD_NOT_FOUND: &str = "Can not find Legal Hold ARN.";

/// `POST /legal-holds` — `CreateLegalHold`'s HTTP binding.
async fn create_legal_hold(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "POST", "/legal-holds", Some(&request.to_string())).await
}

/// `GET /legal-holds` — `ListLegalHolds`'s HTTP binding, with the query string
/// the caller's page parameters bind to.
async fn list_legal_holds(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/legal-holds{query}"), None).await
}

/// `GET /legal-holds/{LegalHoldId}` — `GetLegalHold`'s HTTP binding.
async fn get_legal_hold(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/legal-holds/{id}"), None).await
}

/// The refusal sentence of an error body, whichever casing the envelope spells
/// the member with: a refusal the boundary validator words carries `message`,
/// one the handler returns carries the modelled `Message`.
fn refusal_message(body: &Value) -> String {
    for member in ["Message", "message"] {
        let text = string_at(body, member);
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

/// The `LegalHolds` array of a listing.
fn listed(body: &Value) -> Vec<Value> {
    body.get("LegalHolds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Seconds since the epoch, `offset` from now — the Unix form the model's
/// timestamps take, used to build a `DateRange` relative to the test's clock.
fn epoch_seconds(offset: Duration, ahead: bool) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs() as i64;
    if ahead {
        now + offset.as_secs() as i64
    } else {
        now - offset.as_secs() as i64
    }
}

/// A past instant, `days` before now.
fn days_ago(days: u64) -> i64 {
    epoch_seconds(Duration::from_secs(days * 24 * 60 * 60), false)
}

/// The create requests the round-trip table walks: the rows differ in how the
/// recovery-point selection picks recovery points (vault names, resource
/// identifiers, a date range, all three), in the length and alphabet of the
/// title and description and in whether tags and an idempotency token are
/// carried — which is what the "read back what was stored" rule generalises
/// over.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a selection naming one vault and nothing else",
            json!({
                "Title": "Audit 2026",
                "Description": "hold for the 2026 audit",
                "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
            }),
        ),
        (
            "a selection naming several vaults",
            json!({
                "Title": "VaultScoped",
                "Description": "every recovery point in two vaults",
                "RecoveryPointSelection": {"VaultNames": ["primary-vault", "cold_vault_2"]},
            }),
        ),
        (
            "a selection naming resource ARNs",
            json!({
                "Title": "ResourceScoped",
                "Description": "two protected resources",
                "RecoveryPointSelection": {
                    "ResourceIdentifiers": [
                        "arn:aws:ec2:us-west-2:000000000000:volume/vol-0abc",
                        "arn:aws:dynamodb:us-west-2:000000000000:table/ledger",
                    ],
                },
            }),
        ),
        (
            "a selection carrying a past date range",
            json!({
                "Title": "RangeScoped",
                "Description": "a closed window of recovery points",
                "RecoveryPointSelection": {
                    "DateRange": {"FromDate": days_ago(30), "ToDate": days_ago(1)},
                },
            }),
        ),
        (
            "every selection member at once, with tags and a token",
            json!({
                "Title": "FullySpecified",
                "Description": "vaults, resources and a window",
                "IdempotencyToken": "round-trip-token",
                "Tags": {"owner": "legal", "matter": "case-17"},
                "RecoveryPointSelection": {
                    "VaultNames": ["primary-vault"],
                    "ResourceIdentifiers": ["arn:aws:rds:us-west-2:000000000000:db:ledger"],
                    "DateRange": {"FromDate": days_ago(7), "ToDate": days_ago(7)},
                },
            }),
        ),
        (
            "a one-character title and description",
            json!({
                "Title": "t",
                "Description": "d",
                "RecoveryPointSelection": {"VaultNames": ["ab"]},
            }),
        ),
        (
            "a long title and a description of punctuation the model admits",
            json!({
                "Title": "L".repeat(512),
                "Description": "spaces + hyphens = . _ : / and UTF-8 \u{00e9}\u{00fc}\u{4e2d}",
                "RecoveryPointSelection": {
                    "ResourceIdentifiers": ["arn:aws:ec2:us-west-2:000000000000:volume/vol-0def"],
                },
            }),
        ),
    ]
}

/// Every member a legal hold is created with is reported by a later listing of
/// it, the ARN the create minted names that hold, and a hold whose creation has
/// not settled reports the non-terminal status it is in.
#[tokio::test]
async fn a_created_legal_hold_is_listed_member_for_member() {
    for (case, request) in round_trip_cases() {
        // One router per row: each row's hold is the only one the account
        // holds, so a row cannot see what another row stored.
        let router = router();
        let (status, created) = create_legal_hold(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLegalHold 200, got {status} {created:?}"
        );
        for member in ["Title", "Description", "RecoveryPointSelection"] {
            assert_eq!(
                created.get(member),
                request.get(member),
                "case {case:?}: {member} must report what the create was given, got {created:?}"
            );
        }
        let id = string_at(&created, "LegalHoldId");
        let expected_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:legal-hold:{id}");
        assert_eq!(
            string_at(&created, "LegalHoldArn"),
            expected_arn,
            "case {case:?}: expected the synthesized legal-hold ARN, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "Status"),
            STATUS_CREATING,
            "case {case:?}: a fresh hold is still being placed, got {created:?}"
        );
        assert!(
            created["CreationDate"].is_number(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        let (status, page) = list_legal_holds(&router, "").await;
        let holds = listed(&page);
        assert_eq!(
            (status, holds.len()),
            (200, 1),
            "case {case:?}: expected the one stored hold listed, got {status} {page:?}"
        );
        let hold = &holds[0];
        for member in [
            "Title",
            "Status",
            "Description",
            "LegalHoldId",
            "LegalHoldArn",
            "CreationDate",
        ] {
            assert_eq!(
                hold.get(member),
                created.get(member),
                "case {case:?}: listed {member} differs from the created one, got {hold:?}"
            );
        }
        assert!(
            hold.get("CancellationDate").is_none(),
            "case {case:?}: a hold nobody cancelled has no CancellationDate, got {hold:?}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: a complete page carries no cursor, got {page:?}"
        );

        // A listing changes nothing a later reader sees beyond the activation
        // the clock owes: a second listing reports the same hold.
        let (status, again) = list_legal_holds(&router, "").await;
        assert_eq!(
            (status, listed(&again).len(), listed(&again)[0].get("LegalHoldArn")),
            (200, 1, hold.get("LegalHoldArn")),
            "case {case:?}: two listings must agree, got {again:?}"
        );

        // The hold reads back by id too, with the selection the create was
        // given — the member the list projection does not carry.
        let (status, read) = get_legal_hold(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLegalHold 200, got {status} {read:?}"
        );
        for member in [
            "Title",
            "Status",
            "Description",
            "LegalHoldId",
            "LegalHoldArn",
            "CreationDate",
            "RecoveryPointSelection",
        ] {
            assert_eq!(
                read.get(member),
                created.get(member),
                "case {case:?}: read {member} differs from the created one, got {read:?}"
            );
        }
        for member in ["CancellationDate", "CancelDescription", "RetainRecordUntil"] {
            assert!(
                read.get(member).is_none(),
                "case {case:?}: a hold nobody cancelled has no {member}, got {read:?}"
            );
        }
    }
}

/// A legal hold id this account holds nothing under is refused by the read,
/// with the sentence AWS states for the condition, and an id it does hold is
/// not.
#[tokio::test]
async fn a_read_of_an_unheld_legal_hold_id_is_refused() {
    let router = router();
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Held",
            "Description": "the one hold this account holds",
            "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLegalHold 200, got {status} {created:?}"
    );
    let id = string_at(&created, "LegalHoldId");

    // An id of another shape, an id of the right shape that was never minted,
    // and the held id with its last character changed.
    let mut neighbour: Vec<char> = id.chars().collect();
    let last = neighbour.len() - 1;
    neighbour[last] = if neighbour[last] == '0' { '1' } else { '0' };
    let neighbour: String = neighbour.into_iter().collect();
    for unheld in [
        "never-minted",
        "00000000-0000-0000-0000-000000000000",
        neighbour.as_str(),
    ] {
        let (status, missing) = get_legal_hold(&router, unheld).await;
        assert_eq!(
            (
                status,
                error_code(&missing).as_str(),
                refusal_message(&missing).as_str()
            ),
            (
                400,
                "InvalidParameterValueException",
                LEGAL_HOLD_NOT_FOUND,
            ),
            "id {unheld:?}: expected the read to be refused, got {status} {missing:?}"
        );
    }

    let (status, read) = get_legal_hold(&router, &id).await;
    assert_eq!(
        (status, read.get("LegalHoldArn")),
        (200, created.get("LegalHoldArn")),
        "the id the account does hold reads back, got {status} {read:?}"
    );
}

/// The activation a create starts is owed by the clock: the hold reports
/// `CREATING` straight after the call and `ACTIVE` once the span has passed,
/// and nothing else about it changes.
#[tokio::test]
async fn a_legal_hold_settles_from_creating_to_active() {
    let router = router();
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Settling",
            "Description": "watch the status move",
            "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLegalHold 200, got {status} {created:?}"
    );

    let (status, page) = list_legal_holds(&router, "").await;
    let placing = listed(&page);
    assert_eq!(
        (status, placing.len(), string_at(&placing[0], "Status")),
        (200, 1, STATUS_CREATING.to_string()),
        "expected the transient status, got {status} {page:?}"
    );

    std::thread::sleep(PAST_LEGAL_HOLD_SETTLE);

    let (status, page) = list_legal_holds(&router, "").await;
    let placed = listed(&page);
    assert_eq!(
        (status, placed.len(), string_at(&placed[0], "Status")),
        (200, 1, STATUS_ACTIVE.to_string()),
        "expected the hold to be in force, got {status} {page:?}"
    );
    for member in ["LegalHoldId", "LegalHoldArn", "Title", "CreationDate"] {
        assert_eq!(
            placed[0].get(member),
            placing[0].get(member),
            "settling must not change {member}, got {:?}",
            placed[0]
        );
    }
}

/// Walking the cursors of a paged listing visits every stored hold exactly
/// once, whatever the page size: the rows span page sizes shorter than, equal
/// to and longer than the number of stored holds.
#[tokio::test]
async fn paging_a_listing_visits_every_stored_hold_once() {
    let cases: [(usize, usize); 6] = [(1, 1), (3, 1), (3, 2), (4, 4), (5, 7), (9, 4)];

    for (count, page_size) in cases {
        let case = format!("{count} holds, pages of {page_size}");
        let router = router();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..count {
            let (status, hold) = create_legal_hold(
                &router,
                &json!({
                    "Title": format!("Hold {index}"),
                    "Description": format!("the {index}th hold of this account"),
                    "RecoveryPointSelection": {
                        "VaultNames": [format!("vault-{index}")],
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateLegalHold 200, got {status} {hold:?}"
            );
            created.insert(string_at(&hold, "LegalHoldId"));
        }
        assert_eq!(
            created.len(),
            count,
            "case {case:?}: every create must mint its own id, got {created:?}"
        );

        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        let mut pages = 0;
        loop {
            let (status, page) = list_legal_holds(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListLegalHolds 200, got {status} {page:?}"
            );
            let holds = listed(&page);
            assert!(
                holds.len() <= page_size,
                "case {case:?}: a page must hold at most MaxResults entries, got {holds:?}"
            );
            seen.extend(holds.iter().map(|hold| string_at(hold, "LegalHoldId")));
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {seen:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            (unique.len(), &unique),
            (seen.len(), &created),
            "case {case:?}: the pages must cover every stored hold exactly once, got {seen:?}"
        );
    }
}

/// A repeated `IdempotencyToken` replays the hold that token made: the same id
/// and ARN, and no second hold in the listing.
#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_first_legal_hold() {
    let router = router();
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Tokened",
            "Description": "created once",
            "IdempotencyToken": "token-1",
            "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the first CreateLegalHold 200, got {status} {created:?}"
    );

    // The same token with a DIFFERENT title: no action is taken, so the answer
    // is the hold the token already made.
    let (status, replayed) = create_legal_hold(
        &router,
        &json!({
            "Title": "Different",
            "Description": "a different hold entirely",
            "IdempotencyToken": "token-1",
            "RecoveryPointSelection": {"VaultNames": ["other-vault"]},
        }),
    )
    .await;
    assert_eq!(
        (status, &replayed),
        (200, &created),
        "a replayed token must answer with the hold it made, got {status} {replayed:?}"
    );

    let (status, page) = list_legal_holds(&router, "").await;
    assert_eq!(
        (status, listed(&page).len()),
        (200, 1),
        "a replayed token must create no second hold, got {status} {page:?}"
    );
}

/// A create carrying a member AWS does not accept is refused with the modelled
/// exception for that condition and stores nothing. The last element of a row is
/// the sentence AWS states for the condition, where this service's own handler
/// words it; `None` leaves the wording to the boundary validator and only
/// requires that the refusal states something.
#[tokio::test]
async fn a_create_outside_what_aws_accepts_is_refused_and_stores_nothing() {
    let cases: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "no Title member at all",
            json!({
                "Description": "a hold with no title",
                "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
            }),
            "MissingParameterValueException",
            None,
        ),
        (
            "no Description member at all",
            json!({
                "Title": "Untitled",
                "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
            }),
            "MissingParameterValueException",
            None,
        ),
        (
            "neither required member",
            json!({}),
            "MissingParameterValueException",
            None,
        ),
        (
            "no RecoveryPointSelection member at all",
            json!({"Title": "Unscoped", "Description": "a hold that holds nothing"}),
            "MissingParameterValueException",
            Some(MISSING_RESOURCE_SELECTION),
        ),
        (
            "a DateRange FromDate in the future",
            json!({
                "Title": "FutureFrom",
                "Description": "a window that has not started",
                "RecoveryPointSelection": {
                    "DateRange": {
                        "FromDate": epoch_seconds(Duration::from_secs(3_600), true),
                        "ToDate": days_ago(1),
                    },
                },
            }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "a DateRange ToDate in the future",
            json!({
                "Title": "FutureTo",
                "Description": "a window that has not ended",
                "RecoveryPointSelection": {
                    "DateRange": {
                        "FromDate": days_ago(2),
                        "ToDate": epoch_seconds(Duration::from_secs(86_400), true),
                    },
                },
            }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "a ResourceIdentifiers entry that is not an ARN",
            json!({
                "Title": "BadResource",
                "Description": "a resource is addressed by ARN",
                "RecoveryPointSelection": {"ResourceIdentifiers": ["vol-0abc"]},
            }),
            "InvalidParameterValueException",
            None,
        ),
    ];

    for (case, request, expected, sentence) in cases {
        let router = router();
        let (status, refused) = create_legal_hold(&router, &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {refused:?}"
        );
        let stated = refusal_message(&refused);
        match sentence {
            Some(expected_sentence) => assert_eq!(
                stated, expected_sentence,
                "case {case:?}: expected the sentence AWS states, got {stated:?}"
            ),
            None => assert!(
                !stated.is_empty(),
                "case {case:?}: the refusal states the condition, got {refused:?}"
            ),
        }

        let (status, page) = list_legal_holds(&router, "").await;
        assert_eq!(
            (status, listed(&page).len()),
            (200, 0),
            "case {case:?}: a refused create must store nothing, got {status} {page:?}"
        );
    }
}

/// A page size the modelled range admits is served; one outside it is refused,
/// and the refusal carries the modelled body a client reads `Code` out of.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = router();
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Paged",
            "Description": "one hold to page over",
            "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLegalHold 200, got {status} {created:?}"
    );

    for admitted in [MAX_RESULTS_MIN, 2, 500, MAX_RESULTS_MAX] {
        let (status, page) = list_legal_holds(&router, &format!("?maxResults={admitted}")).await;
        assert_eq!(
            (status, listed(&page).len()),
            (200, 1),
            "maxResults {admitted}: the model admits this page size, got {status} {page:?}"
        );
    }

    for refused_size in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, refused) =
            list_legal_holds(&router, &format!("?maxResults={refused_size}")).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "maxResults {refused_size}: expected the modelled refusal, got {status} {refused:?}"
        );
        assert!(
            !refusal_message(&refused).is_empty(),
            "maxResults {refused_size}: the refusal states the condition, got {refused:?}"
        );
    }
}

/// A cursor this result set never handed out names no page of it, and is
/// refused rather than silently answered from the first hold.
#[tokio::test]
async fn a_cursor_from_no_result_set_is_refused() {
    let router = router();
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Cursored",
            "Description": "one hold to page over",
            "RecoveryPointSelection": {"VaultNames": ["primary-vault"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLegalHold 200, got {status} {created:?}"
    );

    for token in ["not-a-token", "6e6f742d616e2d6964", ""] {
        let (status, refused) = list_legal_holds(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "token {token:?}: expected the modelled refusal, got {status} {refused:?}"
        );
    }
}
