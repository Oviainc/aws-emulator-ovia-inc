// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE selection rule every list of this service applies to
//! a filtered member that is SINGLE-valued on the record, driven over the wire
//! through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule, stated once: a `*Filter` member is a LIST of ACCEPTED VALUES for the one
//! record member it names, so a record matches it when its value for that member is
//! one of them; a filter the caller did not send (or sent carrying no value) narrows
//! nothing; a record that recorded no value at all for the filtered member matches
//! none of the accepted values, so the filter narrows it out; and naming several
//! values selects the union of what each of them selects, in the list's own order.
//!
//! Both lists under test select by that one rule, through the one helper that states
//! it ([`service_com_amazonaws_outposts_outpostsolafservice::state::filter_accepts`]):
//! `ListCatalogItems$ItemClassFilter`, selecting on the class of a published catalog
//! item, and `ListSites$OperatingAddressCountryCodeFilter`, selecting on the country
//! of a stored site's operating address. So this suite asserts the SAME rows of the
//! same table against each of them: whatever one list answers for its single-valued
//! filter, the other answers for its own.
//!
//! The one row a list only has where its filtered member's value space is open is the
//! value NO record carries: a `CountryCode` is any `^[A-Z]{2}$` string, while the
//! modelled `CatalogItemClass` value set (`RACK` | `SERVER`) is carried whole by the
//! published catalog, and a value outside that set is refused at the boundary (400
//! `ValidationException`, pinned in [`crate::catalog_items`]) rather than selecting
//! nothing. Every other row is asserted against both.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{address, query_value, text, Outposts};

/// One list under test: the operation, the route the model binds it to, the query
/// parameter its single-valued filter travels in, the output member its page arrives
/// in, and the member that identifies one record of that page.
struct Listing {
    operation: &'static str,
    route: &'static str,
    parameter: &'static str,
    page_member: &'static str,
    id_member: &'static str,
}

/// One group of the partition the filtered member induces: the value the records in
/// it carry, and the ids expected to carry it when the case placed those records
/// itself.
///
/// `expected` is `None` for a group whose records are reference data the service
/// publishes rather than records the case created, so no list of ids is written here:
/// the rows then hold the groups' answers against each other (disjoint, covering, and
/// their union is what naming several values selects).
struct Group {
    value: &'static str,
    expected: Option<Vec<String>>,
}

/// `ListCatalogItems`, whose `ItemClassFilter` selects on the single `CatalogItemClass`
/// of a published item.
const CATALOG_ITEMS: Listing = Listing {
    operation: "ListCatalogItems",
    route: "/catalog/items",
    parameter: "ItemClassFilter",
    page_member: "CatalogItems",
    id_member: "CatalogItemId",
};

/// `ListSites`, whose `OperatingAddressCountryCodeFilter` selects on the single
/// country code of a stored site's operating address.
const SITES: Listing = Listing {
    operation: "ListSites",
    route: "/sites",
    parameter: "OperatingAddressCountryCodeFilter",
    page_member: "Sites",
    id_member: "SiteId",
};

/// The modelled `CatalogItemClass` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="CatalogItemClass")` reports them.
const ITEM_CLASSES: &[&str] = &["RACK", "SERVER"];

/// Country codes for the sites this suite places: each a well-formed modelled
/// `CountryCode` (`^[A-Z]{2}$`), as `smithy_query(query="shape", shape="CountryCode")`
/// reports the shape.
const COUNTRIES: &[&str] = &["US", "DE", "JP"];

/// A well-formed `CountryCode` no site of this suite operates in, so a filter naming
/// it selects nothing.
const ABSENT_COUNTRY: &str = "ZZ";

/// Ask one list for the records the accepted values select, answering their ids in the
/// order the list reported them. An empty `accepted` sends no filter at all.
async fn select(outposts: &Outposts, listing: &Listing, accepted: &[&str]) -> Vec<String> {
    let query = accepted
        .iter()
        .map(|value| format!("{}={}", listing.parameter, query_value(value)))
        .collect::<Vec<String>>()
        .join("&");
    let path = if query.is_empty() {
        listing.route.to_string()
    } else {
        format!("{}?{}", listing.route, query)
    };
    let output = outposts.ok("GET", &path, None).await;
    output
        .get(listing.page_member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "{} answers a {} list, got {output}",
                listing.operation, listing.page_member
            )
        })
        .iter()
        .map(|record| text(record, listing.id_member))
        .collect()
}

/// Whether `selected` appears inside `whole` in the same relative order.
fn is_subsequence(whole: &[String], selected: &[String]) -> bool {
    let mut remaining = whole.iter();
    selected
        .iter()
        .all(|id| remaining.any(|candidate| candidate == id))
}

/// Drive the whole table against ONE list: `groups` partitions the records that carry
/// a value for the filtered member, `valueless` names the stored records that carry
/// none, and `unmatched` is a well-formed value no record carries where the member's
/// value space leaves room for one.
async fn assert_selects_by_the_rule(
    outposts: &Outposts,
    listing: &Listing,
    groups: &[Group],
    valueless: &[String],
    unmatched: Option<&str>,
) {
    let operation = listing.operation;

    // A filter the caller did not send narrows nothing.
    let whole = select(outposts, listing, &[]).await;
    assert!(
        !whole.is_empty(),
        "case {operation:?} unfiltered: expected the records the rows select from, got {whole:?}"
    );

    // One value selects exactly the records carrying it, in the list's own order.
    let mut per_value: Vec<Vec<String>> = Vec::new();
    for group in groups {
        let selected = select(outposts, listing, &[group.value]).await;
        if let Some(expected) = group.expected.as_ref() {
            assert_eq!(
                &selected, expected,
                "case {operation:?} {}={}: expected {expected:?}, got {selected:?}",
                listing.parameter, group.value
            );
        }
        assert!(
            !selected.is_empty() && is_subsequence(&whole, &selected),
            "case {operation:?} {}={}: expected a non-empty run of {whole:?} in its order, \
             got {selected:?}",
            listing.parameter,
            group.value
        );

        // Naming one value twice selects what naming it once does.
        let twice = select(outposts, listing, &[group.value, group.value]).await;
        assert_eq!(
            twice, selected,
            "case {operation:?} {}={} twice: expected {selected:?}, got {twice:?}",
            listing.parameter, group.value
        );

        // A value no record carries widens nothing it is given beside.
        if let Some(absent) = unmatched {
            let beside = select(outposts, listing, &[group.value, absent]).await;
            assert_eq!(
                beside, selected,
                "case {operation:?} {}={} beside {absent}: expected {selected:?}, got {beside:?}",
                listing.parameter, group.value
            );
        }
        per_value.push(selected);
    }

    // The member is single-valued, so no record is selected by two of its values.
    for (left, right) in (0..groups.len()).flat_map(|left| {
        ((left + 1)..groups.len()).map(move |right| (left, right))
    }) {
        let shared: Vec<&String> = per_value[left]
            .iter()
            .filter(|id| per_value[right].contains(id))
            .collect();
        assert!(
            shared.is_empty(),
            "case {operation:?} {}={} against ={}: expected no record in both, got {shared:?}",
            listing.parameter,
            groups[left].value,
            groups[right].value
        );
    }

    // Naming every value selects the union of what each selects, in the list's order
    // — which is every record except the ones carrying no value for the member.
    let values: Vec<&str> = groups.iter().map(|group| group.value).collect();
    let union: Vec<String> = whole
        .iter()
        .filter(|id| per_value.iter().any(|selected| selected.contains(id)))
        .cloned()
        .collect();
    let together = select(outposts, listing, &values).await;
    assert_eq!(
        together, union,
        "case {operation:?} {}={values:?}: expected {union:?}, got {together:?}",
        listing.parameter
    );
    let narrowed_out: Vec<String> = whole
        .iter()
        .filter(|id| !union.contains(id))
        .cloned()
        .collect();
    assert_eq!(
        narrowed_out, valueless,
        "case {operation:?} {}={values:?}: expected exactly {valueless:?} narrowed out of \
         {whole:?}, got {narrowed_out:?}",
        listing.parameter
    );

    // A value no record carries selects nothing at all.
    if let Some(absent) = unmatched {
        let none = select(outposts, listing, &[absent]).await;
        assert!(
            none.is_empty(),
            "case {operation:?} {}={absent}: expected no record, got {none:?}",
            listing.parameter
        );
    }
}

/// The one rule, asserted through BOTH lists that select on a single-valued filtered
/// member: the catalog's item class and a site's operating-address country.
#[tokio::test]
async fn one_rule_selects_for_every_single_valued_filter() {
    let outposts = Outposts::new();

    // The catalog is reference data this service publishes: every published item has
    // exactly one class, and the modelled value set is what the classes come from.
    let catalog_groups: Vec<Group> = ITEM_CLASSES
        .iter()
        .map(|class| Group {
            value: class,
            expected: None,
        })
        .collect();
    assert_selects_by_the_rule(&outposts, &CATALOG_ITEMS, &catalog_groups, &[], None).await;

    // The sites, placed by this case: two in the first country (so a group holds more
    // than one record), one in each of the others, and one with no operating address
    // at all — the record carrying no value for the filtered member.
    let mut site_groups: Vec<Group> = COUNTRIES
        .iter()
        .map(|country| Group {
            value: country,
            expected: Some(Vec::new()),
        })
        .collect();
    for (index, country) in COUNTRIES.iter().enumerate() {
        let placed = if index == 0 { 2 } else { 1 };
        for ordinal in 0..placed {
            let site = outposts
                .create_site(&json!({
                    "Name": format!("filter-rule-{country}-{ordinal}"),
                    "OperatingAddress": address(country, "WA", "Seattle"),
                }))
                .await;
            site_groups[index]
                .expected
                .as_mut()
                .expect("the sites of this case are the ids it placed")
                .push(text(&site, "SiteId"));
        }
    }
    let addressless = outposts
        .create_site(&json!({ "Name": "filter-rule-no-address" }))
        .await;
    let valueless = vec![text(&addressless, "SiteId")];

    assert_selects_by_the_rule(
        &outposts,
        &SITES,
        &site_groups,
        &valueless,
        Some(ABSENT_COUNTRY),
    )
    .await;
}
