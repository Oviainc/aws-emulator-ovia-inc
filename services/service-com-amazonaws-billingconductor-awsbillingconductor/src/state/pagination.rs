// SPDX-License-Identifier: CC0-1.0

//! Paging one listing: the page size, the opaque `NextToken` this service mints and reads
//! back, and the cut over a sequence both halves agree on.

use super::*;

/// The page size the list operations apply when the request names none.
///
/// `ListAccountAssociations` is `@paginated` with a `NextToken` but models no
/// `MaxResults` at all, and `ListBillingGroups`'s `MaxResults` is optional, so in
/// both cases the SERVER chooses. This is the upper bound the service itself models
/// for that member (`MaxBillingGroupResults`, range 1..100), so one response is
/// bounded however much state exists, and a caller that wants more pages the
/// `NextToken` it already gets.
pub const DEFAULT_PAGE_SIZE: i32 = 100;

/// The input member every paginated read of this service carries its cursor in, as the
/// wire spells it — what a refusal about that cursor names in `Fields`.
pub const NEXT_TOKEN_MEMBER: &str = "NextToken";

/// The separator between the SCOPE a pagination token was minted for and the cursor it
/// carries, inside the bytes the token encodes.
///
/// A unit separator occurs in no scope ([`page_scope_within`] and [`page_scope_shaped`]
/// join with [`PAGE_SCOPE_SEPARATOR`] instead) and in none of the keys this service pages
/// over (ARNs, account ids, fixed-width indices), and only the FIRST one is read as the
/// separator, so a cursor is handed back exactly as it was minted.
pub(super) const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// The separator between the parts of a SCOPE that names more than the operation, distinct
/// from [`PAGE_TOKEN_SEPARATOR`] so the scope of a token is read back whole however many
/// parts it has.
pub(super) const PAGE_SCOPE_SEPARATOR: char = '\u{1e}';

/// How many bytes of the digest a token carries ahead of the value it encodes, so a token
/// a client truncated, re-typed or edited one character of no longer decodes at all.
///
/// A CHOSEN width: 64 bits is far past the chance of an edited token still agreeing with
/// its own digest, while keeping the token short.
pub(super) const PAGE_TOKEN_TAG_BYTES: usize = 8;

/// The `NextToken` a truncated page hands back: the hex encoding of an integrity tag over
/// the `scope` that minted it and the `cursor` the next page resumes from.
///
/// A pagination token is OPAQUE — the only thing a caller may do with one is hand it back,
/// unchanged, to the listing that returned it — so the cursor does not travel in the clear
/// as the store key it is, and the scope travels with it: a token decodes to a cursor only
/// under the scope it was minted for, which is what stops one listing's token from being a
/// cursor into another's sequence. See [`page_scope_within`] for the scope of a listing
/// read within one resource.
///
/// The tag is what makes a token that was not minted whole DECIDABLE rather than read as
/// some other cut point: a value that does not agree with its own tag was not produced
/// here, so it is refused instead of resuming the listing somewhere the caller never asked
/// for.
pub fn encode_next_token(scope: &str, cursor: &str) -> String {
    let payload = format!("{scope}{PAGE_TOKEN_SEPARATOR}{cursor}");
    let mut encoded = page_token_tag(payload.as_bytes());
    encoded.extend_from_slice(payload.as_bytes());
    ::hex::encode(encoded)
}

/// A CHOSEN ceiling on the characters a `NextToken` this service will even decode may
/// have. The model constrains `Token` not at all, so the value is caller-sized: the
/// longest token this service mints is an operation name, a resource ARN, the request
/// values that shape the sequence being read ([`page_scope_shaped`]) and a store key, all
/// far inside this, and anything longer was not minted here, so it is refused before its
/// length decides any work.
pub(super) const MAX_PAGE_TOKEN_LENGTH: usize = 1024;

/// The cursor `token` carries for the listing `scope`, or `None` when `token` is not one
/// this service minted for that listing — a value longer than any token it mints, one that
/// is not hex, is too short to carry a tag, does not agree with the tag it carries, does
/// not decode to UTF-8, carries no scope at all, carries a different sequence's scope, or
/// is spelled any way other than the ONE spelling [`encode_next_token`] writes.
///
/// Decoding the bytes is only the first half of reading a token: `::hex::decode` reads more
/// spellings than `::hex::encode` writes — it accepts `A-F` as well as the `a-f` the minter
/// emits — and the tag is computed over the DECODED payload, so it is identical for every
/// spelling of one cursor. A token is therefore kept only when it is, byte for byte, the
/// token this service would mint for the cursor it decodes to: re-minting is what compares
/// the two, so the accepted spelling is the ENCODER's rather than a second grammar
/// maintained beside it, and a re-cased copy of an issued token — a value no response of
/// this service ever carried, so not one the caller handed back unchanged — is refused with
/// [`invalid_next_token`] like every other value this service did not mint.
pub fn decode_next_token(scope: &str, token: &str) -> Option<String> {
    if token.len() > MAX_PAGE_TOKEN_LENGTH {
        return None;
    }
    let encoded = ::hex::decode(token).ok()?;
    if encoded.len() < PAGE_TOKEN_TAG_BYTES {
        return None;
    }
    let (tag, payload) = encoded.split_at(PAGE_TOKEN_TAG_BYTES);
    if tag != page_token_tag(payload).as_slice() {
        return None;
    }
    let decoded = std::str::from_utf8(payload).ok()?;
    let (minted_for, cursor) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    if minted_for != scope {
        return None;
    }
    (encode_next_token(scope, cursor) == token).then(|| cursor.to_string())
}

/// [`PAGE_TOKEN_TAG_BYTES`] bytes of the SHA-256 digest of a token's payload.
///
/// An integrity check over a value this service produced and the caller hands back
/// unchanged, not a secret: it tells a token that was minted whole from one that was not.
pub(super) fn page_token_tag(payload: &[u8]) -> Vec<u8> {
    use ::sha2::Digest;
    ::sha2::Sha256::digest(payload)[..PAGE_TOKEN_TAG_BYTES].to_vec()
}

/// The scope of the tokens a listing read WITHIN one resource mints: the operation and
/// that resource.
///
/// The scope identifies the SEQUENCE a cursor points into, and for these reads that
/// sequence belongs to one parent resource (a plan's rules, an item's versions, a group's
/// cost rows), so a cursor minted while paging one parent names no position in another's
/// sequence. The filters a request narrows a sequence with are not part of it: they select
/// which elements of the one sequence are reported, not which sequence is being read.
pub fn page_scope_within(operation: &str, resource_arn: &str) -> String {
    format!("{operation}{PAGE_SCOPE_SEPARATOR}{resource_arn}")
}

/// The scope of the tokens a read WITHIN one resource mints when the REQUEST, and not that
/// resource alone, decides which sequence is being read: the operation, the resource, and
/// each request value that shapes the sequence.
///
/// [`page_scope_within`] is the whole scope of a read whose request members only SELECT
/// elements of one fixed sequence. A member that decides what ONE element IS — which
/// attributes a reported group aggregates over, and so what the cursor key of an element
/// even spells, or which periods an element's amounts cover — names a DIFFERENT sequence
/// for every value it takes, and a cursor into one of those sequences names no position in
/// another, so it travels in the scope: a token minted under one shape is then refused with
/// [`invalid_next_token`], the answer every token this service did not mint for the
/// sequence being read gets, instead of being read as a cut point into a keyspace it names
/// nothing in.
///
/// Each shaping value is passed ALREADY CANONICAL — one spelling per sequence, and the
/// values that merely order or repeat within it normalised away — so two requests that name
/// the same sequence resume the same one however they spelled it.
pub fn page_scope_shaped(operation: &str, resource_arn: &str, shaping: &[&str]) -> String {
    let mut scope = page_scope_within(operation, resource_arn);
    for value in shaping {
        scope.push(PAGE_SCOPE_SEPARATOR);
        scope.push_str(value);
    }
    scope
}

/// The modelled refusal every paginated read of this service answers a `NextToken` it did
/// not mint with.
///
/// A token the service did not issue (or one a client corrupted) carries no page boundary
/// at all, so it is a fault in the INPUT rather than a listing to serve from an arbitrary
/// cut point: `ValidationExceptionReason` enumerates `CANNOT_PARSE` for exactly a value
/// the service cannot read
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`), so the answer is the
/// 400 `ValidationException` with that reason. ONE wording, built here, so a token refused
/// by a read whose cursor is a stored key reads exactly like one refused by a read whose
/// cursor is a position.
///
/// The message states the member and the requirement and NOTHING of the rejected token:
/// the token is caller input, and a refusal that echoed it would copy it into every client
/// log that records the failure.
pub fn invalid_next_token() -> crate::ops::types::ValidationException {
    validation_exception(
        "CANNOT_PARSE",
        NEXT_TOKEN_MEMBER,
        format!(
            "Value at '{NEXT_TOKEN_MEMBER}' failed to satisfy constraint: {NEXT_TOKEN_MEMBER} \
             must be a pagination token a previous page of this operation returned"
        ),
    )
}

/// Cut one page out of an ordered, already-filtered sequence of `(key, element)`.
///
/// The `NextToken` carries the key of the first element left out, so it is a cursor rather
/// than an offset: resuming skips every key below it. It is minted and read by
/// [`encode_next_token`] / [`decode_next_token`] under `scope` — the listing it belongs to
/// — so a token this service did not mint for THIS listing is refused with
/// [`invalid_next_token`] instead of being interpreted as a cut point. The page size is the
/// request's `max_results` CLAMPED to 1..=[`DEFAULT_PAGE_SIZE`] — a request that
/// names none, or names a value the modelled range does not allow, is served the
/// default — so a page is bounded whatever reaches this helper. An operation whose own
/// `MaxResults` range reaches past that default pages through
/// [`paginate_bounded`] instead.
pub fn paginate<T>(
    scope: &str,
    items: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), crate::ops::types::ValidationException> {
    paginate_bounded(scope, items, next_token, max_results, DEFAULT_PAGE_SIZE)
}

/// [`paginate`], for an operation whose own `MaxResults` member models a range the
/// default page size does not fit.
///
/// `page_size_bound` is the largest page that operation can be asked for — the maximum
/// of its modelled `MaxResults` range — and is also the size the server picks when the
/// request names none, so a caller that asks for a page the model allows is served it
/// whole and one that asks for nothing still gets a bounded page.
pub fn paginate_bounded<T>(
    scope: &str,
    items: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    page_size_bound: i32,
) -> Result<(Vec<T>, Option<String>), crate::ops::types::ValidationException> {
    let start = match next_token {
        Some(token) => Some(decode_next_token(scope, token).ok_or_else(invalid_next_token)?),
        None => None,
    };
    let mut page: Vec<(String, T)> = match start.as_deref() {
        Some(start) => items
            .into_iter()
            .filter(|(key, _)| key.as_str() >= start)
            .collect(),
        None => items,
    };
    let requested = max_results
        .filter(|max| *max > 0)
        .unwrap_or(page_size_bound)
        .min(page_size_bound);
    let limit = usize::try_from(requested).unwrap_or(1);
    let token = page
        .get(limit)
        .map(|(key, _)| encode_next_token(scope, key));
    page.truncate(limit);
    Ok((page.into_iter().map(|(_, element)| element).collect(), token))
}

/// The first value that appears more than once in `values`, if any.
pub fn first_duplicate(values: &[String]) -> Option<&String> {
    let mut seen = BTreeSet::new();
    values.iter().find(|value| !seen.insert(value.as_str()))
}
