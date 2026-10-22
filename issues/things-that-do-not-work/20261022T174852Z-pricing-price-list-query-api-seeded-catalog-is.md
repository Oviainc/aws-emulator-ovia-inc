Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pricing.describe-services-pagination-no-duplicates pricing.get-attribute-values-ec2-location-known-region-present pricing.get-attribute-values-pagination-respects-max-results pricing.get-price-list-file-url-native-download-round-trip pricing.describe-services-ec2-attribute-names-present pricing.get-attribute-values-no-duplicate-values pricing.get-products-term-match-filters-return-single-matching-product pricing.get-products-any-of-filter-matches-provided-values pricing.get-products-contains-filter-matches-substring-not-exact pricing.get-products-none-of-filter-excludes-value pricing.get-products-pricing-terms-include-ondemand-and-reserved pricing.unknown-service-code-returns-empty-services pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected pricing.get-price-list-file-url-nonexistent-arn-resource-not-found-rejected pricing.list-price-lists-unsupported-currency-resource-not-found-rejected pricing.invalid-next-token-rejected
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing.describe-services-pagination-no-duplicates|run#5|AssertionError|assert len(codes) > 25, f"expected to traverse more than one page, got {len(codes)} codes"
Issue-Key: pricing.get-attribute-values-ec2-location-known-region-present|run#6|AssertionError|assert "Asia Pacific (Tokyo)" in values, (
Issue-Key: pricing.get-attribute-values-pagination-respects-max-results|run#6|AssertionError|assert token, "expected instanceType to span more than one page of 10"
Issue-Key: pricing|ListPriceLists|NotFoundException
Issue-Key: pricing.describe-services-ec2-attribute-names-present|run#8|AssertionError|assert expected.issubset(attribute_names), attribute_names - expected
Issue-Key: pricing|GetAttributeValues|NotFoundException
Issue-Key: pricing.get-products-term-match-filters-return-single-matching-product|run#5|AssertionError|assert len(price_list) == 1, f"expected exactly 1 matching SKU, got {len(price_list)}"
Issue-Key: pricing.get-products-any-of-filter-matches-provided-values|run#7|AssertionError|assert found_types == expected_types, f"expected {expected_types}, got {found_types}"
Issue-Key: pricing.get-products-contains-filter-matches-substring-not-exact|run#4|AssertionError|assert price_list, "expected at least one HDD volume product"
Issue-Key: pricing.get-products-none-of-filter-excludes-value|run#3|AssertionError|assert len(control["PriceList"]) == 1, "positive control: t2.micro SKU must exist"
Issue-Key: pricing.get-products-pricing-terms-include-ondemand-and-reserved|run#4|AssertionError|assert len(price_list) == 1, f"expected exactly 1 matching SKU, got {len(price_list)}"
Issue-Key: pricing|DescribeServices|NotFoundException
Issue-Key: pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected|run#4|AssertionError|assert code == "InvalidParameterException", (code, msg)
Issue-Key: pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected|run#4|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: pricing.get-price-list-file-url-nonexistent-arn-resource-not-found-rejected|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: pricing.list-price-lists-unsupported-currency-resource-not-found-rejected|run#4|AssertionError|assert code == "AccessDeniedException", (code, msg)
Issue-Key: pricing.invalid-next-token-rejected|run#4|AssertionError|assert msg == "NextToken is invalid. ", msg
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Pricing (Price List Query API): seeded catalog is a 2-service toy and client-error classification diverges from AWS (8 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
The behavioral suite exercised 22 pricing checks; 17 failed. Clustering them gives 8 distinct root causes, all in this service's seeded catalog state and its error classification. The emulator answers every operation (nothing is a 501 stub here), so these are fidelity divergences, not bootstrap gaps. No open issue mentions pricing (list_existing_issues query='pricing' returned none).

=== Cause 1 of 8: the catalog publishes only 2 service codes and 3 Regions, so no paginated listing ever spans a page ===
AWS: Smithy: DescribeServices is @paginated (inputToken NextToken / outputToken NextToken / items Services / pageSize MaxResults, MaxResults range 1..100); GetAttributeValues likewise. The Price List Query API publishes a service code for essentially every AWS service (hundreds, far more than one page of 25) and AmazonEC2's `location` attribute enumerates every AWS Region display name (recorded behavior: well over 10 values, including 'Asia Pacific (Tokyo)'), and AmazonEC2's `instanceType` attribute has hundreds of values so MaxResults=10 always returns a NextToken.
Emulator: DescribeServices traverses to completion after 2 service codes (AmazonEC2, AmazonS3). GetAttributeValues(AmazonEC2, location) returns exactly ['EU (Ireland)', 'US East (N. Virginia)', 'US West (Oregon)']. GetAttributeValues(AmazonEC2, instanceType, MaxResults=10) returns no NextToken because only 3 distinct instance types exist. ListPriceLists(ServiceCode='AmazonSNS') is refused with NotFoundException 'service code 'AmazonSNS' not found', so the price-list file data plane (GetPriceListFileUrl -> follow the URL -> parse the offer document, which asserts offerCode=='AmazonSNS') is unreachable and was never exercised.
Why: a cost-estimation client that enumerates service codes, Regions or instance types, or that pages any of these listings, sees a catalog that silently omits almost everything and never exercises its pagination path; a client priced against a Region or service outside the three seeded ones gets a hard NotFoundException where AWS returns data.
Fix: the service's seeded state — broaden the seeded service catalog, the Region/location coverage and the per-attribute value sets, and seed price lists for more than AmazonEC2/AmazonS3, so paginated listings genuinely span pages.
Traces: pricing.describe-services-pagination-no-duplicates ('expected to traverse more than one page, got 2 codes'); pricing.get-attribute-values-ec2-location-known-region-present; pricing.get-attribute-values-pagination-respects-max-results; pricing.get-price-list-file-url-native-download-round-trip. Seeds: services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:543 (seeded_catalog), :725 (EC2_PRODUCTS), :845 (S3_PRODUCTS), :905 (SEEDED_PRICE_LISTS).

=== Cause 2 of 8: AmazonEC2's product set carries no EBS volume products and no t2 instance family, so GetProducts matches nothing and `volumeType` is not an AmazonEC2 attribute at all ===
AWS: Smithy: DescribeServices.Services[].AttributeNames lists the attribute names the service publishes; Filter.Field valid values are 'ServiceCode and all attribute names', and the model's own example of a filter is 'the AmazonEC2 service code and the volumeType attribute name to get the prices for only Amazon EC2 volumes'. Recorded behavior: AmazonEC2 AttributeNames includes at least instanceType, volumeType, location, operatingSystem and tenancy; GetAttributeValues(AmazonEC2, volumeType) returns the EBS volume-type labels including 'General Purpose', 'Provisioned IOPS' and 'Magnetic'; and the t2 family (t2.micro/t2.small/t2.medium) is priced in US East (N. Virginia).
Emulator: AmazonEC2's AttributeNames is derived purely from the 5 seeded compute products (state.rs:99 ServiceCatalog::new unions the product attribute keys), so volumeType is absent and GetAttributeValues(ServiceCode=AmazonEC2, AttributeName=volumeType) is refused NotFoundException "attribute 'volumeType' is not available for service code 'AmazonEC2'". Every GetProducts query over a t2 type returns an empty PriceList: TERM_MATCH on instanceType=t2.micro + location='US East (N. Virginia)' + operatingSystem=Linux + tenancy=Shared + preInstalledSw=NA + capacitystatus=Used + licenseModel='No License required' yields 0 SKUs (the seeded EC2 products are t3.micro, t3.small, m5.large only, and none carries licenseModel at all); ANY_OF instanceType='t2.micro,t2.small,t2.medium' yields set(); CONTAINS volumeType='HDD' yields 0 products; and the NONE_OF check never reaches its exclusion assertion because its t2.micro positive control is already empty.
Why: GetProducts under the documented on-demand filter set is the primary use of this API — a client computing a running-cost estimate gets an empty PriceList and concludes the instance type is unpriced, and the CONTAINS/ANY_OF/NONE_OF filter semantics cannot be relied on because no data exists to match. The filter comparison code itself (get_products.rs:266 filter_matches) implements all five FilterTypes correctly; the data behind it is what is missing, including the licenseModel attribute the standard EC2 on-demand filter set keys on.
Fix: the service's seeded state — seed the EC2 product set with the real on-demand compute SKUs (including the t2 family and the licenseModel attribute) and with EBS volume products carrying volumeType values, so AttributeNames, GetAttributeValues and the GetProducts filters agree with AWS.
Traces: pricing.describe-services-ec2-attribute-names-present; pricing.get-attribute-values-no-duplicate-values; pricing.get-products-term-match-filters-return-single-matching-product ('expected exactly 1 matching SKU, got 0'); pricing.get-products-any-of-filter-matches-provided-values; pricing.get-products-contains-filter-matches-substring-not-exact; pricing.get-products-none-of-filter-excludes-value.

=== Cause 3 of 8: offer documents carry OnDemand terms only — no Reserved term ===
AWS: recorded behavior only: an aws_v1 product-offer document for an EC2 compute SKU carries both a non-empty terms.OnDemand and a non-empty terms.Reserved, each Reserved term's termAttributes.LeaseContractLength being '1yr' or '3yr'.
Emulator: offer_document builds terms with an OnDemand block and nothing else, so terms.Reserved is absent on every product in the catalog; the rendered price-list file likewise emits only {"terms": {"OnDemand": ...}}.
Why: a client that reads reserved-instance pricing out of the offer document KeyErrors on terms['Reserved'] instead of getting a lease-length-keyed term set; a savings comparison between on-demand and reserved pricing is impossible against the emulator.
Fix: the catalog seeding/offer-document builder for this service (state.rs:592 offer_document, :431 the file renderer which only copies /terms/OnDemand).
Traces: pricing.get-products-pricing-terms-include-ondemand-and-reserved (it currently fails earlier, at 'expected exactly 1 matching SKU, got 0' from Cause 2; the Reserved-terms assertion is the next one it reaches once a matching SKU exists, so both must be fixed for this check to pass).

=== Cause 4 of 8: DescribeServices with an unknown ServiceCode is refused instead of returning an empty Services list ===
AWS: recorded behavior: DescribeServices(ServiceCode='NotARealPricingServiceXYZ123') returns HTTP 200 with Services == []. Smithy: DescribeServices models no not-found error of its own (only ExpiredNextTokenException and InvalidNextTokenException, plus the service-common errors), and ServiceCode is an unconstrained optional String.
Emulator: answers NotFoundException with message "service code 'NotARealPricingServiceXYZ123' not found" (the catalog lookup state.rs:232 ComAmazonawsPricingAwspricelistserviceState::catalog returning None is turned into a modeled error by the handler).
Why: a client that probes whether a service is priced — or that enumerates candidate service codes — gets an exception it must catch instead of an empty result set, so a defensive 'if not resp["Services"]' branch never runs and the call fails hard.
Fix: the DescribeServices handler's unknown-service-code path (services/service-com-amazonaws-pricing-awspricelistservice/src/ops/describe_services.rs).
Traces: pricing.unknown-service-code-returns-empty-services.

=== Cause 5 of 8: input-constraint violations are answered as ValidationException, an error code this service does not model ===
AWS: Smithy: GetPriceListFileUrl.PriceListArn is @pattern ^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$ (@length 18..2048) and ListPriceLists.CurrencyCode is @pattern ^[A-Z]{3}$. The service models InvalidParameterException (HTTP 400, 'One or more parameters had an invalid value') as a common error and models NO ValidationException shape at all. AWS answers a constraint violation with InvalidParameterException and a message built from the request — '1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' — so the member name, the offending value and the pattern are the requirement, not the recorded literal.
Emulator: answers error code 'ValidationException' for both. The message body already follows AWS's template (e.g. "1 validation error detected: Value 'arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id' at 'priceListArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...$"), so only the error code is wrong.
Why: a client catching pricing.exceptions.InvalidParameterException (the only code this API's model exposes for bad input) does not catch the emulator's response at all — botocore surfaces it as a generic ClientError with an unmodeled code, so retry/branching logic keyed on the modeled error never fires.
Fix: the constraint-validation seam for this service's deserialization path — pattern/length violations must be rendered as the modeled InvalidParameterException (400) rather than a generic ValidationException.
Traces: pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected; pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected.

=== Cause 6 of 8: GetPriceListFileUrl answers ResourceNotFoundException (404) for an unknown-but-well-formed ARN where AWS answers NotFoundException (400) ===
AWS: recorded behavior: GetPriceListFileUrl(PriceListArn='arn:aws:pricing:::price-list/aws/AmazonEC2/USD/99999999999999/us-east-1', FileFormat='json') is refused with error code NotFoundException, message 'Price list file with arn <arn> and file format <format> is not found.' (the message is built from the request's ARN and FileFormat, so that is the rule; the exact sentence above is the recorded example). Smithy: this operation models BOTH NotFoundException (client, HTTP 400) and ResourceNotFoundException (client, HTTP 404); AWS uses the former here.
Emulator: answers error code ResourceNotFoundException, HTTP 404.
Why: both codes are modeled, so a client written against AWS catches pricing.exceptions.NotFoundException and misses the emulator's ResourceNotFoundException entirely; code that branches on the 400-vs-404 status also takes the wrong path.
Fix: the GetPriceListFileUrl handler's unknown-ARN path (src/ops/get_price_list_file_url.rs) — select NotFoundException for an ARN/format that resolves to no price-list file.
Traces: pricing.get-price-list-file-url-nonexistent-arn-resource-not-found-rejected ('expected NotFoundException, got ResourceNotFoundException').

=== Cause 7 of 8: ListPriceLists with a well-formed but unpublished currency is refused InvalidParameterException instead of AccessDeniedException ===
AWS: recorded behavior: ListPriceLists(ServiceCode='AmazonEC2', EffectiveDate=now, CurrencyCode='ZZZ') — pattern-valid three uppercase letters, but not a currency the API publishes — is refused with error code AccessDeniedException and the message 'Caller is not authorized to access requested price lists'. Smithy: AccessDeniedException is modeled for this operation (client, HTTP 401); the message is a fixed sentence carrying no request data, so the recorded text is the requirement for this condition.
Emulator: answers InvalidParameterException with a bespoke message "CurrencyCode 'ZZZ' is not a currency this API publishes price lists in; supported currencies are CNY, USD" (the SUPPORTED_CURRENCY_CODES check, state.rs:42).
Why: AWS models an unpublished currency as an authorization outcome, not a bad parameter; a client that treats AccessDenied as 'not entitled to this price list' and InvalidParameter as 'my request is malformed' takes the opposite branch, and one catching AccessDeniedException does not catch the emulator's answer.
Fix: the ListPriceLists handler's currency-code gate (src/ops/list_price_lists.rs) — a pattern-valid currency that is not published must be refused AccessDeniedException, keeping InvalidParameterException for pattern violations (Cause 5).
Traces: pricing.list-price-lists-unsupported-currency-resource-not-found-rejected.

=== Cause 8 of 8: the InvalidNextTokenException message is bespoke ===
AWS: recorded behavior only: a NextToken the service never issued is refused with error code InvalidNextTokenException (modeled, HTTP 400) and the invariant message 'NextToken is invalid. ' (trailing space included). The sentence carries no request data and does not vary with the token, so it is a fixed API constant for this condition rather than a per-request message.
Emulator: the error code and status are correct; the message is 'the pagination token is invalid' (state.rs paginate() callers, e.g. get_products.rs:243).
Why: lowest-impact of the eight — a client branching on the code is unaffected; only message-matching assertions and user-facing passthrough of the message diverge.
Fix: the shared pagination-token rejection path for this service's handlers.
Traces: pricing.invalid-next-token-rejected ('AssertionError: the pagination token is invalid' at the message assertion; the code assertion above it passed).

## Evidence
Suite: 5/22 pricing checks passed (23%), 17 failed. Representative expected-vs-actual:
- GetProducts TERM_MATCH {instanceType=t2.micro, location=US East (N. Virginia), operatingSystem=Linux, tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel=No License required}: expected exactly 1 SKU with positive USD on-demand rate; got 0 ('expected exactly 1 matching SKU, got 0').
- GetProducts ANY_OF instanceType='t2.micro,t2.small,t2.medium': expected {'t2.medium','t2.micro','t2.small'}; got set().
- GetProducts CONTAINS volumeType='HDD': expected >=1 HDD volume product; got 0.
- GetAttributeValues(AmazonEC2, volumeType, MaxResults=100): expected the EBS volume-type catalog incl. {'General Purpose','Provisioned IOPS','Magnetic'}; got NotFoundException "attribute 'volumeType' is not available for service code 'AmazonEC2'".
- DescribeServices(AmazonEC2).AttributeNames: expected {instanceType, volumeType, location, operatingSystem, tenancy} to be a subset; it is not (emulator reports only the union of 5 seeded compute products: capacitystatus, instanceFamily, instanceType, location, locationType, memory, operatingSystem, operation, preInstalledSw, regionCode, servicecode, tenancy, termType, usagetype, vcpu).
- DescribeServices full traversal: expected >25 service codes; got 2.
- GetAttributeValues(AmazonEC2, location): expected >10 values incl. 'Asia Pacific (Tokyo)'; got ['EU (Ireland)','US East (N. Virginia)','US West (Oregon)'].
- GetAttributeValues(AmazonEC2, instanceType, MaxResults=10): expected a NextToken (page 2 non-empty, disjoint); got no token (3 distinct values total).
- ListPriceLists(ServiceCode='AmazonSNS', USD, us-east-1): expected a downloadable price list; got NotFoundException "service code 'AmazonSNS' not found".
- DescribeServices(ServiceCode='NotARealPricingServiceXYZ123'): expected HTTP 200 Services==[]; got NotFoundException.
- GetPriceListFileUrl(PriceListArn='arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id'): expected InvalidParameterException; got ValidationException (message template already correct).
- ListPriceLists(CurrencyCode='US'): expected InvalidParameterException; got ValidationException.
- GetPriceListFileUrl(PriceListArn='arn:aws:pricing:::price-list/aws/AmazonEC2/USD/99999999999999/us-east-1'): expected NotFoundException (400); got ResourceNotFoundException (404).
- ListPriceLists(CurrencyCode='ZZZ'): expected AccessDeniedException 'Caller is not authorized to access requested price lists'; got InvalidParameterException "CurrencyCode 'ZZZ' is not a currency this API publishes price lists in; supported currencies are CNY, USD".
- DescribeServices(NextToken='this-was-never-issued-by-pricing'): code InvalidNextTokenException correct; message expected 'NextToken is invalid. ', got 'the pagination token is invalid'.
Source grounding: services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:42 (SUPPORTED_CURRENCY_CODES), :99 (AttributeNames derived from seeded products), :232 (catalog lookup), :543 (seeded_catalog: only AmazonEC2 + AmazonS3), :592 (offer_document emits OnDemand terms only), :725/:845/:905 (the five EC2 / three S3 product seeds and the price-list seeds); src/ops/get_products.rs:266 (filter_matches implements all five FilterTypes correctly — the gap is the data), :243 (next-token message).

## Steps to Reproduce
Boot the emulator and point a botocore pricing client at it with region us-east-1 (the Price List Query API endpoint), then:
1. pricing.get_products(ServiceCode='AmazonEC2', Filters=[TERM_MATCH instanceType=t2.micro, location='US East (N. Virginia)', operatingSystem=Linux, tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel='No License required'], MaxResults=10) -> PriceList is empty (AWS: exactly one SKU, with OnDemand and Reserved terms).
2. pricing.get_products(ServiceCode='AmazonEC2', Filters=[{Type:'ANY_OF',Field:'instanceType',Value:'t2.micro,t2.small,t2.medium'}, ...]) -> empty; same with {Type:'CONTAINS',Field:'volumeType',Value:'HDD'}.
3. pricing.get_attribute_values(ServiceCode='AmazonEC2', AttributeName='volumeType', MaxResults=100) -> NotFoundException.
4. pricing.describe_services(ServiceCode='AmazonEC2') -> AttributeNames lacks volumeType; paginate describe_services() to exhaustion -> 2 service codes.
5. pricing.get_attribute_values(ServiceCode='AmazonEC2', AttributeName='location') -> 3 values; AttributeName='instanceType', MaxResults=10 -> no NextToken.
6. pricing.list_price_lists(ServiceCode='AmazonSNS', EffectiveDate=now, CurrencyCode='USD', RegionCode='us-east-1', MaxResults=1) -> NotFoundException.
7. pricing.describe_services(ServiceCode='NotARealPricingServiceXYZ123') -> NotFoundException instead of 200 + Services==[].
8. pricing.get_price_list_file_url(PriceListArn='arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id', FileFormat='json') -> ValidationException; with 'arn:aws:pricing:::price-list/aws/AmazonEC2/USD/99999999999999/us-east-1' -> ResourceNotFoundException (404).
9. pricing.list_price_lists(ServiceCode='AmazonEC2', EffectiveDate=now, CurrencyCode='US') -> ValidationException; CurrencyCode='ZZZ' -> InvalidParameterException.
10. pricing.describe_services(NextToken='this-was-never-issued-by-pricing') -> InvalidNextTokenException with message 'the pagination token is invalid'.
Or replay the check ids in trace_ids.
