// SPDX-License-Identifier: CC0-1.0

//! How a paginated read serves one page: the page a catalog listing and a
//! resource listing cut, and the sentence an unusable page token is refused
//! with.

use super::*;

/// One page of `reported`: at most `page_size` entries from the one `page_token`
/// names onward, and the id of the first entry of the page after it — present
/// only while entries remain.
///
/// A page token NAMES an entry (through `id_of`) rather than carrying an index,
/// so a catalog that gained or lost an entry between two pages cannot make the
/// walk skip or repeat another one. `None` is the answer when the token names no
/// entry of `reported`: there is no page to report, which the caller turns into a
/// refusal. An absent token asks for the first page.
///
/// `id_of` answers a [`std::borrow::Cow`], so an entry whose token is a member it
/// already holds lends it (`Cow::Borrowed`) while one whose token is DERIVED from a
/// member can compute it (`Cow::Owned`) — one walk serves both.
///
/// Shared by every listing this service pages (blueprints, instance bundles, the
/// HTTPS setup history), so each one walks its records exactly the same way.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token
/// is only ever COMPARED against each entry's id, the start comes from
/// `position`, so it is an index this slice holds, the page is taken by
/// `skip`/`take` rather than by a slice range, and the next page's first index is
/// a `saturating_add` — so there is no indexing, no slicing, no unwrap and no
/// arithmetic that can overflow or panic, and a token naming no entry answers
/// `None`.
pub(super) fn catalog_page<T: Clone>(
    reported: &[T],
    page_size: usize,
    page_token: Option<&str>,
    id_of: impl Fn(&T) -> Option<std::borrow::Cow<'_, str>>,
) -> Option<(Vec<T>, Option<String>)> {
    // A page carries at least one entry, so a walk over a non-empty catalog
    // always advances.
    let page_size = page_size.max(1);
    let start = match page_token {
        None => 0,
        Some(token) => reported
            .iter()
            .position(|entry| id_of(entry).as_deref() == Some(token))?,
    };
    let page = reported
        .iter()
        .skip(start)
        .take(page_size)
        .cloned()
        .collect::<Vec<_>>();
    let next_page_token = reported
        .get(start.saturating_add(page_size))
        .and_then(|entry| id_of(entry).map(std::borrow::Cow::into_owned));
    Some((page, next_page_token))
}

// ---------------------------------------------------------------------------
// Resource paging
// ---------------------------------------------------------------------------

/// The records of a name-keyed map from the one `page_token` names onward, in the
/// map's own total order — where a page of a stored-resource listing starts.
///
/// A page token NAMES a record rather than carrying an index, so a record created
/// or deleted between two pages cannot make the walk skip or repeat another one.
/// No token asks for the first page; whether the token names a record of the walk
/// is [`resource_page`]'s to answer, which is why this starts the walk at the first
/// name at or after it either way.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token is
/// only ever COMPARED against the keys, so there is no indexing, no slicing, no
/// unwrap and no arithmetic that can overflow or panic.
pub fn records_from<'a, R>(
    records: &'a BTreeMap<String, R>,
    page_token: Option<&str>,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records
        .range(page_token.unwrap_or_default().to_string()..)
        .map(|(name, record)| (name.as_str(), record))
}

/// The records of a `(region, name)`-keyed map that belong to `region`, from the
/// one `page_token` names onward, in name order.
///
/// The same start [`records_from`] computes over a map keyed by name alone, for a
/// resource whose identity includes the Region it was created in: the key pair
/// orders this Region's records together, so the walk leaves the Region behind at
/// the first key of the next one and never reports another Region's record.
pub fn regional_records_from<'a, R>(
    records: &'a BTreeMap<(String, String), R>,
    region: &'a str,
    page_token: Option<&str>,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records
        .range((region.to_string(), page_token.unwrap_or_default().to_string())..)
        .take_while(move |((record_region, _), _)| record_region.as_str() == region)
        .map(|((_, name), record)| (name.as_str(), record))
}

/// The records of a name-keyed map that are LOCATED in `region` — `region_of`
/// reads the Region off each record — from the one `page_token` names onward, in
/// name order.
///
/// What a listing the model scopes to "your AWS account and region" reports: the
/// resource is keyed by name alone, because its name is unique across the
/// account, but it was created in one Region and keeps that Region on its stored
/// location, so the Region the request is signed for is what decides whether this
/// listing reports it. The same narrowing [`regional_records_from`] gets from the
/// key pair of a resource whose identity includes its Region, expressed as the
/// filter [`resource_page`] documents — so the page size counts the records
/// reported, the next page starts at the first record of this Region after them,
/// and a token naming a record of ANOTHER Region names no page of this listing.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token
/// and the Region are only ever COMPARED against the records, so there is no
/// indexing, no slicing, no unwrap and no arithmetic that can overflow or panic.
pub fn records_in_region_from<'a, R>(
    records: &'a BTreeMap<String, R>,
    region: &'a str,
    page_token: Option<&str>,
    region_of: impl Fn(&R) -> &str + 'a,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records_from(records, page_token).filter(move |(_, record)| region_of(record) == region)
}

/// One page of a resource listing: at most `page_size` of `records`, each as
/// `view_of` reports it, plus the name of the first record AFTER them — named only
/// while records remain.
///
/// `None` is the answer when `page_token` names no record this listing reports:
/// there is no page to report, which the caller turns into a refusal. A token is
/// only ever answered by the record it NAMES — the first record of the page it
/// opens — so a token the listing never handed out is refused rather than resumed
/// at the next record after it, which would answer a well-formed page with every
/// earlier record silently missing. An absent token asks for the first page.
///
/// Shared by every stored-resource listing this service pages, so each one walks
/// its records exactly the same way; [`catalog_page`] is the same walk over this
/// service's static catalogs. The start of the page comes from [`records_from`],
/// [`regional_records_from`] or [`records_in_region_from`], and a listing whose
/// request NARROWS which records it reports puts that filter on the iterator
/// ([`Iterator::filter`]): the page size then counts the records reported, the
/// next page starts at the first record after them that this listing reports too,
/// and a token naming a record this listing does not report names no page of it.
///
/// TOTAL on every input: the token is only ever COMPARED against the names, records
/// are only ever taken from the iterator in order and the page is bounded by
/// `page_size`, so there is no indexing, no slicing, no unwrap and no arithmetic
/// that can overflow or panic. `page_size` is at least one, so a walk over a
/// listing that holds records always advances.
pub fn resource_page<'a, R: 'a, T>(
    records: impl IntoIterator<Item = (&'a str, &'a R)>,
    page_token: Option<&str>,
    page_size: usize,
    mut view_of: impl FnMut(&'a str, &'a R) -> T,
) -> Option<(Vec<T>, Option<String>)> {
    let page_size = page_size.max(1);
    let mut records = records.into_iter().peekable();
    if let Some(token) = page_token.filter(|token| !token.is_empty()) {
        // The walk starts at the first name at or after the token, so the token
        // names a record of it exactly when that record IS the token.
        if records.peek().map(|(name, _)| *name) != Some(token) {
            return None;
        }
    }
    let mut page = Vec::new();
    for (name, record) in records {
        if page.len() == page_size {
            return Some((page, Some(name.to_string())));
        }
        page.push(view_of(name, record));
    }
    Some((page, None))
}

/// The message a stored-resource listing refuses an unrecognised `page_token`
/// with, where `reported` names what that listing reports ("an instance").
///
/// One wording for all of them: the refusal says which token was sent, that it
/// names nothing the listing reports, and what a page token is — so a caller that
/// sent a token of another listing, or one it invented, is told what to send
/// instead.
pub fn invalid_page_token_message(token: &str, reported: &str) -> String {
    format!(
        "pageToken {token:?} does not name {reported} this listing reports; a page token is the \
         name a previous page reported as nextPageToken"
    )
}
