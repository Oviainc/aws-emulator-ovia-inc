Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sustainability.carbon-dimension-values-echo-requested-dimension sustainability.carbon-dimension-values-invalid-dimension-enum-rejected sustainability.carbon-dimension-values-pagination-respects-max-results sustainability.carbon-dimension-values-subset-of-groupby-results sustainability.carbon-emissions-concurrent-pagination-walks-independent sustainability.carbon-emissions-dimension-discovery-then-filter-e2e-workflow sustainability.carbon-emissions-group-by-multiple-dimensions sustainability.carbon-emissions-group-by-service-key-present sustainability.carbon-emissions-group-by-usage-account-id-matches-caller sustainability.carbon-emissions-groupby-region-key-present sustainability.carbon-emissions-invalid-emissions-type-rejected sustainability.carbon-emissions-invalid-groupby-enum-rejected sustainability.carbon-emissions-max-results-cap-respected-and-out-of-range-rejected sustainability.carbon-emissions-missing-permission-access-denied-rejected sustainability.carbon-emissions-pagination-no-duplicate-across-pages sustainability.carbon-emissions-pagination-respects-max-results sustainability.carbon-emissions-repeated-query-deterministic sustainability.carbon-emissions-response-shape-and-time-period-bounds sustainability.carbon-time-period-end-before-start-rejected sustainability.concurrent-identical-queries-consistent sustainability.dimension-values-echo-requested-dimension sustainability.dimension-values-end-before-start-rejected sustainability.dimension-values-invalid-dimension-enum-rejected sustainability.dimension-values-multi-dimension-request sustainability.dimension-values-pagination-consistency sustainability.explicit-deny-policy-overrides-allow-rejected sustainability.get-carbon-emissions-denied-without-permission-rejected sustainability.water-allocation-deterministic-repeat-call sustainability.water-allocation-invalid-allocation-type-rejected sustainability.water-allocation-pagination-consistency sustainability.water-dimension-values-subset-of-groupby-results sustainability.water-unsupported-monthly-granularity-rejected
Issue-Shape-Id: com.amazonaws.sustainability#AwsSustainabilityApiService
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|UnknownService
Issue-Key: sustainability.carbon-dimension-values-invalid-dimension-enum-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability|GetEstimatedCarbonEmissions|UnknownService
Issue-Key: sustainability.carbon-emissions-invalid-emissions-type-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability.carbon-emissions-invalid-groupby-enum-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability.carbon-time-period-end-before-start-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability.dimension-values-end-before-start-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability.dimension-values-invalid-dimension-enum-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability.explicit-deny-policy-overrides-allow-rejected|run#20|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: sustainability.get-carbon-emissions-denied-without-permission-rejected|run#11|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: sustainability|GetEstimatedWaterAllocation|UnknownService
Issue-Key: sustainability.water-allocation-invalid-allocation-type-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|UnknownService
Issue-Key: sustainability.water-unsupported-monthly-granularity-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Sustainability: every request is refused with UnknownService — the service never claims a sigv4a-signed request, so all 4 operations are unreachable

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEstimatedCarbonEmissions

## Description
Cause 1 of 1: the emulator's request router resolves no target service for any AWS Sustainability request, so all four modeled operations (GetEstimatedCarbonEmissions, GetEstimatedCarbonEmissionsDimensionValues, GetEstimatedWaterAllocation, GetEstimatedWaterAllocationDimensionValues) answer an `UnknownService` client error instead of their modeled response or their modeled error. All 32 behavioral checks for this service fail on this one refusal, whether they expected a successful 200 body, a ValidationException, or an AccessDeniedException. Failing check ids for this cause: all 32 listed in trace_ids.

AWS: AwsSustainabilityApiService (Smithy: sdkId `Sustainability`, apiVersion 2018-05-10, protocol `restJson1`, sigv4Name `sustainability`, authSchemes `["sigv4", "sigv4a"]` with `aws.auth#sigv4a` listed first, endpointResolution urlTemplates `https://api.sustainability.{Region}...` / `https://sustainability.global...`). A request that reaches the service endpoint and names one of the four modeled routes (Smithy http bindings: POST /v1/estimated-carbon-emissions, POST /v1/estimated-carbon-emissions-dimension-values, POST /v1/estimated-water-allocation, POST /v1/estimated-water-allocation-dimension-values) is dispatched to that operation; it then either returns the modeled output shape or one of the service's modeled errors (Smithy common_errors: AccessDeniedException, InternalServerException, ThrottlingException, ValidationException). AWS never answers an operation of this service with an `UnknownService` error code — that code is not in the model at all. The region pinning the checks use (us-east-1) is the documented one for this service's partitional endpoint.

Emulator: every call returns a client error with code `UnknownService` and the message (example) "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)". Observed for all four operations, on both happy-path and rejection checks. The negative checks therefore report `UnknownService` where they asserted `ValidationException` (e.g. sustainability.carbon-time-period-end-before-start-rejected, sustainability.water-unsupported-monthly-granularity-rejected, sustainability.carbon-emissions-invalid-groupby-enum-rejected) or `AccessDeniedException` (sustainability.get-carbon-emissions-denied-without-permission-rejected, sustainability.explicit-deny-policy-overrides-allow-rejected), and the happy-path checks never get past the first call. Nothing in this service was exercised beyond dispatch, so no statement can be made about the fidelity of its handlers until dispatch works.

Hypothesis (neither the model nor the recording states the mechanism): the service's `can_handle` seam claims a request only when `resolve_route_key(parts)` equals the signing name `sustainability`, and the resolver derives that key from the SigV4 credential scope, `X-Amz-Target`, or the Host header. restJson1 carries no `X-Amz-Target`; an endpoint-pinned client sends a Host of the emulator's own address rather than `api.sustainability.<region>.amazonaws.com`; and because this model lists `aws.auth#sigv4a` ahead of `aws.auth#sigv4`, an SDK may sign with AWS4-ECDSA-P256-SHA256, whose `Credential=` scope has no region segment and so does not parse as a plain sigv4 `date/region/service/aws4_request` scope. That would leave all three discriminators unusable and produce exactly the observed refusal.

Why: a client configured for AWS Sustainability cannot make a single call against the emulator. Every operation fails with an error code that does not exist in the service model, so SDK/botocore error handling cannot classify it, retry logic cannot act on it, and no carbon-emissions or water-allocation code path can be tested at all. Negative-path clients are also misled: input that AWS rejects with ValidationException and principals AWS rejects with AccessDeniedException both surface as UnknownService.

Fix: service dispatch/routing for this service — the `can_handle` / route-key resolution seam, so a sustainability request is claimed by its endpoint prefix (`api.sustainability.*` / `sustainability.*`) and by a sigv4a credential scope as well as a plain sigv4 one, instead of only a plain-sigv4 credential scope. Once claimed, the generated restJson1 routing table can resolve the four modeled URIs.

Traces: the 32 failing check ids in trace_ids; representative lines — sustainability.carbon-emissions-response-shape-and-time-period-bounds (GetEstimatedCarbonEmissions, UnknownService), sustainability.dimension-values-echo-requested-dimension (GetEstimatedCarbonEmissionsDimensionValues, UnknownService), sustainability.water-allocation-deterministic-repeat-call (GetEstimatedWaterAllocation, UnknownService), sustainability.water-dimension-values-subset-of-groupby-results (GetEstimatedWaterAllocationDimensionValues, UnknownService). Relevant files: services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs lines 76-84 (`can_handle` matching `resolve_route_key` against the signing name) and lines 86-96 (`override_operation`, currently returning None).

## Evidence
Suite: 0/32 sustainability checks passed (10 mismatched, 22 errored).

Expected (AWS): each of the four modeled operations dispatches and answers with its modeled output, or with a modeled error — ValidationException for an out-of-enum Dimensions/GroupBy/EmissionsType/AllocationTypes member, for an inverted TimePeriod, for MONTHLY granularity on GetEstimatedWaterAllocation, and for MaxResults outside its range; AccessDeniedException for a principal without sustainability:Get* (or with an explicit Deny). `UnknownService` is not a modeled error code for this service.

Actual (emulator), verbatim from the replay:
  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the GetEstimatedCarbonEmissionsDimensionValues operation: could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)
  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the GetEstimatedCarbonEmissions operation: could not resolve a target service from the request (...)
  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the GetEstimatedWaterAllocation operation: could not resolve a target service from the request (...)
  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the GetEstimatedWaterAllocationDimensionValues operation: could not resolve a target service from the request (...)

And via the rejection checks, the code mismatch:
  AssertionError: expected ValidationException, got 'UnknownService'   (carbon-time-period-end-before-start-rejected, dimension-values-end-before-start-rejected, carbon-dimension-values-invalid-dimension-enum-rejected, dimension-values-invalid-dimension-enum-rejected, carbon-emissions-invalid-groupby-enum-rejected, carbon-emissions-invalid-emissions-type-rejected, water-allocation-invalid-allocation-type-rejected, water-unsupported-monthly-granularity-rejected)
  AssertionError: expected AccessDeniedException, got 'UnknownService' (get-carbon-emissions-denied-without-permission-rejected, explicit-deny-policy-overrides-allow-rejected)

The error code is the requirement here; the quoted router message is an example of the emulator's wording, not a required string.

## Steps to Reproduce
1. Point a Sustainability client at the emulator: client = boto3.client('sustainability', region_name='us-east-1', endpoint_url=<emulator>).
2. Call any of the four operations, e.g.
   client.get_estimated_carbon_emissions(TimePeriod={'Start': <month-floor 6 months ago>, 'End': <month-floor now>}, GroupBy=['SERVICE'])
   client.get_estimated_carbon_emissions_dimension_values(TimePeriod={...}, Dimensions=['REGION','SERVICE'])
   client.get_estimated_water_allocation(TimePeriod={...}, Granularity='YEARLY_CALENDAR')
   client.get_estimated_water_allocation_dimension_values(TimePeriod={...}, Dimensions=['REGION'])
3. Every call raises ClientError with Error.Code == 'UnknownService' and the message 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'. Expected: a 200 with the modeled response shape, or a modeled error (ValidationException / AccessDeniedException) when the request or the caller's permissions warrant it.
4. Same result for a deliberately invalid request, e.g. Dimensions=['NOT_A_REAL_DIMENSION'] or an inverted TimePeriod: UnknownService instead of ValidationException.
