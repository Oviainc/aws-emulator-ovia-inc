// SPDX-License-Identifier: CC0-1.0

//! The two halves of the `ListServices` cursor rule, pinned against each other.
//!
//! The rule, in one sentence: a continuation token is a valid resume point only at
//! a position the listing's own token mint can issue — strictly inside the
//! collection being paged (past its first item, before its end) — and a decoded
//! cursor at any other position is refused exactly like a token the service never
//! issued, with the pagination-token `ValidationException`.
//!
//! Every row here is a token the test BUILDS in the handler's own cursor format,
//! so a row can name a position no issued token ever carries. That the format is
//! really the handler's is not assumed: [`forged_cursor`] is checked against a
//! token the service itself minted before any row runs, and every served row
//! asserts the `nextToken` it gets back is the forgery for the position the page
//! ended at.

use crate::wire::{build_router, call, list_page, list_services_uri, whole_catalog};
use serde_json::Value;

/// The cursor plaintext `ListServices` mints: the prefix and the zero-padded
/// 12-digit offset width of `PAGE_TOKEN_PREFIX` / `PAGE_TOKEN_OFFSET_DIGITS` in
/// `src/ops/list_services.rs`, transcribed so a suite can name a position the mint
/// would never reach. [`forged_cursor_matches_an_issued_token`] is what keeps the
/// transcription honest.
fn forged_cursor(offset: usize) -> String {
    aws_smithy_types::base64::encode(format!("uxc/service-catalog/offset={offset:012}"))
}

/// The pagination framework's refusal message for a continuation token that does
/// not decode as a cursor for this listing: a fixed template whose only variable
/// part is the operation name.
fn pagination_token_message(operation: &str) -> String {
    format!(
        "Pagination token exception in operation '{operation}': invalid payload wrapping schema 0"
    )
}

/// The modelled `NextToken` constraints — `@length(32, 512)` and
/// `@pattern ^[-A-Za-z0-9+/_]+=*$`
/// (`smithy_query(query="operation", operation="ListServices")`) — asserted on
/// every row, so no case can accidentally exercise the member validator instead of
/// the cursor guard.
fn assert_well_formed_token(case: &str, token: &str) {
    assert!(
        (32..=512).contains(&token.len()),
        "case {case:?}: a row must satisfy @length(32, 512), got {} chars ({token:?})",
        token.len()
    );
    let (body, padding) = match token.find('=') {
        Some(at) => token.split_at(at),
        None => (token, ""),
    };
    let charset_ok = body
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'/' | b'_'));
    assert!(
        !body.is_empty() && charset_ok && padding.bytes().all(|b| b == b'='),
        "case {case:?}: a row must satisfy @pattern ^[-A-Za-z0-9+/_]+=*$, got {token:?}"
    );
}

/// The forging format is the handler's own: the token the service MINTS for the
/// position a one-item first page ends at is byte-for-byte the forgery for that
/// position. Without this, a "refused" row below could be passing for the wrong
/// reason (a prefix or width the handler never recognises).
#[tokio::test]
async fn forged_cursor_matches_an_issued_token() {
    let router = build_router();

    for size in [1_usize, 2, 7, 50] {
        let (page, token) = list_page(&router, Some(size as i64), None).await;
        let issued = token.unwrap_or_else(|| {
            panic!("case maxResults={size}: a first page of the catalog must carry a nextToken")
        });
        assert_eq!(
            issued,
            forged_cursor(page.len()),
            "case maxResults={size}: the issued token must be the cursor for position {}, got \
             {issued:?}",
            page.len()
        );
    }
}

/// The admitted half of the rule: a cursor naming a position the mint can issue
/// resumes the listing there — serving the catalog's slice from that position and
/// carrying the cursor for the position the page ends at.
///
/// Rows span the resume positions a listing of this size has: its first, the one
/// after it, the middle, and the last one the mint can ever issue — each walked at
/// several page sizes, including a page that runs off the end of the catalog.
#[tokio::test]
async fn a_cursor_at_a_mintable_position_resumes_the_listing() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;
    let len = catalog.len();
    assert!(
        len > 4,
        "the seeded catalog must hold enough entries to page over, got {len}"
    );

    // (case, the position the cursor names) — every one inside 1..len, which is
    // what the mint can issue.
    let rows: Vec<(String, usize)> = vec![
        ("the first mintable position".to_string(), 1),
        ("the second mintable position".to_string(), 2),
        ("the middle of the catalog".to_string(), len / 2),
        ("the last mintable position".to_string(), len - 1),
    ];

    for (case, offset) in rows {
        let token = forged_cursor(offset);
        assert_well_formed_token(&case, &token);

        for size in [None, Some(1_i64), Some(3), Some(1000)] {
            let (page, next) = list_page(&router, size, Some(&token)).await;
            let page_size = size.map_or(500_usize, |max| max as usize).min(500);
            let end = (offset + page_size).min(len);
            assert_eq!(
                page,
                catalog[offset..end].to_vec(),
                "case {case:?} (offset {offset}, maxResults={size:?}): expected the catalog slice \
                 {offset}..{end}, got {page:?}"
            );
            let expected_next = (end < len).then(|| forged_cursor(end));
            assert_eq!(
                next, expected_next,
                "case {case:?} (offset {offset}, maxResults={size:?}): expected the cursor for \
                 position {end} ({expected_next:?}), got {next:?}"
            );
        }
    }
}

/// The refused half of the same rule: a cursor that decodes but names a position
/// the mint can NEVER issue is answered HTTP 400 `ValidationException` with the
/// pagination-token message and no `fieldList`, just like a token this service
/// never issued at all.
///
/// Rows span the positions outside the mintable range in both directions: the zero
/// position (no page ever ends there, so no token ever names it), the position one
/// past the catalog's last entry — the boundary the guard and the mint used to
/// disagree at — the one after that, a position far beyond the catalog, and the
/// largest the 12-digit offset width can carry.
#[tokio::test]
async fn a_cursor_at_a_position_the_mint_cannot_issue_is_refused() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;
    let len = catalog.len();
    let expected_message = pagination_token_message("ListServices");

    // (case, the position the cursor names) — every one outside 1..len.
    let rows: Vec<(String, usize)> = vec![
        ("the position before the first item".to_string(), 0),
        ("one position past the catalog's last entry".to_string(), len),
        ("two positions past the catalog's last entry".to_string(), len + 1),
        ("far beyond the catalog".to_string(), len * 10 + 7),
        ("the largest offset the token width carries".to_string(), 999_999_999_999),
    ];

    for (case, offset) in rows {
        let token = forged_cursor(offset);
        assert_well_formed_token(&case, &token);

        // Both with and without a page size: the refusal does not depend on it.
        for max_results in [None, Some(1_i64), Some(1000)] {
            let uri = list_services_uri(max_results, Some(&token));
            let (status, error_type, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {case:?} (offset {offset}, maxResults={max_results:?}): expected HTTP 400, \
                 got {status} ({body})"
            );
            assert_eq!(
                error_type, "ValidationException",
                "case {case:?} (offset {offset}, maxResults={max_results:?}): expected a \
                 ValidationException, got {error_type:?} ({body})"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(expected_message.as_str()),
                "case {case:?} (offset {offset}, maxResults={max_results:?}): expected the \
                 pagination-token exception message, got {body:?}"
            );
            assert!(
                body.get("fieldList").is_none(),
                "case {case:?} (offset {offset}, maxResults={max_results:?}): a cursor-decode \
                 failure names no member, got {body:?}"
            );
        }
    }
}

/// Sending NO token is still the start of the listing — the refusal above is about
/// a cursor naming the zero position, not about the absence of a cursor — and the
/// refused cursors change nothing a later caller sees.
#[tokio::test]
async fn an_absent_token_still_starts_the_listing_and_a_refusal_changes_nothing() {
    let router = build_router();
    let before = whole_catalog(&router).await;

    for offset in [0_usize, before.len(), before.len() + 3] {
        let uri = list_services_uri(None, Some(&forged_cursor(offset)));
        let (status, _, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "offset {offset}: expected HTTP 400 before the control read, got {status} ({body})"
        );
    }

    let (page, next) = list_page(&router, None, None).await;
    assert_eq!(
        page, before,
        "a request naming no token must still serve the whole catalog, got {} entries",
        page.len()
    );
    assert!(
        next.is_none(),
        "an unsized page holding the whole catalog must carry no nextToken, got {next:?}"
    );
}
