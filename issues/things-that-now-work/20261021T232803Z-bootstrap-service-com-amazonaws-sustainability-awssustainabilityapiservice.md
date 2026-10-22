Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the Sustainability (AwsSustainabilityApiService) emulator

_Filed: 2026-10-21T23:28:03Z_
_Operation: service-com-amazonaws-sustainability-awssustainabilityapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 4 operations this service binds is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. Nothing about the service works.

This is a read-only reporting API (restJson1, sigv4 signing name `sustainability`); there is no create/delete lifecycle. The bootstrap slice is therefore a SEEDED-DATASET query loop: a deterministic in-memory dataset of environmental-impact records that all four operations read, filter, group, aggregate and paginate consistently. 'Stateful' here means every response is computed from that one dataset, so the dimension-values operations and the data operations agree, and repeated/paginated calls are consistent.

Implement these 4 operations, in this order (all `POST`, all `@paginated(inputToken=NextToken, outputToken=NextToken, items=Results)`):

1. **GetEstimatedCarbonEmissionsDimensionValues** — `POST /v1/estimated-carbon-emissions-dimension-values`. Input: `TimePeriod` {`Start` (required, date-time, inclusive), `End` (required, date-time, exclusive)}, `Dimensions` (required, list<Dimension> of USAGE_ACCOUNT_ID|REGION|SERVICE), `MaxResults` (1..5000, default 1000), `NextToken` (len 1..2000). Output: `Results` (list<DimensionEntry{Dimension, Value}>), `NextToken`. Must return exactly the distinct dimension values present in the seeded carbon dataset whose record period overlaps `TimePeriod` — e.g. `REGION`->`us-east-1`, `SERVICE`->`AmazonEC2`, `USAGE_ACCOUNT_ID`->caller account id. These values must be the same strings that later appear in `DimensionsValues` and that `FilterBy` accepts.

2. **GetEstimatedCarbonEmissions** — `POST /v1/estimated-carbon-emissions`. Input: `TimePeriod` (required), `GroupBy` (list<Dimension>), `FilterBy` (FilterExpression{`Dimensions`: map<Dimension, list<value>>}), `EmissionsTypes` (list<EmissionsType>; when absent default to `TOTAL_LBM_CARBON_EMISSIONS` and `TOTAL_MBM_CARBON_EMISSIONS`), `Granularity` (required, default `MONTHLY`), `GranularityConfiguration{FiscalYearStartMonth, default 1}`, `MaxResults` (1..5000, default 1000), `NextToken`. Output: `Results` (required, list<EstimatedCarbonEmissions{`TimePeriod`, `DimensionsValues` (map), `ModelVersion` (semver string matching the modeled pattern, e.g. `v1.0.0`), `EmissionsValues` (map<EmissionsType, double>)}>), `NextToken`. Must bucket the seeded records into periods by `Granularity` (MONTHLY / QUARTERLY_CALENDAR / QUARTERLY_FISCAL / YEARLY_CALENDAR / YEARLY_FISCAL, fiscal buckets offset by `FiscalYearStartMonth`), apply `FilterBy`, aggregate (sum) per `GroupBy` combination, and emit only the requested `EmissionsTypes`. Omitting `GroupBy` must yield one aggregate row per period with an empty `DimensionsValues`.

3. **GetEstimatedWaterAllocationDimensionValues** — `POST /v1/estimated-water-allocation-dimension-values`. Same input/output shape as (1); `Results` is required here. Must read the seeded water dataset.

4. **GetEstimatedWaterAllocation** — `POST /v1/estimated-water-allocation`. Input: `TimePeriod` (required), `GroupBy`, `FilterBy`, `AllocationTypes` (list<WaterAllocationType>, only `TOTAL_WATER_WITHDRAWALS`), `Granularity` (required, default `YEARLY_CALENDAR` — note the different default from carbon), `MaxResults`, `NextToken`. Output: `Results` (required, list<EstimatedWaterAllocation{`TimePeriod`, `DimensionsValues`, `ModelVersion`, `AllocationValues` (map<WaterAllocationType, double>)}>), `NextToken`.

Cross-cutting requirements for the slice:
- **Pagination must be real**: `MaxResults` caps the page; `NextToken` (opaque, 1..2000 chars) resumes at the next unreturned row and is absent on the last page. Walking all pages must yield every row exactly once with no duplicates.
- **Validation** must return `ValidationException` (HTTP 400) for `TimePeriod.End <= TimePeriod.Start`, `MaxResults` outside 1..5000, an unparseable/unknown `NextToken`, and an unknown enum value in `GroupBy`/`Dimensions`/`EmissionsTypes`/`AllocationTypes`.
- **No canned or echoed responses.** Results must be derived from the seeded dataset by filter+group+aggregate. Returning a fixed row list, echoing the request `TimePeriod` as the only period, or inventing random values per call is a failure.

## State Model

Service state (behind the `crate::state::service_state(&*self.engine)` guard):

```
SustainabilityState {
    model_version: String,                 // e.g. "v1.0.0", same value on every emitted row
    carbon: Vec<ImpactRecord>,             // seeded, deterministic
    water:  Vec<ImpactRecord>,             // seeded, deterministic
    page_tokens: Map<String, PageCursor>,  // opaque NextToken -> cursor
}

ImpactRecord {
    month_start: Timestamp,        // start of the smallest supported bucket (MONTHLY)
    usage_account_id: String,      // dimension USAGE_ACCOUNT_ID
    region: String,                // dimension REGION, e.g. "us-east-1"
    service: String,               // dimension SERVICE, e.g. "AmazonEC2"
    values: Map<String, f64>,      // EmissionsType / WaterAllocationType -> value
}

PageCursor {
    operation: &'static str,       // token is only valid for the op that minted it
    request_fingerprint: String,   // hash of TimePeriod/GroupBy/FilterBy/Granularity/types
    offset: usize,                 // index into the computed result rows
}
```

Seed `carbon` and `water` at service construction with a fixed multi-month, multi-region, multi-service grid (several hundred rows is plenty) so that grouping, filtering and pagination all have something to exercise and results are reproducible across runs.

The remaining long tail (none here beyond these 4 operations — all four are in scope) is covered by later gap passes; anything left stubbed keeps answering 501.

## Evidence

OPERATIONS.md at the crate root lists all 4 operations with routes, @paginated/@readonly traits, input members (TimePeriod required, Dimensions/GroupBy, FilterBy, EmissionsTypes/AllocationTypes, Granularity with defaults MONTHLY for carbon and YEARLY_CALENDAR for water, GranularityConfiguration, MaxResults range 1..5000 default 1000, NextToken len 1..2000), output members (Results, NextToken) and the error set AccessDeniedException 403 / InternalServerException 500 / ThrottlingException 429 / ValidationException 400. Shape details (TimePeriod.Start inclusive / .End exclusive, DimensionEntry{Dimension,Value}, EstimatedCarbonEmissions{TimePeriod,DimensionsValues,ModelVersion semver pattern,EmissionsValues}, EstimatedWaterAllocation{...,AllocationValues}, FilterExpression.Dimensions, GranularityConfiguration.FiscalYearStartMonth default 1 range 1..12, Dimension enum = USAGE_ACCOUNT_ID|REGION|SERVICE, EmissionsType 7 values, WaterAllocationType = TOTAL_WATER_WITHDRAWALS) come from smithy_query on the model. Stub proof: services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/ops/get_estimated_carbon_emissions.rs line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 188 `Err(crate::scaffolded::Scaffolded::not_implemented("GetEstimatedCarbonEmissions"))`; the other three files in src/ops/ are the same generated stub. No tests exist for any of them.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `sustainability` client at it.
2. Call GetEstimatedCarbonEmissionsDimensionValues with TimePeriod {Start: 2025-01-01T00:00:00Z, End: 2025-07-01T00:00:00Z} and Dimensions [REGION, SERVICE] — expect 200 with Results containing DimensionEntry rows such as {Dimension: REGION, Value: us-east-1} and {Dimension: SERVICE, Value: AmazonEC2}; today returns HTTP 501 NotImplemented.
3. Call GetEstimatedCarbonEmissions with the same TimePeriod, Granularity MONTHLY, GroupBy [REGION] and FilterBy {Dimensions: {SERVICE: [AmazonEC2]}} — expect 6 monthly rows, each with DimensionsValues {REGION: <value from step 2>}, a semver ModelVersion, and EmissionsValues keyed by TOTAL_LBM_CARBON_EMISSIONS and TOTAL_MBM_CARBON_EMISSIONS (the documented default set).
4. Repeat the previous call with MaxResults 2 and follow NextToken until it is absent — expect the concatenation of pages to equal the unpaginated result exactly once each, with no duplicated or dropped rows.
5. Re-issue the same request with Granularity YEARLY_CALENDAR and verify the monthly values roll up into one 2025 row whose EmissionsValues equal the sums from step 3.
6. Call GetEstimatedWaterAllocationDimensionValues with Dimensions [SERVICE], then GetEstimatedWaterAllocation with that TimePeriod, GroupBy [SERVICE] and no Granularity — expect YEARLY_CALENDAR bucketing by default and AllocationValues keyed by TOTAL_WATER_WITHDRAWALS.
7. Call GetEstimatedCarbonEmissions with TimePeriod End <= Start, and again with MaxResults 0 — expect HTTP 400 ValidationException in both cases, not 501 and not 200.
