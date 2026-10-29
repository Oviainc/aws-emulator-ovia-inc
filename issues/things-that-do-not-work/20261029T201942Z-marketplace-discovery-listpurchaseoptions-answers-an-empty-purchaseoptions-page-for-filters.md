Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-purchase-options-filter-by-product-and-seller
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-purchase-options-filter-by-product-and-seller marketplace-discovery-purchase-options-offer-drilldown marketplace-discovery-purchase-options-offerset-drilldown
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions[*]
Issue-Key: marketplace-discovery|ListPurchaseOptions|2xx->2xx|$.purchaseOptions
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# ListPurchaseOptions answers an empty purchaseOptions page for filters the recording matches

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPurchaseOptions

## Description
This issue bundles 2 causes, both in marketplace-discovery, both of them ListPurchaseOptions returning a 200 with nothing in it where the recording returns a purchase option. Both fixtures are chained: the filter values they send were captured from this same emulator's earlier responses in the same fixture (the recording shows one of them as the capture placeholder for the seller profile id), so the filters name entities the emulator itself has just reported as existing.

Cause 1 of 2: filtering by a product and its seller of record yields no purchase option at all.
AWS: Smithy: ListPurchaseOptions, POST /2026-02-05/listPurchaseOptions, success 200, @paginated(inputToken nextToken, outputToken nextToken, pageSize maxResults, items purchaseOptions). Input `filters` -> PurchaseOptionFilterList @length(1..10) of PurchaseOptionFilter{filterType: PurchaseOptionFilterType required, filterValues required @length(1..10)}, documented "Multiple filters are combined with AND logic. Multiple values within the same filter are combined with OR logic", with PRODUCT_ID taking product identifiers and SELLER_OF_RECORD_PROFILE_ID taking seller profile identifiers. Output `purchaseOptions` -> PurchaseOptionSummaryList @length(..100). Recorded behavior: a product that has an offer is reported as a purchase option when the request filters on that product and on the seller of record of that product's offer; the element is a PurchaseOptionSummary with purchaseOptionType OFFER, the catalog it belongs to, a sellerOfRecord, a badges list, and an associatedEntities list pairing the product (productId/productName/manufacturer) with its offer (offerId/sellerOfRecord).
Emulator: 200 with no element at purchaseOptions[0] - the list is empty, so a product the emulator itself returned from SearchListings/GetProduct has no purchasable option.
Why: this is the step that turns discovery into procurement. A buyer drilling from a product to its purchase options is told there is no way to buy it, and every downstream behavior of the operation (the OFFER/OFFERSET split, badges, associated entities, paging) is unreachable because the page is always empty.
Fix: ListPurchaseOptions' handler filter matching together with the projection it reads - the AND across filters in src/ops/list_purchase_options.rs::matches_every_filter (PRODUCT_ID tested against PurchaseOptionEntry.product_ids, SELLER_OF_RECORD_PROFILE_ID tested against the single PurchaseOptionEntry.seller_profile_id) over the rows crate::state::purchase_options projects from the seeded offers, including the visibility_scope each seeded offer carries and the availability status derived from its expirationTime against the current clock. Hypothesis: the two filters can never both hold because the projected seller_profile_id is taken from the offer's sellerOfRecord while the profile id the fixture captured from an earlier response is the product's publisher/manufacturer profile id; a seeded visibility_scope or a lapsed availability window would have the same effect.
Traces: marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1 (ListPurchaseOptions, expected 200 / got 200, diverging key $.purchaseOptions[0]); the same cluster also fails marketplace-discovery-purchase-options-offer-drilldown#task#1.

Cause 2 of 2: the offer-set drilldown's ListPurchaseOptions call returns an empty list where the recording returns one purchase option.
AWS: same operation and shapes as above. Recorded behavior: after resolving an offer set, the purchase options request made in that flow answers 200 with a non-empty purchaseOptions list - one PurchaseOptionSummary whose associatedEntities pair the product with the offer bundled by that set, with its catalog, sellerOfRecord and badges populated.
Emulator: 200 with purchaseOptions serialized as [] - not a single row survives the request's filters, so the offer-set flow dead-ends.
Why: the same procurement dead-end, reached from the offer-set side: a bundled offer set the emulator serves through GetOfferSet is invisible to the operation that is supposed to list it as a purchase option, so a client cannot discover how to buy any member of the bundle.
Fix: the same component as cause 1, plus the OFFERSET branch of the projection: crate::state::purchase_options builds an OFFERSET row only from state.offer_sets entries whose offer_ids resolve in state.offers, and derives that row's product_ids transitively from those offers' associated entities - an offer set whose offers do not resolve yields a row with empty product_ids, which no PRODUCT_ID filter can ever match, and the row's seller_profile_id is the set's sellerOfRecord rather than any member offer's.
Traces: marketplace-discovery-purchase-options-offerset-drilldown#task#2 (ListPurchaseOptions, expected 200 / got 200, diverging key $.purchaseOptions).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 - marketplace-discovery-purchase-options-filter-by-product-and-seller#task#1, ListPurchaseOptions: expected status 200, actual 200; $.purchaseOptions[0] missing - expected a PurchaseOptionSummary with purchaseOptionType 'OFFER', a catalog, a sellerOfRecord, badges [{badgeType AUTO_RENEW}] and associatedEntities pairing a product with its offer; got None. affected_count 2: the cluster also covers marketplace-discovery-purchase-options-offer-drilldown#task#1. The recorded sellerProfileId in that expectation is the fixture capture placeholder, confirming the filter value was taken from an earlier step of the same chained fixture.

Cause 2 - marketplace-discovery-purchase-options-offerset-drilldown#task#2, ListPurchaseOptions: expected status 200, actual 200; $.purchaseOptions value_mismatch - expected a one-element list (PurchaseOptionSummary, purchaseOptionType 'OFFER', populated associatedEntities/badges/sellerOfRecord), got [].

Code read: src/ops/list_purchase_options.rs lines 140-256 (handler, matches_every_filter, check_values) and src/state.rs lines 915-980 (purchase_options projection, PurchaseOptionEntry.product_ids / seller_profile_id / visibility_scope / availability_status). Model reference: smithy_query operation=ListPurchaseOptions and shapes PurchaseOptionFilter, PurchaseOptionSummary on services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json.

Not claimed as requirements: the recorded purchaseOptionId, sellerProfileId and productId literals are environment-specific identifiers; the defect is that the list is empty, not which ids it holds.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: run fixture marketplace-discovery-purchase-options-filter-by-product-and-seller to step task#1 - it resolves a product and its seller from earlier steps, then POSTs /2026-02-05/listPurchaseOptions with a PRODUCT_ID filter and a SELLER_OF_RECORD_PROFILE_ID filter. Observe 200 with no purchaseOptions element. The same request shape fails identically in marketplace-discovery-purchase-options-offer-drilldown step task#1.
4. Cause 2: run fixture marketplace-discovery-purchase-options-offerset-drilldown to step task#2 - the offer-set drilldown's POST /2026-02-05/listPurchaseOptions. Observe 200 with purchaseOptions == [].
