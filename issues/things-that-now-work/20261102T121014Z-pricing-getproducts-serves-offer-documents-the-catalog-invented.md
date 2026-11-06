Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-ecc-traj-ec2-vocabulary-in-price-filters
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-ecc-traj-ec2-vocabulary-in-price-filters pricing-ecc-traj-get-products-request-mistakes pricing-ecc-traj-upstream-lookup-page-token-and-os-split
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing|GetProducts|2xx->2xx|$.PriceList[*]
Issue-Related: 20261029T203828Z-pricing-catalog-seed-diverges-from-aws-wrong-list.md
Issue-Commit: 8a15654a3e32a09451431e59a712f0f10794d652

# pricing GetProducts serves offer documents the catalog invented instead of the products a filtered query matches

> _Note: this issue was identified against the code base at commit `8a15654a3e32a09451431e59a712f0f10794d652`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
Two net-new root causes in the Pricing (AWS Price List Service) GetProducts read path. They share one fault: the aws_v1 product-offer documents the emulator pages out of its seeded catalog are not the offer documents the service publishes, so a filtered product query either matches nothing at all or is answered with an offer document for a different product. Both are mismatches on an operation that DOES answer (status 200 both ways, the emulator is up and serving pricing), so each is a fidelity defect and not a bootstrap gap.

Cause 1 of 2: a GetProducts query whose filters name a product the service publishes is answered 200 with a PriceList that does not contain it (the list is short/empty), and with no NextToken.
AWS: Smithy GetProducts (model com.amazonaws.pricing#AWSPriceListService, protocol awsJson1_1) takes required ServiceCode plus Filters (list 0..50 of Filter{Type: FilterType enum, Field len 1..1024, Value len 1..1024}) and returns FormatVersion, PriceList (PriceListJsonItems) and NextToken; it is @paginated(inputToken: NextToken, outputToken: NextToken, items: PriceList, pageSize: MaxResults range 1..100). PriceList is documented as "the list of products that match your filters ... both the product metadata and the price information". The recorded AWS response to an EC2 instance-vocabulary query (TERM_MATCH filters over instanceType/operatingSystem/tenancy/capacitystatus/preInstalledSw) carries at least one aws_v1 offer document whose product.attributes describe the requested product (recorded example: instanceType t3.nano, operatingSystem Linux, tenancy Shared, usagetype BoxUsage:t3.nano, location "US East (N. Virginia)", with terms.OnDemand and terms.Reserved each holding priceDimensions keyed <sku>.<offerTermCode>.<rateCode>), plus a NextToken because more offers match than one page holds. The requirement is that every product the service's own catalog publishes for the filtered attribute values be returned as an offer document of that shape — the recorded skus, prices, effectiveDate and token text are per-catalog/per-run data, not requirements.
Emulator: answers 200 with FormatVersion aws_v1 but PriceList[0] absent — the filtered query matches no seeded product — and NextToken null. Its seeded EC2 catalog publishes only a handful of instance types (the same five that surface through GetAttributeValues: c5.large, c5.xlarge, m5.2xlarge, m5.large, m5.xlarge), so every filter naming a t3-family (or any other real) instance type matches nothing and the handler serves an empty page.
Why: a client that discovers a vocabulary and then prices it gets HTTP 200 with zero matching offers. An empty success is indistinguishable from "this product is not priced", so a cost tool silently reports no price rather than failing, and because NextToken is also absent it never pages for the offers it is missing.
Fix: the seeded product catalog in the pricing crate's state (the per-service product seed and the offer_document builder that renders Product.offer_json) so it publishes the products the filters are expected to match, and the GetProducts handler's pagination path over the matched set. The filter matching itself (filter_matches) is not at fault — the products are simply not in the catalog.
Traces: pricing-ecc-traj-ec2-vocabulary-in-price-filters#task#4, and the same cluster at pricing-ecc-traj-get-products-request-mistakes#task#7, pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#1 and #task#4.

Cause 2 of 2: when GetProducts does return an offer document, the document's content is not the offer AWS publishes for that query.
AWS: same operation and model. Each PriceList member is a JSON string holding {product:{productFamily, attributes{...}, sku}, serviceCode, terms:{OnDemand{...}, Reserved{...}}}. For the recorded OS-split lookup the matching offer describes the requested product (recorded example: instanceType t3.micro, operatingSystem Linux, memory "1 GiB", usagetype BoxUsage:t3.micro, normalizationSizeFactor "0.5", productFamily "Compute Instance", serviceCode AmazonEC2, with an OnDemand term whose priceDimension unit is Hrs and whose description reads "$<rate> per On Demand <os> <instanceType> Instance Hour", and Reserved terms carrying Quantity/Hrs dimensions). The requirement is that the document returned for a filtered query describe the product that query selected and carry the aws_v1 structure above; the particular sku, rate and effectiveDate are catalog data.
Emulator: returns an offer document for a different product than the one the filters select ($.PriceList[0] value_mismatch, status 200 both ways) — the attributes, sku and term/rate-code keys all belong to one of its five seeded placeholder products rather than to the requested one.
Why: this is worse than the empty answer of Cause 1, because the client gets a 200 with a document that looks well-formed and then prices the wrong instance: it reads product.attributes.instanceType and terms.OnDemand...pricePerUnit for a product it never asked about.
Fix: same component as Cause 1 — the product seed and offer_document rendering in the crate's state — so that the offer document a matched product yields describes that product; the GetProducts handler's serialization of page members into PriceList is a pass-through and is not the defect.
Traces: pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#3.

Related but distinct open issue, which does NOT cover either cause: 20261029T203828Z-pricing-catalog-seed-diverges-from-aws-wrong-list.md covers the list MEMBERS of GetAttributeValues/DescribeServices/ListPriceLists, the absent GetProducts NextToken on pricing-get-products-filtered-workflow, and the unresolvable price-list file. Neither of the two causes above (GetProducts returning no offer for a matching query, and returning the wrong offer document) carries that issue's replay keys, and the replay reports both as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): 40 steps, 15 passed, 24 mismatched, score 37.5%.

Cause 1 — pricing-ecc-traj-ec2-vocabulary-in-price-filters#task#4 (GetProducts, expected 200, got 200), divergence key pricing|GetProducts|status=200->200|keys=$.NextToken,$.PriceList[0]:
  $.PriceList[0]: missing. Expected an aws_v1 offer document whose product.attributes include instanceType t3.nano, operatingSystem Linux, tenancy Shared, capacitystatus Used, preInstalledSw NA, usagetype BoxUsage:t3.nano, location "US East (N. Virginia)", productFamily "Compute Instance", serviceCode AmazonEC2, with terms.OnDemand.<sku>.JRTCKXETXF.<rateCode> (unit Hrs, description "$0.0052 per On Demand Linux t3.nano Instance Hour") and terms.Reserved; got no element at index 0.
  $.NextToken: expected present, got None.
  Affected (same cluster, 4 members): pricing-ecc-traj-ec2-vocabulary-in-price-filters#task#4, pricing-ecc-traj-get-products-request-mistakes#task#7, pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#1, pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#4.

Cause 2 — pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#3 (GetProducts, expected 200, got 200), divergence key pricing|GetProducts|status=200->200|keys=$.PriceList[0]:
  $.PriceList[0]: value_mismatch. Expected the offer document for instanceType t3.micro / operatingSystem Linux (memory "1 GiB", usagetype BoxUsage:t3.micro, normalizationSizeFactor "0.5", OnDemand description "$0.0104 per On Demand Linux t3.micro Instance Hour", Reserved terms with Quantity "Upfront Fee" and Hrs dimensions); got a different offer document from the emulator's seeded catalog.

Corroborating evidence that the catalog, not the filter logic, is the source: the same replay shows GetAttributeValues for AmazonEC2/instanceType answering c5.large, c5.xlarge, m5.2xlarge, m5.large, m5.xlarge where AWS answers a1.2xlarge, a1.4xlarge, a1.large, a1.medium, a1.metal — i.e. the seeded catalog contains none of the instance types the GetProducts fixtures filter on. The GetProducts handler (services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_products.rs lines 212-256) returns product.offer_json verbatim for every product satisfying every filter, and Product.offer_json is built from the seed (src/state.rs lines 86-127).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive a botocore 'pricing' client at the emulator with region='us-east-1'.
3. Cause 1: fixture pricing-ecc-traj-ec2-vocabulary-in-price-filters, step task#4 — get_products(ServiceCode='AmazonEC2', Filters=[TERM_MATCH instanceType=t3.nano, TERM_MATCH operatingSystem=Linux, TERM_MATCH tenancy=Shared, TERM_MATCH capacitystatus=Used, TERM_MATCH preInstalledSw=NA], bounded MaxResults). Observe HTTP 200 with an empty PriceList and no NextToken where AWS returns at least one aws_v1 offer document for that instance type plus a token.
4. Same cause, other members: pricing-ecc-traj-get-products-request-mistakes#task#7 and pricing-ecc-traj-upstream-lookup-page-token-and-os-split#task#1 / #task#4 issue the equivalent filtered queries and are answered the same way.
5. Cause 2: fixture pricing-ecc-traj-upstream-lookup-page-token-and-os-split, step task#3 — the OS-split lookup (instanceType t3.micro with operatingSystem split across Linux/Windows). Parse PriceList[0] as JSON and compare product.attributes.instanceType / product.sku / terms against the product the filters named: the returned document describes a different product.
6. Confirm the catalog is the source: call get_attribute_values(ServiceCode='AmazonEC2', AttributeName='instanceType') and observe the five c5/m5 placeholder values — none of the instance types the GetProducts fixtures filter on are in the catalog at all.
