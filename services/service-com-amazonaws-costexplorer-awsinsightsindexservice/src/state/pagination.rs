// SPDX-License-Identifier: CC0-1.0

//! Pagination over one of this service's ARN-keyed registries: the opaque cursor codec,
//! the page size a request asks for, and the page walk itself.
//!
//! Every paginated read here answers over a `BTreeMap` keyed on the resource ARN, so they
//! all page the same way — one stable total order, a cursor naming the ARN the next page
//! resumes at, and a page cut at the caller's `MaxResults`. That walk lives here once so
//! two reads cannot drift apart on where a page starts or when a cursor is issued.

use std::collections::BTreeMap;
use std::ops::Bound;

/// The page a request that names no `MaxResults` gets: every record the filters admit.
///
/// `PageSize` carries no modelled range and the paginated operations document no default
/// page size, so a request that asks for no page size is answered with the whole result in
/// one page rather than cut at a size this emulator invented.
pub const UNPAGED: usize = usize::MAX;

/// Encode the ARN a page resumes at as an opaque `NextToken`.
///
/// The cursor is SELF-CONTAINED rather than a key into server-side state: minting it is
/// part of answering a read, and a read must not change what a later caller sees. Hex of
/// the ARN is opaque to a client and survives records being added or deleted between
/// pages, because the next page is resolved by ARN order, not by a saved offset.
pub fn encode_next_token(resume_at: &str) -> String {
    ::hex::encode(resume_at.as_bytes())
}

/// Decode a `NextToken` back into the ARN its page resumes at.
///
/// `None` for anything that is not a well-formed cursor (not hex, or not UTF-8). A
/// well-formed cursor naming no stored ARN simply resumes at the first ARN after it,
/// which is an empty page when none follows.
pub fn decode_next_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The page size a request asks for: the caller's `MaxResults`, or [`UNPAGED`] when they
/// named none.
///
/// `Err` carries the refusal text for a page size that is not one — a count below one, or
/// one that is not a count at all. The member is a plain integer in the model, so the
/// bound is not expressible there and the refusal is the caller's to read.
pub fn requested_page_size(max_results: Option<i32>) -> Result<usize, String> {
    match max_results {
        None => Ok(UNPAGED),
        Some(requested) => usize::try_from(requested)
            .ok()
            .filter(|size| *size >= 1)
            .ok_or_else(|| {
                format!("MaxResults {requested} is not a page size; it must be at least 1")
            }),
    }
}

/// One page of a registry: the records it reports, and the cursor that resumes after them.
#[derive(Debug)]
pub struct Page<'a, T> {
    /// The records on this page, in the registry's ARN order.
    pub records: Vec<&'a T>,
    /// The cursor the next page resumes at, present only when a record the filters admit
    /// is left over — so the last page of a result carries none.
    pub next_token: Option<String>,
}

/// Walk `registry` from the ARN `resume_at` names, reporting at most `page_size` of the
/// records `admits` accepts.
///
/// One stable total order (the ARN ordering the registry is keyed on) is what makes the
/// cursor resolvable: the next page is "everything at or after this ARN", so a record
/// created or deleted between pages neither duplicates nor skips the rest.
pub fn page<'a, T>(
    registry: &'a BTreeMap<String, T>,
    resume_at: Option<&String>,
    page_size: usize,
    admits: impl Fn(&T) -> bool,
) -> Page<'a, T> {
    let mut selected = registry
        .range::<String, _>(resume_at.map_or((Bound::Unbounded, Bound::Unbounded), |from| {
            (Bound::Included(from), Bound::Unbounded)
        }))
        .filter(|(_, record)| admits(record));

    let mut records = Vec::new();
    for (_, record) in selected.by_ref().take(page_size) {
        records.push(record);
    }
    let next_token = selected.next().map(|(arn, _)| encode_next_token(arn));

    Page {
        records,
        next_token,
    }
}
