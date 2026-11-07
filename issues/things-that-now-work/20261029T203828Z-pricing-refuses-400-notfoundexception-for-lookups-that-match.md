Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-unknown-filters-return-empty-results
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-unknown-filters-return-empty-results
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing|GetProducts|2xx->4xx|$.FormatVersion
Issue-Key: pricing|GetProducts|2xx->4xx|$.PriceList
Issue-Key: pricing|GetProducts|2xx->4xx|status
Issue-Key: pricing|GetAttributeValues|2xx->4xx|$.AttributeValues
Issue-Key: pricing|GetAttributeValues|2xx->4xx|status
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# pricing refuses 400 NotFoundException for lookups that match nothing instead of answering 200 with an empty list

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
Two net-new root causes sharing one fault: the Pricing read operations treat "this query selects nothing" as a client fault and refuse it with the modeled NotFoundException (HTTP 400), where AWS answers 200 with an empty result list. The emulator is up and serving pricing, so these are fidelity defects on operations that do answer.

Cause 1 of 2: GetProducts answers HTTP 400 NotFoundException for a query whose filters select no product, instead of 200 with an empty PriceList.
AWS: Smithy GetProducts (com.amazonaws.pricing, awsJson1_1) is documented as "returns a list of all products that match the filter criteria" and returns FormatVersion plus PriceList (PriceListJsonItems); NotFoundException (HTTP 400) is documented as "the requested resource can't be found" — a non-existent resource, not an empty match. Recorded behavior: the request returns HTTP 200 with FormatVersion 'aws_v1' and PriceList [] when the supplied filters (unknown field/value, or a service dimension the catalog has no offers for) match no product. An empty match is a successful query with no rows.
Emulator: answers HTTP 400 with the modeled NotFoundException, so no FormatVersion and no PriceList are returned at all ($.FormatVersion expected 'aws_v1', got None; $.PriceList expected [], got None). The refusal comes from the handler's pre-filter lookup in ops/get_products.rs, which converts an unresolved service/filter lookup into NotFoundException (message fixed text "service code '" ... "' not found", with the service code taken from the request) before the filter pass can simply match zero products. The requirement for a zero-match query is a 200 body, not any error message.
Why: "no results" and "your request is invalid" are opposite answers. A cost-explorer or inventory client that probes filter combinations sees a client-fault 400, aborts or retries pointlessly, and cannot distinguish an unsupported filter from a region/instance type that simply has no offers — whereas on AWS it reads an empty PriceList and moves on. Any code path that treats 4xx as fatal breaks on a perfectly legal query.
Fix: the GetProducts handler's lookup/validation ordering (services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_products.rs) — a query that resolves to zero matching products must fall through to the normal success serializer with FormatVersion set and PriceList empty; NotFoundException should be reserved for a resource that genuinely cannot exist.
Traces: pricing-unknown-filters-return-empty-results#task#2.

Cause 2 of 2: GetAttributeValues answers HTTP 400 NotFoundException for an attribute/service pair with no values, instead of 200 with an empty AttributeValues list.
AWS: Smithy GetAttributeValues requires ServiceCode and AttributeName and returns AttributeValues (AttributeValueList) plus optional NextToken; NotFoundException (HTTP 400) again means the resource can't be found. Recorded behavior: the request returns HTTP 200 with AttributeValues [] when the named attribute yields no values for that service code.
Emulator: answers HTTP 400 NotFoundException and returns no AttributeValues member ($.AttributeValues expected [], got None). The handler (ops/get_attribute_values.rs) refuses before paging whenever the catalog lookup misses — either the service code (message fixed text "service code '" ... "' not found") or the attribute name (fixed text "attribute '" ... "' is not available for service code '" ... "'"), with the names taken from the request. For a zero-value lookup the requirement is the 200 body, not either message.
Why: a client enumerating the attributes it discovered from DescribeServices hits a 400 on the first attribute that happens to carry no values, so the whole enumeration fails rather than yielding an empty list for that one attribute. Combined with the DescribeServices/GetAttributeValues catalog disagreement filed separately, this turns a discovery sweep into a hard error.
Fix: the GetAttributeValues handler's catalog-miss path — return 200 with an empty AttributeValues list (and no NextToken) when the pair resolves to no values, keeping NotFoundException for inputs that cannot name a resource at all.
Traces: pricing-unknown-filters-return-empty-results#task#3.

Related open issues: none cover these; the other pricing-named open issues in the tracker belong to billingconductor, outposts, bcm-pricing-calculator and sesv2, not to this service.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, serving pricing): 19 steps, 9 passed, 10 mismatched, score 47.4%.

Cause 1 — pricing-unknown-filters-return-empty-results#task#2 (GetProducts): status expected 200, got 400; $.FormatVersion expected 'aws_v1', got None; $.PriceList expected [], got None. Divergence key service=pricing|GetProducts|status=200->400|keys=$.FormatVersion,$.PriceList,status.

Cause 2 — pricing-unknown-filters-return-empty-results#task#3 (GetAttributeValues): status expected 200, got 400; $.AttributeValues expected [], got None. Divergence key service=pricing|GetAttributeValues|status=200->400|keys=$.AttributeValues,status.

Source of the refusals: ops/get_products.rs returns GetProductsErrorModeled::NotFoundException on an unresolved catalog lookup before filtering; ops/get_attribute_values.rs returns GetAttributeValuesErrorModeled::NotFoundException on an unknown service code or an attribute the catalog does not publish, both ahead of the pagination/serialization path.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture pricing-unknown-filters-return-empty-results, step task#2 — issue GetProducts with filters that select no product; expect HTTP 200 {FormatVersion: 'aws_v1', PriceList: []}, observe HTTP 400 NotFoundException with no body members.
4. Cause 2: same fixture, step task#3 — issue GetAttributeValues for an attribute/service pair with no values; expect HTTP 200 {AttributeValues: []}, observe HTTP 400 NotFoundException.
