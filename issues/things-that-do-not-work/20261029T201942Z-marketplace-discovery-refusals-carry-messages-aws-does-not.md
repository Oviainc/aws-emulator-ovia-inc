Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-error-resource-not-found
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-error-resource-not-found marketplace-discovery-error-validation-bad-inputs
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|GetOfferSet|4xx->4xx|$.message
Issue-Key: marketplace-discovery|SearchListings|4xx->4xx|$.message
Issue-Key: marketplace-discovery|SearchFacets|4xx->4xx|$.message
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# marketplace-discovery refusals carry messages AWS does not word that way on GetOfferSet, SearchListings and SearchFacets

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOfferSet

## Description
This issue bundles 3 causes, all in marketplace-discovery. In each one the emulator returns the right HTTP status and the right error shape but a `message` the service does not word that way. For restJson1 the message is the only human-readable part of a fault and AWS's wording for each of these error classes is fixed text plus values taken from the request, so every one of these is a defect in how the refusal is built, not a per-run difference. None of the three diverging messages contains a request id, a clock reading or any other per-run value.

Cause 1 of 3: GetOfferSet's ResourceNotFoundException spells the resource label as prose instead of the modelled resource name.
AWS: Smithy: GetOfferSet errors ResourceNotFoundException (errorKind client, httpError 404), structure member `message` -> ExceptionMessage, documented "The specified resource doesn't exist." The model does not state the message text, so recorded behavior only: the recording answers 404 with message 'OfferSet not found'. The whole message is fixed text of the form "<ResourceName> not found", and the resource name is the looked-up resource spelled as the model's shape family spells it (OfferSet / OfferSetId / GetOfferSet), not re-spelled as English prose. No part of it is request-derived.
Emulator: 404 with message 'Offer set not found' - same status and same error shape, but the resource name is re-spelled with an interior space and a lowercase 's'.
Why: a client or test that keys the no-such-offer-set case off the message text (the only discriminator the shape offers beyond the shared 404) does not recognise the emulator's refusal, and a human diffing a transcript sees a fault this API never emits.
Fix: GetOfferSet's handler, where it builds the not-found refusal, and the crate's shared not-found message builder it calls - the resource label it passes in must be the modelled resource name. Traces: src/ops/get_offer_set.rs:165-169 passes "Offer set" into crate::state::not_found_message.
Traces: marketplace-discovery-error-resource-not-found#task#4 (GetOfferSet, expected 404 / got 404, diverging key $.message).

Cause 2 of 3: the enum-value-set listing in SearchListings' ValidationException is rendered in model declaration order rather than the fixed order this service emits.
AWS: Smithy: SearchListings.filters -> SearchFilterList (@length 1..30) of SearchFilter whose `filterType` targets the SearchFilterType enum; a value outside the enum is refused with ValidationException (errorKind client, httpError 400). Recorded behavior for the message: "1 validation error detected: Value '<rejected value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY]". The variable parts are the rejected value and the member path, both taken from the request. The bracketed value set is FIXED text - a constant of the service, independent of the request - and the order AWS emits it in is NOT the model's declaration order.
Emulator: same status, same shape, same member path and same echoed value, but it renders the value set in the model's declaration order: [MIN_AVERAGE_CUSTOMER_RATING, MAX_AVERAGE_CUSTOMER_RATING, CATEGORY, PUBLISHER, FULFILLMENT_OPTION_TYPE, PRICING_MODEL, PRICING_UNIT, DEPLOYED_ON_AWS, NUMBER_OF_PRODUCTS].
Why: the enumerated set is the actionable part of this refusal - it is what tells a caller which filter types exist. A client that parses or snapshots it, and any transcript comparison, sees a constant that differs from the service's, while every value is in fact supported.
Fix: the input-validation path's enum renderer as this service configures it (the SearchFilterType value-set listing used to build ValidationException messages) - the set must be emitted in this service's fixed order, not in model order.
Traces: marketplace-discovery-error-validation-bad-inputs#task#1 (SearchListings, expected 400 / got 400, diverging key $.message).

Cause 3 of 3: SearchFacets' newline refusal for `searchText` is left to the generic @pattern walk, so the service's own message never appears.
AWS: Smithy: SearchFacets.searchText -> SearchText, @length(1..512), @pattern "[^\n]*" (unanchored); ValidationException (client, httpError 400). Recorded behavior: 400 with message 'searchText must not contain newlines'. That message is entirely fixed text apart from naming the offending member; it carries no "N validation error detected" prefix, does not echo the submitted value, and does not expose the regular expression.
Emulator: 400 with the generic framework pattern message instead - "1 validation error detected: Value '<submitted searchText, newline included>' at 'searchText' failed to satisfy constraint: Member must satisfy regular expression pattern: (?:[^\n]*)". It also echoes the rejected input back verbatim, including the newline, which the service's own message deliberately does not do.
Why: a caller gets a different error class of text for a check the service words specifically, and the emulator leaks the compiled pattern and the raw input into the fault; anything matching on the documented refusal does not match.
Fix: SearchFacets' validation - add the service-level searchText newline check and have it raise this service's own message, instead of letting the framework's generic @pattern message escape. The same member on SearchListings takes the same pattern and should be checked by the same code. Traces: src/state.rs:23-24 documents a `validate_member_patterns` helper for exactly "the member-@pattern check the framework's input walk cannot make for an unanchored expression", but no such function exists anywhere in src/state.rs (3780 lines, no definition), so the check currently falls through to the generic walk.
Traces: marketplace-discovery-error-validation-bad-inputs#task#3 (SearchFacets, expected 400 / got 400, diverging key $.message).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 46.4%, 12 of 28 steps mismatched).

Cause 1 - marketplace-discovery-error-resource-not-found#task#4, GetOfferSet: expected status 404, actual 404; $.message value_mismatch: expected 'OfferSet not found', got 'Offer set not found'.

Cause 2 - marketplace-discovery-error-validation-bad-inputs#task#1, SearchListings: expected status 400, actual 400; $.message value_mismatch. Expected "1 validation error detected: Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.1.member.filterType' failed to satisfy constraint: Member must satisfy enum value set: [MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY]"; got the same sentence with the set ordered "[MIN_AVERAGE_CUSTOMER_RATING, MAX_AVERAGE_CUSTOMER_RATING, CATEGORY, PUBLISHER, FULFILLMENT_OPTION_TYPE, PRICING_MODEL, PRICING_UNIT, DEPLOYED_ON_AWS, NUMBER_OF_PRODUCTS]", which is exactly the order SearchFilterType declares its members in (smithy_query shape=SearchFilterType).

Cause 3 - marketplace-discovery-error-validation-bad-inputs#task#3, SearchFacets: expected status 400, actual 400; $.message value_mismatch: expected 'searchText must not contain newlines', got "1 validation error detected: Value 'line one\nline two' at 'searchText' failed to satisfy constraint: Member must satisfy regular expression pattern: (?:[^\n]*)".

Model references: smithy_query(model_path="services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json") for operations GetOfferSet/SearchListings/SearchFacets and shapes ResourceNotFoundException, ValidationException, SearchFilterType.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: POST /2026-02-05/getOfferSet with an offerSetId that matches ^[\w\-]+$ but names no offer set in the catalog (fixture marketplace-discovery-error-resource-not-found, step task#4). Observe 404 with message 'Offer set not found' where the recording has 'OfferSet not found'.
4. Cause 2: POST /2026-02-05/searchListings with a filters list whose second element has a filterType outside SearchFilterType (fixture marketplace-discovery-error-validation-bad-inputs, step task#1). Observe the 400 ValidationException message listing the enum value set in model declaration order.
5. Cause 3: POST /2026-02-05/searchFacets with a searchText containing a newline (fixture marketplace-discovery-error-validation-bad-inputs, step task#3). Observe the generic "1 validation error detected ... regular expression pattern" message instead of 'searchText must not contain newlines'.
