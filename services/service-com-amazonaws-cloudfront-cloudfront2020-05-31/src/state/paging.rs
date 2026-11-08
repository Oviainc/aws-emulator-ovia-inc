// SPDX-License-Identifier: CC0-1.0

//! The unit tests of the PAGE SIZE every listing of this service serves one response at:
//! the clamp [`super::page_size_at`] applies, read off the function rather than through the
//! wire. The end-to-end behaviour of a page is pinned by the crate's `tests/all/` suites.

use super::*;

/// The size served is the size asked for while it is within the listing's ceiling, the
/// CEILING above it, zero for a request that asks for no items at all, and nothing at all
/// below zero.
#[test]
fn a_page_size_is_the_size_asked_for_bounded_by_the_listings_ceiling() {
    // (case, MaxItems, the listing's own default, the size served)
    let rows: [(&str, Option<i32>, i32, Option<usize>); 9] = [
        ("no size named", None, MAX_ITEMS, Some(MAX_ITEMS as usize)),
        ("one item", Some(1), MAX_ITEMS, Some(1)),
        ("no items", Some(0), MAX_ITEMS, Some(0)),
        (
            "the ceiling itself",
            Some(MAX_ITEMS),
            MAX_ITEMS,
            Some(MAX_ITEMS as usize),
        ),
        (
            "one past the ceiling",
            Some(MAX_ITEMS + 1),
            MAX_ITEMS,
            Some(MAX_ITEMS as usize),
        ),
        (
            "far past the ceiling",
            Some(i32::MAX),
            MAX_ITEMS,
            Some(MAX_ITEMS as usize),
        ),
        (
            "a listing with its own ceiling",
            Some(i32::MAX),
            MAX_ITEMS_ORIGIN_ACCESS_CONTROLS,
            Some(MAX_ITEMS_ORIGIN_ACCESS_CONTROLS as usize),
        ),
        ("one below zero", Some(-1), MAX_ITEMS, None),
        ("far below zero", Some(i32::MIN), MAX_ITEMS, None),
    ];
    for (case, max_items, default, expected) in rows {
        let actual = page_size_at(max_items, default);
        assert_eq!(
            actual, expected,
            "case {case}: expected the page size {expected:?}, got {actual:?}"
        );
    }
}
