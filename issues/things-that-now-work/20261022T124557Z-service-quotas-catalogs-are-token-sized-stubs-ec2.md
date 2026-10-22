Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.list-aws-default-service-quotas-pagination service-quotas.list-service-quotas-pagination-no-duplicates service-quotas.list-services-pagination-no-duplicates
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas.list-aws-default-service-quotas-pagination|run#7|AssertionError|assert len(seen) >= 50, f"expected at least 50 EC2 default quotas, got {len(seen)}"
Issue-Key: service-quotas.list-service-quotas-pagination-no-duplicates|run#6|AssertionError|assert pages10 > 1, f"expected MaxResults=10 to force multiple pages, got {pages10}"
Issue-Key: service-quotas.list-services-pagination-no-duplicates|run#6|AssertionError|assert len(seen) >= 50, f"expected at least 50 services, got {len(seen)}"
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# Service Quotas catalogs are token-sized stubs: ec2 defaults, ec2 applied quotas and ListServices never paginate

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceQuotas

## Description
The data Service Quotas serves out of its state store is a handful of sample rows rather than a service catalog, so every enumeration path returns a single short page and NextToken/MaxResults pagination is never exercised at all. Three causes.

Cause 1 of 3 — ListAWSDefaultServiceQuotas('ec2') returns 2 default quotas in a single page.
AWS: ListAWSDefaultServiceQuotas is @paginated (Smithy: inputToken NextToken, outputToken NextToken, items Quotas, pageSize MaxResults; ListAWSDefaultServiceQuotasRequest.MaxResults @range 1-100, NextToken @length 0-2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$) and returns the published AWS default for every quota of the service. For ec2 that catalog is hundreds of rows (recorded behavior: well above 50), so a walk with MaxResults=10 takes many round trips and each non-final page carries a NextToken.
Emulator: the whole walk yields 2 QuotaCodes in 1 page, with no NextToken, so the response both is incomplete and declares itself complete.
Why: a client enumerating the default catalog (drift detection against applied values, quota inventory, picking a quota code to request an increase for) silently sees two quotas and concludes the account has no others; and a client whose pagination logic is wrong is never caught because the emulator never emits a second page.
Fix: the default-quota catalog rows in the service's state module (services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs) plus the MaxResults/NextToken paging in ops/list_aws_default_service_quotas.rs.
Traces: service-quotas.list-aws-default-service-quotas-pagination.

Cause 2 of 3 — ListServiceQuotas('ec2') with MaxResults=10 completes in one page.
AWS: ListServiceQuotas is @paginated with MaxResults @range 1-100 and returns the account's applied quotas for the service; ec2's applied set exceeds even the maximum page size, so a MaxResults=10 walk and a MaxResults=100 walk both take multiple pages and converge on the identical duplicate-free set of QuotaCodes (recorded behavior: >= 50 codes).
Emulator: the MaxResults=10 walk terminates after a single page, i.e. the applied set is at most 10 quotas and the cursor is never advanced.
Why: anything that selects an applied quota by name from this list (request an increase, tag the quota, read its utilization) cannot find the quota it needs, and the pagination contract clients depend on is unverifiable. The open issue 20261022T115709Z-servicequotas-service-quotas-returns-the-wrong-quota-record-and.md records the same stub for ServiceCode 'lambda' from the wire corpus (its cause 5); this check shows it for 'ec2' through the paginated walk and is not covered by that issue's keys.
Fix: the applied-quota state backing ListServiceQuotas (src/state.rs) and that handler's NextToken/MaxResults pagination (ops/list_service_quotas.rs).
Traces: service-quotas.list-service-quotas-pagination-no-duplicates.

Cause 3 of 3 — ListServices returns only 5 ServiceCodes in total.
AWS: ListServices lists every Amazon Web Services service integrated with Service Quotas (Smithy: ListServices, errors IllegalArgumentException/InvalidPaginationTokenException, paginated with MaxResults @range 1-100); recorded behavior is hundreds of ServiceInfo entries, so a MaxResults=5 walk makes many NextToken-chained calls.
Emulator: the full MaxResults=5 walk yields 5 ServiceCodes and then stops.
Why: ListServices is the documented way to discover a valid ServiceCode before any other call; a 5-entry catalog makes almost every real service code look nonexistent, and downstream calls for those codes then fail with NoSuchResourceException (as seen in the cloudformation history check).
Fix: the service catalog rows in src/state.rs and ops/list_services.rs pagination.
Traces: service-quotas.list-services-pagination-no-duplicates.

## Evidence
Cause 1: check service-quotas.list-aws-default-service-quotas-pagination — walking ListAWSDefaultServiceQuotas(ServiceCode='ec2', MaxResults=10) via NextToken, AssertionError: 'expected at least 50 EC2 default quotas, got 2'; the walk also finished in a single page, so MaxResults/NextToken were never exercised.
Cause 2: check service-quotas.list-service-quotas-pagination-no-duplicates — walking ListServiceQuotas(ServiceCode='ec2', MaxResults=10), AssertionError: 'expected MaxResults=10 to force multiple pages, got 1'.
Cause 3: check service-quotas.list-services-pagination-no-duplicates — walking ListServices(MaxResults=5) via NextToken, AssertionError: 'expected at least 50 services, got 5'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite.
2. Cause 1: POST ListAWSDefaultServiceQuotas ServiceCode='ec2', MaxResults=10 and follow NextToken to exhaustion -> 2 QuotaCodes, 1 page, no NextToken.
3. Cause 2: POST ListServiceQuotas ServiceCode='ec2', MaxResults=10 and follow NextToken to exhaustion -> 1 page.
4. Cause 3: POST ListServices MaxResults=5 and follow NextToken to exhaustion -> 5 ServiceCodes.
