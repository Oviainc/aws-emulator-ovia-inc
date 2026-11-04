Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__pricing.ecprice-max-results-with-instance-ids-rejected ec2__pricing.ecprice-running-filter-excludes-stopped-instance-from-estimate
Issue-Key: ec2__pricing.ecprice-max-results-with-instance-ids-rejected|_one_product#8|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-running-filter-excludes-stopped-instance-from-estimate|_one_product#18|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Pricing GetProducts still matches no SKU for the canonical EC2 on-demand filter set — two more ec2__pricing checks blocked (1 root cause)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
Cause 1 of 1: GetProducts(ServiceCode='AmazonEC2') returns zero SKUs for the canonical seven-filter EC2 on-demand lookup, so no instance can be priced.

NOTE ON OVERLAP: open issue 20261103T021956Z-ec2-pricing-pricing-getproducts-matches-no-sku-for-the-standard.md describes this same root cause as its Cause 1 of 4, but its Issue-Trace-Ids / Issue-Key list does NOT carry the two checks named below, so a fixer closing that issue will never re-replay them. This issue exists to carry those two replay keys; whoever fixes the catalogue should close both together. It is not a second defect.

AWS: Smithy (com-amazonaws-pricing-awspricelistservice) models GetProducts with required ServiceCode, Filters (list, @length 0..50), and output PriceList (list of JSON offer documents) plus NextToken; the modeled client errors are AccessDeniedException(401), ExpiredNextTokenException(400), InvalidNextTokenException(400), InvalidParameterException(400), NotFoundException(400), ThrottlingException(429). The model says nothing about catalogue contents, so for the matching semantics this is recorded behavior only: GetProducts(ServiceCode='AmazonEC2') with the seven TERM_MATCH filters that pin exactly one on-demand shape — instanceType, operatingSystem, location (or regionCode), tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel='No License required' — answers 200 with exactly ONE PriceList entry, whose product.attributes echo the filtered attributes and whose terms.OnDemand carries the hourly USD pricePerUnit. This is the documented way to price a running instance.

Emulator: the same request answers 200 with an EMPTY PriceList — 0 matching SKUs — for t3.nano/Linux/US West (Oregon). No error code and no message are returned; the response is simply empty, so the divergence is the catalogue/filter result set, not a refusal. The failing assertion reads: "the filter set [... instanceType=t3.nano, operatingSystem=Linux, location=US West (Oregon), tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel=No License required ...] matched 0 SKUs: []" (the instance type, OS and location here are the request's values, not part of the requirement; the requirement is that any filter set naming a type/OS/location the catalogue publishes resolves to exactly one SKU).

Why: a client that prices infrastructure — showback, budget guards, right-sizing, cost-estimate tooling — gets "no such product" for an instance type that is running in the same account and region. It then either divides by zero, raises on an empty PriceList, or silently reports a cost of zero. Both of the checks below never reach the EC2 behavior they were written to verify (the InstanceIds+MaxResults InvalidParameterCombination refusal, and the instance-state-name=running filter dropping a stopped instance): the pricing control lookup in the middle of each check aborts them first, so those two EC2 behaviors remain entirely unverified for this build.

Fix: the AmazonEC2 offer data seeded into the price-list catalogue behind GetProducts, and the TERM_MATCH filter evaluation in the GetProducts handler that intersects those filters against the catalogue.

Traces: ec2__pricing.ecprice-max-results-with-instance-ids-rejected, ec2__pricing.ecprice-running-filter-excludes-stopped-instance-from-estimate.

## Evidence
Replayed failing checks under `### ec2__pricing`, neither marked ALREADY FILED nor OWNED by any open issue:

1. [mismatch] ec2__pricing.ecprice-max-results-with-instance-ids-rejected
   check_ec2__pricing_ecprice_max_results_with_instance_ids_rejected.py line 31, in run:
     attrs = _one_product(pricing, _price_filters("t3.nano", "Linux", ctx.region))["product"]["attributes"]
   -> line 111, in _one_product: assert len(got) == 1
   AssertionError: the filter set [{'Type': 'TERM_MATCH', 'Field': 'instanceType', 'Value': 't3.nano'}, {'Type': 'TERM_MATCH', 'Field': 'operatingSystem', 'Value': 'Linux'}, {'Type': 'TERM_MATCH', 'Field': 'location', 'Value': 'US West (Oregon)'}, {'Type': 'TERM_MATCH', 'Field': 'tenancy', 'Value': 'Shared'}, {'Type': 'TERM_MATCH', 'Field': 'preInstalledSw', 'Value': 'NA'}, {'Type': 'TERM_MATCH', 'Field': 'capacitystatus', 'Value': 'Used'}, {'Type': 'TERM_MATCH', 'Field': 'licenseModel', 'Value': 'No License required'}] matched 0 SKUs: []

2. [mismatch] ec2__pricing.ecprice-running-filter-excludes-stopped-instance-from-estimate
   check_ec2__pricing_ecprice_running_filter_excludes_stopped_instance_from_estimate.py line 52, in run:
     product = _one_product(pricing, _price_filters("t3.nano", "Linux", ctx.region), 1)
   -> line 123, in _one_product: assert len(got) == 1
   AssertionError: identical message — the same seven-filter set matched 0 SKUs: [].

Expected: PriceList length 1, with product.attributes.instanceType == 't3.nano' and a positive terms.OnDemand[...].priceDimensions[...].pricePerUnit.USD.
Actual: PriceList length 0 (HTTP 200, no error code, no message).

Both checks pin the pricing client to us-east-1 (the Price List Query API is only served from us-east-1, eu-central-1 and ap-south-1) while the EC2 client runs in ctx.region (us-west-2 in this replay), so the empty result is not a region-endpoint artefact of the check.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite. The two checks appear under `### ec2__pricing` with no ALREADY FILED / OWNED marker:
   - ecprice-max-results-with-instance-ids-rejected
   - ecprice-running-filter-excludes-stopped-instance-from-estimate
2. Standalone repro (pricing client pinned to us-east-1):
   pricing.get_products(ServiceCode='AmazonEC2', Filters=[
     {'Type':'TERM_MATCH','Field':'instanceType','Value':'t3.nano'},
     {'Type':'TERM_MATCH','Field':'operatingSystem','Value':'Linux'},
     {'Type':'TERM_MATCH','Field':'location','Value':'US West (Oregon)'},
     {'Type':'TERM_MATCH','Field':'tenancy','Value':'Shared'},
     {'Type':'TERM_MATCH','Field':'preInstalledSw','Value':'NA'},
     {'Type':'TERM_MATCH','Field':'capacitystatus','Value':'Used'},
     {'Type':'TERM_MATCH','Field':'licenseModel','Value':'No License required'}])
   Expected: PriceList has exactly one entry, parseable JSON, with terms.OnDemand present and USD pricePerUnit > 0.
   Actual: PriceList == [].
3. After the catalogue/filter fix, re-replay BOTH check ids above — they go on to assert EC2 DescribeInstances behavior (InstanceIds+MaxResults must be refused InvalidParameterCombination with message "The parameter instancesSet cannot be used with the parameter maxResults"; a stopped instance must drop out of the instance-state-name=running set while the tag filter alone still returns it) that has never executed in this build and may surface further divergences.
