Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-facets-refine-search
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-facets-refine-search marketplace-discovery-search-listings-pagination
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.nextToken
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# SearchListings returns no nextToken so a paginating client cannot reach past the first page

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchListings

## Description
This issue has 1 cause, in marketplace-discovery.

Cause 1 of 1: SearchListings omits the continuation token although its own totalResults says more listings match than the page it served.
AWS: Smithy: SearchListings is @paginated(inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: listingSummaries). Output members: totalResults (NonNegativeCount, required) and listingSummaries (ListingSummaryList, required), plus nextToken -> NextToken @length(0..8192) @pattern ^[a-zA-Z0-9+/=]+$, documented "If nextToken is returned, there are more results available. Make the call again using the returned token to retrieve the next page." Input maxResults -> MaxResults @range(1..100) default 25. Recorded behavior: the response carries a non-empty opaque base64 nextToken alongside the page of listingSummaries whenever the match set is larger than the page served, and the same request repeated with that token returns the following page. The requirement is the PRESENCE of a token (and that it round-trips as an input cursor for the same query) while more results remain; the token's literal bytes are an opaque, environment-specific cursor and are explicitly NOT part of the contract.
Emulator: 200 with the listingSummaries page serialized but nextToken absent (null) - both on the first page of a paginated SearchListings and on the follow-up request in the pagination fixture, and likewise on the SearchListings call made inside the facet-refinement flow.
Why: a paginating client (and every AWS SDK paginator, which stops as soon as the output token is absent) sees only the first page and silently treats a truncated catalog as the whole result set, while the required totalResults in the very same response reports a larger number. There is no way for a caller to enumerate the rest, and the input-side nextToken path is never exercised at all.
Fix: SearchListings' handler and the shared cursor it uses - the match set it pages (crate::state::search_matches, paged through crate::state::page, which mints a token only when items remain past the served page) and the placement of the minted token on the output. Hypothesis: either the keyword/filter predicate in search_matches narrows the match set to at most one page for these queries, or the token page() returns is dropped on the way into SearchListingsOutput; the paging helper itself does mint and validate tokens for the other paginated operations in this crate.
Traces: marketplace-discovery-search-facets-refine-search#task#3 (SearchListings, expected 200 / got 200, diverging key $.nextToken, expected a non-empty opaque base64 token, got None). The same cluster also fails marketplace-discovery-search-listings-pagination#task#1 and marketplace-discovery-search-listings-pagination#task#2, i.e. both the first page and the attempt to continue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

marketplace-discovery-search-facets-refine-search#task#3, SearchListings: expected status 200, actual 200; $.nextToken missing - expected a non-empty opaque base64 continuation token, got None. divergence_key service=marketplace-discovery|SearchListings|status=200->200|keys=$.nextToken, affected_count 3, affected: marketplace-discovery-search-facets-refine-search#task#3, marketplace-discovery-search-listings-pagination#task#1, marketplace-discovery-search-listings-pagination#task#2.

The recorded token's literal value (a long base64 KMS-style envelope) is not asserted here as a requirement - only that a token is returned while more results remain and that it is accepted back as the cursor for the same query.

Model reference: smithy_query operation=SearchListings on services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json - paginated {inputToken nextToken, outputToken nextToken, pageSize maxResults, items listingSummaries}; output totalResults and listingSummaries required. Code read: src/state.rs lines 593-620 (search_matches) and 1180-1196 (page / token minting), src/state.rs 1160-1178 (decode_page_token).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Run fixture marketplace-discovery-search-listings-pagination: step task#1 POSTs /2026-02-05/searchListings with a maxResults smaller than the number of matching listings. Observe 200 with listingSummaries and totalResults populated but no nextToken; step task#2's attempt to fetch the next page therefore has no cursor and also returns no token.
4. Run fixture marketplace-discovery-search-facets-refine-search step task#3 (the SearchListings call after facet refinement) and observe the same missing $.nextToken.
