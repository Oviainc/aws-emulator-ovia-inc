Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-discovery.get-listing-malformed-id-pattern-rejected marketplace-discovery.get-listing-invalid-id-pattern-rejected marketplace-discovery.search-listings-searchtext-too-long-rejected marketplace-discovery.list-purchase-options-invalid-filter-type-rejected
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery.get-listing-malformed-id-pattern-rejected|run#5|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: marketplace-discovery.get-listing-invalid-id-pattern-rejected|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: marketplace-discovery.search-listings-searchtext-too-long-rejected|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: marketplace-discovery.list-purchase-options-invalid-filter-type-rejected|run#7|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 0078f2cb19327c3a88b6597067286c38efcb9847

# Marketplace Discovery constraint-validation messages diverge from AWS's ValidationException format in four distinct ways

> _Note: this issue was identified against the code base at commit `0078f2cb19327c3a88b6597067286c38efcb9847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetListing

## Description
This bundle has 4 causes. All four are in the component that renders a Smithy constraint violation into the `message` of a `ValidationException`. In every one of them the emulator already returns the RIGHT error code (`ValidationException`, Smithy errorKind client, @httpError 400) and the right HTTP status, and already uses AWS's outer template `1 validation error detected: Value '<value>' at '<member-path>' failed to satisfy constraint: <constraint-clause>`. What diverges is how the renderer fills that template, and each of the four is a separate rule in that renderer, so each must be closed separately.

Cause 1 of 4: the pattern-constraint clause emits the model's regex verbatim, anchors included, where AWS emits it unanchored.
AWS: Smithy (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json, query=operation GetListing) gives GetListingInput.listingId @required, @length 1..255, @pattern ^[\w\-]+$, with ValidationException (errorKind client, @httpError 400) bound to the operation. The message is built from the request — it names the violating member and its constraint — so its construction rule is a requirement, not an example. Recorded behavior shows AWS renders the pattern clause as `Member must satisfy regular expression pattern: [\w\-]+`: the leading `^` and trailing `$` of the modeled pattern are stripped before the regex is interpolated.
Emulator: emits `Member must satisfy regular expression pattern: ^[\w\-]+$`, i.e. the modeled pattern string copied through unchanged.
Why: clients and test suites that assert on, log-scrape, or surface the constraint clause to end users (and SDK-side re-validation that compiles the echoed pattern) see a different pattern than AWS ever emits for the same rejected input; the anchors make the echoed regex a different expression from the one AWS reports.
Fix: the shared constraint-violation message renderer's pattern-constraint branch — strip leading `^` / trailing `$` from the modeled pattern before interpolation.
Traces: marketplace-discovery.get-listing-malformed-id-pattern-rejected, marketplace-discovery.get-listing-invalid-id-pattern-rejected.

Cause 2 of 4: the echoed value is not entity-escaped, where AWS entity-escapes it twice.
AWS: same operation and same member as Cause 1 (listingId @pattern ^[\w\-]+$). Recorded behavior only: for a rejected listingId containing an ampersand, AWS echoes that ampersand as `&amp;amp;` — the value is run through HTML/XML entity escaping twice before being interpolated into `Value '<value>' at 'listingId'`. Hypothesis: AWS's validation frontend escapes once when building the message and the serializer escapes again. The rule is the double escape of the request-derived value; the surrounding template is unchanged.
Emulator: interpolates the raw value, so a literal `&` appears where AWS has `&amp;amp;`.
Why: a client that renders the message in a web UI, or compares it against an AWS-recorded baseline, gets different text for the same input; any value carrying `&`, `<` or `>` diverges.
Fix: the same constraint-violation message renderer — apply the value-escaping step to the echoed value before interpolation, not just for the pattern constraint but wherever `Value '<value>'` is produced.
Traces: marketplace-discovery.get-listing-invalid-id-pattern-rejected.

Cause 3 of 4: the echoed value is truncated to a byte-count placeholder, where AWS echoes it in full.
AWS: Smithy (query=operation SearchListings) gives SearchListingsInput.searchText @length min 1 max 512, with ValidationException @httpError 400 among SearchListings' errors. For a 513-character searchText AWS answers `1 validation error detected: Value '<the full 513-character value>' at 'searchText' failed to satisfy constraint: Member must have length less than or equal to 512` — the complete offending value is echoed, however long it is. The message is built from the request, so this is a requirement.
Emulator: returns the same template but replaces the tail of the value with an elision marker, e.g. `Value 'xxxxx…xxxxx... [513 bytes]' at 'searchText' failed to satisfy constraint: Member must have length less than or equal to 512`. The length clause and the error code are correct; only the value echo is clipped.
Why: the message no longer contains the input that was rejected, so a client that parses it back out to report which value failed — the usual reason to read this field at all — cannot recover it, and the length-constraint path is the one place where the value is by definition long enough to be truncated.
Fix: the same constraint-violation message renderer — remove the length cap / byte-count elision applied to the echoed value.
Traces: marketplace-discovery.search-listings-searchtext-too-long-rejected.

Cause 4 of 4: the enum-constraint clause lists the enum's permitted values in model-declaration order, not AWS's order.
AWS: Smithy (query=shape PurchaseOptionFilterType) declares the enum in the order PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS; ListPurchaseOptionsInput.filters is a PurchaseOptionFilterList (@length 1..10) whose member filterType targets that enum, and ListPurchaseOptions binds ValidationException @httpError 400. Recorded behavior only: for an unrecognized filterType, AWS renders the clause as `Member must satisfy enum value set: [PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, PRODUCT_ID, AVAILABILITY_STATUS, SELLER_OF_RECORD_PROFILE_ID]` — a fixed order that is NOT the model's declaration order. The member path `filters.1.member.filterType` is already correct in the emulator.
Emulator: renders `[PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS]` — exactly the model's declaration order, confirming the renderer walks the model's member list.
Why: the enum value set in the message is the documented discovery path for which filter types a caller may send; a client (or a generated fixture) that compares it against AWS's response, or that splits it positionally, diverges. The set membership is identical, so this is the lowest-impact cause in the bundle, but it is a distinct rule in the same renderer.
Fix: the same constraint-violation message renderer's enum-constraint branch — it needs the service's emitted value order for the enum rather than the model's declaration order.
Traces: marketplace-discovery.list-purchase-options-invalid-filter-type-rejected.

Note on a related open issue, which does NOT cover these checks: '20261022T002146Z-marketplace-discovery-router-refuses-every-request-as-ambiguousservice' carries replay keys for marketplace-discovery.get-listing-malformed-id-pattern-rejected and others, but those keys are bound to the error-CODE assertions (`assert error.get("Code") == "ValidationException"`), which now PASS — the router resolves and the handlers run. The failures filed here are at the subsequent `Message` assertions and were not reported as OWNED by that issue.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator: Total 40/78 passed (51.3%); Mismatched 12, Errored 26, Skipped 0, Unloadable 0. Per-service: marketplace-discovery 40/78 (51%), 38 failed. All four checks below are reported [mismatch] — the call was made, the emulator answered, and the error Code matched; only the Message assertion failed. None was marked ALREADY FILED or OWNED by.

Cause 1 — pattern anchors (two checks, identical divergence):
  GetListing(listingId='not a valid id!')
    Code: ValidationException (MATCHES AWS)
    Expected Message: "1 validation error detected: Value 'not a valid id!' at 'listingId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w\\-]+"
    Actual   Message: "1 validation error detected: Value 'not a valid id!' at 'listingId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w\\-]+$"
    Check: marketplace-discovery.get-listing-malformed-id-pattern-rejected (assertion at run line 23)
  The same `^`/`$` divergence recurs in marketplace-discovery.get-listing-invalid-id-pattern-rejected (assertion at run line 54).

Cause 2 — unescaped ampersand:
  GetListing(listingId='not a valid id! (has spaces & punctuation)')
    Code: ValidationException (MATCHES AWS)
    Expected Message: "1 validation error detected: Value 'not a valid id! (has spaces &amp;amp; punctuation)' at 'listingId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w\\-]+"
    Actual   Message: "1 validation error detected: Value 'not a valid id! (has spaces & punctuation)' at 'listingId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w\\-]+$"
    Check: marketplace-discovery.get-listing-invalid-id-pattern-rejected. The check's own comment states the AWS rule: "AWS echoes the value with its '&' entity-escaped twice." This check exhibits BOTH Cause 1 and Cause 2, which is why it appears in both trace lists.

Cause 3 — truncated value echo:
  SearchListings(searchText='x'*513, maxResults=1)
    Code: ValidationException (MATCHES AWS)
    Expected Message echoes all 513 'x' characters, then "... at 'searchText' failed to satisfy constraint: Member must have length less than or equal to 512"
    Actual   Message echoes a clipped prefix of the value followed by the literal marker "... [513 bytes]", then the identical constraint clause.
    Positive controls in the same check PASS: searchText='cloud infrastructure' and searchText='x'*512 (the exact boundary) both return 200 with totalResults, so the length constraint itself is enforced at the right boundary.
    Check: marketplace-discovery.search-listings-searchtext-too-long-rejected (assertion at run line 48)

Cause 4 — enum value set order:
  ListPurchaseOptions(filters=[{filterType:'NOT_A_REAL_FILTER_TYPE', filterValues:['x']}, {filterType:'PRODUCT_ID', filterValues:[<a real productId>]}])
    Code: ValidationException (MATCHES AWS)
    Expected Message: "1 validation error detected: Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.1.member.filterType' failed to satisfy constraint: Member must satisfy enum value set: [PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, PRODUCT_ID, AVAILABILITY_STATUS, SELLER_OF_RECORD_PROFILE_ID]"
    Actual   Message: "... Member must satisfy enum value set: [PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS]"
    The member path `filters.1.member.filterType` is byte-identical — only the bracketed list order differs.
    Check: marketplace-discovery.list-purchase-options-invalid-filter-type-rejected (assertion at run line 43)

Smithy evidence (model_path services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json):
- query=operation GetListing -> input listingId: @required, @length {min 1, max 255}, @pattern "^[\\w\\-]+$"; errors include ValidationException (errorKind client, httpError 400) and ResourceNotFoundException (client, 404); http POST /2026-02-05/getListing.
- query=operation SearchListings -> input searchText: @length {min 1, max 512}, @pattern "[^\n]*"; maxResults @range 1..100 default 25; errors include ValidationException (client, 400); http POST /2026-02-05/searchListings.
- query=operation ListPurchaseOptions -> input filters: PurchaseOptionFilterList @length {min 1, max 10}; errors include ValidationException (client, 400); http POST /2026-02-05/listPurchaseOptions.
- query=shape PurchaseOptionFilterType -> enum, values in declaration order: [PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS]. This is exactly the emulator's emitted order, which is the evidence that the renderer is sourcing the list from the model's member order.
- The model states nothing about message text, value escaping, anchor stripping or enum ordering; those four points are grounded in recorded AWS responses as noted per cause.

## Steps to Reproduce
1. Clone the repo and boot the emulator; confirm Marketplace Discovery is reachable (SearchListings(maxResults=1) must return 200 with a non-empty listingSummaries array — it does on this commit, so the earlier AmbiguousService routing refusal no longer applies).
2. Point a botocore client for Marketplace Discovery (sdkId "Marketplace Discovery", protocol restJson1, SigV4 signing name 'aws-marketplace', endpointPrefix 'discovery-marketplace') at the emulator's data listener.
3. Cause 1 — pattern anchors: call GetListing(listingId='not a valid id!'). Expect ValidationException (HTTP 400) whose Message ends with "Member must satisfy regular expression pattern: [\\w\\-]+". Observe the identical message except the pattern is rendered anchored as "^[\\w\\-]+$".
4. Cause 2 — value escaping: call GetListing(listingId='not a valid id! (has spaces & punctuation)'). Expect the echoed value to read "not a valid id! (has spaces &amp;amp; punctuation)" (the ampersand entity-escaped twice). Observe a raw "&". Note this call also reproduces Cause 1.
5. Cause 3 — value truncation: call SearchListings(searchText='x'*513, maxResults=1). Expect ValidationException whose Message echoes the complete 513-character value. Observe a clipped value ending in the marker "... [513 bytes]". Then confirm the boundary is otherwise right: SearchListings(searchText='x'*512, maxResults=1) must return 200 with totalResults (it does).
6. Cause 4 — enum order: discover a real productId via SearchListings(maxResults=20) -> listingSummaries[].associatedEntities[].product.productId, then call ListPurchaseOptions(filters=[{'filterType':'NOT_A_REAL_FILTER_TYPE','filterValues':['x']},{'filterType':'PRODUCT_ID','filterValues':[productId]}]). Expect the enum clause "[PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, PRODUCT_ID, AVAILABILITY_STATUS, SELLER_OF_RECORD_PROFILE_ID]". Observe the model's declaration order "[PRODUCT_ID, SELLER_OF_RECORD_PROFILE_ID, PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, AVAILABILITY_STATUS]".
7. In every step above confirm the error Code is already ValidationException and the HTTP status already 400 — the divergence is confined to the Message construction, so a fix must not change the code or status.
8. Re-replay simparity-behavioral-aws-v1 and re-run the four checks in trace_ids to confirm all four causes are closed.
