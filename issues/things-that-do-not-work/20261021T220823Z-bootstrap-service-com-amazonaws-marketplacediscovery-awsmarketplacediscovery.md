Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the AWS Marketplace Discovery emulator

_Filed: 2026-10-21T22:08:23Z_
_Operation: service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 9 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). Implement a FIRST minimal slice so a realistic client can run the service's core read-only drill-down: search the catalog, open a listing, open the product behind it, enumerate purchase options, and fetch one offer.

Marketplace Discovery is a **read-only** API (every operation is `@readonly`; there are no Create/Put operations in the model), so the emulator must own a **seeded catalog** in its service state at startup (a handful of listings, each with one product, fulfillment options, purchase options, offer sets, offers and offer terms). Every response below MUST be computed by looking the requested id up in that seeded catalog — no canned bodies, no echoing the input back, no synthesizing an id that was never stored. A `GetListing` for an id that is not in the catalog must answer the modeled `ResourceNotFoundException` (HTTP 404), and a `GetListing` for an id that `SearchListings` just returned must answer the same data the search summary described.

Happy path, in order (routes and fields from OPERATIONS.md; wire protocol `restJson1`, all routes `POST /2026-02-05/<op>`):

1. **SearchListings** — in: `searchText`, `filters`, `maxResults` (range 1..100, default 25, required), `sortBy` (RELEVANCE|AVERAGE_CUSTOMER_RATING, default RELEVANCE), `sortOrder` (ASCENDING|DESCENDING, default DESCENDING), `nextToken`. Out: `totalResults` (long, required), `listingSummaries` (required), `nextToken`. Must filter the seeded catalog by `searchText` (case-insensitive substring over listing name / short description is sufficient for the first slice), honor `sortBy`/`sortOrder`, set `totalResults` to the number of MATCHES (not the page size), truncate the page to `maxResults`, and return an opaque `nextToken` (base64, pattern `^[a-zA-Z0-9+/=]+$`) only when more matches remain. Feeding that token back must resume after the last returned item with no duplicates or skips.
2. **GetListing** — in: `listingId` (required, `^[\w\-]+$`), `locale`. Out: the full listing record — `listingId`, `listingName`, `catalog`, `shortDescription`, `longDescription`, `logoThumbnailUrl`, `categories`, `highlights`, `badges`, `pricingModels`, `pricingUnits`, `fulfillmentOptionSummaries`, `associatedEntities`, `publisher` (SellerInformation), `promotionalMedia`, `resources`, `sellerEngagements`, `useCases`, `reviewSummary`, `locale`. Must return the SEEDED record for `listingId` and `ResourceNotFoundException` (404) otherwise. All `@required` output members must be populated (empty lists are acceptable where the catalog has no entries, but the members must be present).
3. **GetProduct** — in: `productId` (required), `locale`. Out: `productId`, `listingId`, `catalog`, `productName`, `manufacturer`, `deployedOnAws` (DEPLOYED|NOT_APPLICABLE|NOT_DEPLOYED), `shortDescription`, `longDescription`, `logoThumbnailUrl`, `fulfillmentOptionSummaries`, `categories`, `highlights`, `promotionalMedia`, `resources`, `sellerEngagements`. The `listingId` it returns must be the listing whose `associatedEntities` pointed at this product — the link must be real in state, in both directions. Unknown `productId` -> `ResourceNotFoundException` (404).
4. **ListPurchaseOptions** — in: `filters` (1..10), `maxResults` (1..100, default 25), `nextToken`. Out: `purchaseOptions` (list<PurchaseOptionSummary>, len ..100), `nextToken`. Must return the purchase options stored for the matching entities, apply the `filters`, page by `maxResults` with a working `nextToken`, and return an empty list (not an error) when nothing matches.
5. **GetOffer** — in: `offerId` (required), `locale`. Out: `offerId`, `catalog`, `offerName`, `expirationTime`, `availableFromTime`, `sellerOfRecord`, `associatedEntities`, `agreementProposalId` (required), `replacementAgreementId`, `pricingModel` (required), `badges`. Must resolve an `offerId` that `ListPurchaseOptions` surfaced, from state; unknown id -> `ResourceNotFoundException` (404).
6. **GetOfferTerms** — in: `offerId` (required), `maxResults` (default 10), `nextToken`. Out: `offerTerms` (required), `nextToken`. Must return the terms stored against that offer, paged by `maxResults`/`nextToken`; unknown `offerId` -> `ResourceNotFoundException` (404).

Shared requirements for the slice: input constraints from the model are enforced centrally, but handlers must still reject semantically bad input the model cannot express (e.g. an unparseable/foreign `nextToken` -> validation error rather than a panic or a silent restart from page 1), and `locale` echoed on output must be the locale that was requested (or the catalog default when absent), not a hardcoded string.

## State Model

Add to `ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState` (replacing the `TODO`), seeded with a small fixture catalog on `Default`:

- `catalog: String` — the catalog name stamped on every response (e.g. `"AWSMarketplace"`).
- `listings: BTreeMap<String /*listingId*/, Listing>` where `Listing { listing_id, listing_name, catalog, short_description, long_description, logo_thumbnail_url, integration_guide: Option<String>, categories: Vec<Category>, highlights: Vec<String>, badges: Vec<ListingBadge>, pricing_models: Vec<PricingModel>, pricing_units: Vec<PricingUnit>, fulfillment_option_summaries: Vec<FulfillmentOptionSummary>, associated_entities: Vec<ListingAssociatedEntity> /* -> product ids, offer-set ids */, publisher: SellerInformation, promotional_media: Vec<PromotionalMedia>, resources: Vec<Resource>, seller_engagements: Vec<SellerEngagement>, use_cases: Vec<UseCaseEntry>, review_summary: Option<ReviewSummary>, average_customer_rating: f64 /* for sortBy=AVERAGE_CUSTOMER_RATING */, search_blob: String /* lowercased name+descriptions for searchText matching */ }`.
- `products: BTreeMap<String /*productId*/, Product>` where `Product { product_id, listing_id, catalog, product_name, manufacturer: SellerInformation, deployed_on_aws, short_description, long_description, logo_thumbnail_url, categories, highlights, promotional_media, resources, seller_engagements, fulfillment_option_summaries }`.
- `fulfillment_options: BTreeMap<String /*productId*/, Vec<FulfillmentOption>>` — the full records `ListFulfillmentOptions` pages over.
- `offer_sets: BTreeMap<String /*offerSetId*/, OfferSet>` where `OfferSet { offer_set_id, catalog, offer_set_name, available_from_time, expiration_time, buyer_notes, seller_of_record, badges, associated_entities /* -> offer ids, listing ids */ }`.
- `offers: BTreeMap<String /*offerId*/, Offer>` where `Offer { offer_id, catalog, offer_name, expiration_time, available_from_time, seller_of_record, associated_entities, agreement_proposal_id, replacement_agreement_id, pricing_model, badges, offer_set_id: Option<String> }`.
- `offer_terms: BTreeMap<String /*offerId*/, Vec<OfferTerm>>` — the paged list behind `GetOfferTerms`.
- `purchase_options: Vec<PurchaseOptionSummary>` plus the index fields needed by `PurchaseOptionFilter` (listing id, product id, offer/offer-set id, pricing model) so `ListPurchaseOptions` filters from state rather than returning a fixed list.
- `facets: BTreeMap<SearchFacetType, Vec<ListingFacet>>` — derived (not hardcoded) from `listings` when `SearchFacets` is implemented.

Pagination for all four `@paginated` operations should share ONE helper: encode the opaque `nextToken` as a base64 cursor (matching `^[a-zA-Z0-9+/=]+$`) carrying the offset/last-key plus the query identity, so a token from one query is not silently honored by another.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): `SearchFacets`, `ListFulfillmentOptions`, `GetOfferSet`.

Done when an integration test runs SearchListings -> GetListing -> GetProduct -> ListPurchaseOptions -> GetOffer -> GetOfferTerms against the seeded catalog, asserts the ids and names agree across the chain (not just 2xx), asserts a second page via `nextToken` returns different items, and asserts an unknown id yields `ResourceNotFoundException` with HTTP 404.

## Evidence

OPERATIONS.md (crate root) lists all 9 operations with `restJson1` routes under `POST /2026-02-05/...`, the required input/output members quoted above, and `ResourceNotFoundException (HTTP 404)` as the only modeled error on the six Get/List-by-id operations; the four `@paginated(inputToken=nextToken, outputToken=nextToken, ...)` traits on SearchListings, SearchFacets, GetOfferTerms and ListFulfillmentOptions ground the pagination requirement, and `maxResults` ranges/defaults (1..100 default 25 for SearchListings/ListPurchaseOptions, default 10 for GetOfferTerms) ground the page-bound requirement. `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 9 handler files (get_listing.rs, get_offer.rs, get_offer_set.rs, get_offer_terms.rs, get_product.rs, list_fulfillment_options.rs, list_purchase_options.rs, search_facets.rs, search_listings.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. `src/state.rs` defines `ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState` with only `// TODO: add service-specific state fields`, so no catalog exists to read from. Every operation is `@readonly`, which grounds the seeded-catalog approach: the API offers no way for a client to create the data it reads.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with signing name `aws-marketplace`, endpoint prefix `discovery-marketplace`.
2. POST /2026-02-05/searchListings with {"searchText":"<term present in the seeded catalog>","maxResults":2} — expect 200 with `totalResults` >= 1, up to 2 `listingSummaries`, and a `nextToken` when more matches remain. Actual today: HTTP 501 NotImplemented.
3. Re-POST searchListings with the returned `nextToken` — expect a different page of listings, no duplicates against page 1.
4. POST /2026-02-05/getListing with the `listingId` from page 1 — expect 200 whose `listingName`/`catalog` match the search summary and whose `@required` members are all present.
5. POST /2026-02-05/getProduct with the productId from that listing's `associatedEntities` — expect 200 whose `listingId` equals the listing from the previous step.
6. POST /2026-02-05/listPurchaseOptions with a filter selecting that listing and {"maxResults":1} — expect 200 with one `purchaseOptions` entry and a `nextToken` if more exist.
7. POST /2026-02-05/getOffer with an `offerId` surfaced by that purchase option — expect 200 with `agreementProposalId` and `pricingModel` populated from state.
8. POST /2026-02-05/getOfferTerms with the same `offerId` and {"maxResults":1} — expect 200 with one `offerTerms` entry and pagination that advances.
9. POST /2026-02-05/getListing with {"listingId":"does-not-exist"} — expect HTTP 404 `ResourceNotFoundException`, not 501 and not a hollow 200.
