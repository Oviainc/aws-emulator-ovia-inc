Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-error-validation-bad-inputs
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-error-validation-bad-inputs
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchFacets|4xx->2xx|$.message
Issue-Key: marketplace-discovery|SearchFacets|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: marketplace-discovery|SearchFacets|4xx->2xx|status
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# SearchFacets accepts searchText containing a newline and returns 200 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchFacets

## Description
This issue has ONE (1) cause.

Cause 1 of 1: SearchFacets performs no constraint check on searchText, so a value the model forbids is accepted and answered successfully.
AWS: Smithy SearchFacetsInput$searchText targets SearchText with @length min 1 max 512 and @pattern "[^\n]*" — a searchText containing a newline violates the member's pattern, so the request is rejected before the query runs. SearchFacets declares the service-common ValidationException (errorKind client, httpError 400). Recorded behavior: HTTP 400 with header x-amzn-errortype naming ValidationException (recorded as 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.marketplace.discovery.api.v20260205/') and a message identifying the offending member (recorded example: 'searchText must not contain newlines'). The requirement is the 400 status, the ValidationException error code and that the error names searchText and its constraint; the exact recorded wording is only an example.
Emulator: HTTP 200 with a normal SearchFacets payload, no x-amzn-errortype header and no message member — the newline-bearing searchText is run as a query. services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/search_facets.rs validates locale (resolve_requested_locale) and facetTypes (requested_facet_types) and then does input.search_text...trim(), but never checks the value against the SearchText pattern or length.
Why: a client relying on AWS's rejection cannot distinguish malformed input from a legitimately empty/odd result set: against AWS it catches ValidationException and fixes the query, against the emulator it silently proceeds on facets computed from a query AWS would never have executed. Any test that asserts the 400 passes on AWS and fails here, and input-sanitisation bugs upstream stay hidden.
Fix: SearchFacets' input validation — enforce the SearchText @pattern/@length constraints for SearchFacetsInput$searchText (either in the shared member-constraint validation the request walk runs before handle, or at the top of handle in ops/search_facets.rs) and refuse with 400 ValidationException naming searchText. Note the same model constraint applies to SearchListingsInput$searchText, so fix it where both operations share it.
Traces: marketplace-discovery-error-validation-bad-inputs#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture marketplace-discovery-error-validation-bad-inputs, step task#3 (SearchFacets), verdict mismatch:
- status: expected 400, got 200
- $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.marketplace.discovery.api.v20260205/', got None
- $.message: expected 'searchText must not contain newlines' (example wording), got None
Model: services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json — SearchFacets (POST /2026-02-05/searchFacets), input member searchText -> SearchText @length 1..512 @pattern [^\n]*; errors include ValidationException (client, httpError 400).
Code: src/ops/search_facets.rs handle() validates locale and facetTypes only; searchText is taken with input.search_text.as_deref().unwrap_or("").trim() and never constraint-checked.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Fixture marketplace-discovery-error-validation-bad-inputs, step task#3: POST /2026-02-05/searchFacets with the recorded body whose searchText contains an embedded newline.
4. Observe HTTP 200 with a facets payload; AWS answers HTTP 400 with x-amzn-errortype ValidationException and a message naming searchText.
5. Re-replay that step after the fix; it must report 400/ValidationException.
