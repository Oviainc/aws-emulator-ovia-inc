// SPDX-License-Identifier: CC0-1.0

//! `ListInvitations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `ListInvitations` reports exactly the invitations the current account's inbox
//!    holds — every one delivered to it and nothing else — in inviting-account-id
//!    order, each projected with every member the `Invitation` shape carries
//!    (`smithy_query(shape="Invitation")`): the inviting account, the invitation id,
//!    the relationship status the pending invitation is in, and the instant it was
//!    sent as text.
//! 2. It is PAGINATED (`smithy_query(operation="ListInvitations")` reports
//!    `inputToken`/`outputToken` `NextToken`, `items` `Invitations`, `pageSize`
//!    `MaxResults`): walking the pages with any page size yields each invitation
//!    exactly once in the unpaged order, the last page carries no `NextToken`, and no
//!    page is longer than `Invitations`'s own `len 0..50` whatever the caller asks
//!    for.
//! 3. It REPORTS: listing changes nothing a later caller sees — the invitations it
//!    names stay PENDING, so an acceptance afterwards still succeeds, and only that
//!    acceptance takes an invitation out of the inbox the next list reports.
//! 4. The operation names no detector, so the inbox it reports is the one the Region
//!    the request was signed for serves: another Region's endpoint reports its own
//!    inbox, and an endpoint holding none reports an empty list rather than a
//!    failure.
//! 5. A `nextToken` this service never issued is refused with HTTP 400 under the
//!    `InvalidInputException` class naming the member at fault and echoing no value,
//!    and a `maxResults` outside the modelled `range 1..50` is refused under the
//!    modelled `BadRequestException` — never a hollow page.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), so a row can hold an
//! invitation from any account id in the space rather than only from the one account
//! id this emulator presents, which is the account `InviteMembers` delivers an
//! invitation from; the delivery itself is the `invitation_delivery` suite's subject.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state;
use crate::client_rejections::page_token_refusal;

use crate::detector_lifecycle::{call_in, error_code, error_shape, router_over, Router};
use crate::negative_class_rejections::seed_invitation;

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE a 400 is enveloped in, as `x-amzn-errortype` carries it —
/// the only client error any operation of this service declares.
const BAD_REQUEST: &str = "BadRequestException";

/// `Invitations` is `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListInvitations")`), so one page carries at most 50
/// invitations however many the inbox holds.
const INVITATIONS_MAX: usize = 50;

/// `Invitation.RelationshipStatus` for an invitation that is pending — the status the
/// inbox holds one in until it is answered.
const INVITED: &str = "Invited";

/// The Region most rows use.
const REGION: &str = "us-east-1";

/// One seeded invitation: the inviting account, the invitation id, the instant it was
/// sent in epoch milliseconds, and the text `invitedAt` must report it as.
///
/// The expected text is hand-computed from the epoch second rather than taken from the
/// emulator, so the assertion is against the ISO-8601 instant the member means and not
/// against the renderer: 0 is the epoch itself, 946_684_800 is 2000-01-01T00:00:00Z,
/// 1_000_000_000 is 2001-09-09T01:46:40Z and 1_700_000_000 is 2023-11-14T22:13:20Z.
type Seed = (&'static str, &'static str, i64, &'static str);

/// Inboxes to list, each row a SET of invitations differing along what the rule
/// generalises over: the COUNT the inbox holds, the inviting account id across the
/// 12-digit `AccountId` space (`len 12..12` — `smithy_query(shape="AccountId")`), the
/// shape of the invitation id (modelled as a plain `String`), the instant the
/// invitation was sent, and the ORDER the invitations were delivered in, which is not
/// the order a list reports.
fn inbox_rows() -> Vec<(&'static str, Vec<Seed>)> {
    vec![
        ("an inbox holding no invitation", Vec::new()),
        (
            "a single invitation, sent at the epoch instant",
            vec![(
                "111122223333",
                "d4b94fc952d6912b8f3060768220e2e7",
                0,
                "1970-01-01T00:00:00.000Z",
            )],
        ),
        (
            "three invitations delivered in an order that is not account-id order",
            vec![
                (
                    "999999999999",
                    "11111111-2222-3333-4444-555555555555",
                    1_700_000_000_123,
                    "2023-11-14T22:13:20.123Z",
                ),
                ("111122223333", "i", 946_684_800_500, "2000-01-01T00:00:00.500Z"),
                (
                    "555566667777",
                    "0123456789abcdef0123456789abcdef",
                    1_000_000_000_001,
                    "2001-09-09T01:46:40.001Z",
                ),
            ],
        ),
        (
            "both ends of the 12-digit account-id space, and one in between",
            vec![
                (
                    "999999999999",
                    "beef0123456789abcdef0123456789ab",
                    1_700_000_000_000,
                    "2023-11-14T22:13:20.000Z",
                ),
                (
                    "000000000000",
                    "00000000-0000-0000-0000-000000000000",
                    0,
                    "1970-01-01T00:00:00.000Z",
                ),
                (
                    "123456789012",
                    "a-longer-invitation-id-with-dashes",
                    1_000_000_000_001,
                    "2001-09-09T01:46:40.001Z",
                ),
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: the list reports exactly the stored inbox, fully projected, in order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_every_invitation_the_inbox_holds_in_order() {
    for (case, seeds) in inbox_rows() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;

        // Before any invitation the list is empty — not absent, not canned, and not
        // a 501.
        let reported = page_invitations(&router, REGION, "", case).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: an inbox with no invitation must report an empty list, got {reported:?}"
        );

        for (account_id, invitation_id, millis, _) in &seeds {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }

        // Inviting-account-id order over every seeded invitation, whatever order they
        // were delivered in, each carrying every member the shape models.
        let mut expected: Vec<Seed> = seeds.clone();
        expected.sort_by_key(|(account_id, _, _, _)| *account_id);
        let expected: Vec<Value> = expected
            .iter()
            .map(|(account_id, invitation_id, _, invited_at)| {
                json!({
                    "accountId": account_id,
                    "invitationId": invitation_id,
                    "relationshipStatus": INVITED,
                    "invitedAt": invited_at,
                })
            })
            .collect();

        let reported = page_invitations(&router, REGION, "", case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stored inbox in account-id order {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: paging yields each invitation exactly once, in the unpaged order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_invitation_exactly_once_in_order() {
    // A FULL inbox: `MAX_INVITATIONS_PER_DETECTOR` is the ceiling the store holds, and
    // `Invitations` carries at most 50 in one page, so this is the largest list the
    // operation can report — every page size below is exercised on it.
    let full: Vec<(String, String, i64)> = (0..INVITATIONS_MAX)
        .map(|index| {
            (
                format!("{:012}", 100_000_000_000_u64 + index as u64 * 7),
                format!("invitation-{index:04}"),
                1_700_000_000_000 + index as i64 * 1_000,
            )
        })
        .collect();

    // And an inbox whose size is not a multiple of most page sizes, so a short last
    // page is covered too.
    let few: Vec<(String, String, i64)> = (0..7)
        .map(|index| {
            (
                format!("{:012}", 999_999_999_999_u64 - index as u64 * 11),
                format!("few-{index}"),
                946_684_800_000 + index as i64 * 250,
            )
        })
        .collect();

    for (case, seeds) in [("a full inbox", &full), ("an inbox of seven", &few)] {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;
        for (account_id, invitation_id, millis) in seeds {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }

        let mut expected: Vec<String> = seeds
            .iter()
            .map(|(account_id, _, _)| account_id.clone())
            .collect();
        expected.sort();

        // Every page size the model allows (`range 1..50`, both published bounds
        // included) plus the service's own page size when the caller names none.
        let mut page_sizes: Vec<Option<i64>> = (1..=INVITATIONS_MAX as i64).map(Some).collect();
        page_sizes.push(None);

        for size in page_sizes {
            let walked = match size {
                Some(size) => format!("{case}, maxResults={size}"),
                None => format!("{case}, no maxResults"),
            };
            let (reported, page_lengths) = walk_pages(&router, REGION, size, &walked).await;
            let accounts: Vec<String> = reported
                .iter()
                .map(|invitation| {
                    invitation["accountId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {walked:?}: expected an accountId, got {invitation:?}")
                        })
                        .to_string()
                })
                .collect();
            assert_eq!(
                accounts, expected,
                "case {walked:?}: paging must yield every invitation exactly once in order, got {accounts:?}"
            );

            // Every page but the last is full: that is what makes a token walk
            // terminate in the number of calls a client budgets for.
            let per_page = size
                .unwrap_or(INVITATIONS_MAX as i64)
                .min(INVITATIONS_MAX as i64) as usize;
            for (index, length) in page_lengths.iter().enumerate() {
                if index + 1 < page_lengths.len() {
                    assert_eq!(
                        *length, per_page,
                        "case {walked:?}: page {index} must be full at {per_page}, got {length}"
                    );
                } else {
                    assert!(
                        *length <= per_page,
                        "case {walked:?}: the last page must carry at most {per_page} invitations, got {length}"
                    );
                }
            }
        }

        // A page that covers the whole inbox carries no token at all.
        for size in [seeds.len() as i64, INVITATIONS_MAX as i64] {
            let complete = format!("{case}: a complete page at maxResults={size}");
            let (status, body) =
                list_invitations(&router, REGION, &format!("?maxResults={size}")).await;
            assert_eq!(
                status, 200,
                "case {complete:?}: expected 200, got {status} {body:?}"
            );
            assert!(
                body.get("nextToken").is_none_or(Value::is_null),
                "case {complete:?}: a complete page must carry no nextToken, got {:?}",
                body["nextToken"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: listing reports; only an acceptance empties the inbox.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, REGION).await;

    let seeds: Vec<Seed> = vec![
        (
            "111122223333",
            "d4b94fc952d6912b8f3060768220e2e7",
            1_700_000_000_123,
            "2023-11-14T22:13:20.123Z",
        ),
        (
            "222233334444",
            "11111111-2222-3333-4444-555555555555",
            946_684_800_500,
            "2000-01-01T00:00:00.500Z",
        ),
        (
            "333344445555",
            "i",
            1_000_000_000_001,
            "2001-09-09T01:46:40.001Z",
        ),
    ];
    for (account_id, invitation_id, millis, _) in &seeds {
        seed_invitation(
            &engine,
            REGION,
            &detector_id,
            account_id,
            invitation_id,
            *millis,
        );
    }

    let first = page_invitations(&router, REGION, "", "the first list").await;
    // Several lists, some of them paged, in between.
    for size in [1, 2, 3] {
        let case = format!("an interleaved page at maxResults={size}");
        walk_pages(&router, REGION, Some(size), &case).await;
    }
    let again = page_invitations(&router, REGION, "", "the list after the walks").await;
    assert_eq!(
        again, first,
        "case \"a repeated list\": listing must not change what a later list sees, got {again:?} after {first:?}"
    );

    // Nor does it answer an invitation: every one it named is still PENDING, so an
    // acceptance afterwards succeeds — and only that acceptance takes it out of the
    // inbox the next list reports.
    for (index, (account_id, invitation_id, _, _)) in seeds.iter().enumerate() {
        let case = format!("accepting {invitation_id:?} from {account_id}");
        let (status, body) = call_in(
            &router,
            REGION,
            "POST",
            &format!("/detector/{detector_id}/administrator"),
            Some(json!({"administratorId": account_id, "invitationId": invitation_id})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the invitation the list named must still be pending, got {status} {body:?}"
        );

        let remaining = page_invitations(&router, REGION, "", &case).await;
        let accounts: Vec<&str> = remaining
            .iter()
            .filter_map(|invitation| invitation["accountId"].as_str())
            .collect();
        let expected: Vec<&str> = seeds
            .iter()
            .skip(index + 1)
            .map(|(account_id, _, _, _)| *account_id)
            .collect();
        assert_eq!(
            accounts, expected,
            "case {case:?}: an accepted invitation must leave the inbox and the rest stay, got {accounts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the inbox is the one the request's Region serves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_the_inbox_of_the_region_the_request_was_signed_for() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));

    // One inbox per Region, with invitations of their own. The rows differ along the
    // Region and along how many invitations its inbox holds.
    let rows: [(&str, &[(&str, &str)]); 3] = [
        (
            "us-east-1",
            &[
                ("111122223333", "home-one"),
                ("222233334444", "home-two"),
            ],
        ),
        ("eu-west-1", &[("999999999999", "away-one")]),
        ("ap-southeast-2", &[]),
    ];

    for (region, seeds) in rows {
        let detector_id = enabled_detector_in(&router, region).await;
        for (account_id, invitation_id) in seeds {
            seed_invitation(
                &engine,
                region,
                &detector_id,
                account_id,
                invitation_id,
                1_700_000_000_000,
            );
        }
    }

    for (region, seeds) in rows {
        let case = format!("the {region} inbox");
        let reported = page_invitations(&router, region, "", &case).await;
        let listed: Vec<(&str, &str)> = reported
            .iter()
            .map(|invitation| {
                (
                    invitation["accountId"].as_str().unwrap_or_default(),
                    invitation["invitationId"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let mut expected: Vec<(&str, &str)> = seeds.to_vec();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected only this Region's invitations, got {listed:?}"
        );
    }

    // A Region's endpoint that holds no detector at all reports an empty list: the
    // operation names no detector, so there is nothing for it to refuse.
    let case = "a Region holding no detector";
    let reported = page_invitations(&router, "us-west-2", "", case).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected an empty list, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: an unissued token and a maxResults outside the range are refusals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unissued_token_or_an_out_of_range_page_size_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, REGION).await;
    let seeds = [
        ("111122223333", "refusal-one"),
        ("222233334444", "refusal-two"),
    ];
    for (account_id, invitation_id) in seeds {
        seed_invitation(
            &engine,
            REGION,
            &detector_id,
            account_id,
            invitation_id,
            1_700_000_000_000,
        );
    }

    // Each row: the case and the `nextToken` the list carries — a value that is not a
    // token this service issues, one past the end of the list, the entry just past the
    // last one, and a negative offset.
    let token_rows: [(&str, &str); 4] = [
        ("a nextToken that is not a token this service issues", "not-a-token"),
        ("a nextToken pointing far past the end of the list", "9999"),
        ("a nextToken pointing at the entry just past the last one", "2"),
        ("a negative offset, which is no token at all", "-1"),
    ];

    for (case, token) in token_rows {
        let (status, body) = list_invitations(&router, REGION, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            BAD_REQUEST,
            "case {case:?}: expected the modelled envelope shape, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        // A page token this service did not issue is refused with the ONE sentence
        // the whole pagination surface answers, which names no member at all.
        assert_eq!(
            message,
            page_token_refusal(),
            "case {case:?}: expected the page-token refusal, got {message:?}"
        );
        assert!(
            !message.contains(token),
            "case {case:?}: a refusal must not echo the caller's value {token:?}, got {message:?}"
        );
        // And the refusal leaves the inbox alone.
        let reported = page_invitations(&router, REGION, "", case).await;
        assert_eq!(
            reported.len(),
            seeds.len(),
            "case {case:?}: a refusal must not change the inbox, got {reported:?}"
        );
    }

    // `MaxResults` is `range 1..50`, so each published bound holds and one past either
    // end is refused under the one client error the model declares.
    for size in [0, 51, 1_000] {
        let case = format!("maxResults={size}, outside the modelled range");
        let (status, body) = list_invitations(&router, REGION, &format!("?maxResults={size}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            BAD_REQUEST,
            "case {case:?}: expected the modelled error shape, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            state::INVALID_INPUT_PARAMETERS_MESSAGE,
            "case {case:?}: expected GuardDuty's fixed constraint-refusal text, got {message:?}"
        );
    }
    for size in [1, 50] {
        let case = format!("maxResults={size}, a published bound");
        let (status, body) = list_invitations(&router, REGION, &format!("?maxResults={size}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a published bound must be accepted, got {status} {body:?}"
        );
    }

    // The token the service DOES issue is accepted on the same list: the refusals
    // above are about tokens it never handed out, not about paging itself.
    let (status, body) = list_invitations(&router, REGION, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} {body:?}"
    );
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the first page\": expected a nextToken, got {body:?}"))
        .to_string();
    let second = page_invitations(
        &router,
        REGION,
        &format!("?maxResults=1&nextToken={token}"),
        "the issued token",
    )
    .await;
    let accounts: Vec<&str> = second
        .iter()
        .filter_map(|invitation| invitation["accountId"].as_str())
        .collect();
    assert_eq!(
        accounts,
        vec!["222233334444"],
        "case \"the issued token\": expected the second page, got {accounts:?}"
    );
}

// ---------------------------------------------------------------------------
// The calls and the shared helpers.
// ---------------------------------------------------------------------------

/// One `ListInvitations` call — `GET /invitation`
/// (`smithy_query(operation="ListInvitations")`). `query` is the raw query string
/// (`""` for none).
async fn list_invitations(router: &Router, region: &str, query: &str) -> (u16, Value) {
    call_in(router, region, "GET", &format!("/invitation{query}"), None).await
}

/// The `Invitations` of one page, failing the test if the list did not succeed.
pub(crate) async fn page_invitations(
    router: &Router,
    region: &str,
    query: &str,
    case: &str,
) -> Vec<Value> {
    let (status, body) = list_invitations(router, region, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-invitations{query} expected 200, got {status} {body:?}"
    );
    invitations_in(&body, case)
}

fn invitations_in(body: &Value, case: &str) -> Vec<Value> {
    body["invitations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an invitations array, got {body:?}"))
        .clone()
}

/// Walk every page with `page_size` (`None` asks for the service's own page size) and
/// return the invitations in the order the pages reported them, plus each page's
/// length.
async fn walk_pages(
    router: &Router,
    region: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<Value>, Vec<usize>) {
    let mut reported = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push_str(if query.is_empty() { "?" } else { "&" });
            query.push_str(&format!("nextToken={token}"));
        }
        let (status, body) = list_invitations(router, region, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-invitations{query} expected 200, got {status} {body:?}"
        );
        let page = invitations_in(&body, case);
        assert!(
            page.len() <= INVITATIONS_MAX,
            "case {case:?}: a page must carry at most {INVITATIONS_MAX} invitations, got {}",
            page.len()
        );
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                page.len()
            );
        }
        page_lengths.push(page.len());
        reported.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (reported, page_lengths),
        }
    }
    panic!("case {case:?}: list-invitations never ran out of pages");
}

/// Create an enabled detector through `region`'s endpoint and return its id — the
/// inbox the seeds are written into hangs off it.
pub(crate) async fn enabled_detector_in(router: &Router, region: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in {region}: expected 200, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector in {region}: got {body:?}"))
        .to_string()
}
