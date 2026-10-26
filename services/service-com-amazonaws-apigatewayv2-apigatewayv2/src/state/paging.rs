// SPDX-License-Identifier: CC0-1.0

//! Pagination for this service's collection readers: the page size a request may ask
//! for, the opaque `NextToken` that resumes a walk, and the one page a reader serves.
//!
//! A module of its own under [`crate::state`], because this is the BOUND every
//! paginated operation depends on: a caller chooses both `MaxResults` and `NextToken`,
//! so the clamp that caps the work one call costs, and the parser that turns a value
//! this service did not issue into a refusal rather than a panic, are what a reader of
//! one of those handlers has to be able to check. [`crate::state`] re-exports every
//! public item here, so a handler reaches them the way it reaches the rest of this
//! service's state helpers.

use std::collections::BTreeMap;

/// The largest page either paginated operation serves, and the page size it uses when
/// the request names no `MaxResults`.
///
/// A caller-supplied `MaxResults` is clamped DOWN to it, so no single request can walk
/// and serialize the whole store: the work one call costs is bounded server-side, and
/// what does not fit is reported through `NextToken`.
pub const MAX_PAGE_SIZE: usize = 100;

/// Resolve a `MaxResults` query value into the page size to serve.
///
/// `MaxResults` is modelled as `__string` on these operations, so the numeric rule is
/// the handler's to enforce: a positive decimal integer. The result is clamped to
/// [`MAX_PAGE_SIZE`], which is also the page size an absent `MaxResults` gets, so the
/// page size is always bounded server-side rather than by what the caller asked for.
/// TOTAL over its input — every other string, however long or malformed, answers `Err`,
/// which carries the message for the operation's `BadRequestException`.
pub fn parse_max_results(raw: Option<&str>) -> Result<usize, String> {
    let Some(raw) = raw else {
        return Ok(MAX_PAGE_SIZE);
    };
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(usize::try_from(n).unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE)),
        _ => Err(format!(
            "Invalid value for MaxResults: '{raw}'. MaxResults must be a positive integer."
        )),
    }
}

/// Resolve a modelled-integer `MaxResults` into the page size to serve.
///
/// Several of this service's paginated operations model `MaxResults` as the `MaxResults`
/// integer shape (range 1..100), so the boundary validator has already refused a value
/// outside that window and there is nothing left to parse — only the server-side bound to
/// apply: the request's value clamped into `1..=`[`MAX_PAGE_SIZE`], and [`MAX_PAGE_SIZE`]
/// for a request that names none. TOTAL and monotone over its input, so a value a future
/// widening of the modelled range let through still costs one bounded page.
pub fn resolve_max_results(requested: Option<i32>) -> usize {
    let Some(asked) = requested else {
        return MAX_PAGE_SIZE;
    };
    // A negative page size is below the window, so it takes the window's floor the same
    // way zero does.
    usize::try_from(asked).unwrap_or(1).clamp(1, MAX_PAGE_SIZE)
}

/// The sentence a paginated reader refuses a `NextToken` it did not issue with.
///
/// A pagination token is OPAQUE: the service mints it and the caller echoes it back
/// unchanged, so a string that is not one of this service's tokens for the collection
/// being read names no position in it, and the request is not valid
/// (`BadRequestException`: "The request is not valid, for example, the input is
/// incomplete or incorrect"). API Gateway names the pagination cursor the POSITION in
/// the collection, and this is the one sentence it refuses a cursor it cannot read
/// with: it names nothing from the request, so the refusal a caller sees does not vary
/// with the token it sent.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid position parameter";

/// Separates the collection a token was issued for from the key its page resumes at.
/// A unit separator, so it cannot occur in either part.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// The collection a pagination token is issued for: `Stage`s of one api, `Api`s of the
/// account, and so on. Two readers of different collections never accept each other's
/// tokens, so `parts` is every name that identifies the collection — the resource path
/// of the collection being read.
pub fn page_scope(parts: &[&str]) -> String {
    parts.join("/")
}

/// Mint the opaque `NextToken` that resumes `scope` at `key`.
///
/// The token's CONTENT is the service's own: the model documents `NextToken` only as
/// "The next page of elements from this collection", so what a caller may do with one
/// is echo it back. Hex, so the token needs no escaping in a query string and carries
/// no resource name in the clear, and the collection travels with the key so a token
/// is only ever accepted by the reader that issued it.
fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("{scope}{PAGE_TOKEN_SEPARATOR}{key}"))
}

/// The longest a token this service minted can be, in bytes.
///
/// A minted token is hex over the collection's resource path, one separator and a key —
/// and every key this store holds is bounded (a `StageName` by
/// `crate::state::MAX_STAGE_NAME_LEN`, the others comparably), so a token past this
/// length is one this service cannot have issued. Checked BEFORE the decode, so the
/// work a rejected token costs does not grow with the length a caller chose for it.
const MAX_PAGE_TOKEN_LEN: usize = 2048;

/// The key a token issued for `scope` resumes at, or `Err` when this service did not
/// issue `token` for that collection.
///
/// TOTAL over its input: a token longer than any this service mints, one that is not
/// hex, one whose bytes are not UTF-8, one carrying no separator and one issued for
/// another collection all answer the same refusal, so a caller-supplied token can only
/// resume a walk or be refused — never panic and never widen the page.
fn decode_page_token(scope: &str, token: &str) -> Result<String, String> {
    if token.len() > MAX_PAGE_TOKEN_LEN {
        return Err(INVALID_NEXT_TOKEN_MESSAGE.to_string());
    }
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?;
    let (token_scope, key) = decoded
        .split_once(PAGE_TOKEN_SEPARATOR)
        .ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?;
    if token_scope != scope {
        return Err(INVALID_NEXT_TOKEN_MESSAGE.to_string());
    }
    Ok(key.to_string())
}

/// One page of the key-ordered collection `scope`.
///
/// Walks `items` in key order from where `next_token` left off (inclusive) and takes at
/// most `max_results`, so the page a request can obtain is capped by the page size its
/// handler resolved and is never the whole collection. The returned token resumes at the
/// first item that did NOT fit, so it is present only when more remain and resuming from
/// it continues exactly where this page stopped. `Err` carries the message for the
/// operation's `BadRequestException`: a token this service did not issue for `scope` is
/// refused rather than served as a page of its own.
pub fn page<'a, V>(
    items: &'a BTreeMap<String, V>,
    scope: &str,
    next_token: Option<&str>,
    max_results: usize,
) -> Result<(Vec<&'a V>, Option<String>), String> {
    let resume = match next_token {
        Some(token) => Some(decode_page_token(scope, token)?),
        None => None,
    };
    // `range` seeks straight to the resume key, so a full paginated walk costs one
    // traversal rather than one scan per page.
    let start = match &resume {
        Some(key) => ::std::ops::Bound::Included(key.as_str()),
        None => ::std::ops::Bound::Unbounded,
    };
    let mut rest = items.range::<str, _>((start, ::std::ops::Bound::Unbounded));
    let page: Vec<&V> = rest
        .by_ref()
        .take(max_results)
        .map(|(_, value)| value)
        .collect();
    let token = rest
        .next()
        .map(|(key, _)| encode_page_token(scope, key.as_str()));
    Ok((page, token))
}

#[cfg(test)]
mod tests {
    use super::{page, page_scope, parse_max_results, resolve_max_results, MAX_PAGE_SIZE};
    use std::collections::BTreeMap;

    /// A collection of `count` records, keyed the way the store keys its children.
    fn collection(count: usize) -> BTreeMap<String, usize> {
        (0..count)
            .map(|index| (format!("key{index:04}"), index))
            .collect()
    }

    #[test]
    fn a_page_size_a_caller_asks_for_is_clamped_to_the_server_side_maximum() {
        // (the `MaxResults` the request carries, the page size it resolves to)
        let rows = [
            (None, MAX_PAGE_SIZE),
            (Some("1"), 1),
            (Some("17"), 17),
            (Some("100"), 100),
            // A signed positive decimal, which `i64::from_str` and AWS's own integer
            // parse both accept.
            (Some("+5"), 5),
            (Some("101"), MAX_PAGE_SIZE),
            (Some("1000000"), MAX_PAGE_SIZE),
            // The widest decimal an `i64` holds: still the maximum, not what was asked.
            (Some("9223372036854775807"), MAX_PAGE_SIZE),
        ];
        for (raw, expected) in rows {
            let resolved = parse_max_results(raw);
            assert_eq!(
                resolved,
                Ok(expected),
                "case {raw:?}: expected the page size {expected}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_max_results_that_is_no_positive_integer_is_refused_rather_than_interpreted() {
        // The rows span the forms a caller can send: zero, negatives, other notations,
        // a number past `i64`, text, padding, emptiness and non-ASCII digits.
        let rows = [
            "0",
            "-1",
            "-100",
            "1.5",
            "1e3",
            "0x10",
            " 5",
            "5 ",
            "",
            "abc",
            "١٢٣",
            "99999999999999999999999999",
        ];
        for raw in rows {
            let resolved = parse_max_results(Some(raw));
            assert!(
                resolved.is_err(),
                "case {raw:?}: expected a refusal, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_modelled_integer_page_size_is_clamped_to_the_server_side_maximum() {
        // (the `MaxResults` the request carries, the page size it resolves to). The rows
        // span the modelled 1..100 window, its two bounds, what a request naming none
        // gets, and the values outside the window the validator refuses before a handler
        // ever sees them — which still resolve to a bounded page here.
        let rows = [
            (None, MAX_PAGE_SIZE),
            (Some(1), 1),
            (Some(17), 17),
            (Some(100), MAX_PAGE_SIZE),
            (Some(101), MAX_PAGE_SIZE),
            (Some(i32::MAX), MAX_PAGE_SIZE),
            (Some(0), 1),
            (Some(-1), 1),
            (Some(i32::MIN), 1),
        ];
        for (asked, expected) in rows {
            let resolved = resolve_max_results(asked);
            assert_eq!(
                resolved, expected,
                "case {asked:?}: expected the page size {expected}, got {resolved}"
            );
        }
    }

    #[test]
    fn a_page_carries_at_most_the_page_size_it_was_asked_for() {
        let items = collection(250);
        let scope = page_scope(&["apis", "a1", "routes"]);
        for max_results in [1_usize, 2, 7, 100] {
            let (records, token) =
                page(&items, &scope, None, max_results).expect("a first page needs no token");
            assert_eq!(
                records.len(),
                max_results,
                "case {max_results}: expected a page of {max_results}, got {}",
                records.len()
            );
            assert!(
                token.is_some(),
                "case {max_results}: expected a token while records remain"
            );
        }

        // A page size past the collection reports what there is and no token: the walk
        // is over, so there is nothing to resume.
        let (records, token) =
            page(&items, &scope, None, 1_000).expect("a first page needs no token");
        assert_eq!(
            records.len(),
            items.len(),
            "expected the whole collection, got {} of {}",
            records.len(),
            items.len()
        );
        assert_eq!(token, None, "expected no token at the end of the walk");
    }

    #[test]
    fn a_walk_resumed_by_its_own_token_reports_every_record_once() {
        let items = collection(45);
        let scope = page_scope(&["apis", "a1", "routes", "r1", "routeresponses"]);
        let mut seen: Vec<usize> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let (records, token) = page(&items, &scope, next_token.as_deref(), 7)
                .expect("the token is this walk's own");
            assert!(
                records.len() <= 7,
                "expected at most the page size, got {}",
                records.len()
            );
            seen.extend(records.into_iter().copied());
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        let expected: Vec<usize> = (0..items.len()).collect();
        assert_eq!(
            seen, expected,
            "expected every record exactly once in key order, got {seen:?}"
        );
    }

    #[test]
    fn a_token_this_service_did_not_mint_for_the_collection_is_refused() {
        let items = collection(10);
        let scope = page_scope(&["apis", "a1", "routes"]);
        let (_, issued) = page(&items, &scope, None, 1).expect("a first page needs no token");
        let issued = issued.expect("records remain, so a token was issued");

        // The rows span what can arrive in place of a token this collection issued:
        // another collection's token, text that is not hex, hex that is not UTF-8, hex
        // carrying no separator, a token longer than any this service mints, nothing.
        let other_scope = page_scope(&["apis", "a2", "routes"]);
        let rows: Vec<(&str, String)> = vec![
            ("another collection's own token", issued.clone()),
            ("text that is not hex", "not-a-token".to_string()),
            ("an odd number of hex digits", "abc".to_string()),
            ("hex whose bytes are not UTF-8", "ff".to_string()),
            ("hex carrying no separator", ::hex::encode("no separator")),
            ("a token past any this service mints", "61".repeat(4_096)),
            ("the empty token", String::new()),
        ];
        for (case, token) in rows {
            let refused = page(&items, &other_scope, Some(&token), 5);
            assert!(
                refused.is_err(),
                "case {case:?}: expected the refusal of a token this collection never \
                 issued, got a page"
            );
        }

        // The token IS accepted by the reader that issued it, so the refusals above are
        // about those tokens and not about tokens in general.
        let resumed = page(&items, &scope, Some(&issued), 5);
        assert!(
            resumed.is_ok(),
            "expected the issuing collection to accept its own token, got {resumed:?}"
        );
    }
}
