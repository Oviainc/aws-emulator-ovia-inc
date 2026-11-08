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
/// the handler's to enforce: a positive decimal integer. A value that is not one is
/// refused with [`MAX_RESULTS_NOT_AN_INTEGER_MESSAGE`] rather than paged on a guess —
/// the sentence API Gateway answers a `maxResults` it cannot read a number out of, which
/// a value that reads as a number but names no page size (zero, a negative) takes too,
/// since the service words one refusal for the member and none of this service's
/// recordings shows a second. What is accepted is clamped to [`MAX_PAGE_SIZE`], which is
/// also the page size an absent `MaxResults` gets, so the page size is always bounded
/// server-side rather than by what the caller asked for.
/// TOTAL over its input — every other string, however long or malformed, answers `Err`,
/// which carries the message for the operation's `BadRequestException`.
pub fn parse_max_results(raw: Option<&str>) -> Result<usize, String> {
    let Some(raw) = raw else {
        return Ok(MAX_PAGE_SIZE);
    };
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(usize::try_from(n).unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE)),
        _ => Err(MAX_RESULTS_NOT_AN_INTEGER_MESSAGE.to_string()),
    }
}

/// The sentence a paginated reader refuses a `MaxResults` that is no page size it can
/// read with.
///
/// Fixed text: it does NOT echo the value the request sent, so the refusal a caller sees
/// does not vary with what it asked for, and a client matching on the sentence matches
/// every one of them. Held here once, so every collection reader of this service — each
/// of which resolves its page size through [`parse_max_results`] — refuses the same
/// value the same way.
pub const MAX_RESULTS_NOT_AN_INTEGER_MESSAGE: &str = "maxResults must be an integer.";

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

/// A record whose collection reader reports it in the order the account CREATED it,
/// rather than in the order of the identifier the service minted for it.
///
/// A minted identifier carries no ordering — it is drawn from a bijection over the id
/// space, so the identifier of the record created second may sort before the first's —
/// which is why a reader that has to report a collection oldest-first needs the record's
/// own position in that order.
pub trait CreatedInOrder {
    /// This record's position in the order its collection was created in. Strictly
    /// increasing within one collection, so no two records share a position and the
    /// order the reader walks is total.
    fn creation_order(&self) -> u64;
}

/// A BORROWED record is ordered by the record itself, so a reader that pages over a
/// FILTERED view of a collection — a map of references, which is the cheapest way to leave
/// records out without copying them — reports the same order the whole collection has.
impl<V: CreatedInOrder> CreatedInOrder for &V {
    fn creation_order(&self) -> u64 {
        (*self).creation_order()
    }
}

/// The position in `scope` a caller-supplied `NextToken` resumes at, or the refusal of a
/// token this service did not issue for that collection; `None` for a request carrying
/// none, which starts at the beginning.
///
/// Split from [`page_in_creation_order`] so a reader that walks more than one collection
/// resolves the position once and still refuses an unissued token whatever it goes on to
/// report.
pub fn page_position(scope: &str, next_token: Option<&str>) -> Result<Option<u64>, String> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    let key = decode_page_token(scope, token)?;
    // A token this service minted for a creation-ordered collection carries a decimal
    // position; anything else is not one of its own, however well-formed the rest is.
    key.parse::<u64>()
        .map(Some)
        .map_err(|_| INVALID_NEXT_TOKEN_MESSAGE.to_string())
}

/// Whether `token` is one this service issued for `scope`.
///
/// The check a reader with more than one collection to walk makes to decide WHICH of them
/// a token resumes: a token travels with the collection it was issued for (see
/// [`encode_page_token`]), so this answers that question without reading the position out
/// of it.
pub fn token_is_for_scope(scope: &str, token: &str) -> bool {
    decode_page_token(scope, token).is_ok()
}

/// The token that resumes `scope` at its FIRST record: the position before every
/// creation-ordered record there is.
///
/// What a reader hands back when the collection it has just finished is not the last one
/// its walk covers, so the caller's next request continues in the next collection rather
/// than starting the walk again.
pub fn first_page_token(scope: &str) -> String {
    encode_page_token(scope, &FIRST_POSITION.to_string())
}

/// The position [`first_page_token`] resumes at: below every record's, since a collection
/// counts the records it creates from one.
const FIRST_POSITION: u64 = 0;

/// One page of the collection `scope`, in the order its records were created.
///
/// Walks `items` oldest-first from `position` (exclusive of everything before it) and
/// takes at most `max_results`, so the page a request can obtain is capped by the page
/// size its handler resolved. The returned token resumes at the first record that did NOT
/// fit, so it is present only when more of THIS collection remains; a reader whose walk
/// covers another collection after this one decides what to report when it is absent.
///
/// The order is read off the records rather than off the map's keys, so one page costs a
/// sort of the collection — bounded, because every collection served this way is itself
/// bounded server-side (a store's growth bound, e.g. `crate::state::MAX_PORTAL_PRODUCTS`,
/// caps how many records there can be to sort).
pub fn page_in_creation_order<'a, V: CreatedInOrder>(
    items: &'a BTreeMap<String, V>,
    scope: &str,
    position: Option<u64>,
    max_results: usize,
) -> (Vec<&'a V>, Option<String>) {
    let mut ordered: Vec<&V> = items.values().collect();
    ordered.sort_by_key(|record| record.creation_order());
    let mut rest = ordered
        .into_iter()
        .skip_while(|record| position.is_some_and(|from| record.creation_order() < from));
    let page: Vec<&V> = rest.by_ref().take(max_results).collect();
    let token = rest
        .next()
        .map(|record| encode_page_token(scope, &record.creation_order().to_string()));
    (page, token)
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
    use super::{
        first_page_token, page, page_in_creation_order, page_position, page_scope,
        parse_max_results, resolve_max_results, token_is_for_scope, CreatedInOrder,
        MAX_PAGE_SIZE, MAX_RESULTS_NOT_AN_INTEGER_MESSAGE,
    };
    use std::collections::BTreeMap;

    /// A collection of `count` records, keyed the way the store keys its children.
    fn collection(count: usize) -> BTreeMap<String, usize> {
        (0..count)
            .map(|index| (format!("key{index:04}"), index))
            .collect()
    }

    /// A record of a creation-ordered collection: its position in the creation order, and
    /// the key it is stored under.
    #[derive(Debug, PartialEq, Eq)]
    struct Created {
        order: u64,
        key: String,
    }

    impl CreatedInOrder for Created {
        fn creation_order(&self) -> u64 {
            self.order
        }
    }

    /// A collection of `count` records whose KEY order is the REVERSE of their creation
    /// order, so a reader that walks keys and one that walks creation order cannot agree
    /// by accident.
    fn created_collection(count: u64) -> BTreeMap<String, Created> {
        (1..=count)
            .map(|order| {
                let key = format!("key{:04}", count - order);
                (
                    key.clone(),
                    Created {
                        order,
                        key: key.clone(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_creation_ordered_page_reports_the_oldest_record_first() {
        let items = created_collection(5);
        let scope = page_scope(&["portals"]);
        let (page, token) = page_in_creation_order(&items, &scope, None, 100);
        let seen: Vec<u64> = page.iter().map(|record| record.order).collect();
        assert_eq!(
            seen,
            vec![1, 2, 3, 4, 5],
            "expected the records oldest first whatever their keys sort like, got {seen:?}"
        );
        assert_eq!(
            token, None,
            "expected nothing left to resume at the end of the walk, got {token:?}"
        );
    }

    #[test]
    fn a_creation_ordered_walk_resumed_by_its_own_token_reports_every_record_once() {
        let items = created_collection(23);
        let scope = page_scope(&["portalproducts"]);
        for page_size in [1_usize, 2, 7, 23, 24] {
            let mut seen: Vec<u64> = Vec::new();
            let mut next_token: Option<String> = None;
            loop {
                let position = page_position(&scope, next_token.as_deref())
                    .expect("the token is this walk's own");
                let (records, token) =
                    page_in_creation_order(&items, &scope, position, page_size);
                assert!(
                    records.len() <= page_size,
                    "case {page_size}: expected at most the page size, got {}",
                    records.len()
                );
                seen.extend(records.iter().map(|record| record.order));
                match token {
                    Some(token) => next_token = Some(token),
                    None => break,
                }
            }
            let expected: Vec<u64> = (1..=23).collect();
            assert_eq!(
                seen, expected,
                "case {page_size}: expected every record once in creation order, got {seen:?}"
            );
        }
    }

    #[test]
    fn the_first_page_token_of_a_collection_resumes_it_at_its_oldest_record() {
        let items = created_collection(3);
        let scope = page_scope(&["portalproducts", "OTHER_ACCOUNTS"]);
        let token = first_page_token(&scope);
        assert!(
            token_is_for_scope(&scope, &token),
            "expected the collection to recognise its own first-page token, got {token:?}"
        );
        assert!(
            !token_is_for_scope(&page_scope(&["portalproducts"]), &token),
            "expected another collection not to recognise it, got {token:?}"
        );
        let position = page_position(&scope, Some(&token)).expect("its own token");
        let (records, _) = page_in_creation_order(&items, &scope, position, 100);
        let seen: Vec<&String> = records.iter().map(|record| &record.key).collect();
        assert_eq!(
            seen.len(),
            items.len(),
            "expected the whole collection from its first page, got {seen:?}"
        );
    }

    #[test]
    fn a_creation_ordered_reader_refuses_a_position_it_did_not_issue() {
        let scope = page_scope(&["portals"]);
        // The rows span what can arrive in place of a position this reader issued: text
        // that is not hex, hex carrying no separator, a token issued for another
        // collection, a token whose key is no position at all, and nothing.
        let other = page_scope(&["portalproducts"]);
        let rows: Vec<(&str, String)> = vec![
            ("text that is not hex", "not-a-token".to_string()),
            ("hex carrying no separator", ::hex::encode("no separator")),
            ("another collection's token", first_page_token(&other)),
            (
                "a key that is no position",
                ::hex::encode(format!("{scope}\u{1f}somewhere")),
            ),
            ("a negative position", ::hex::encode(format!("{scope}\u{1f}-1"))),
            ("the empty token", String::new()),
        ];
        for (case, token) in rows {
            let refused = page_position(&scope, Some(&token));
            assert!(
                refused.is_err(),
                "case {case:?}: expected the refusal of a position this reader never \
                 issued, got {refused:?}"
            );
        }
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
        // a number past `i64`, text, padding, emptiness and non-ASCII digits. Every one
        // takes the SAME sentence, which echoes none of them back.
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
            assert_eq!(
                resolved,
                Err(MAX_RESULTS_NOT_AN_INTEGER_MESSAGE.to_string()),
                "case {raw:?}: expected the refusal of a value that is no page size, got \
                 {resolved:?}"
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
