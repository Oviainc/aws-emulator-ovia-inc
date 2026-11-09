// SPDX-License-Identifier: CC0-1.0

//! Which chunks a retrieval's `filter` admits.
//!
//! A `Retrieve` request may narrow the chunks it ranks with a `RetrievalFilter` over the
//! METADATA each chunk carries (`smithy_query(query="shape",shape="RetrievalFilter")`).
//! The filter is evaluated here, against the fields the ingestion really recorded for
//! the chunk ([`crate::state::knowledge_base_retrieval::RetrievedChunk::metadata`]): a
//! chunk the filter excludes is not reported, and a chunk carrying no attribute of the
//! name the filter tests satisfies no comparison over it — "no such attribute" is not a
//! match.
//!
//! The filter a caller may send is BOUNDED in both directions it can grow. `andAll` and
//! `orAll` hold filters of their own, so a request could nest them arbitrarily deep, and
//! the modelled `RetrievalFilterList` states a minimum of two elements and NO maximum, so
//! one `andAll` at depth two could hold any number of comparisons. A handler asks
//! [`within_bounds`] ONCE, before it ranks anything, and refuses a filter past
//! [`MAX_FILTER_DEPTH`] or [`MAX_FILTER_NODES`] with the modelled `ValidationException`
//! naming the bound — never a silent "nothing matched". Both bounds together are what
//! caps the work of a retrieval, because the filter is evaluated once per chunk the index
//! holds: the PRODUCT of the two factors is bounded, not each of them alone. Every walk
//! here stops at the depth bound as well, so none can recurse without end whatever a
//! request carries.

use std::cmp::Ordering::{Equal, Greater, Less};
use std::collections::BTreeMap;

use ::aws_smithy_types::{Document, Number};

use crate::ops::types::{FilterAttribute, RetrievalFilter};

/// How deeply a request's `filter` may nest `andAll` / `orAll` before it is refused.
///
/// This crate's own bound, so a filter cannot drive unbounded recursion here, and chosen
/// BELOW the nesting the deserializer itself admits so that the service's own refusal —
/// which names the member and the bound — is the one a caller past it sees, rather than
/// the framework's generic depth refusal. Far deeper than any filter a caller writes: the
/// modelled `andAll`/`orAll` hold two filters at least, so 32 levels is a filter over
/// billions of comparisons.
pub const MAX_FILTER_DEPTH: usize = 32;

/// How many nodes a request's `filter` may hold in total — every comparison and every
/// `andAll`/`orAll` that holds them — before it is refused.
///
/// DEPTH alone is not a bound on the work: the modelled `RetrievalFilterList` has no
/// maximum length, so one `andAll` at depth two can hold as many comparisons as a request
/// body carries, and the filter is evaluated once per chunk the index holds. The same
/// bound, for the same reason, as the one the vector store this retrieval reads applies to
/// its own query filter (`vectors_state::MAX_FILTER_NODES`), so a caller meets one number
/// whichever plane it filters through.
pub const MAX_FILTER_NODES: usize = 4096;

/// Why a `filter` is refused before anything is ranked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTooBig {
    /// It nests `andAll`/`orAll` past [`MAX_FILTER_DEPTH`].
    Depth,
    /// It holds more than [`MAX_FILTER_NODES`] nodes.
    Nodes,
}

/// Whether `filter` is inside both bounds, and which one it passed when it is not.
///
/// Asked once per request, before anything is ranked: a filter past either bound is
/// refused rather than evaluated, so [`admits`] is only ever asked about one whose whole
/// evaluation is bounded. The walk itself stops at [`MAX_FILTER_DEPTH`] and counts at most
/// [`MAX_FILTER_NODES`] nodes, so deciding this costs no more than the bounds it states.
pub fn within_bounds(filter: &RetrievalFilter) -> Result<(), FilterTooBig> {
    fn walk(filter: &RetrievalFilter, depth: usize, nodes: &mut usize) -> Result<(), FilterTooBig> {
        if depth > MAX_FILTER_DEPTH {
            return Err(FilterTooBig::Depth);
        }
        *nodes += 1;
        if *nodes > MAX_FILTER_NODES {
            return Err(FilterTooBig::Nodes);
        }
        match filter {
            RetrievalFilter::andAll(filters) | RetrievalFilter::orAll(filters) => {
                for inner in filters {
                    walk(inner, depth + 1, nodes)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    let mut nodes = 0;
    walk(filter, 0, &mut nodes)
}

/// Whether `metadata` satisfies `filter`.
///
/// Total: a sub-filter deeper than [`MAX_FILTER_DEPTH`] is treated as not satisfied, which
/// a served request never reaches because [`within_bounds`] refused it first.
pub fn admits(metadata: &BTreeMap<String, Document>, filter: &RetrievalFilter) -> bool {
    admits_at(metadata, filter, 0)
}

/// [`admits`], carrying how deep into `andAll`/`orAll` the walk already is.
fn admits_at(
    metadata: &BTreeMap<String, Document>,
    filter: &RetrievalFilter,
    depth: usize,
) -> bool {
    if depth > MAX_FILTER_DEPTH {
        return false;
    }
    match filter {
        RetrievalFilter::Equals(attribute) => compares(metadata, attribute, |held, value| {
            same_value(held, value)
        }),
        RetrievalFilter::notEquals(attribute) => compares(metadata, attribute, |held, value| {
            !same_value(held, value)
        }),
        RetrievalFilter::greaterThan(attribute) => {
            compares(metadata, attribute, |held, value| ordering(held, value) == Some(Greater))
        }
        RetrievalFilter::greaterThanOrEquals(attribute) => {
            compares(metadata, attribute, |held, value| {
                matches!(ordering(held, value), Some(Greater | Equal))
            })
        }
        RetrievalFilter::lessThan(attribute) => {
            compares(metadata, attribute, |held, value| ordering(held, value) == Some(Less))
        }
        RetrievalFilter::lessThanOrEquals(attribute) => {
            compares(metadata, attribute, |held, value| {
                matches!(ordering(held, value), Some(Less | Equal))
            })
        }
        RetrievalFilter::In(attribute) => compares(metadata, attribute, |held, value| {
            items(value).is_some_and(|list| list.iter().any(|item| same_value(held, item)))
        }),
        RetrievalFilter::notIn(attribute) => compares(metadata, attribute, |held, value| {
            items(value).is_some_and(|list| !list.iter().any(|item| same_value(held, item)))
        }),
        RetrievalFilter::startsWith(attribute) => compares(metadata, attribute, |held, value| {
            match (text(held), text(value)) {
                (Some(held), Some(value)) => held.starts_with(value),
                _ => false,
            }
        }),
        RetrievalFilter::stringContains(attribute) => {
            compares(metadata, attribute, |held, value| {
                match (text(held), text(value)) {
                    (Some(held), Some(value)) => held.contains(value),
                    _ => false,
                }
            })
        }
        RetrievalFilter::listContains(attribute) => compares(metadata, attribute, |held, value| {
            items(held).is_some_and(|list| list.iter().any(|item| same_value(item, value)))
        }),
        RetrievalFilter::andAll(filters) => filters
            .iter()
            .all(|inner| admits_at(metadata, inner, depth + 1)),
        RetrievalFilter::orAll(filters) => filters
            .iter()
            .any(|inner| admits_at(metadata, inner, depth + 1)),
    }
}

/// Apply one comparison to the attribute `attribute.key` names, which the chunk has to
/// carry: a chunk with no such attribute satisfies no comparison over it.
fn compares(
    metadata: &BTreeMap<String, Document>,
    attribute: &FilterAttribute,
    verdict: impl Fn(&Document, &Document) -> bool,
) -> bool {
    metadata
        .get(&attribute.key)
        .is_some_and(|held| verdict(held, &attribute.value))
}

/// Whether two attribute values are the same value: compared as text, as a number or as
/// a flag, and otherwise not the same.
fn same_value(held: &Document, value: &Document) -> bool {
    match (held, value) {
        (Document::String(held), Document::String(value)) => held == value,
        (Document::Bool(held), Document::Bool(value)) => held == value,
        _ => match (number(held), number(value)) {
            (Some(held), Some(value)) => held == value,
            _ => false,
        },
    }
}

/// How two attribute values order, for the four range comparisons: numerically when both
/// are numbers, lexicographically when both are text, and `None` when they cannot be
/// compared at all.
fn ordering(held: &Document, value: &Document) -> Option<::std::cmp::Ordering> {
    if let (Some(held), Some(value)) = (number(held), number(value)) {
        return held.partial_cmp(&value);
    }
    match (text(held), text(value)) {
        (Some(held), Some(value)) => Some(held.cmp(value)),
        _ => None,
    }
}

/// The text one attribute value carries, or `None` for a value that is not text.
fn text(value: &Document) -> Option<&str> {
    match value {
        Document::String(text) => Some(text.as_str()),
        _ => None,
    }
}

/// The number one attribute value carries, or `None` for a value that is not a number.
fn number(value: &Document) -> Option<f64> {
    match value {
        Document::Number(Number::PosInt(unsigned)) => Some(*unsigned as f64),
        Document::Number(Number::NegInt(signed)) => Some(*signed as f64),
        Document::Number(Number::Float(float)) => Some(*float),
        _ => None,
    }
}

/// The items one attribute value carries, or `None` for a value that is not a list.
fn items(value: &Document) -> Option<&[Document]> {
    match value {
        Document::Array(items) => Some(items.as_slice()),
        _ => None,
    }
}
