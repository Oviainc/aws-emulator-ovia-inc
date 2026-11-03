Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__pricing.ecprice-breakdown-counts-instances-across-reservations ec2__pricing.ecprice-cli-trajectory-matches-sdk-estimate ec2__pricing.ecprice-describe-malformed-instance-id-rejected ec2__pricing.ecprice-describe-max-results-below-five-rejected ec2__pricing.ecprice-instance-type-filter-partitions-the-estimate ec2__pricing.ecprice-instance-usage-operation-matches-priced-sku-operation ec2__pricing.ecprice-paginated-describe-covers-every-running-instance ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle ec2__pricing.ecprice-platform-filter-partitions-windows-from-linux ec2__pricing.ecprice-linux-instance-omits-platform-and-prices-as-linux ec2__pricing.ecprice-estimate-is-scoped-to-the-instances-region
Issue-Key: ec2__pricing.ecprice-breakdown-counts-instances-across-reservations|_one_product#16|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-cli-trajectory-matches-sdk-estimate|_one_product#10|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-describe-malformed-instance-id-rejected|_one_product#8|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-describe-max-results-below-five-rejected|_one_product#10|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-instance-type-filter-partitions-the-estimate|_one_product#16|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-instance-usage-operation-matches-priced-sku-operation|_one_product#12|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-paginated-describe-covers-every-running-instance|_one_product#20|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected|_one_product#7|AssertionError|assert len(got) == 1, f"the filter set {filters!r} matched {len(got)} SKUs: {skus!r}"
Issue-Key: ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__pricing|DescribeInstances|InvalidParameterValue
Issue-Key: ec2__pricing.ecprice-linux-instance-omits-platform-and-prices-as-linux|run#11|AssertionError|assert windows.get("Platform") == "windows", (
Issue-Key: ec2__pricing.ecprice-estimate-is-scoped-to-the-instances-region|run#11|AssertionError|assert there == [], f"the {other} sweep returned {there!r} for an instance running in {ctx.region}"
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Pricing GetProducts matches no SKU for the standard EC2 on-demand filter set, and EC2 reports the instance platform in the wrong case and ignores the platform filter (4 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
Four net-new root causes replayed under the `ec2__pricing` suite heading. Every cost-estimation check in the group fails at the same first step.

Cause 1 of 4: GetProducts returns zero SKUs for the canonical EC2 on-demand filter set.
AWS: recorded behavior only. GetProducts(ServiceCode='AmazonEC2') with the seven TERM_MATCH filters that identify one on-demand shape - instanceType, operatingSystem, location, tenancy, preInstalledSw, capacitystatus, licenseModel - matches exactly one SKU, whose OnDemand terms carry the hourly price. This is the documented way to price an instance.
Emulator: the same filter set matches 0 SKUs and returns an empty PriceList, so no price can be derived at all.
Why: this is the entire purpose of the Pricing API. Every client that estimates cost - showback, budget guards, right-sizing tools - gets "no such product" for an instance type that is running in the same account, and either divides by zero or silently reports a cost of nothing.
Fix: the price-list catalogue behind GetProducts (the AmazonEC2 offer data) and the TERM_MATCH filter evaluation in the GetProducts handler.
Traces: ec2__pricing.ecprice-breakdown-counts-instances-across-reservations, ec2__pricing.ecprice-cli-trajectory-matches-sdk-estimate, ec2__pricing.ecprice-describe-malformed-instance-id-rejected, ec2__pricing.ecprice-describe-max-results-below-five-rejected, ec2__pricing.ecprice-instance-type-filter-partitions-the-estimate, ec2__pricing.ecprice-instance-usage-operation-matches-priced-sku-operation, ec2__pricing.ecprice-paginated-describe-covers-every-running-instance, ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected, ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle

Cause 2 of 4: EC2 DescribeInstances refuses the `platform` filter.
AWS: `platform` is a supported DescribeInstances filter (its only value is `windows`); it is how a caller partitions Windows from Linux instances server-side.
Emulator: refuses it with ('InvalidParameterValue', "The filter 'platform' is not one this emulator resolves") - a supported filter rejected, with the implementation named in the message a client reads.
Why: cost and licence attribution depends on splitting Windows from Linux. The caller cannot narrow server-side and the self-referential message is not an AWS error string.
Fix: add `platform` to the filter table in the DescribeInstances handler and use AWS's unknown-filter template for genuinely unknown names.
Traces: ec2__pricing.ecprice-platform-filter-partitions-windows-from-linux

Cause 3 of 4: the instance Platform value is reported capitalised.
AWS: an instance's Platform member is the lower-case literal `windows`, and the member is OMITTED for a Linux instance.
Emulator: reports `Windows`.
Why: Platform is matched as a literal (it is also the filter value), so a case-shifted value fails every equality test and every `platform` filter round-trip - the instance is classified as neither Windows nor Linux.
Fix: the Platform field in instance state / the DescribeInstances serializer.
Traces: ec2__pricing.ecprice-linux-instance-omits-platform-and-prices-as-linux

Cause 4 of 4: the per-region instance sweep returns instances from another region.
AWS: DescribeInstances is region-scoped; a us-east-1 client cannot enumerate an instance running in us-west-2.
Emulator: the us-east-1 sweep returns the us-west-2 instance.
Why: a cost estimate must be scoped to the region whose price list it uses; cross-region leakage prices instances against the wrong location filter and double-counts them in every region's estimate. (This is the same shared-global-store defect reported for the ec2 service itself; it is recorded here because this check is the pricing group's own failing evidence and carries its own replay key.)
Fix: region partitioning in EC2 state and the DescribeInstances scoping predicate.
Traces: ec2__pricing.ecprice-estimate-is-scoped-to-the-instances-region

## Evidence
Replayed failing checks under `### ec2__pricing` (none marked ALREADY FILED / OWNED by):

1. Nine checks fail with the identical assertion: "the filter set [{'Type': 'TERM_MATCH', 'Field': 'instanceType', 'Value': 't3.nano'}, {'Type': 'TERM_MATCH', 'Field': 'operatingSystem', 'Value': 'Linux'}, {'Type': 'TERM_MATCH', 'Field': 'location', 'Value': 'US West (Oregon)'}, {'Type': 'TERM_MATCH', 'Field': 'tenancy', 'Value': 'Shared'}, {'Type': 'TERM_MATCH', 'Field': 'preInstalledSw', 'Value': 'NA'}, {'Type': 'TERM_MATCH', 'Field': 'capacitystatus', 'Value': 'Used'}, {'Type': 'TERM_MATCH', 'Field': 'licenseModel', 'Value': 'No License required'}] matched 0 SKUs: []" - ecprice-breakdown-counts-instances-across-reservations, ecprice-cli-trajectory-matches-sdk-estimate, ecprice-describe-malformed-instance-id-rejected, ecprice-describe-max-results-below-five-rejected, ecprice-instance-type-filter-partitions-the-estimate, ecprice-instance-usage-operation-matches-priced-sku-operation, ecprice-paginated-describe-covers-every-running-instance, ecprice-unknown-instance-state-filter-name-rejected, and the e2e lifecycle ecprice-e2e-estimate-running-instance-costs-lifecycle.
2. ec2__pricing.ecprice-platform-filter-partitions-windows-from-linux -> botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribeInstances operation: The filter 'platform' is not one this emulator resolves
3. ec2__pricing.ecprice-linux-instance-omits-platform-and-prices-as-linux -> AssertionError: the Windows instance reports Platform='Windows', expected 'windows'
4. ec2__pricing.ecprice-estimate-is-scoped-to-the-instances-region -> AssertionError: the us-east-1 sweep returned ['i-00000000000000001'] for an instance running in us-west-2

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the checks appear under `### ec2__pricing`, unmarked.
2. Standalone repros:
   - pricing.get_products(ServiceCode='AmazonEC2', Filters=[{'Type':'TERM_MATCH','Field':'instanceType','Value':'t3.nano'},{'Type':'TERM_MATCH','Field':'operatingSystem','Value':'Linux'},{'Type':'TERM_MATCH','Field':'location','Value':'US West (Oregon)'},{'Type':'TERM_MATCH','Field':'tenancy','Value':'Shared'},{'Type':'TERM_MATCH','Field':'preInstalledSw','Value':'NA'},{'Type':'TERM_MATCH','Field':'capacitystatus','Value':'Used'},{'Type':'TERM_MATCH','Field':'licenseModel','Value':'No License required'}]) must return exactly one SKU with OnDemand terms.
   - ec2.describe_instances(Filters=[{'Name':'platform','Values':['windows']}]) must be accepted and must return only Windows instances.
   - RunInstances from a Windows AMI, then DescribeInstances: Platform must read `windows`; a Linux instance must omit Platform entirely.
   - RunInstances in us-west-2, then DescribeInstances in us-east-1: zero reservations.
