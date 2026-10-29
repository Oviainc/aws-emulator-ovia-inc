Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-attribute-values-pagination
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-attribute-values-pagination pricing-discover-services-and-attributes pricing-get-products-filtered-workflow pricing-price-list-download-workflow
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing|GetAttributeValues|2xx->2xx|$.AttributeValues[*]
Issue-Key: pricing|DescribeServices|2xx->2xx|$.Services[*]
Issue-Key: pricing|GetAttributeValues|2xx->2xx|$.NextToken
Issue-Key: pricing|GetProducts|2xx->2xx|$.NextToken
Issue-Key: pricing|ListPriceLists|2xx->2xx|$.PriceLists[*]
Issue-Key: pricing|GetPriceListFileUrl|2xx->4xx|$.Url
Issue-Key: pricing|GetPriceListFileUrl|2xx->4xx|status
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# pricing catalog seed diverges from AWS: wrong list members, no pagination tokens, and an unresolvable price-list file

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAttributeValues

## Description
Six net-new root causes in the Pricing (AWS Price List Service) read surface. They share one fault: the service's seeded catalog is a placeholder that is both too small and inconsistent with itself, so every list operation returns members AWS does not return, the modeled pagination tokens are never minted, and the price-list file a client is told to fetch cannot be resolved. All six are mismatches on operations that DO answer (the emulator is up and serving pricing), so each is a fidelity defect, not a bootstrap gap.

Cause 1 of 6: GetAttributeValues returns attribute values that are not the service's published values, in no defined order.
AWS: Smithy GetAttributeValues (model com.amazonaws.pricing, protocol awsJson1_1) returns AttributeValues (AttributeValueList) for the required (ServiceCode, AttributeName) pair, documented as "a list of attribute values ... similar to the details in a Price List API offer file" — i.e. the values the service actually publishes for that attribute, paged with MaxResults (range 1..10000) and NextToken. The recorded AWS response for AmazonEC2 instanceType begins a1.2xlarge, a1.4xlarge, a1.large, a1.medium, a1.metal — ascending lexicographic order over the full published value set.
Emulator: returns a five-element placeholder set derived from a handful of seeded products — c5.large, c5.xlarge, m5.2xlarge, m5.large, m5.xlarge — so index 0..4 all differ from AWS. Status is 200 both ways.
Why: a client enumerating valid values for an attribute (to build a filter, validate user input, or drive a UI) is handed instance types that do not exist in the recorded catalog and never sees the ones that do, so every downstream GetProducts filter it constructs from the result matches nothing on AWS.
Fix: the service's seeded pricing catalog in the crate's state (the product/attribute seed data that GetAttributeValues derives values from via catalog.attribute_values), plus a defined ascending order over that value set.
Traces: pricing-attribute-values-pagination#task#1 (and #task#2, the second page of the same enumeration).

Cause 2 of 6: DescribeServices reports an AttributeNames set for a service code that does not match the service's published attributes.
AWS: Smithy DescribeServices returns Services (ServiceList) where each Service carries ServiceCode and AttributeNames — "the metadata for one service". The recorded AWS response for AmazonEC2 reports the attribute names termType, productFamily, servicecode, location, instanceType, usagetype.
Emulator: reports a different, hard-coded 23-element list (capacitystatus, currentGeneration, instanceFamily, instanceType, licenseModel, location, locationType, maxIopsvolume, maxThroughputvolume, memory, operatingSystem, operation, physicalProcessor, preInstalledSw, regionCode, servicecode, servicename, storageMedia, tenancy, usagetype, vcpu, volumeApiName, volumeType) — so $.Services[0] diverges wholesale. Status is 200 both ways.
Why: DescribeServices is the discovery entry point: a client reads AttributeNames and then calls GetAttributeValues/GetProducts for each one. Advertising attributes the catalog does not actually publish makes those follow-up calls fail or come back empty, and omitting ones it does publish hides filterable dimensions.
Fix: the DescribeServices handler's service-metadata source in the seeded catalog — AttributeNames must be exactly the attribute set the same catalog serves to GetAttributeValues, so discovery and value lookup agree.
Traces: pricing-discover-services-and-attributes#task#1.

Cause 3 of 6: GetAttributeValues never returns NextToken even when more values remain than the requested page holds.
AWS: Smithy marks GetAttributeValues @paginated(inputToken: NextToken, outputToken: NextToken, items: AttributeValues, pageSize: MaxResults); NextToken is documented as "the pagination token that indicates the next set of results to retrieve". The recorded response for the bounded request returns a first page plus a non-empty NextToken (the fixture asserts its presence, not its text — the token value itself is per-run and is NOT a requirement).
Emulator: returns the page with NextToken absent (null), and the page members are the placeholder values (c5.large, c5.xlarge) rather than the recorded a1.large, c4.large.
Why: a paginating client stops after the first page and silently treats a truncated list as complete — the failure mode is missing data with a 200, which no retry or error handling catches.
Fix: the GetAttributeValues handler's pagination path (crate::state::paginate must mint a token whenever the matched value set outruns the effective page size), together with Cause 1's seed so there is more than one page of values to return.
Traces: pricing-discover-services-and-attributes#task#4.

Cause 4 of 6: GetProducts never returns NextToken, so a filtered product query is reported as a single complete page.
AWS: Smithy marks GetProducts @paginated(inputToken: NextToken, outputToken: NextToken, items: PriceList, pageSize: MaxResults), MaxResults range 1..100. The recorded response to the filtered query returns a first page and a non-empty NextToken (again: presence is the requirement; the recorded token text is per-run).
Emulator: returns PriceList with NextToken absent (null) for the same filtered query, because the seeded product catalog holds fewer offers than one page.
Why: identical to Cause 3 but on the priced data itself — a cost tool pages once, sees no token, and computes against a fraction of the matching offers while believing it has them all.
Fix: the GetProducts handler's pagination path (ops/get_products.rs, the crate::state::paginate call) and the product seed it pages over.
Traces: pricing-get-products-filtered-workflow#task#1 (and #task#2, the same query re-issued).

Cause 5 of 6: ListPriceLists reports a price-list entry whose version and FileFormats are not the ones the service publishes.
AWS: Smithy ListPriceLists takes required ServiceCode, EffectiveDate and CurrencyCode (pattern ^[A-Z]{3}$) plus optional RegionCode, and returns PriceLists entries carrying PriceListArn, RegionCode, CurrencyCode and FileFormats — the formats that price list is actually published in. The recorded AWS entry for AmazonEC2/USD/us-east-1 carries FileFormats [json, csv] and an ARN whose trailing segment is the published version for the requested EffectiveDate.
Emulator: returns one entry with FileFormats [json] only and a fixed version segment (…/USD/20230101000000/us-east-1) that is independent of the requested EffectiveDate, so $.PriceLists[0] is reported both missing and extra. Status is 200 both ways. (The exact version timestamp in any one recording is environment data; the requirement is that the entry advertise the formats the catalog can actually serve and a version consistent with the requested EffectiveDate.)
Why: FileFormats is a contract about what can be downloaded. A client that reads [json] never offers the CSV download AWS supports, and the ARN it caches is keyed to a version that does not correspond to the effective date it asked for.
Fix: the price-list records in the service's seeded state (every format the catalog actually holds a file for must appear in FileFormats) and the ListPriceLists handler's EffectiveDate-to-version selection.
Traces: pricing-price-list-download-workflow#task#1.

Cause 6 of 6: GetPriceListFileUrl refuses HTTP 400 NotFoundException for the (PriceListArn, FileFormat) pair ListPriceLists just reported.
AWS: Smithy GetPriceListFileUrl takes required PriceListArn (pattern ^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$) and required FileFormat, and returns Url. The recorded AWS response is 200 with Url pointing at the offer file for the ARN and format taken from the preceding ListPriceLists result (recorded form: .../offers/v1.0/aws/<ServiceCode>/<version>/<RegionCode>/index.<format>; host and version are environment/catalog data, not requirements). An ARN/format that genuinely does not exist is modeled as ResourceNotFoundException (HTTP 404) or NotFoundException (HTTP 400) — but a pair the service itself just listed must resolve.
Emulator: answers HTTP 400 NotFoundException with message "Price list file with arn <PriceListArn> and file format <FileFormat> is not found." (fixed text "Price list file with arn " / " and file format " / " is not found.", variable parts taken verbatim from the request's PriceListArn and FileFormat), and no Url, because the seeded catalog holds neither that version's ARN nor a csv file for it — the direct consequence of Cause 5. Expected 200, got 400; $.Url missing.
Why: it breaks the whole documented download workflow — list, then fetch — at its second step. A client that follows the service's own output gets a client-fault 400 it cannot act on, and the price-list download is impossible for any format other than json.
Fix: the price-list file catalog in the service's state so that every (ARN, FileFormat) pair ListPriceLists advertises resolves to a served file in the GetPriceListFileUrl handler; the refusal path itself should be reached only for pairs the catalog never listed.
Traces: pricing-price-list-download-workflow#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200): 19 steps, 9 passed, 10 mismatched, score 47.4%.

Cause 1 — pricing-attribute-values-pagination#task#1 (GetAttributeValues, 200 vs 200): $.AttributeValues[0] expected {'Value': 'a1.2xlarge'}, got {'Value': 'c5.large'}; [1] expected 'a1.4xlarge', got 'c5.xlarge'; [2] expected 'a1.large', got 'm5.2xlarge'; [3] expected 'a1.medium', got 'm5.large'; [4] expected 'a1.metal', got 'm5.xlarge'. Also affected: #task#2.

Cause 2 — pricing-discover-services-and-attributes#task#1 (DescribeServices, 200 vs 200): $.Services[0] expected {'ServiceCode': 'AmazonEC2', 'AttributeNames': ['termType','productFamily','servicecode','location','instanceType','usagetype']}, got ServiceCode 'AmazonEC2' with 23 AttributeNames ['capacitystatus','currentGeneration','instanceFamily','instanceType','licenseModel','location','locationType','maxIopsvolume','maxThroughputvolume','memory','operatingSystem','operation','physicalProcessor','preInstalledSw','regionCode','servicecode','servicename','storageMedia','tenancy','usagetype','vcpu','volumeApiName','volumeType'].

Cause 3 — pricing-discover-services-and-attributes#task#4 (GetAttributeValues, 200 vs 200): $.AttributeValues[0] expected 'a1.large', got 'c5.large'; [1] expected 'c4.large', got 'c5.xlarge'; $.NextToken expected present (<shaped-not-asserted>), got None.

Cause 4 — pricing-get-products-filtered-workflow#task#1 (GetProducts, 200 vs 200): $.NextToken expected present, got None. Also affected: #task#2.

Cause 5 — pricing-price-list-download-workflow#task#1 (ListPriceLists, 200 vs 200): $.PriceLists[0] expected {'PriceListArn': 'arn:aws:pricing:::price-list/aws/AmazonEC2/USD/<version>/us-east-1', 'RegionCode': 'us-east-1', 'CurrencyCode': 'USD', 'FileFormats': ['json','csv']}, got FileFormats ['json'] and a fixed version segment 20230101000000 instead of the version for the requested EffectiveDate (recorded: 20240703191414).

Cause 6 — pricing-price-list-download-workflow#task#2 (GetPriceListFileUrl): status expected 200, got 400 (NotFoundException, message "Price list file with arn <arn> and file format <format> is not found."); $.Url expected an offer-file URL of the form .../offers/v1.0/aws/AmazonEC2/<version>/us-east-1/index.csv, got None.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture pricing-attribute-values-pagination, step task#1 — GetAttributeValues(ServiceCode=AmazonEC2, AttributeName=instanceType, bounded MaxResults); compare $.AttributeValues members and their order.
4. Cause 2: fixture pricing-discover-services-and-attributes, step task#1 — DescribeServices(ServiceCode=AmazonEC2); compare $.Services[0].AttributeNames.
5. Cause 3: same fixture, step task#4 — GetAttributeValues with a page size smaller than the published value set; observe $.NextToken is absent.
6. Cause 4: fixture pricing-get-products-filtered-workflow, step task#1 — GetProducts(ServiceCode=AmazonEC2, Filters=..., bounded MaxResults); observe $.NextToken is absent.
7. Cause 5: fixture pricing-price-list-download-workflow, step task#1 — ListPriceLists(ServiceCode=AmazonEC2, CurrencyCode=USD, RegionCode=us-east-1, EffectiveDate=...); observe FileFormats lacks csv and the ARN version ignores EffectiveDate.
8. Cause 6: same fixture, step task#2 — feed the PriceListArn from step 7 (AWS's recorded one) with FileFormat=csv into GetPriceListFileUrl; the emulator answers 400 NotFoundException instead of 200 with Url.
