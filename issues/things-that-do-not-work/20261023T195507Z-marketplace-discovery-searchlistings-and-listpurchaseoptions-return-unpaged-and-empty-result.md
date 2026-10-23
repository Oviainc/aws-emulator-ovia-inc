Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-listings-and-drill-into-listing
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-listings-and-drill-into-listing marketplace-discovery-purchase-options-filter-by-product-and-seller marketplace-discovery-purchase-options-offer-drilldown marketplace-discovery-purchase-options-offerset-drilldown marketplace-discovery-search-listings-pagination
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.nextToken
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions[*]
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions
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
Issue-Commit: 01e237c642deafa92593aaa1758fc7a0ef03fa22

# SearchListings and ListPurchaseOptions return unpaged and empty result sets

> _Note: this issue was identified against the code base at commit `01e237c642deafa92593aaa1758fc7a0ef03fa22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchListings

## Description
This bundle carries 4 CAUSES, all in marketplace-discovery's paginated read path: the two @paginated discovery entry points answer 200 but hand the client a result set that is either silently truncated with no continuation token, or empty. All four must be closed before this issue is submitted.

Cause 1 of 4: SearchListings omits `nextToken` when the match set is larger than the page it returned.
AWS: Smithy model services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json - SearchListings is @paginated {inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: listingSummaries}; SearchListingsOutput.nextToken (NextToken, @length 0..8192, @pattern ^[a-zA-Z0-9+/=]+$) is documented "If nextToken is returned, there are more results available. Make the call again using the returned token to retrieve the next page." `totalResults` is @required and is the total number of matches, not the page size. So when the returned `listingSummaries` is shorter than `totalResults`, an opaque `nextToken` MUST be present. The concrete token value is opaque and environment-specific (the recording's value is an example only).
Emulator: 200 with `nextToken` absent (`$.nextToken: missing`) even though the response reports more matches than it returned listings for.
Why: a paging client (and every AWS SDK paginator) treats an absent nextToken as end-of-results, so it stops after page one and silently loses every remaining listing. There is no error for it to notice.
Fix: SearchListings' handler/pagination seam - the page cursor emitted by crate::state::page must yield Some(token) whenever matches remain after the page, and the handler must propagate it into SearchListingsOutput.nextToken.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#1 (representative).

Cause 2 of 4: ListPurchaseOptions returns an empty `purchaseOptions` list for a PRODUCT_ID + SELLER_OF_RECORD_PROFILE_ID filter that names entities the same session just discovered.
AWS: ListPurchaseOptions is @paginated {items: purchaseOptions}; PurchaseOptionFilter documents PRODUCT_ID and SELLER_OF_RECORD_PROFILE_ID as identifier filters combined with AND across filters and OR within one. A product that has an offer must therefore surface that offer as a PurchaseOptionSummary of type OFFER, with its `associatedEntities`, `badges`, `catalog`, `purchaseOptionId` and `sellerOfRecord` populated. Recorded behavior: the step returns one purchaseOptions entry of purchaseOptionType OFFER for the filtered product/seller.
Emulator: 200 with the first element absent - `$.purchaseOptions[0]: missing (... got None)`, i.e. the list came back empty for a filter that must select at least the offer the drill-down just resolved.
Why: the purchase-option drill-down is the step that turns a discovered product into something buyable; a client gets "no purchase options exist" for a product that has them and cannot proceed.
Fix: ListPurchaseOptions' handler plus the catalog projection it reads - the entries produced by state::purchase_options()/matches_every_filter must actually resolve the PRODUCT_ID and SELLER_OF_RECORD_PROFILE_ID of the offers reachable from the listings this service serves (today the projected product_ids/seller_profile_id values do not join the ids the corpus addresses, so every filtered query falls through to zero matches).
Traces: marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (cluster representative; also affects marketplace-discovery-purchase-options-offer-drilldown#task#1).

Cause 3 of 4: ListPurchaseOptions returns `purchaseOptions: []` on the offer-set drill-down.
AWS: same modeled contract as Cause 2; an offer set in the catalog is a purchase option of purchaseOptionType OFFERSET, whose `associatedEntities` list the member offers' products. Recorded behavior: one OFFERSET/OFFER purchase option is returned.
Emulator: 200 with `$.purchaseOptions: value_mismatch (expected [ ... one entry ... ], got [])` - the whole list is empty.
Why: a client enumerating offer sets to compare bundles sees none, so the OFFERSET path of the API is unreachable through the emulator even though the model and the catalog both have it.
Fix: as Cause 2, specifically the OFFERSET branch of state::purchase_options() and the filter match for it.
Traces: marketplace-discovery-purchase-options-offerset-drilldown#task#2.

Cause 4 of 4: the dedicated SearchListings pagination scenario returns a first page that is neither the corpus's page nor continuable - `nextToken` is again absent.
AWS: as Cause 1 - with maxResults smaller than totalResults the response must carry nextToken, and the page's `listingSummaries` entries must be the matches for the requested query (each ListingSummary carrying listingId, listingName, categories, fulfillmentOptionSummaries, pricingModels, pricingUnits, publisher, reviewSummary, associatedEntities).
Emulator: 200 with `$.nextToken` among the diverging keys, plus the page's single summary differing in listingId and in every projected member (ids/names/urls here are examples, not requirements - the requirement is that the page is the query's match set and that it is continuable).
Why: the pagination contract is the only way a client walks more than maxResults listings; with no token the sweep ends at the first page.
Fix: SearchListings' handler/pagination seam, same cursor emission as Cause 1; the body divergence of this page is the catalog-content cause filed separately as the seeded-catalog issue for this service.
Traces: marketplace-discovery-search-listings-pagination#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, 28 steps, 10 passed, 12 mismatched, score 35.7%, state_reset_failures=0).

Cause 1 - fixture marketplace-discovery-search-listings-and-drill-into-listing step task#1, SearchListings: expected 200 / actual 200; detail `$.nextToken: missing (expected 'opaque-continuation-token-placeholder-000', got None)`. Expected: an opaque continuation token present because matches remain. Actual: member absent.

Cause 2 - fixture marketplace-discovery-purchase-options-filter-by-product-and-seller step task#1, ListPurchaseOptions: expected 200 / actual 200; detail `$.purchaseOptions[0]: missing (expected {'purchaseOptionType': 'OFFER', 'catalog': 'AWSMarketplace', 'purchaseOptionId': ..., 'sellerOfRecord': {...}, 'badges': [{'badgeType': 'AUTO_RENEW', ...}], 'associatedEntities': [{'offer': {...}, 'product': {...}}]}, got None)`. Same cluster: marketplace-discovery-purchase-options-offer-drilldown#task#1.

Cause 3 - fixture marketplace-discovery-purchase-options-offerset-drilldown step task#2, ListPurchaseOptions: expected 200 / actual 200; detail `$.purchaseOptions: value_mismatch (expected [ {one OFFER purchase option with associatedEntities and badges} ], got [])`.

Cause 4 - fixture marketplace-discovery-search-listings-pagination step task#1, SearchListings: expected 200 / actual 200; diverging keys include `$.nextToken` together with `$.listingSummaries[0].listingId`, `.categories[2..6]`, `.fulfillmentOptionSummaries[1..2]`, `.pricingModels[0]`, `.reviewSummary.reviewSourceSummaries[0]`.

In all four the HTTP status matched (200); the divergence is in the response body, so no client-visible error signals the loss.

## Steps to Reproduce
1. git clone the mainline checkout and build/boot the emulator from it (data listener as reported by the replay harness; /__admin/health answers 200).
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus with no arguments).
3. Cause 1: fixture marketplace-discovery-search-listings-and-drill-into-listing, step task#1 - POST /2026-02-05/searchListings with a searchText matching more listings than maxResults; observe 200 with no `nextToken` while `totalResults` exceeds the returned `listingSummaries` length.
4. Cause 2: fixture marketplace-discovery-purchase-options-filter-by-product-and-seller, step task#1 - POST /2026-02-05/listPurchaseOptions with filters PRODUCT_ID=<a product resolved from the preceding search/getListing step> and SELLER_OF_RECORD_PROFILE_ID=<that product's seller>; observe 200 with an empty purchaseOptions list.
5. Cause 3: fixture marketplace-discovery-purchase-options-offerset-drilldown, step task#2 - the offer-set drill-down ListPurchaseOptions call; observe `purchaseOptions: []`.
6. Cause 4: fixture marketplace-discovery-search-listings-pagination, step task#1 - POST /2026-02-05/searchListings with maxResults below the match count; observe 200 with `nextToken` absent, so no second page can be requested.
