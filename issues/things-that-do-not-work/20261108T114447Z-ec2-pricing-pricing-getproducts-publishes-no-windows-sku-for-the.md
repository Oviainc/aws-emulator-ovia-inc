Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle
Issue-Key: ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 250d476ecb357920d67b44d09bfeff057fcec9e1

# pricing GetProducts publishes no Windows SKU for the canonical EC2 on-demand filter set, so a mixed-OS estimate silently prices every Windows instance at $0.0000 (1 root cause)

> _Note: this issue was identified against the code base at commit `250d476ecb357920d67b44d09bfeff057fcec9e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
One net-new root cause surviving under the `ec2__pricing` suite heading. The deploy/pre_invoke lifecycle check now gets past the EC2 sweep and prices the Linux groups, and fails on the Windows group alone — a replay key that no open issue carries (the e2e check's key in 20261103T021956Z-ec2-pricing-pricing-getproducts-matches-no-sku-for-the-standard.md is a `_poll` Timeout, and 20261104T043653Z-ec2-pricing-pricing-getproducts-still-matches-no-sku-for-the.md / 20261103T043017Z-pricing-getproducts-5-net-new-root-causes-ec2.md describe the all-OS / catalog-wide emptiness; this is the narrowed, still-live Windows case and is not covered by those keys).

Cause 1 of 1: GetProducts(ServiceCode='AmazonEC2') matches zero SKUs when the on-demand filter set names operatingSystem=Windows, while the identical set with operatingSystem=Linux now matches.
AWS: recorded behavior only — the Smithy model types the request but not the catalog (Smithy: GetProducts input ServiceCode required, Filters @length max 50, MaxResults @range 1..100; output PriceList as JSON offer documents; errors AccessDeniedException/InvalidParameterException/NotFoundException/ExpiredNextTokenException/InvalidNextTokenException/ThrottlingException/InternalErrorException). The recorded behavior is that the seven TERM_MATCH filters that identify one on-demand shape — instanceType, location, operatingSystem, tenancy=Shared, preInstalledSw=NA, capacitystatus=Used, licenseModel='No License required' — match exactly ONE Compute Instance SKU for a Windows operating system as well as for Linux, and that SKU's terms.OnDemand.*.priceDimensions.*.pricePerUnit.USD is the hourly rate (strictly greater than the Linux rate for the same instance type, because the Windows licence is in the rate). HTTP 200 with a one-element PriceList, no error.
Emulator: the Linux lookups return a priced SKU, but the same filter set with operatingSystem=Windows returns HTTP 200 with an EMPTY PriceList. No error code is raised — the response is simply SKU-less, so a caller iterating `PriceList` derives no rate at all. The check's upstream-faithful lookup therefore falls back to 0.0 and the grouped estimate reports `1x t3.micro Windows @ $0.0000/hr = $0.0000/hr`.
Why: a client relying on AWS's behavior gets a cost estimate that is silently and unboundedly wrong rather than failing: the Windows instances — the most expensive part of a mixed fleet, because of the licence component — contribute nothing to the total, and the invariant that a Windows rate exceeds the Linux rate for the same instance type cannot hold at all. Showback, budget-guard and right-sizing clients under-report; nothing in the response signals the omission, since an empty PriceList is also AWS's legitimate answer for a genuinely non-existent product.
Fix: the AmazonEC2 price-list catalogue behind GetProducts must publish the Windows Compute Instance SKUs (operatingSystem=Windows with preInstalledSw=NA, licenseModel='No License required', tenancy=Shared, capacitystatus=Used, per location) with OnDemand terms whose USD rate exceeds the corresponding Linux SKU; and the TERM_MATCH evaluation in the GetProducts handler must resolve operatingSystem=Windows against them.
Traces: ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle (phase 2/5 pre_invoke). The same Windows filter set also returns 0 SKUs in ec2__pricing.ecprice-cli-trajectory-matches-sdk-estimate, which is already OWNED by 20261103T021956Z and is listed here only as corroboration, not as a second cause.

## Evidence
Failing check under `### ec2__pricing`, not marked ALREADY FILED and not marked OWNED by any open issue:

ec2__pricing.ecprice-e2e-estimate-running-instance-costs-lifecycle ->
AssertionError: [phase 2/5 pre_invoke] the reference priced [('t3.micro', 'Windows')] at [0.0]: the upstream lookup falls back to 0.0 when no SKU matches, which is a silently wrong answer

The phase reached that assertion only after the Linux groups priced successfully and after the running sweep grouped exactly as the stack deployed ({('t3.micro','Linux'): 1, ('t3.micro','Windows'): 1, ('t3.nano','Linux'): 2}), so the divergence is scoped to the operatingSystem=Windows lookup, not to the filter set as a whole.

The diverging request (per-group, MaxResults=1, pricing client pinned to us-east-1):
  pricing.get_products(ServiceCode='AmazonEC2', MaxResults=1, Filters=[
    {'Type':'TERM_MATCH','Field':'instanceType','Value':'<type>'},
    {'Type':'TERM_MATCH','Field':'location','Value':'<region display name>'},
    {'Type':'TERM_MATCH','Field':'operatingSystem','Value':'Windows'},
    {'Type':'TERM_MATCH','Field':'tenancy','Value':'Shared'},
    {'Type':'TERM_MATCH','Field':'preInstalledSw','Value':'NA'},
    {'Type':'TERM_MATCH','Field':'capacitystatus','Value':'Used'},
    {'Type':'TERM_MATCH','Field':'licenseModel','Value':'No License required'}])
Expected: 200, PriceList with exactly one offer document carrying terms.OnDemand with a USD pricePerUnit above the Linux rate for the same instance type.
Actual: 200, PriceList == [] (no error code, no Message).
The instance type, the location display name and the rate are values taken from the request and the catalogue; the fixed part of the requirement is that this attribute combination resolves to one Windows SKU with an OnDemand USD rate, and that the rate exceeds the Linux rate for the same type.

Corroborating (already OWNED, listed for context only): ec2__pricing.ecprice-cli-trajectory-matches-sdk-estimate -> AssertionError: the filter set [... 'operatingSystem': 'Windows' ... 't3.micro' ... 'US West (Oregon)' ...] matched 0 SKUs: [].

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the check appears under `### ec2__pricing` as [mismatch] ecprice-e2e-estimate-running-instance-costs-lifecycle, unmarked.
2. Standalone repro, with the pricing client pinned to us-east-1:
   a. p = boto3.client('pricing', region_name='us-east-1')
   b. linux = p.get_products(ServiceCode='AmazonEC2', MaxResults=1, Filters=[{'Type':'TERM_MATCH','Field':'instanceType','Value':'t3.micro'},{'Type':'TERM_MATCH','Field':'location','Value':'US West (Oregon)'},{'Type':'TERM_MATCH','Field':'operatingSystem','Value':'Linux'},{'Type':'TERM_MATCH','Field':'tenancy','Value':'Shared'},{'Type':'TERM_MATCH','Field':'preInstalledSw','Value':'NA'},{'Type':'TERM_MATCH','Field':'capacitystatus','Value':'Used'},{'Type':'TERM_MATCH','Field':'licenseModel','Value':'No License required'}])
      -> returns one SKU today.
   c. Re-issue (b) with operatingSystem='Windows' -> observe PriceList == []. It must return exactly one SKU whose terms.OnDemand.*.priceDimensions.*.pricePerUnit.USD parses as a float.
   d. Assert float(windows_rate) > float(linux_rate) for the same instance type and location.
3. Equivalently: deploy a mixed fleet (1x t3.micro Linux, 1x t3.micro Windows, 2x t3.nano Linux), group running instances by (InstanceType, Platform) via DescribeInstances, and price each group with the filter set above; no group may price at 0.0.
