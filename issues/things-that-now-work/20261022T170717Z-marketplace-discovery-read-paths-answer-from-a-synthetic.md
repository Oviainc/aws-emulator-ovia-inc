Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-listings-and-drill-into-listing
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-listings-and-drill-into-listing marketplace-discovery-search-listings-pagination marketplace-discovery-search-facets-refine-search marketplace-discovery-purchase-options-filter-by-product-and-seller marketplace-discovery-purchase-options-offer-drilldown marketplace-discovery-purchase-options-offerset-drilldown
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
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.nextToken
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
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.manufacturer.displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.manufacturer.sellerProfileId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.productId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.productName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].categories[*]
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].categories[*].categoryId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].categories[*].displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].fulfillmentOptionSummaries[*]
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].fulfillmentOptionSummaries[*].displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].fulfillmentOptionSummaries[*].fulfillmentOptionType
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].listingId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].listingName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].logoThumbnailUrl
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].pricingModels[*].displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].pricingModels[*].pricingModelType
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].pricingUnits[*].displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].pricingUnits[*].pricingUnitType
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].publisher.displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].publisher.sellerProfileId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].reviewSummary.reviewSourceSummaries[*].averageRating
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].reviewSummary.reviewSourceSummaries[*].sourceUrl
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].reviewSummary.reviewSourceSummaries[*].totalReviews
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].shortDescription
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.CATEGORY
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.FULFILLMENT_OPTION_TYPE
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.PRICING_MODEL
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions[*]
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Marketplace Discovery read paths answer from a synthetic catalog instead of the recorded records

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetListing

## Description
This issue bundles EIGHT (8) causes, all in marketplace-discovery and all one shared fault: every read path of AWSMarketplaceDiscovery (restJson1, sdkId 'Marketplace Discovery') answers 200 from a small synthetic in-process catalog (Borealis / Nimbus / Orion style records) rather than from the listing, product, offer, seller-profile, category and purchase-option records the corpus exercises. The request routing, projection and filter logic are reached and return the right SHAPE; what they read is the wrong record set, so a request for a recorded listingId/productId gets a different entity back, a filter on a recorded productId/sellerProfileId matches nothing, facet buckets are computed over the wrong match set, and a truncated search page mints no continuation token. A fixer MUST close ALL EIGHT causes below before submitting; each names its own fixture and step, and every one of the eight steps must pass on re-replay.

Note on prior issues: '20261022T132304Z-marketplace-discovery-seeded-marketplace-catalog-does-not-match-recorded-corpus' is RESOLVED (in issues/things-that-now-work/) yet all eight steps still diverge on the current mainline checkout, so it does not cover them; this is a net-new filing against the current commit. The open/resolved marketplace-catalog and marketplace-deployment issues are for other services.

Cause 1 of 8 — GetListing returns a different listing for the requested listingId.
AWS: Smithy GetListing (POST /2026-02-05/getListing), input listingId @required @length 1..255 @pattern ^[\w\-]+$; GetListingOutput declares associatedEntities, badges, catalog, categories, fulfillmentOptionSummaries, highlights, listingId, listingName, logoThumbnailUrl, longDescription, pricingModels, pricingUnits, promotionalMedia, publisher, resources, sellerEngagements, shortDescription and useCases all @required — i.e. the stored record for that id. Recorded behavior for the fixture's listingId: 200 with four categories (Data Integration, Data Analytics, Infrastructure Software, Data Preparation), fulfillmentOptionSummaries[0] fulfillmentOptionType SAAS / displayName 'SaaS', an associatedEntities[0] naming the recorded product and offer (offerName present), three highlights, and populated publisher/resources/sellerEngagements/useCases/promotionalMedia.
Emulator: 200 for the same listingId but the record differs across ~33 members: categories[0] is 'data-products'/'Data Products', categories[1] is 'financial-services-data'/'Financial Services Data' and categories[2..3] are absent; fulfillmentOptionSummaries[0] is DATA_EXCHANGE/'AWS Data Exchange'; associatedEntities[0] names a different product and offer; highlights[0..2], listingName, shortDescription, longDescription, logoThumbnailUrl, publisher.*, pricingUnits[0].*, purchaseIndicators, resources[0..3], sellerEngagements[0..1], promotionalMedia[0] and useCases[0] all carry other values.
Why: a client that drills in on a listingId it got from SearchListings renders a different product than AWS returns, and code branching on categories, fulfillmentOptionType or sellerEngagements sees a combination that never occurs for that listing on AWS.
Fix: the service's seeded catalog (the listing/product/offer/seller records in the crate's state module) and the GetListing handler's record lookup — not its output projection, which is already shaped correctly.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#2.

Cause 2 of 8 — GetProduct returns a different product, including different enum values, for the requested productId.
AWS: Smithy GetProduct (POST /2026-02-05/getProduct) returns GetProductOutput with productName, manufacturer, deployedOnAws, shortDescription, longDescription, logoThumbnailUrl, fulfillmentOptionSummaries, categories, highlights, resources and sellerEngagements required for the requested productId; errors include ResourceNotFoundException (404). Recorded behavior: 200 with deployedOnAws NOT_DEPLOYED, four categories, fulfillmentOptionSummaries[0] SAAS/'SaaS', three highlights, and the recorded manufacturer/descriptions/logo/resources/sellerEngagements.
Emulator: 200 with deployedOnAws NOT_APPLICABLE, two synthetic categories ('data-products', 'financial-services-data') and categories[2..3] absent, fulfillmentOptionSummaries[0] DATA_EXCHANGE/'AWS Data Exchange', a single highlight, and a different productName/manufacturer/shortDescription/longDescription/logoThumbnailUrl/resources/sellerEngagements/purchaseIndicators.
Why: deployedOnAws and fulfillmentOptionType are modeled enums that buyer-side logic branches on (deployment eligibility, which fulfillment flow to run); returning a different enum value for the same product id makes that logic take a branch AWS never selects.
Fix: the seeded product records in the crate's state module (same seed as cause 1), read by the GetProduct handler.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#3.

Cause 3 of 8 — ListFulfillmentOptions returns the wrong union variant for the product.
AWS: Smithy ListFulfillmentOptions (POST /2026-02-05/listFulfillmentOptions), productId @required, output fulfillmentOptions @required list of the fulfillment-option union; @paginated. Recorded behavior: 200 with fulfillmentOptions[0] = saasFulfillmentOption (fulfillmentOptionType SAAS, a fulfillmentUrl, quickLaunch, deploymentTemplates, availableFromTime).
Emulator: 200 with fulfillmentOptions[0] = dataExchangeFulfillmentOption (fulfillmentOptionType DATA_EXCHANGE, dataArtifacts) — a different union member, so a client deserializing the SaaS variant finds none.
Why: the union member chosen determines which fields exist at all; a client following the recorded SaaS fulfillment path gets no fulfillmentUrl and cannot launch the product.
Fix: the seeded fulfillment-option records for the product in the state module; the handler's union projection is driven entirely by that seed.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#4.

Cause 4 of 8 — SearchListings omits nextToken when the result set is truncated (drill-down fixture).
AWS: Smithy SearchListings is @paginated (inputToken/outputToken nextToken, pageSize maxResults, items listingSummaries); maxResults @range 1..100 default 25; output nextToken @length 0..8192 @pattern ^[a-zA-Z0-9+/=]+$ and is returned while further results remain. Recorded behavior: 200 carrying a nextToken (recorded as the opaque placeholder 'opaque-continuation-token-placeholder-000').
Emulator: 200 with nextToken absent.
Why: an SDK paginator or a hand-rolled paging client terminates after the first page and silently under-reports the catalog. The token VALUE is opaque and environment-specific — the requirement is that a token is PRESENT whenever the match set exceeds the returned page.
Fix: the paging/result-set source behind the SearchListings handler — the match set must be large enough and a token minted whenever a page is truncated.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#1.

Cause 5 of 8 — SearchListings returns different listings and no next page on the pagination fixture.
AWS: as cause 4; listingSummaries and totalResults are @required. Recorded behavior: 200 whose listingSummaries[0] is the recorded listing (its associatedEntities product and manufacturer, badges, seven categories, fulfillmentOptionSummaries, pricingModels, pricingUnits, publisher, reviewSummary.reviewSourceSummaries) plus a nextToken.
Emulator: 200 whose listingSummaries[0] is an unrelated seeded listing — different listingId, listingName, publisher, logoThumbnailUrl, a different associatedEntities product ('p-0002-firewall-appliance' / 'Nimbus Firewall Appliance' / manufacturer 'Nimbus Networks'), two synthetic categories with categories[2..6] absent, different fulfillmentOptionSummaries/pricingModels/pricingUnits/reviewSummary, shortDescription — and nextToken absent, so the recorded second page cannot be fetched.
Why: search is the entry point to every drill-down; a client paging the result set sees a catalog AWS never returns and stops after one page.
Fix: the seeded listing corpus plus the paging source feeding the SearchListings handler (same component as cause 4).
Traces: marketplace-discovery-search-listings-pagination#task#1.

Cause 6 of 8 — SearchFacets computes facet buckets over the wrong catalog and emits a flat category tree.
AWS: Smithy SearchFacets (POST /2026-02-05/searchFacets) returns totalResults and listingFacets, both @required; listingFacets is a map of facet type to facet values, each with displayName, value, count and an optional parent. Recorded behavior: 200 whose CATEGORY, PRICING_MODEL and FULFILLMENT_OPTION_TYPE entries are the facet values of the recorded catalog WITH parent links (e.g. 'Operating Systems', 'Security' and 'Data Analytics' all carrying a parent of the 'Infrastructure Software' category value, 'Implementation'/'Managed Services'/'Assessments' under 'Professional Services').
Emulator: 200 whose three facet maps are derived from its own small seeded catalog, so the values, displayNames and counts do not match, and no value carries a parent at all — the emitted hierarchy is flat.
Why: facets drive refine-search; wrong values make the client filter on a value the catalog never had, and a flat facet tree loses the parent/child refinement AWS returns, so a nested category filter can never be constructed.
Fix: the seeded catalog and its category hierarchy in the state module, plus the parent projection in the SearchFacets handler once categories carry parents.
Traces: marketplace-discovery-search-facets-refine-search#task#1.

Cause 7 of 8 — ListPurchaseOptions returns no entry for a PRODUCT_ID + seller-profile filter.
AWS: Smithy ListPurchaseOptions (POST /2026-02-05/listPurchaseOptions), filters is a PurchaseOptionFilterList @length 1..10 whose filterType targets PurchaseOptionFilterType [PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS]; output purchaseOptions @length max 100. Recorded behavior: 200 with one purchaseOptions entry — purchaseOptionType OFFER, purchaseOptionId, catalog 'AWSMarketplace', badges [AUTO_RENEW], sellerOfRecord, and associatedEntities naming the filtered product and offer.
Emulator: 200 with purchaseOptions[0] absent — nothing matched, because the seeded catalog holds neither the filtered productId nor that seller profile. Hypothesis: the availability window the handler applies against the current clock can additionally drop seeded offers whose recorded availability dates are in the past, so seeding records alone may not be sufficient.
Why: a buyer that lists purchase options for a product it just retrieved concludes the product has no purchasable offer — indistinguishable from an unlisted product — and the whole purchase flow stops.
Fix: the seeded offers/offer-sets and their availability windows in the state module; the filter matching in the ListPurchaseOptions handler already handles the modeled filter types.
Traces: marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (the same cluster also covers marketplace-discovery-purchase-options-offer-drilldown#task#1).

Cause 8 of 8 — ListPurchaseOptions returns an empty list on the offer-set drill-down.
AWS: as cause 7. Recorded behavior: 200 with a one-element purchaseOptions list (an OFFER option with its associatedEntities, badges, catalog and sellerOfRecord).
Emulator: 200 with purchaseOptions = [].
Why: the offer-set drill-down cannot proceed — there is no purchaseOptionId or offerId to carry into GetOffer/GetOfferSet/GetOfferTerms, so every downstream step of that scenario is unreachable.
Fix: the same seeded purchase-option/offer-set records and availability handling as cause 7.
Traces: marketplace-discovery-purchase-options-offerset-drilldown#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, '/__admin/health answered 200 and the :34051 data listener accepts'; steps_total 28, passed 9, mismatched 13, blocked 6, score_pct 32.1, state_reset_failures=0 — so no cause below is contaminated by leaked state). All eight clusters are verdict=mismatch with no stub_path and no owned_by. Expected = recorded AWS, actual = emulator:

1) GetListing — marketplace-discovery-search-listings-and-drill-into-listing#task#2 — status 200 vs 200. $.categories[0].categoryId expected 'f42ec728-09b3-46d2-956a-8a0cde3f8ee6' ('Data Integration'), got 'data-products' ('Data Products'); $.categories[1] expected 'adb29c06-d13c-4cfa-a2c3-944475c7e1bd' ('Data Analytics'), got 'financial-services-data' ('Financial Services Data'); $.categories[2] and [3] expected present ('Infrastructure Software', 'Data Preparation'), got None. $.fulfillmentOptionSummaries[0].fulfillmentOptionType expected 'SAAS' ('SaaS'), got 'DATA_EXCHANGE' ('AWS Data Exchange'). $.associatedEntities[0] expected the recorded product+offer (productName 'Data', offerName 'Offer created on ...', sellerProfileId 'seller-r6vsz7pygcd7u'), got a different product/offer ('Borealis Market Data Feed' / 'sp-borealis'). $.highlights[0] expected 'IBM i software front end and back end modernization', got 'Fulfilled as AWS Data Exchange'. Divergent keys in the cluster: $.associatedEntities[0], $.categories[0..3], $.fulfillmentOptionSummaries[0].displayName/.fulfillmentOptionType, $.highlights[0..2], $.listingName, $.logoThumbnailUrl, $.longDescription, $.pricingUnits[0].displayName/.pricingUnitType, $.promotionalMedia[0], $.publisher.displayName/.sellerProfileId, $.purchaseIndicators, $.resources[0..3], $.sellerEngagements[0..1], $.shortDescription, $.useCases[0].

2) GetProduct — ...drill-into-listing#task#3 — 200 vs 200. $.deployedOnAws expected 'NOT_DEPLOYED', got 'NOT_APPLICABLE'. $.fulfillmentOptionSummaries[0].fulfillmentOptionType expected 'SAAS', got 'DATA_EXCHANGE'. $.categories[0..3] expected the four recorded categories, got 'data-products'/'financial-services-data' and two absent. $.logoThumbnailUrl expected the recorded CloudFront URL, got 'https://example.com/logos/borealis-market-data.png'. Also divergent: $.highlights[0..2], $.longDescription, $.manufacturer.displayName/.sellerProfileId, $.productName, $.purchaseIndicators, $.resources[0..3], $.sellerEngagements[0..1], $.shortDescription.

3) ListFulfillmentOptions — ...drill-into-listing#task#4 — 200 vs 200. $.fulfillmentOptions[0] expected {saasFulfillmentOption: {fulfillmentOptionType 'SAAS', fulfillmentOptionDisplayName 'SaaS', fulfillmentUrl, quickLaunch 'DISABLED', deploymentTemplates [], availableFromTime}}, got {dataExchangeFulfillmentOption: {fulfillmentOptionType 'DATA_EXCHANGE', fulfillmentOptionDisplayName 'AWS Data Exchange', dataArtifacts [...]}} — a different union variant.

4) SearchListings — ...drill-into-listing#task#1 — 200 vs 200. $.nextToken expected present (recorded placeholder 'opaque-continuation-token-placeholder-000'), got None (reported as '$.nextToken: missing').

5) SearchListings — marketplace-discovery-search-listings-pagination#task#1 — 200 vs 200. $.listingSummaries[0].associatedEntities[0].product.productId expected 'fc9ecb41-af3a-4e4f-ae3d-36fb2f0405d3' ('CloudHealth', manufacturer 'VMware Tanzu CloudHealth' / '63ac9e70-1bdf-4bb2-8aef-e0a197ebafc6'), got 'p-0002-firewall-appliance' ('Nimbus Firewall Appliance', manufacturer 'Nimbus Networks' / 'sp-nimbus'). $.listingSummaries[0].categories[0] expected '44bf32d0-edc0-4357-b679-c608ac1227f9' ('IT Business Management'), got 'infrastructure-software' ('Infrastructure Software'); categories[2..6] expected present, got None. Also divergent: .listingId, .listingName, .logoThumbnailUrl, .fulfillmentOptionSummaries[0..2], .pricingModels[0].*, .pricingUnits[0].*, .publisher.*, .reviewSummary.reviewSourceSummaries[0].averageRating/.sourceUrl/.totalReviews, .shortDescription, and $.nextToken.

6) SearchFacets — marketplace-discovery-search-facets-refine-search#task#1 — 200 vs 200. $.listingFacets.CATEGORY, $.listingFacets.PRICING_MODEL and $.listingFacets.FULFILLMENT_OPTION_TYPE all value_mismatch: expected the recorded catalog's facet values with counts and parent links (e.g. {'count': 1937, 'displayName': 'Operating Systems', 'parent': <the Infrastructure Software category value>}, {'count': 2346, 'displayName': 'Implementation', 'parent': 'ba3a2bf7-795d-4d9f-83fc-6166fa98780d'}), got buckets derived from the emulator's own seeded listings with no 'parent' on any value.

7) ListPurchaseOptions — marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (cluster affected_count 1, also covering marketplace-discovery-purchase-options-offer-drilldown#task#1) — 200 vs 200. $.purchaseOptions[0] expected one entry {purchaseOptionType 'OFFER', purchaseOptionId, catalog 'AWSMarketplace', badges [{AUTO_RENEW, 'Auto-renew'}], sellerOfRecord, associatedEntities [{offer, product 'CloudHealth'}]}, got None.

8) ListPurchaseOptions — marketplace-discovery-purchase-options-offerset-drilldown#task#2 — 200 vs 200. $.purchaseOptions expected a one-element list (the OFFER option above with sellerProfileId '63ac9e70-1bdf-4bb2-8aef-e0a197ebafc6'), got [].

Model consulted: services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json — restJson1, sigv4/sigv4a, 9 operations; GetListing/GetProduct output members @required; SearchListings @paginated {nextToken, maxResults, listingSummaries} with nextToken @pattern ^[a-zA-Z0-9+/=]+$; SearchFacets output totalResults + listingFacets @required; ListFulfillmentOptions output fulfillmentOptions @required; ListPurchaseOptions filters @length 1..10 over PurchaseOptionFilterType.

Not filed from this replay (reported for completeness, not defects): three further marketplace-discovery mismatches differ ONLY in an error message whose construction rule is not established — ListFulfillmentOptions 404 (marketplace-discovery-error-resource-not-found#task#6, status and ResourceNotFoundException match; recorded message 'Product not found' vs the emulator's longer catalog-naming text), SearchListings 400 (…-error-validation-bad-inputs#task#1, identical enum membership in a different bracket order; the model's declaration order for SearchFilterType is exactly the emulator's order and no known rule produces the recorded permutation), and SearchFacets 400 (…-error-validation-bad-inputs#task#3, status 400 and ValidationException match; the emulator renders AWS's standard constraint template where the recording carries prose).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout; confirm Marketplace Discovery is reachable (SearchListings(maxResults=1) answers 200 with a non-empty listingSummaries).
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the marketplace-discovery divergence clusters.
3. Causes 1, 2, 3 and 4 — fixture marketplace-discovery-search-listings-and-drill-into-listing: POST /2026-02-05/searchListings (step task#1), /getListing (task#2), /getProduct (task#3) and /listFulfillmentOptions (task#4) with the fixture's recorded request bodies, and compare every response member against the recording. Expect task#1 to carry a nextToken, task#2 four categories and a SAAS fulfillmentOptionSummary, task#3 deployedOnAws NOT_DEPLOYED, task#4 a saasFulfillmentOption variant.
4. Cause 5 — fixture marketplace-discovery-search-listings-pagination, step task#1: POST /2026-02-05/searchListings with the recorded search criteria and maxResults; expect the recorded listingSummaries[0] and a nextToken, then fetch the recorded second page with that token.
5. Cause 6 — fixture marketplace-discovery-search-facets-refine-search, step task#1: POST /2026-02-05/searchFacets with the recorded searchText/filters/facetTypes; compare listingFacets.CATEGORY / .PRICING_MODEL / .FULFILLMENT_OPTION_TYPE values, counts and (for nested categories) parent.
6. Cause 7 — fixture marketplace-discovery-purchase-options-filter-by-product-and-seller, step task#1: POST /2026-02-05/listPurchaseOptions with the recorded PRODUCT_ID + seller-profile filters; expect one OFFER purchase option, not an empty list. Re-check marketplace-discovery-purchase-options-offer-drilldown#task#1, which is in the same cluster.
7. Cause 8 — fixture marketplace-discovery-purchase-options-offerset-drilldown, step task#2: POST /2026-02-05/listPurchaseOptions with the recorded offer-set filters; expect a one-element purchaseOptions list, then carry its id into the fixture's GetOfferSet/GetOfferTerms steps.
8. Re-replay simparity-wire-aws-v1 and confirm ALL EIGHT steps named above pass before closing: …drill-into-listing#task#1, #task#2, #task#3, #task#4; …search-listings-pagination#task#1; …search-facets-refine-search#task#1; …purchase-options-filter-by-product-and-seller#task#1; …purchase-options-offerset-drilldown#task#2.
