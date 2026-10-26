Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# GetProfile refuses modeled, valid requests (maxDepth, Accept, non-enum ISO-8601 period) with 400 ValidationException

_Filed: 2026-10-24T06:11:47Z_
_Operation: GetProfile | Class: error-response_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three documented inputs that the real service honours are turned into 400 ValidationException by the emulator:

1. `maxDepth` (integer, range 1..10000, @httpQuery): the handler returns ValidationException unconditionally when it is present ("maxDepth is not supported: this emulator ... cannot truncate its call stacks"). Real AWS answers 200 with the aggregated profile truncated to that depth; a request inside the modeled range is never a client error.
2. `accept` (@httpHeader(Accept)): the handler refuses any media type that is not literally the Content-Type the agent posted (only `*/*` or an exact match passes). The modeled output `contentType` doc says the profile "is either application/json or the default application/x-amzn-ion", i.e. the real service serves both representations — so `Accept: application/x-amzn-ion` (the service DEFAULT) against a JSON-posted profile is a 400 here where AWS answers 200.
3. `period` (shape `Period`: string, len 1..64 — NOT the AggregationPeriod enum) is restricted to exactly PT5M/PT1H/P1D. The member's own model doc says "Used with startTime or endTime to specify the time range for the returned aggregated profile. Specify using the ISO 8601 format. For example, P1DT1H1M1S" — the model's own example value is refused with 400 by this handler, as is any other well-formed ISO-8601 duration (e.g. PT7M, PT30M).

Expected: a well-formed request carrying any of these three is not a ValidationException; the response is the profile (or, where no aggregated profile covers the window, 404 ResourceNotFoundException). Actual: 400 ValidationException.

## Evidence

Smithy model: GetProfileInput.maxDepth is `@range(min:1,max:10000)` and @httpQuery; `period` targets `Period` (string, length 1..64) with the doc example `P1DT1H1M1S`; GetProfileOutput.contentType doc names both application/json and application/x-amzn-ion as the representations the service serves. Handler: src/ops/get_profile.rs — `if input.max_depth.is_some() { return Err(...validation...) }`, the `accept` media-type match block, and `requested_window`'s `aggregation_period_seconds(period).ok_or_else(... validation ...)` restricted to crate::state::AGGREGATION_PERIODS. Tests currently lock the divergence in rather than cover it: tests/all/profiling_group_lifecycle.rs asserts ?maxDepth=2 -> 400, ?period=PT7M -> 400 and Accept: application/x-amzn-ion -> 400. Not covered by the open GetProfile issue, which concerns the un-aggregated 200 on the success path.

## Steps to Reproduce

1. Create profiling group N and PostAgentProfile a profile for it with Content-Type: application/json.
2. GET /profilingGroups/N/profile?period=PT5M&maxDepth=2 -> 400 ValidationException (real AWS: a profile, or 404 if nothing is aggregated for the window).
3. GET /profilingGroups/N/profile?period=P1DT1H1M1S (the model's own documented example duration) -> 400 ValidationException "period must be one of P1D, PT1H, PT5M".
4. GET /profilingGroups/N/profile with header `Accept: application/x-amzn-ion` -> 400 ValidationException, although that is the service's default profile media type.
