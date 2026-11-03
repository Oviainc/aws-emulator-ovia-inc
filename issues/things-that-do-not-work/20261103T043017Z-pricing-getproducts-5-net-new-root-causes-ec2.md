Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pricing.ecprice-region-code-and-location-filters-resolve-the-same-sku pricing.ecprice-upstream-filter-set-identifies-exactly-one-sku pricing.ecprice-windows-sku-carries-its-own-operation-and-a-higher-rate pricing.ecprice-dropping-capacitystatus-admits-reservation-skus pricing.ecprice-term-match-values-compare-case-insensitively pricing.ecprice-get-products-unknown-filter-type-rejected
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing.ecprice-region-code-and-location-filters-resolve-the-same-sku|_one_product#2|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: pricing.ecprice-upstream-filter-set-identifies-exactly-one-sku|run#2|AssertionError|assert len(full["PriceList"]) == 1, (
Issue-Key: pricing.ecprice-windows-sku-carries-its-own-operation-and-a-higher-rate|_one_product#2|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: pricing.ecprice-dropping-capacitystatus-admits-reservation-skus|run#7|AssertionError|assert statuses == ["AllocatedCapacityReservation", "UnusedCapacityReservation", "Used"], (
Issue-Key: pricing.ecprice-term-match-values-compare-case-insensitively|_one_product#3|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: pricing.ecprice-get-products-unknown-filter-type-rejected|run#8|AssertionError|assert (code, message) == ("InvalidParameterException", expected_message), (
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# pricing GetProducts: 5 net-new root causes — EC2 catalog publishes no Windows or t3.nano SKUs and only capacitystatus=Used, TERM_MATCH compares case-sensitively, and the filter-type enum refusal names the wrong value set

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
Five net-new behavioral root causes replayed under the `### pricing` suite heading, all in the Price List Query API's GetProducts read path. Every one is a mismatch on an operation that DOES answer (HTTP 200 / HTTP 400 as appropriate, the emulator is up and serving pricing), so each is a fidelity defect and not a bootstrap gap.

Cause 1 of 5: the seeded AmazonEC2 catalog publishes no Windows SKU for the instance types it prices, so the canonical on-demand filter set with operatingSystem=Windows matches nothing.
AWS: recorded behavior only (the Smithy model — com.amazonaws.pricing#AWSPriceListService, awsJson1_1 — only constrains the request: GetProducts takes required ServiceCode plus Filters (list 0..50 of Filter{Type: FilterType, Field len 1..1024, Value len 1..1024}) and returns FormatVersion/PriceList/NextToken; PriceList is documented as "the list of products that match your filters"). The recorded behavior is that the seven-filter on-demand lookup — TERM_MATCH on instanceType, operatingSystem, location (or regionCode), tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel — matches EXACTLY ONE SKU for operatingSystem=Windows just as it does for Linux, for every (instanceType, region) the EC2 plane can launch, and that Windows SKU is a different product from the Linux one (its own sku, licenseModel "License included", operation RunInstances:0002, a higher OnDemand Hrs rate). The requirement is the per-OS coverage, not any particular sku or rate.
Emulator: the identical filter set with operatingSystem=Windows is answered 200 with an empty PriceList and no NextToken — 0 SKUs — while the Linux form of the same lookup answers one SKU. The catalog seed is the source: services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs EC2_COMPUTE_PLAN (lines 1214-1257) carries only two Windows plans at all, each listing the single instance type m5.large (us-west-2 and ap-northeast-1); every other (region, instance type) pair is Linux-only.
Why: a client that prices a Windows instance — licence/showback attribution, right-sizing, budget guards — gets HTTP 200 with zero matching offers. An empty success is indistinguishable from "this product is not priced", so the cost tool silently reports no Windows cost instead of failing, and cannot even compare Windows against Linux.
Fix: the seeded product catalog in the pricing crate's state (the EC2 compute plan / product seed in src/state.rs), so each priced instance type is published under both operating systems in the regions it is offered.
Traces: pricing.ecprice-region-code-and-location-filters-resolve-the-same-sku, pricing.ecprice-upstream-filter-set-identifies-exactly-one-sku

Cause 2 of 5: the catalog prices no t3.nano product at all, under any operating system or region.
AWS: recorded behavior only. t3.nano is an on-demand instance type the Price List API publishes; the seven-filter lookup for instanceType=t3.nano, operatingSystem=Linux matches exactly one SKU with OnDemand terms priced per Hrs. The requirement is that every instance type the EC2 control plane can launch is priced by the Price List catalog (so an estimate of a running instance can be derived), not any specific rate.
Emulator: the lookup for t3.nano (even for Linux, where the lookup works for t3.micro) is answered 200 with an empty PriceList. src/state.rs EC2_INSTANCE_TYPES (lines 1153-1169) seeds t2.micro/small/medium, t3.micro/small/medium/large, m5/c5/r5/m6g shapes — no t3.nano entry — and ec2_products() (line 1477) skips any plan entry with no instance-type seed.
Why: the instance type a caller is actually running is unpriceable, so the whole cost-estimate path returns nothing for it; the client either divides by zero or reports a cost of zero for a real running instance.
Fix: the instance-type seed table and the compute plan in the pricing crate's state, so the catalog prices the instance types the emulator's EC2 plane offers.
Traces: pricing.ecprice-windows-sku-carries-its-own-operation-and-a-higher-rate

Cause 3 of 5: capacitystatus is a seeded constant "Used", so the catalog publishes no capacity-reservation SKUs and the field is not load-bearing.
AWS: recorded behavior only. Dropping capacitystatus=Used from the otherwise-identical filter set widens the result from one SKU to three, whose capacitystatus values are AllocatedCapacityReservation, UnusedCapacityReservation and Used; the Used one is the same SKU the strict lookup returned. That is why the field is part of the documented on-demand filter set at all.
Emulator: the loosened lookup returns only the single capacitystatus=Used product — "without capacitystatus the lookup matched capacity statuses ['Used']". src/state.rs line 1492 hardcodes ("capacitystatus", "Used") on every compute product draft, so no other value exists in the catalog.
Why: a client that omits capacitystatus (or that uses MaxResults=1 over the loose filter set, as cost tools do) silently believes the filter is redundant. Against real AWS the same code can price an arbitrary one of three SKUs — a reservation rate instead of the on-demand rate — and the emulator makes that whole class of bug untestable.
Fix: the EC2 compute product seed in the pricing crate's state, so each priced (instanceType, OS, region) publishes the reservation capacity-status variants beside the Used one.
Traces: pricing.ecprice-dropping-capacitystatus-admits-reservation-skus

Cause 4 of 5: TERM_MATCH filter values are compared case-SENSITIVELY, so a re-cased value matches nothing.
AWS: recorded behavior only (the model's Filter$Type doc says TERM_MATCH "Returns only products that match both the given filter field and the given value" without stating case handling). The recorded behavior is that the value comparison is case-insensitive: operatingSystem='linux', operatingSystem='WINDOWS' and instanceType='T3.MICRO' each resolve to the SAME single SKU as the canonical casing, and the matched product still reports the CANONICAL attribute values ('t3.micro', 'Linux').
Emulator: the re-cased lookup is answered 200 with an empty PriceList (0 SKUs), while the canonical-cased lookup returns one. services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_products.rs filter_matches() line 278 compares TERM_MATCH/EQUALS with `field_value == Some(filter.value.as_str())`, a byte-exact comparison.
Why: a client that forwards a user- or config-supplied attribute value (the EC2 API itself reports the platform in a different case than the Price List attribute) gets zero products where AWS prices the instance, and again reads an empty 200 as "not priced".
Fix: the TERM_MATCH (and EQUALS) comparison in the GetProducts handler's filter_matches, so value comparison is case-insensitive while the returned product keeps its canonical attribute values.
Traces: pricing.ecprice-term-match-values-compare-case-insensitively

Cause 5 of 5: the InvalidParameterException for an out-of-enum filter Type names the wrong enum value set in its message.
AWS: recorded behavior only for the message text; the model declares FilterType with values [TERM_MATCH, EQUALS, CONTAINS, ANY_OF, NONE_OF] and GetProducts models InvalidParameterException (HTTP 400). AWS refuses Filters[n].Type='EXACT' with code InvalidParameterException and the message "1 validation error detected: Value '<value>' at 'filters.<index>.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_CONTAIN, ANY_OF, TERM_MATCH, NONE_OF, CONTAINS, EQUALS]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: " and the bracketed value set itself, which is a service-side constant listing SIX members in that order (it includes TERM_CONTAIN, which the public model omits). Variable parts taken from the request: the rejected value ('EXACT') and the member path index ('filters.1.member.type').
Emulator: same status and same error code, but the bracketed set is the public model's five members in model order — "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_MATCH, EQUALS, CONTAINS, ANY_OF, NONE_OF]" — because the refusal is rendered by the framework's generic enum-constraint validation from the on-disk model enum.
Why: the enum value set is part of the error message a client logs, asserts on, or shows a user; a message naming a different set (and a different number of members) than the service's diverges from the AWS error contract for this error class.
Fix: the enum-constraint refusal message for pricing's FilterType — the validation/serialization path that renders the "enum value set" clause for this member (or an explicit check in the GetProducts handler, whose own hand-written message at src/ops/get_products.rs lines 197-210 is also not AWS's wording and would diverge if it ever fired).
Traces: pricing.ecprice-get-products-unknown-filter-type-rejected

Open issues that look similar but do NOT cover these causes, and none of which the replay reports as owning any of the six checks above: 20261102T121014Z-pricing-getproducts-serves-offer-documents-the-catalog-invented.md (wire-corpus; the catalog missing t3-family products and serving the wrong offer document, keyed on wire fixture steps, and its evidence describes a five-product c5/m5 catalog that has since been reseeded — t3.micro Linux now resolves, Windows/t3.nano/capacity-reservation coverage does not); 20261029T203828Z-pricing-catalog-seed-diverges-from-aws-wrong-list.md (list MEMBERS of GetAttributeValues/DescribeServices/ListPriceLists and the unresolvable price-list file); 20261102T121014Z-pricing-getproducts-400-validation-refusals-carry-no-error.md (GetProducts 400s with NO Message at all — here a Message IS present and only its value set diverges); 20261029T205548Z-pricing-renders-pattern-validation-messages-with-the-regex.md (@pattern anchors, a different constraint class, and it owns only the two pattern checks).

## Evidence
Replayed failing checks under `### pricing` (none of the six marked ALREADY FILED / OWNED by). Suite total 21/44 passed; pricing 21/30 (9 failed, of which 2 are OWNED and 1 is excluded as a runner TLS artifact).

Cause 1 — Windows SKUs absent:
- pricing.ecprice-upstream-filter-set-identifies-exactly-one-sku -> AssertionError: "Windows: the filter set matched 0 SKUs at MaxResults=100" (the Linux iteration of the same loop passed).
- pricing.ecprice-region-code-and-location-filters-resolve-the-same-sku -> AssertionError: the filter set [{'Type':'TERM_MATCH','Field':'instanceType','Value':'t3.micro'}, {'Type':'TERM_MATCH','Field':'operatingSystem','Value':'Windows'}, {'Type':'TERM_MATCH','Field':'location','Value':'US West (Oregon)'}, {'Type':'TERM_MATCH','Field':'tenancy','Value':'Shared'}, {'Type':'TERM_MATCH','Field':'preInstalledSw','Value':'NA'}, {'Type':'TERM_MATCH','Field':'capacitystatus','Value':'Used'}, {'Type':'TERM_MATCH','Field':'licenseModel','Value':'No License required'}] matched 0 SKUs: [] (the Linux iteration, including the regionCode-keyed form, passed).
- Source: src/state.rs EC2_COMPUTE_PLAN lines 1242 and 1250 are the only Windows plans, each listing exactly ["m5.large"].

Cause 2 — t3.nano absent:
- pricing.ecprice-windows-sku-carries-its-own-operation-and-a-higher-rate -> AssertionError: the filter set [... instanceType 't3.nano', operatingSystem 'Linux', location 'US West (Oregon)', tenancy 'Shared', preInstalledSw 'NA', capacitystatus 'Used', licenseModel 'No License required'] matched 0 SKUs: [] — it fails on the LINUX lookup, before it can compare Windows.
- Source: src/state.rs EC2_INSTANCE_TYPES lines 1153-1169 contain no t3.nano seed.

Cause 3 — capacitystatus constant:
- pricing.ecprice-dropping-capacitystatus-admits-reservation-skus -> AssertionError: "without capacitystatus the lookup matched capacity statuses ['Used']", expected ['AllocatedCapacityReservation', 'UnusedCapacityReservation', 'Used'].
- Source: src/state.rs line 1492, ("capacitystatus", "Used".to_string()) on every compute draft.

Cause 4 — case-sensitive TERM_MATCH:
- pricing.ecprice-term-match-values-compare-case-insensitively -> AssertionError: the filter set [... instanceType 't3.micro', operatingSystem 'linux', location 'US West (Oregon)', tenancy 'Shared', preInstalledSw 'NA', capacitystatus 'Used', licenseModel 'No License required'] matched 0 SKUs: [] — the same filters with 'Linux' return one SKU in the immediately preceding call.
- Source: src/ops/get_products.rs line 278.

Cause 5 — wrong enum value set in the refusal message:
- pricing.ecprice-get-products-unknown-filter-type-rejected -> AssertionError: Type='EXACT' was answered with ('InvalidParameterException', "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_MATCH, EQUALS, CONTAINS, ANY_OF, NONE_OF]"), expected ('InvalidParameterException', "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_CONTAIN, ANY_OF, TERM_MATCH, NONE_OF, CONTAINS, EQUALS]"). Code and status agree; only the bracketed value set diverges.
- Model: smithy_query shape=FilterType on services/service-com-amazonaws-pricing-awspricelistservice/smithy-model/com-amazonaws-pricing-awspricelistservice.json returns exactly the five-member list the emulator echoes.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the six checks appear under `### pricing`, unmarked.
2. Standalone repros with a botocore 'pricing' client pinned to region us-east-1 (the Price List Query API region), ServiceCode='AmazonEC2', and FS = the seven TERM_MATCH filters [instanceType=<type>, operatingSystem=<os>, location=<region display name> (or regionCode=<region>), tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel=<No License required for Linux / License included for Windows>]:
   - Cause 1: get_products(ServiceCode='AmazonEC2', Filters=FS with instanceType='t3.micro', operatingSystem='Windows', MaxResults=100) must return exactly one SKU (its own sku, operation RunInstances:0002, an OnDemand Hrs rate above the Linux one) and no NextToken; today PriceList is empty. The same call with operatingSystem='Linux' returns one SKU.
   - Cause 2: the same call with instanceType='t3.nano', operatingSystem='Linux' must return exactly one SKU with OnDemand terms; today PriceList is empty for every OS and region.
   - Cause 3: drop the capacitystatus filter from FS (t3.micro / Linux) and page with MaxResults=100: the matched products' capacitystatus values must be AllocatedCapacityReservation, UnusedCapacityReservation and Used, with the Used one being the SKU the full filter set returns; today only Used comes back.
   - Cause 4: issue FS with operatingSystem='linux' (and separately instanceType='T3.MICRO', operatingSystem='WINDOWS'): each must resolve to the same single SKU as the canonical casing, and the returned product.attributes must still read instanceType='t3.micro' / operatingSystem='Linux'; today each returns an empty PriceList.
   - Cause 5: get_products(ServiceCode='AmazonEC2', Filters=[dict(FS[0], Type='EXACT')] + FS[1:], MaxResults=1) must be refused HTTP 400 InvalidParameterException with Message "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_CONTAIN, ANY_OF, TERM_MATCH, NONE_OF, CONTAINS, EQUALS]" (rejected value and member index from the request); today the bracketed set reads [TERM_MATCH, EQUALS, CONTAINS, ANY_OF, NONE_OF].
