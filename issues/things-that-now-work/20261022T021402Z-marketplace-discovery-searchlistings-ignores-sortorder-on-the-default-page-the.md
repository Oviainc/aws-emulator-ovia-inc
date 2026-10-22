Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-discovery.search-listings-default-sortorder-matches-explicit-descending marketplace-discovery.discovery-catalog-full-browse-workflow marketplace-discovery.get-offer-terms-each-term-has-single-variant marketplace-discovery.get-listing-nonexistent-id-not-found marketplace-discovery.get-listing-not-found-error marketplace-discovery.get-listing-not-found-rejected marketplace-discovery.search-listings-rating-filter-multiple-values-rejected marketplace-discovery.search-listings-rating-filter-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery.search-listings-default-sortorder-matches-explicit-descending|run#9|AssertionError|assert ascending_ids != explicit_ids, (
Issue-Key: marketplace-discovery.discovery-catalog-full-browse-workflow|run#3|AssertionError|raise AssertionError(
Issue-Key: marketplace-discovery.get-offer-terms-each-term-has-single-variant|run#3|AssertionError|raise AssertionError(
Issue-Key: marketplace-discovery.get-listing-nonexistent-id-not-found|run#11|AssertionError|assert error.get("Message") == "Listing not found", (
Issue-Key: marketplace-discovery.get-listing-not-found-error|run#11|AssertionError|assert error.get("Message") == "Listing not found", (
Issue-Key: marketplace-discovery.get-listing-not-found-rejected|run#5|AssertionError|assert error.get("Message") == "Listing not found", (
Issue-Key: marketplace-discovery.search-listings-rating-filter-multiple-values-rejected|run#7|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: marketplace-discovery.search-listings-rating-filter-out-of-range-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 0078f2cb19327c3a88b6597067286c38efcb9847

# SearchListings ignores sortOrder on the default page, the seeded catalog exposes no listing offer associations, and GetListing/range-filter refusal messages diverge

> _Note: this issue was identified against the code base at commit `0078f2cb19327c3a88b6597067286c38efcb9847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchListings

## Description
This bundle has 4 causes, all in the Marketplace Discovery service crate (its SearchListings handler, its seeded catalog state, and its GetListing / SearchListings refusal paths). They are independent and must each be closed.

Cause 1 of 4: sortOrder has no observable effect when results tie, so ASCENDING and DESCENDING return byte-identical pages.
AWS: Smithy (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json, query=operation SearchListings) models sortBy (SearchListingsSortBy, default RELEVANCE) and sortOrder (SearchListingsSortOrder, default DESCENDING) as part of the request, and the operation's own doc says results can be sorted "by relevance or customer rating", with sortOrder documented as "The sort direction. Valid values are DESCENDING and ASCENDING." sortOrder is therefore a request parameter that must reverse the result ordering; AWS returns a different page for ASCENDING than for DESCENDING on the same query, and the no-sort-parameter page equals the explicit RELEVANCE/DESCENDING page (the modeled defaults).
Emulator: for a query with no searchText, SearchListings(sortBy=RELEVANCE, sortOrder=ASCENDING) and SearchListings(sortBy=RELEVANCE, sortOrder=DESCENDING) return the SAME listingIds in the SAME order. The cause is visible in the handler: the relevance score is 0.0 for every listing when searchText is empty, so every comparison ties on score and falls through to a secondary comparator that orders by listingId ascending unconditionally — the secondary key is not reversed with sortOrder. The default-vs-explicit-DESCENDING equality the check also asserts is consequently never reached.
Why: a client that pages a catalog with sortOrder=ASCENDING silently receives the DESCENDING page. There is no error to detect, so the client's ordering contract is broken invisibly, and any UI or job that relies on "oldest/lowest first" shows the wrong records. This is the highest-severity cause in the bundle.
Fix: the SearchListings handler's ordering stage — the sort comparator must apply sortOrder to the full ordering, including the tie-break key used when scores are equal, rather than only to the score comparison.
Traces: marketplace-discovery.search-listings-default-sortorder-matches-explicit-descending.

Cause 2 of 4: no listing in the seeded catalog carries an offer association, so the listing -> offer -> terms chain is unreachable.
AWS: Smithy (query=operation GetListing) makes GetListingOutput.associatedEntities @required (ListingAssociatedEntityList, @length max 100), and query=shape ListingAssociatedEntity gives it two members: `product` ("Information about the product associated with the listing") and `offer` ("Information about the default offer associated with the listing"). GetOfferTerms exists and is bound (POST /2026-02-05/getOfferTerms, GetOfferTermsOutput.offerTerms @required, maxResults @range 1..25 default 10), and GetOffer resolves an offerId to associatedEntities that cross-reference the same productId. In AWS a listing with a default offer therefore exposes an associatedEntities element populating BOTH `product` and `offer`, which is the only documented way a client discovers an offerId from a listing.
Emulator: scanning the first 20 listings returned by SearchListings and calling GetListing on each, NOT ONE associatedEntities element has both `product` and `offer` populated — the offer side is never present. Both checks abort at that scan, before GetOfferTerms is ever called, with "no scanned listing exposed both a product and an offer association within the scan window" / "no scanned listing exposed a resolvable default offer within the scan window".
Why: GetOffer and GetOfferTerms become unreachable through the documented discovery path. A client written against AWS walks SearchListings -> GetListing -> associatedEntities[].offer.offerId -> GetOffer/GetOfferTerms; against this emulator that walk dead-ends at step three with no error, so pricing and terms cannot be reached at all even though both operations are implemented.
Fix: the service's seeded catalog state — at least one listing's associatedEntities must populate the `offer` member alongside `product`, pointing at an offerId that GetOffer resolves and whose GetOfferTerms returns a non-empty offerTerms list, with the offer's own associatedEntities referencing that same productId.
Traces: marketplace-discovery.discovery-catalog-full-browse-workflow, marketplace-discovery.get-offer-terms-each-term-has-single-variant.

Cause 3 of 4: GetListing's not-found message is request-derived and leaks internal catalog detail, where AWS returns a fixed string.
AWS: Smithy (query=operation GetListing) binds ResourceNotFoundException (errorKind client, @httpError 404) to GetListing; the status and code are modeled requirements and the emulator already gets both right. Recorded behavior only, and the rule is determinate: across three independent recordings with three DIFFERENT well-formed-but-unknown listingIds, AWS returned the identical constant message "Listing not found". The message carries no part of the request, so the rule is a fixed string rather than a template — the three differing inputs are what establish that.
Emulator: returns ResourceNotFoundException (correct code, correct 404) with a message interpolating the requested id and the catalog name, e.g. "Listing 'sim2real-nonexistent-listing-fa15928f287b' does not exist in catalog 'AWSMarketplace'".
Why: a client that matches on the not-found message (or compares it against an AWS baseline) diverges on every call, and the emulator's variant echoes a catalog identifier AWS does not disclose on this path. Positive controls pass in all three checks — a real listingId discovered via SearchListings still resolves — so only the not-found message diverges.
Fix: the GetListing handler's not-found path — emit the fixed message for ResourceNotFoundException instead of formatting the requested listingId and catalog into it.
Traces: marketplace-discovery.get-listing-nonexistent-id-not-found, marketplace-discovery.get-listing-not-found-error, marketplace-discovery.get-listing-not-found-rejected.

Cause 4 of 4: SearchListings range-filter refusals use the emulator's own wording instead of AWS's "Range filter <TYPE> ..." template.
AWS: Smithy (query=shape SearchFilter) documents SearchFilter.filterValues (@required, SearchFilterValueList @length 1..30) as "Term filters accept multiple values (OR logic). Range filters (MIN/MAX_AVERAGE_CUSTOMER_RATING) accept a single value between 0.0 and 5.0." — so both the one-value rule and the 0.0-5.0 bound are modeled, and SearchListings binds ValidationException (errorKind client, @httpError 400). The messages are built from the request (they name the offending filterType and, for the range case, the offending value), so their construction rule is a requirement. Recorded behavior gives the template: a two-value range filter yields "Range filter MIN_AVERAGE_CUSTOMER_RATING requires exactly one value"; an out-of-range value yields "Range filter MIN_AVERAGE_CUSTOMER_RATING value must be between 0.0 and 5.0, got: 6.0".
Emulator: returns ValidationException with the right code and status but its own wording — "filters: MIN_AVERAGE_CUSTOMER_RATING accepts exactly one filterValues entry, got 2" and "filters: MIN_AVERAGE_CUSTOMER_RATING expects a value between 0.0 and 5.0, got '6.0'" (note also the quoting of the value, where AWS emits it bare after "got: ").
Why: a client branching on these two refusal messages to tell "wrong number of values" from "value out of range" — the two cases share one error code, so the message is the only discriminator — matches neither string. Positive controls pass: a single-value filter of 1.0, 4.0 and 5.0 all succeed and return a non-negative totalResults, so the validation rules themselves are enforced correctly at the right boundaries.
Fix: the SearchListings handler's range-filter validation — the two refusal messages must use AWS's "Range filter <filterType> ..." template rather than the handler's "filters: ..." wording.
Traces: marketplace-discovery.search-listings-rating-filter-multiple-values-rejected, marketplace-discovery.search-listings-rating-filter-out-of-range-rejected.

Note on a related open issue, which does NOT cover these checks: '20261022T002146Z-marketplace-discovery-router-refuses-every-request-as-ambiguousservice' carries replay keys for marketplace-discovery.get-listing-not-found-rejected and marketplace-discovery.search-listings-rating-filter-out-of-range-rejected, but those keys are bound to the error-CODE assertions, which now PASS. The failures filed here are at the later Message/ordering/scan assertions and were not reported as OWNED by that issue.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator: Total 40/78 passed (51.3%); Mismatched 12, Errored 26, Skipped 0, Unloadable 0. Per-service: marketplace-discovery 40/78 (51%), 38 failed. None of the eight checks below was marked ALREADY FILED or OWNED by. (Two other failures in this service WERE marked ALREADY FILED — SearchFacets blocking 16 checks and ListFulfillmentOptions blocking 7 — and are deliberately excluded here.)

Cause 1 — sortOrder not honored [mismatch]:
  SearchListings(maxResults=5)                                        -> ['l-0001-analytics','l-0002-firewall','l-0003-datafeed','l-0004-mlkit','l-0005-devtools']
  SearchListings(maxResults=5, sortBy=RELEVANCE, sortOrder=DESCENDING) -> ['l-0001-analytics','l-0002-firewall','l-0003-datafeed','l-0004-mlkit','l-0005-devtools']
  SearchListings(maxResults=5, sortBy=RELEVANCE, sortOrder=ASCENDING)  -> ['l-0001-analytics','l-0002-firewall','l-0003-datafeed','l-0004-mlkit','l-0005-devtools']
  Expected: the ASCENDING page differs from the DESCENDING page for the same sortBy=RELEVANCE query. Actual: identical.
  Harness assertion: "sortOrder=ASCENDING must yield a different page than sortOrder=DESCENDING for the same sortBy=RELEVANCE query (... == ...); the target may be ignoring sortOrder entirely" (run line 28).
  Check: marketplace-discovery.search-listings-default-sortorder-matches-explicit-descending.
  Corroborating code citation: services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/search_listings.rs lines 192-199 — the comparator switches on input.sort_order for the score (`"ASCENDING" => left.total_cmp(right)`, else `right.total_cmp(left)`) and then unconditionally appends `.then_with(|| left_listing.listing_id.cmp(&right_listing.listing_id))`; lines 228-240 (`fn relevance`) return 0.0 for every listing when the needle is empty, so the score always ties and the unreversed listingId key decides the whole order.

Cause 2 — no listing offer associations [mismatch, 2 checks]:
  Scan performed by both checks: SearchListings(maxResults=20), then GetListing(listingId=...) for each summary, inspecting detail['associatedEntities'] for an element with both 'offer' and 'product' non-null.
  Expected: at least one such element, yielding an offerId that GetOffer resolves and GetOfferTerms returns a non-empty offerTerms list for, with the offer's associatedEntities referencing the same productId.
  Actual: no element across all 20 scanned listings has 'offer' populated.
  AssertionError (discovery-catalog-full-browse-workflow, run line 38): "no scanned listing exposed both a product and an offer association within the scan window; a listing with a resolvable default offer is required to exercise this chain"
  AssertionError (get-offer-terms-each-term-has-single-variant, run line 34): "no scanned listing exposed a resolvable default offer within the scan window"
  Note SearchListings and GetListing themselves succeed in both checks — the product side of associatedEntities IS populated (other checks resolve productIds from it), so this is specifically the missing `offer` member.

Cause 3 — GetListing not-found message [mismatch, 3 checks]. In all three the Code assertion passes (ResourceNotFoundException, HTTP 404) and only the Message assertion fails:
  GetListing(listingId='sim2real-nonexistent-listing-fa15928f287b')
    Expected Message: 'Listing not found'   Actual: "Listing 'sim2real-nonexistent-listing-fa15928f287b' does not exist in catalog 'AWSMarketplace'"
    Check: marketplace-discovery.get-listing-nonexistent-id-not-found (run line 46)
  GetListing(listingId='sim2real-behav-nonexistent-listing-aef9a0945828')
    Expected Message: 'Listing not found'   Actual: "Listing 'sim2real-behav-nonexistent-listing-aef9a0945828' does not exist in catalog 'AWSMarketplace'"
    Check: marketplace-discovery.get-listing-not-found-error (run line 33)
  GetListing(listingId='sim2real-nonexistent-listing-00000000000000000000000000000000')
    Expected Message: 'Listing not found'   Actual: "Listing 'sim2real-nonexistent-listing-00000000000000000000000000000000' does not exist in catalog 'AWSMarketplace'"
    Check: marketplace-discovery.get-listing-not-found-rejected (run line 21)
  Three different listingIds, one identical expected message: that is the evidence the AWS message is a request-independent constant and the emulator's is a template. Positive controls (GetListing on a real id discovered via SearchListings) pass in all three.

Cause 4 — range-filter refusal messages [mismatch, 2 checks]. Both return the correct Code (ValidationException, 400):
  SearchListings(filters=[{filterType:'MIN_AVERAGE_CUSTOMER_RATING', filterValues:['1.0','2.0']}], maxResults=1)
    Expected Message: 'Range filter MIN_AVERAGE_CUSTOMER_RATING requires exactly one value'
    Actual   Message: 'filters: MIN_AVERAGE_CUSTOMER_RATING accepts exactly one filterValues entry, got 2'
    Positive control (filterValues=['1.0']) passes and returns a non-negative int totalResults.
    Check: marketplace-discovery.search-listings-rating-filter-multiple-values-rejected (run line 48)
  SearchListings(filters=[{filterType:'MIN_AVERAGE_CUSTOMER_RATING', filterValues:['6.0']}])
    Expected Message: 'Range filter MIN_AVERAGE_CUSTOMER_RATING value must be between 0.0 and 5.0, got: 6.0'
    Actual   Message: "filters: MIN_AVERAGE_CUSTOMER_RATING expects a value between 0.0 and 5.0, got '6.0'"
    Positive controls (4.0 and 5.0, the documented maximum) both pass.
    Check: marketplace-discovery.search-listings-rating-filter-out-of-range-rejected (run line 24)
  Corroborating code citation: services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/search_listings.rs lines 334-350 (`fn rating_bound`) is where both strings are formatted.

Smithy evidence (model_path services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json):
- query=operation SearchListings -> input sortBy: SearchListingsSortBy, default "RELEVANCE"; sortOrder: SearchListingsSortOrder, default "DESCENDING"; maxResults @range 1..100 default 25; output totalResults @required (NonNegativeCount @range min 0) and listingSummaries @required; errors AccessDeniedException(403)/InternalServerException(500)/ThrottlingException(429)/ValidationException(400); http POST /2026-02-05/searchListings; paginated by nextToken over listingSummaries.
- query=shape SearchListingsSortOrder -> enum values [DESCENDING, ASCENDING]. query=shape SearchListingsSortBy -> enum values [RELEVANCE, AVERAGE_CUSTOMER_RATING].
- query=operation GetListing -> output associatedEntities @required (ListingAssociatedEntityList, @length max 100); errors include ResourceNotFoundException (errorKind client, httpError 404) and ValidationException (client, 400); http POST /2026-02-05/getListing.
- query=shape ListingAssociatedEntity -> structure with members `product` (ProductInformation, "Information about the product associated with the listing") and `offer` (OfferInformation, "Information about the default offer associated with the listing"); shape doc: "A product and offer associated with a listing."
- query=operation GetOfferTerms -> input offerId @required (@length 1..255, @pattern ^[\\w\\-]+$), maxResults @range 1..25 default 10; output offerTerms @required; errors include ResourceNotFoundException(404) and ValidationException(400); http POST /2026-02-05/getOfferTerms.
- query=shape SearchFilter -> filterType @required (SearchFilterType enum), filterValues @required (@length 1..30) documented: "Term filters accept multiple values (OR logic). Range filters (MIN/MAX_AVERAGE_CUSTOMER_RATING) accept a single value between 0.0 and 5.0."
- The model states nothing about the text of the not-found or range-filter messages; those are grounded in recorded responses as noted per cause.

## Steps to Reproduce
1. Clone the repo and boot the emulator; confirm Marketplace Discovery is reachable — SearchListings(maxResults=1) returns 200 with a non-empty listingSummaries array on this commit.
2. Point a botocore client for Marketplace Discovery (sdkId "Marketplace Discovery", protocol restJson1, SigV4 signing name 'aws-marketplace', endpointPrefix 'discovery-marketplace') at the emulator's data listener.
3. Cause 1 — sortOrder: issue three calls with no searchText: SearchListings(maxResults=5); SearchListings(maxResults=5, sortBy='RELEVANCE', sortOrder='DESCENDING'); SearchListings(maxResults=5, sortBy='RELEVANCE', sortOrder='ASCENDING'). Collect [s['listingId'] for s in listingSummaries] from each. Expected: the ASCENDING list differs from the DESCENDING list, and the no-parameter list equals the DESCENDING list (the modeled defaults). Observed: all three lists are identical, so sortOrder had no effect. Then confirm the mechanism: because no searchText was sent, every listing's relevance score ties, and the tie-break on listingId is applied in the same direction for both sortOrder values.
4. Cause 2 — missing offer associations: call SearchListings(maxResults=20); for each summary call GetListing(listingId=summary['listingId']) and scan detail['associatedEntities'] for an element where BOTH 'offer' and 'product' are non-null. Expected: at least one such element; take its offer['offerId'] and call GetOffer(offerId=...) (its associatedEntities must reference the same productId) and GetOfferTerms(offerId=..., maxResults=25) (offerTerms must be non-empty, each element a union with exactly one of byolPricingTerm/configurableUpfrontPricingTerm/fixedUpfrontPricingTerm/freeTrialPricingTerm/legalTerm/paymentScheduleTerm/recurringPaymentTerm/renewalTerm/supportTerm/usageBasedPricingTerm/validityTerm/variablePaymentTerm/netPaymentTerm populated). Observed: no scanned listing has 'offer' populated at all, so the offerId can never be obtained and the chain cannot be started.
5. Cause 3 — not-found message: call GetListing with three DIFFERENT well-formed but nonexistent listingIds (each matching ^[\\w\\-]+$, e.g. 'sim2real-nonexistent-listing-00000000000000000000000000000000'). Each must raise ResourceNotFoundException (HTTP 404) with Message exactly 'Listing not found' — identical for all three, since AWS's message carries no part of the request. Observed: the correct code and status, but a message of the form "Listing '<the id you sent>' does not exist in catalog 'AWSMarketplace'". Also call GetListing on a real listingId from step 3 to confirm the positive control still resolves.
6. Cause 4 — range-filter messages: call SearchListings(filters=[{'filterType':'MIN_AVERAGE_CUSTOMER_RATING','filterValues':['1.0','2.0']}], maxResults=1). Expect ValidationException with Message 'Range filter MIN_AVERAGE_CUSTOMER_RATING requires exactly one value'; observe 'filters: MIN_AVERAGE_CUSTOMER_RATING accepts exactly one filterValues entry, got 2'. Then call SearchListings(filters=[{'filterType':'MIN_AVERAGE_CUSTOMER_RATING','filterValues':['6.0']}]). Expect Message 'Range filter MIN_AVERAGE_CUSTOMER_RATING value must be between 0.0 and 5.0, got: 6.0'; observe "filters: MIN_AVERAGE_CUSTOMER_RATING expects a value between 0.0 and 5.0, got '6.0'". Confirm the boundaries are otherwise correct: filterValues of ['1.0'], ['4.0'] and ['5.0'] each return 200 with a non-negative int totalResults.
7. Re-replay simparity-behavioral-aws-v1 and re-run all eight checks in trace_ids to confirm all four causes are closed.
