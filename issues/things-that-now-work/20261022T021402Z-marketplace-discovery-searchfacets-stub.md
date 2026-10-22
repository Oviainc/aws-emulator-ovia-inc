Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-discovery.get-product-deployed-on-aws-matches-filter marketplace-discovery.search-facets-category-counts-consistent-with-search-listings marketplace-discovery.search-facets-category-present-with-counts marketplace-discovery.search-facets-count-matches-filtered-search-total marketplace-discovery.search-facets-facet-types-scoped-to-request marketplace-discovery.search-facets-filter-matches-search-listings-total marketplace-discovery.search-facets-invalid-facet-type-rejected marketplace-discovery.search-facets-returns-requested-facet-types-only marketplace-discovery.search-facets-total-results-matches-search-listings marketplace-discovery.search-listings-category-filter-monotonic-reduction marketplace-discovery.search-listings-category-filter-narrows-results marketplace-discovery.search-listings-nonexistent-category-returns-empty marketplace-discovery.search-listings-pricing-model-filter-matches marketplace-discovery.search-listings-pricing-model-filter-matches-results marketplace-discovery.search-listings-too-many-filters-rejected marketplace-discovery.search-listings-totalresults-stable-across-sort-order
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchFacets|NotImplemented
Issue-Commit: 0078f2cb19327c3a88b6597067286c38efcb9847

# Implement marketplace-discovery SearchFacets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0078f2cb19327c3a88b6597067286c38efcb9847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchFacets

## Description
`SearchFacets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/search_facets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `SearchFacets` call, so each one unblocks only when `SearchFacets` works: `marketplace-discovery.get-product-deployed-on-aws-matches-filter`, `marketplace-discovery.search-facets-category-counts-consistent-with-search-listings`, `marketplace-discovery.search-facets-category-present-with-counts`, `marketplace-discovery.search-facets-count-matches-filtered-search-total`, `marketplace-discovery.search-facets-facet-types-scoped-to-request`, `marketplace-discovery.search-facets-filter-matches-search-listings-total`, `marketplace-discovery.search-facets-invalid-facet-type-rejected`, `marketplace-discovery.search-facets-returns-requested-facet-types-only`, `marketplace-discovery.search-facets-total-results-matches-search-listings`, `marketplace-discovery.search-listings-category-filter-monotonic-reduction`, `marketplace-discovery.search-listings-category-filter-narrows-results`, `marketplace-discovery.search-listings-nonexistent-category-returns-empty`, `marketplace-discovery.search-listings-pricing-model-filter-matches`, `marketplace-discovery.search-listings-pricing-model-filter-matches-results`, `marketplace-discovery.search-listings-too-many-filters-rejected`, `marketplace-discovery.search-listings-totalresults-stable-across-sort-order`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/ops/search_facets.rs`. Replaying the corpus, the emulator refused the `SearchFacets` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SearchFacets operation: operation SearchFacets is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the marketplace-discovery emulator
Call SearchFacets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
