// SPDX-License-Identifier: CC0-1.0

//! The per-category tier of the seeded catalog: one listing in EVERY subcategory of the
//! catalog's taxonomy, each classified under a second subcategory as well.
//!
//! AWS Marketplace is a catalog far larger than one page of search results, and
//! `SearchListings` is `@paginated` with `maxResults` ranging 1..100: "[i]f nextToken is
//! returned, there are more results available"
//! (`smithy_query(query="operation",operation="SearchListings")`). A continuation token can
//! only exist when the match set is larger than the page served, so what a client can page
//! at all is decided by how many listings a query matches — for a search over the whole
//! catalog AND for one a client has refined by a facet value. This tier is what makes both
//! continuable: it holds a listing in every subcategory the taxonomy declares
//! ([`CATEGORY_TAXONOMY`]) — which fills that subcategory's ancestors too — and classifies
//! each one under a second subcategory from another part of the tree, so every category,
//! and every top-level category carrying its whole subtree's listings, matches several
//! listings rather than one.
//!
//! The records are this emulator's own seed data: each row's listing, product, offer, offer
//! terms and (for some rows) offer set are built by [`seed_rows`], the same path the
//! hand-written rows above go through, so no tier's records are shaped differently from
//! another's.

use super::*;

/// How far apart the two subcategories one row is classified under sit in
/// [`subcategories`]'s order.
///
/// [`pair_step`] turns it into a stride that always lands on ANOTHER subcategory, so every
/// subcategory gets two listings of this tier (its own row's and the row that paired with
/// it); this value is large enough that the pair usually sits in another top-level subtree,
/// so a listing is classified across the taxonomy the way a real one is.
const PAIR_STRIDE: usize = 17;

/// The first line of this tier's `555-01xx` support telephone block, clear of the numbers
/// the hand-written rows (`0101..0106`) and the breadth rows (`0110..`) take. The `555-01xx`
/// range is reserved for fictional use in every area code, so this tier takes the next area
/// code of [`PHONE_AREA_CODES`] each time it runs out of lines in the block.
const PHONE_BLOCK: usize = 140;

/// How many lines of the `555-01xx` block this tier takes per area code: `0140..0199`.
const PHONE_LINES: usize = 60;

/// The area codes this tier's fictional telephone numbers are taken from, in turn, so the
/// numbers stay inside the reserved `555-01xx` range however many categories the taxonomy
/// declares.
const PHONE_AREA_CODES: &[&str] = &["206", "212", "312", "415", "512"];

/// The first sequence number of this tier's id block, clear of the hand-written records
/// (`0001..`) and the breadth rows (`0101..`).
const SEQUENCE_BLOCK: usize = 201;

/// What kind of product each row is, cycled over the rows.
///
/// Marketplace listings are not all one kind of thing, and the word is part of what a
/// `searchText` keyword can match, so the catalog carries several.
const PRODUCT_KINDS: &[&str] = &[
    "Platform",
    "Suite",
    "Service",
    "Hub",
    "Workbench",
    "Toolkit",
    "Gateway",
    "Studio",
];

/// Seed one listing (with its product, offer and terms) per subcategory of the taxonomy.
pub(super) fn seed_taxonomy_rows(
    state: &mut ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState,
    now: i64,
) {
    let categories = subcategories();
    let count = categories.len();
    if count == 0 {
        // A taxonomy with no subcategory has no row to derive, and nothing to pair with.
        return;
    }
    let step = pair_step(count);
    let built: Vec<BuiltRow> = categories
        .iter()
        .enumerate()
        .map(|(index, category)| {
            let paired = categories[(index + step) % count];
            BuiltRow::new(index, category, paired)
        })
        .collect();
    let rows: Vec<CatalogRow<'_>> = built.iter().map(BuiltRow::row).collect();
    seed_rows(state, now, &rows);
}

/// The stride that pairs a subcategory with another one, for a taxonomy of `count`
/// subcategories.
///
/// [`PAIR_STRIDE`] itself lands back on the row's own subcategory whenever `count` divides
/// it, which would classify a listing twice under one category and leave that category with
/// a single listing — the opposite of what this tier is for. A taxonomy with ONE subcategory
/// has no other to pair with, and then the row carries its own category alone.
fn pair_step(count: usize) -> usize {
    match PAIR_STRIDE % count {
        0 => usize::from(count > 1),
        step => step,
    }
}

/// Every category of the taxonomy that sits under another one, in the taxonomy's order.
///
/// A record is classified at a SUBcategory and carries its ancestors through
/// [`classified_under`], so seeding the subcategories is what fills the top-level
/// categories too.
fn subcategories() -> Vec<&'static str> {
    CATEGORY_TAXONOMY
        .iter()
        .filter(|(_, _, parent)| parent.is_some())
        .map(|(category_id, _, _)| *category_id)
        .collect()
}

/// One row of this tier, holding the text it derived so [`CatalogRow`] can borrow it.
struct BuiltRow {
    index: usize,
    slug: String,
    name: String,
    short_description: String,
    categories: Vec<&'static str>,
    support_phone: String,
}

impl BuiltRow {
    /// Derive a row from the subcategory it belongs to and the one it is also classified
    /// under: the category's own display name is what the listing is named and described
    /// by, so a `searchText` keyword or a `CATEGORY` facet value reaches it by the same
    /// word a client reads.
    fn new(index: usize, category: &'static str, paired: &'static str) -> Self {
        let kind = PRODUCT_KINDS[index % PRODUCT_KINDS.len()];
        let brand = brand(index);
        let display_name = taxonomy_category(category).display_name;
        let paired_name = taxonomy_category(paired).display_name;
        Self {
            index,
            slug: format!("{}-{}", category, kind.to_lowercase()),
            name: format!("{brand} {display_name} {kind}"),
            short_description: format!(
                "{display_name} {} with automation, monitoring and {} integration.",
                kind.to_lowercase(),
                paired_name.to_lowercase()
            ),
            categories: vec![category, paired],
            support_phone: format!(
                "+1-{}-555-{:04}",
                PHONE_AREA_CODES[(index / PHONE_LINES) % PHONE_AREA_CODES.len()],
                PHONE_BLOCK + index % PHONE_LINES
            ),
        }
    }

    /// This row as the seeder's own description of a record block.
    fn row(&self) -> CatalogRow<'_> {
        let index = self.index;
        CatalogRow {
            sequence: SEQUENCE_BLOCK + index,
            slug: &self.slug,
            name: &self.name,
            short_description: &self.short_description,
            categories: &self.categories,
            // Every fulfillment option type the catalog displays
            // ([`FULFILLMENT_OPTION_TYPE_DISPLAY_NAMES`], read off the model's
            // `FulfillmentOptionType` enum), taken in turn, so this tier spans the whole
            // fulfillment union as well.
            fulfillment: FULFILLMENT_OPTION_TYPE_DISPLAY_NAMES
                [index % FULFILLMENT_OPTION_TYPE_DISPLAY_NAMES.len()]
            .0,
            seller: BREADTH_SELLERS[index % BREADTH_SELLERS.len()],
            // The pricing model, unit and review summary are offset from one another, so two
            // rows of this tier rarely carry the same combination of them.
            pricing_model: PRICING_MODEL_DISPLAY_NAMES
                [(index + 1) % PRICING_MODEL_DISPLAY_NAMES.len()],
            pricing_unit: PRICING_UNIT_DISPLAY_NAMES
                [(index + 3) % PRICING_UNIT_DISPLAY_NAMES.len()],
            reviews: BREADTH_REVIEWS[(index + 2) % BREADTH_REVIEWS.len()],
            support_phone: &self.support_phone,
            // Every sixth row's offer belongs to an offer set, so this tier reaches
            // `OFFERSET` purchase options across the catalog as well.
            offer_set: index % 6 == 5,
        }
    }
}

/// The brand a row's name starts with: the first word of the seller that publishes it, so
/// the listing name and its publisher read as the same vendor's.
fn brand(index: usize) -> &'static str {
    let (_, seller_name) = BREADTH_SELLERS[index % BREADTH_SELLERS.len()];
    seller_name
        .split_whitespace()
        .next()
        .unwrap_or(seller_name)
}
