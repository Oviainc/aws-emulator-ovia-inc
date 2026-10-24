Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-listings-and-drill-into-listing
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-listings-and-drill-into-listing marketplace-discovery-search-facets-refine-search
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.associatedEntities[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.categories[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.categories[*].categoryId
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.categories[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.fulfillmentOptionSummaries[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.fulfillmentOptionSummaries[*].fulfillmentOptionType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.highlights[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.listingName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.logoThumbnailUrl
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.longDescription
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.pricingUnits[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.pricingUnits[*].pricingUnitType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.promotionalMedia[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.publisher.displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.publisher.sellerProfileId
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.purchaseIndicators
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].contentType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].resourceType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].value
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.sellerEngagements[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.shortDescription
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.useCases[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.categories[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.deployedOnAws
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.fulfillmentOptionSummaries[*].displayName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.fulfillmentOptionSummaries[*].fulfillmentOptionType
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.highlights[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.logoThumbnailUrl
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.longDescription
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.manufacturer.displayName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.manufacturer.sellerProfileId
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.productName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.purchaseIndicators
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].contentType
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].displayName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].resourceType
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].value
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.sellerEngagements[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.shortDescription
Issue-Key: marketplace-discovery|ListFulfillmentOptions|2xx->2xx|$.fulfillmentOptions[*]
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.CATEGORY
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.FULFILLMENT_OPTION_TYPE
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.PRICING_MODEL
Issue-Commit: 01e237c642deafa92593aaa1758fc7a0ef03fa22

# Seeded catalog records diverge from the corpus for addressed listing, product and facet ids

> _Note: this issue was identified against the code base at commit `01e237c642deafa92593aaa1758fc7a0ef03fa22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetListing

## Description
This bundle carries 4 CAUSES with one shared fault: for the ids the corpus addresses, the emulator's seeded catalog answers 200 with a DIFFERENT record than the recorded one - the divergences are not only text, they change member TYPES, enum values, collection cardinality and union arms, which is what a client decodes against. All four must be closed before this issue is submitted. (Per-record prose, logo URLs, descriptions and generated ids in the recordings are EXAMPLES, not requirements; the requirements below are the typed/structural parts.)

Cause 1 of 4: GetListing returns a listing whose typed members contradict the recorded listing for the same listingId.
AWS: Smithy GetListing -> GetListingOutput (errors: ResourceNotFoundException 404) carries `associatedEntities` (each entry an offer + product projection), `categories` (Category list), `fulfillmentOptionSummaries` (FulfillmentOptionSummary.fulfillmentOptionType from the FulfillmentOptionType enum), `highlights`, `pricingUnits` (PricingUnit.pricingUnitType), `promotionalMedia`, `publisher`, `purchaseIndicators`, `resources` (Resource.resourceType/contentType), `sellerEngagements`, `shortDescription`/`longDescription`, `useCases`. Recorded behavior: 4 categories including the taxonomy ancestors, 3 highlights, 4 resources, fulfillmentOptionType SAAS, pricingUnitType per the recorded listing.
Emulator: 200, but `fulfillmentOptionSummaries[0].fulfillmentOptionType` is DATA_EXCHANGE instead of SAAS, `categories` holds 2 entries where 4 are recorded (`$.categories[2]`, `$.categories[3]` absent), `highlights` 1 of 3, `resources` 2 of 4 with different resourceType/contentType, and `associatedEntities[0]`, `promotionalMedia[0]`, `sellerEngagements[0..1]`, `purchaseIndicators`, `publisher` all differ.
Why: fulfillmentOptionType and pricingUnitType are ENUM values a client branches on - a buyer flow that follows SAAS takes the DATA_EXCHANGE path instead; a client that walks the category path or reads resources/promotionalMedia by index reads short or absent collections. None of it is signalled by an error.
Fix: the service's seeded catalog state (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/state.rs - the Listing records and the classified_under/support_resources projections) plus GetListing's handler projection, so the ids the corpus addresses resolve to records with the recorded fulfillment type, category path depth and collection cardinality.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#2 (representative).

Cause 2 of 4: GetProduct shows the same record divergence for the same productId, plus a wrong `deployedOnAws` enum.
AWS: Smithy GetProduct -> GetProductOutput (ResourceNotFoundException 404) carries `categories`, `deployedOnAws`, `fulfillmentOptionSummaries`, `highlights`, `logoThumbnailUrl`, `longDescription`/`shortDescription`, `manufacturer`, `purchaseIndicators`, `resources`, `sellerEngagements`. Recorded behavior: deployedOnAws = NOT_DEPLOYED, fulfillmentOptionType = SAAS, 4 categories, 3 highlights, 4 resources.
Emulator: 200 with `deployedOnAws` = NOT_APPLICABLE (a different enum value), `fulfillmentOptionSummaries[0].fulfillmentOptionType` = DATA_EXCHANGE, `categories` 2 of 4, `highlights` 1 of 3, `resources` 2 of 4, and manufacturer/sellerEngagements/purchaseIndicators differing.
Why: deployedOnAws and fulfillmentOptionType are enum members clients filter and badge on, so a product that is NOT_DEPLOYED is presented as NOT_APPLICABLE; the truncated category path breaks hierarchical browse.
Fix: as Cause 1 - the seeded Product records and GetProduct's projection in state.rs/ops/get_product.rs.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#3.

Cause 3 of 4: ListFulfillmentOptions returns a different FulfillmentOption UNION ARM for the product the corpus drills into.
AWS: Smithy ListFulfillmentOptions -> ListFulfillmentOptionsOutput.fulfillmentOptions (@required, @paginated items) is a list of the FulfillmentOption union, one arm per fulfillment type (saasFulfillmentOption, dataExchangeFulfillmentOption, amazonMachineImageFulfillmentOption, ...). Recorded behavior for this productId: a `saasFulfillmentOption` with fulfillmentOptionType SAAS, fulfillmentUrl, quickLaunch and deploymentTemplates members.
Emulator: 200 with `dataExchangeFulfillmentOption` (fulfillmentOptionType DATA_EXCHANGE, dataArtifacts) in `fulfillmentOptions[0]` - a different union member entirely, and consistent with the SAAS->DATA_EXCHANGE mismatch in Causes 1 and 2.
Why: a client deserializes the union by arm; the SaaS arm's members (fulfillmentUrl, quickLaunch) simply are not there, so SaaS subscription flows cannot be exercised against the emulator and a strict client may fail to match any known arm it expected.
Fix: the product's seeded fulfillment_options in state.rs (and the summary projection that must agree with it), so the fulfillment arm served for a product matches the one its listing/product summaries advertise.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#4.

Cause 4 of 4: SearchFacets returns a facet map whose entries do not match the corpus for CATEGORY, FULFILLMENT_OPTION_TYPE and PRICING_MODEL.
AWS: Smithy SearchFacets -> SearchFacetsOutput { totalResults @required, listingFacets @required (TypeToFacetMap of facet type -> ListingFacet list) }; ListingFacet carries `value`, `displayName`, `count` and `parent` ("The parent facet value for hierarchical facets, such as subcategories"), so the CATEGORY facet is hierarchical and subcategory entries must report a `parent`. Recorded behavior: many CATEGORY entries, several carrying `parent`, ordered by descending count, plus FULFILLMENT_OPTION_TYPE and PRICING_MODEL facet lists.
Emulator: 200 with the facet lists under all three of `$.listingFacets.CATEGORY`, `$.listingFacets.FULFILLMENT_OPTION_TYPE` and `$.listingFacets.PRICING_MODEL` diverging in entries and cardinality from the recorded map (counts themselves are catalog-size dependent and are examples; the facet-value set, the presence of `parent` on subcategory entries, and the descending-count ordering are the requirements).
Why: SearchFacets is the refine-search step - a client builds its filter UI from this map and then sends those values back as SearchListings filters, so a facet set that does not match the catalog yields filters that select nothing.
Fix: SearchFacets' handler and the facet derivation over the seeded match set (state.rs search_matches/category taxonomy), so every facet value present in the catalog is reported with its parent link.
Traces: marketplace-discovery-search-facets-refine-search#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, 28 steps, 10 passed, 12 mismatched, score 35.7%, state_reset_failures=0). Every cause below matched on HTTP status (200 expected / 200 actual) and diverged in the response body.

Cause 1 - fixture marketplace-discovery-search-listings-and-drill-into-listing step task#2 (GetListing): `$.fulfillmentOptionSummaries[0].fulfillmentOptionType: value_mismatch (expected 'SAAS', got 'DATA_EXCHANGE')`; `$.fulfillmentOptionSummaries[0].displayName: (expected 'SaaS', got 'AWS Data Exchange')`; `$.categories[2]: missing (expected {'categoryId': ..., 'displayName': 'Infrastructure Software'}, got None)`; `$.categories[3]: missing (... 'Data Preparation' ..., got None)`; `$.highlights[1]`, `$.highlights[2]`, `$.resources[2]`, `$.resources[3]`, `$.sellerEngagements[0]`, `$.sellerEngagements[1]`, `$.promotionalMedia[0]`, `$.purchaseIndicators`, `$.associatedEntities[0]`, `$.publisher.*` also listed in the cluster's diverging keys.

Cause 2 - same fixture step task#3 (GetProduct): `$.deployedOnAws: value_mismatch (expected 'NOT_DEPLOYED', got 'NOT_APPLICABLE')`; `$.fulfillmentOptionSummaries[0].fulfillmentOptionType: (expected 'SAAS', got 'DATA_EXCHANGE')`; `$.categories[2]`/`[3]: missing (... got None)`; `$.highlights[1]`/`[2]: missing (... got None)`; `$.resources[2]`/`[3]: missing`; `$.sellerEngagements[0]`/`[1]`, `$.manufacturer.*`, `$.purchaseIndicators` diverging.

Cause 3 - same fixture step task#4 (ListFulfillmentOptions): `$.fulfillmentOptions[0]: missing (expected {'saasFulfillmentOption': {'fulfillmentOptionType': 'SAAS', 'fulfillmentUrl': ..., 'quickLaunch': 'DISABLED', 'deploymentTemplates': []}}, got {'dataExchangeFulfillmentOption': {'fulfillmentOptionType': 'DATA_EXCHANGE', 'dataArtifacts': [{'resourceType': 'DATA_SET', ...}]}})` - a different union arm.

Cause 4 - fixture marketplace-discovery-search-facets-refine-search step task#1 (SearchFacets): diverging keys `$.listingFacets.CATEGORY`, `$.listingFacets.FULFILLMENT_OPTION_TYPE`, `$.listingFacets.PRICING_MODEL`; recorded CATEGORY list has many entries, several with a `parent` facet value, where the emulator's differs in entries and cardinality.

## Steps to Reproduce
1. git clone the mainline checkout and build/boot the emulator from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus with no arguments).
3. Cause 1: fixture marketplace-discovery-search-listings-and-drill-into-listing, step task#2 - POST /2026-02-05/getListing with the listingId resolved by step task#1; compare fulfillmentOptionSummaries[0].fulfillmentOptionType, the length of categories/highlights/resources, and sellerEngagements/promotionalMedia against the recorded response.
4. Cause 2: same fixture, step task#3 - POST /2026-02-05/getProduct with that listing's productId; observe deployedOnAws NOT_APPLICABLE vs NOT_DEPLOYED and the same truncated collections.
5. Cause 3: same fixture, step task#4 - POST /2026-02-05/listFulfillmentOptions {productId}; observe the dataExchangeFulfillmentOption arm where the saasFulfillmentOption arm is recorded.
6. Cause 4: fixture marketplace-discovery-search-facets-refine-search, step task#1 - POST /2026-02-05/searchFacets with the recorded searchText/facetTypes; compare listingFacets.CATEGORY (facet values and their `parent` links), .FULFILLMENT_OPTION_TYPE and .PRICING_MODEL with the recorded map, then feed a returned CATEGORY value back into SearchListings as a filter.
