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
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.manufacturer.displayName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.manufacturer.sellerProfileId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.productId
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].associatedEntities[*].product.productName
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].badges[*]
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].badges[*].badgeType
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].badges[*].displayName
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
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.nextToken
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.CATEGORY
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.FULFILLMENT_OPTION_TYPE
Issue-Key: marketplace-discovery|SearchFacets|2xx->2xx|$.listingFacets.PRICING_MODEL
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions[*]
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# Seeded marketplace catalog does not match recorded corpus: wrong record fields, empty purchase-option and facet results, missing pagination token

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetListing

## Description
This issue bundles EIGHT (8) causes, all in marketplace-discovery and all one shared fault: every read path of AWSMarketplaceDiscovery answers from a seeded catalog whose listings, products, offers, sellers and categories are NOT the records the corpus exercises. The handlers themselves filter, project and page correctly (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/*.rs); what they read is a small synthetic catalog (Orion/Nimbus records), so a request for a recorded listingId/productId gets a 200 carrying a different record, a filter on a recorded productId/sellerProfileId matches nothing, facet buckets are computed over the wrong match set, and a search that should have a further page has none. A fixer must close ALL EIGHT causes below before submitting; each names its own fixture and step.

Cause 1 of 8 — GetListing returns a different listing for the requested listingId.
AWS: Smithy GetListing (POST /2026-02-05/getListing) takes listingId (required, pattern ^[\w\-]+$) and returns GetListingOutput whose associatedEntities, badges, categories, fulfillmentOptionSummaries, highlights, listingName, logoThumbnailUrl, longDescription, pricingModels, pricingUnits, publisher, resources, sellerEngagements, shortDescription and useCases are all @required — i.e. the stored record for that id, verbatim. Recorded behavior: 200 with four categories (Data Integration, Data Analytics, Infrastructure Software, Data Preparation), fulfillmentOptionSummaries[0] SAAS/"SaaS", an associatedEntities[0] whose offer carries offerName, three highlights, publisher/resources/sellerEngagements/useCases populated.
Emulator: 200, but the record for the same listingId differs across ~30 members: categories[0] is 'data-products'/'Data Products' and categories[1..3] are absent, fulfillmentOptionSummaries[0] is DATA_EXCHANGE/'AWS Data Exchange', associatedEntities[0] names a different product and offer, highlights[1..2], resources[2..3] and sellerEngagements[0..1] are absent, and listingName/shortDescription/longDescription/logoThumbnailUrl/publisher/pricingUnits/purchaseIndicators all carry other values.
Why: a client that drilled in on a listingId obtained from SearchListings renders a different product than AWS returns, and code that reads categories, fulfillment type or seller engagements sees a shape that never occurs for that listing on AWS.
Fix: the service's seeded catalog and its listing projection in src/state.rs — the listing/product/offer/seller records the GetListing handler reads, not GetListing's own handler logic.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#2.

Cause 2 of 8 — GetProduct returns a different product for the requested productId.
AWS: Smithy GetProduct returns GetProductOutput with productName, manufacturer, deployedOnAws (DeployedOnAwsStatus enum), shortDescription, longDescription, logoThumbnailUrl, fulfillmentOptionSummaries, categories, highlights, resources, sellerEngagements all @required. Recorded behavior: 200 with deployedOnAws NOT_DEPLOYED, four categories, fulfillmentOptionSummaries[0] SAAS/'SaaS', and the recorded manufacturer/descriptions/resources.
Emulator: 200 with deployedOnAws NOT_APPLICABLE, a single 'data-products' category, fulfillmentOptionSummaries[0] DATA_EXCHANGE/'AWS Data Exchange', a different manufacturer and productName, and missing highlights/resources/sellerEngagements entries.
Why: deployedOnAws and fulfillmentOptionType are enum members clients branch on; returning a different enum value for the same product id makes buyer-side logic (deployment eligibility, fulfillment flow) take the wrong branch.
Fix: the seeded product records in src/state.rs (same seed as cause 1).
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#3.

Cause 3 of 8 — ListFulfillmentOptions returns the wrong union member for the product.
AWS: Smithy ListFulfillmentOptions (productId required) returns fulfillmentOptions (@required list of a fulfillment-option union). Recorded behavior: 200 with fulfillmentOptions[0] = saasFulfillmentOption (fulfillmentOptionType SAAS, a fulfillmentUrl, quickLaunch, deploymentTemplates).
Emulator: 200 with fulfillmentOptions[0] = dataExchangeFulfillmentOption (fulfillmentOptionType DATA_EXCHANGE, dataArtifacts) — a different union variant, so a client deserializing the SAAS variant finds none.
Why: the union member chosen determines which fields exist; a client following the recorded SaaS fulfillment path gets no fulfillmentUrl at all.
Fix: the seeded fulfillment-option records for the product in src/state.rs; the handler's union projection is driven by that seed.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#4.

Cause 4 of 8 — SearchListings returns different listings and no next page.
AWS: Smithy SearchListings is @paginated (inputToken/outputToken nextToken, pageSize maxResults, items listingSummaries) with totalResults and listingSummaries @required; nextToken (pattern ^[a-zA-Z0-9+/=]+$) is returned when more results remain. Recorded behavior: 200 whose listingSummaries[0] is the recorded listing (its associatedEntities product, badges DEPLOYED_ON_AWS, seven categories, pricingModels/pricingUnits, publisher, reviewSummary) plus a nextToken.
Emulator: 200 whose listingSummaries[0] is an unrelated seeded listing (different listingId, publisher, badges — including an extra QUICK_LAUNCH badge — a single category, different pricing and review summary) and whose nextToken is absent, so the recorded second page cannot be fetched.
Why: search results are the entry point to every drill-down; a client paging the result set stops after one page and never sees the listings AWS returns. The token VALUE is opaque and environment-specific — what is required is that a token is present whenever further results exist.
Fix: the seeded listing corpus and the result-set/paging source in src/state.rs feeding ops/search_listings.rs.
Traces: marketplace-discovery-search-listings-pagination#task#1.

Cause 5 of 8 — SearchListings omits nextToken on the drill-down fixture's first page.
AWS: as cause 4 — nextToken is returned while more results remain. Recorded behavior: 200 carrying a nextToken (recorded as an opaque placeholder value).
Emulator: 200 with nextToken absent.
Why: a paginating client (or an SDK paginator) terminates after the first page and silently under-reports the catalog.
Fix: same paging source as cause 4 — the match set must be large enough and the token minted whenever a page is truncated.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#1.

Cause 6 of 8 — SearchFacets returns facet buckets for a different catalog.
AWS: Smithy SearchFacets returns totalResults and listingFacets (@required map of facet type to facet values, each with displayName, value and count). Recorded behavior: 200 whose CATEGORY, PRICING_MODEL and FULFILLMENT_OPTION_TYPE entries are the facet values of the recorded catalog, including parent category relationships.
Emulator: 200 whose buckets are computed over its own small seeded catalog, so the values, displayNames, counts and parent links do not match; src/ops/search_facets.rs additionally reports no parent for any value ("the catalog's categories are flat"), while the recorded CATEGORY facets nest children under parents.
Why: facets drive the refine-search UI; wrong values mean a client then filters on a value the recorded catalog never had, and a flat facet tree loses the parent/child refinement AWS returns.
Fix: the seeded catalog plus its category hierarchy in src/state.rs (and the parent projection in ops/search_facets.rs once categories have parents).
Traces: marketplace-discovery-search-facets-refine-search#task#1.

Cause 7 of 8 — ListPurchaseOptions returns an empty list for a PRODUCT_ID + SELLER_OF_RECORD_PROFILE_ID filter.
AWS: Smithy ListPurchaseOptions takes filters (PurchaseOptionFilterList, 1..10) and returns purchaseOptions; the documented filter types include PRODUCT_ID and SELLER_OF_RECORD_PROFILE_ID. Recorded behavior: 200 with one purchaseOptions entry (purchaseOptionType OFFER, its associatedEntities product/offer, badges AUTO_RENEW, catalog AWSMarketplace, sellerOfRecord).
Emulator: 200 with purchaseOptions[0] absent — nothing matched, because the seeded catalog holds neither the filtered productId nor the sellerProfileId. Hypothesis: the availability window applied in ops/list_purchase_options.rs via state.purchase_options(now_secs(), locale) can also drop seeded offers whose recorded availability dates are in the past, so seeding the records is not sufficient on its own.
Why: a buyer that filters purchase options for a product it just retrieved concludes the product has no purchasable offer, which is indistinguishable from an unlisted product.
Fix: the seeded offers/offer-sets and their availability windows in src/state.rs; the filter matching in ops/list_purchase_options.rs is already correct for the documented filter types.
Traces: marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (same cluster also covers marketplace-discovery-purchase-options-offer-drilldown#task#1).

Cause 8 of 8 — ListPurchaseOptions returns [] on the offer-set drill-down.
AWS: as cause 7. Recorded behavior: 200 with one OFFER purchase option carrying its associatedEntities, badges, catalog and sellerOfRecord.
Emulator: 200 with purchaseOptions = [].
Why: the offer-set drill-down scenario cannot proceed — there is no purchaseOptionId to carry into GetOffer/GetOfferSet/GetOfferTerms.
Fix: same seeded purchase-option records and availability handling as cause 7.
Traces: marketplace-discovery-purchase-options-offerset-drilldown#task#2.

Related open issues (neither covers these steps): the marketplace-catalog and marketplace-deployment issues are for other services.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 32.1%, 13 mismatched steps of 28; emulator_up=true, state_reset_failures=0). Per cause, expected (recorded AWS) vs actual (emulator):
1) GetListing marketplace-discovery-search-listings-and-drill-into-listing#task#2 — status 200 vs 200; $.categories[0].categoryId expected 'f42ec728-09b3-46d2-956a-8a0cde3f8ee6'/'Data Integration', got 'data-products'/'Data Products'; $.categories[1..3] expected present, got None; $.fulfillmentOptionSummaries[0].fulfillmentOptionType expected 'SAAS', got 'DATA_EXCHANGE'; $.associatedEntities[0] expected the recorded product+offer (offerName present), got a different product/offer; ~30 keys diverge in total ($.listingName, $.shortDescription, $.longDescription, $.logoThumbnailUrl, $.publisher.*, $.pricingUnits[0].*, $.purchaseIndicators, $.resources[0..3], $.sellerEngagements[0..1], $.useCases[0], $.highlights[0..2]).
2) GetProduct #task#3 — $.deployedOnAws expected 'NOT_DEPLOYED', got 'NOT_APPLICABLE'; $.fulfillmentOptionSummaries[0].fulfillmentOptionType expected 'SAAS', got 'DATA_EXCHANGE'; $.categories[0] expected 'Data Integration', got 'Data Products', $.categories[1..3] missing; $.productName/$.manufacturer.*/$.shortDescription/$.longDescription/$.logoThumbnailUrl/$.resources/$.sellerEngagements all diverge.
3) ListFulfillmentOptions #task#4 — $.fulfillmentOptions[0] expected {saasFulfillmentOption: {fulfillmentOptionType 'SAAS', fulfillmentUrl, quickLaunch 'DISABLED', deploymentTemplates []}}, got {dataExchangeFulfillmentOption: {fulfillmentOptionType 'DATA_EXCHANGE', dataArtifacts [...]}}.
4) SearchListings marketplace-discovery-search-listings-pagination#task#1 — $.listingSummaries[0].listingId, .publisher.*, .associatedEntities[0].product.*, .badges[0].*, .categories[0..6], .fulfillmentOptionSummaries[0..2], .pricingModels[0].*, .pricingUnits[0].*, .reviewSummary.reviewSourceSummaries[0].* all diverge; $.listingSummaries[0].badges[1] extra ({'badgeType': 'QUICK_LAUNCH'}); $.nextToken expected present, got absent.
5) SearchListings marketplace-discovery-search-listings-and-drill-into-listing#task#1 — $.nextToken expected present (recorded placeholder 'opaque-continuation-token-placeholder-000'), got None.
6) SearchFacets marketplace-discovery-search-facets-refine-search#task#1 — $.listingFacets.CATEGORY, $.listingFacets.PRICING_MODEL and $.listingFacets.FULFILLMENT_OPTION_TYPE value_mismatch: expected the recorded catalog's facet values with counts and 'parent' links (e.g. 'Operating Systems' with parent 'Infrastructure Software'), got buckets derived from the emulator's own seeded listings with no parent on any value.
7) ListPurchaseOptions marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (cluster also affects marketplace-discovery-purchase-options-offer-drilldown#task#1) — $.purchaseOptions[0] expected one OFFER entry (badges AUTO_RENEW, catalog 'AWSMarketplace', associatedEntities product+offer, sellerOfRecord), got None.
8) ListPurchaseOptions marketplace-discovery-purchase-options-offerset-drilldown#task#2 — $.purchaseOptions expected a one-element list, got [].
Model consulted: services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json (restJson1; GetListing/GetProduct required output members; SearchListings/SearchFacets/ListPurchaseOptions/ListFulfillmentOptions @paginated with nextToken pattern ^[a-zA-Z0-9+/=]+$). Emulator code inspected: src/ops/search_facets.rs (flat-categories comment, facet projection), src/ops/list_purchase_options.rs (filter matching + state.purchase_options(now_secs(), locale)).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the marketplace-discovery divergences.
3. Cause 1/2/3/5: fixture marketplace-discovery-search-listings-and-drill-into-listing, steps task#2 (GetListing), task#3 (GetProduct), task#4 (ListFulfillmentOptions), task#1 (SearchListings nextToken) — POST /2026-02-05/getListing, /getProduct, /listFulfillmentOptions and /searchListings with the fixture's recorded bodies and compare each response body member against the recording.
4. Cause 4: fixture marketplace-discovery-search-listings-pagination, step task#1 — POST /2026-02-05/searchListings with the recorded search criteria and maxResults; observe a different listingSummaries[0] and no nextToken.
5. Cause 6: fixture marketplace-discovery-search-facets-refine-search, step task#1 — POST /2026-02-05/searchFacets with the recorded searchText/filters; compare listingFacets.CATEGORY / .PRICING_MODEL / .FULFILLMENT_OPTION_TYPE buckets (values, counts, parent).
6. Cause 7: fixture marketplace-discovery-purchase-options-filter-by-product-and-seller, step task#1 — POST /2026-02-05/listPurchaseOptions with filters PRODUCT_ID + SELLER_OF_RECORD_PROFILE_ID from the recording; observe purchaseOptions empty.
7. Cause 8: fixture marketplace-discovery-purchase-options-offerset-drilldown, step task#2 — POST /2026-02-05/listPurchaseOptions with the recorded offer-set filters; observe purchaseOptions = [].
8. All eight steps must pass on re-replay before this issue is closed.
