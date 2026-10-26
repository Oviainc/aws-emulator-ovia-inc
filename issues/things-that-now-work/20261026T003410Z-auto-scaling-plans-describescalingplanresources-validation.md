Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# DescribeScalingPlanResources accepts a ScalingPlanVersion other than the single documented valid value

_Filed: 2026-10-26T00:34:10Z_
_Operation: DescribeScalingPlanResources | Class: validation_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeScalingPlanResourcesRequest$ScalingPlanVersion` is documented as "The version number of the scaling plan. Currently, the only valid value is 1." The handler never checks the member: it passes the caller's value straight into `state.plan(name, version)`, so `ScalingPlanVersion: 2`, `0`, `-1` or `4294967296` is answered with HTTP 200 and an empty `ScalingPlanResources` list, as if the request were well-formed and merely matched nothing.

This contradicts the sibling handler in the SAME crate: `GetScalingPlanResourceForecastData` carries the identical member sentence and refuses every other version with the modelled `ValidationException`, on exactly the reasoning that also applies here — the operation models no not-found shape, so a version the model declares invalid can only be a request-level refusal. `DescribeScalingPlanResources` models `ValidationException` too, so it has the shape to answer with and simply does not. The crate's own test comment justifies the gap by asserting the member is unbounded ("bounds its `ScalingPlanVersion` member nowhere"), which the model sentence above contradicts. (The same sentence is on `DescribeScalingPlansRequest$ScalingPlanVersion`, where the value is a filter paired with a name; the identifying-key use here is the clear case.)

## Evidence

Smithy model: smithy_query(query="doc",shape="DescribeScalingPlanResourcesRequest",member="ScalingPlanVersion") -> "The version number of the scaling plan. Currently, the only valid value is 1." — reproduced in the handler's own header, src/ops/describe_scaling_plan_resources.rs:63-64: "//! * `ScalingPlanVersion` (ScalingPlanVersion) **required** — The version number of the scaling plan. Currently, the only valid value is 1."

Handler does no check — src/ops/describe_scaling_plan_resources.rs:181-183:
```
        let (total, scaling_plan_resources) =
            match state.plan(&input.scaling_plan_name, input.scaling_plan_version) {
                None => (0usize, Vec::new()),
```
(the only input validation is MaxResults at :162-163 and NextToken at :167-172).

Sibling in the same crate DOES check it — src/ops/get_scaling_plan_resource_forecast_data.rs:375-382:
```
fn validate_plan_version(version: i64) -> Result<(), String> {
    if version != crate::state::ONLY_VERSION {
        return Err(format!(
            "ScalingPlanVersion {version} is not valid: the only valid value is {}",
            crate::state::ONLY_VERSION
```
with the same justification quoted above it ("documents exactly one valid value ... so any other version is a request this service refuses rather than a lookup that misses: this operation models no not-found error at all"). Both operations bind `ValidationException` and neither binds a not-found shape (smithy_query(query="operation",operation="DescribeScalingPlanResources,GetScalingPlanResourceForecastData")).

Test coverage: tests/all/absent_plan_reads.rs:169-203 pins the current behaviour and states the contradicted reason — "`DescribeScalingPlanResources` bounds its `ScalingPlanVersion` member nowhere, so a version this service does not hold is a lookup that misses — the empty set again" — asserting 200/`[]` for versions 2, 0, -1 and 4_294_967_296, while asserting (400, "ValidationException") for the very same versions on the forecast operation. tests/all/forecast_data.rs:672 `only_the_documented_scaling_plan_version_is_accepted` pins the sibling's refusal.

## Steps to Reproduce

1. CreateScalingPlan {"ScalingPlanName":"held plan","ApplicationSource":{"CloudFormationStackARN":"arn:aws:cloudformation:us-east-1:123456789012:stack/s1"},"ScalingInstructions":[...one valid instruction...]} and let it settle to Active.
2. Call GetScalingPlanResourceForecastData for that plan with "ScalingPlanVersion": 2 — observe HTTP 400 ValidationException naming the only valid version.
3. Call DescribeScalingPlanResources {"ScalingPlanName":"held plan","ScalingPlanVersion":2} — observe HTTP 200 with "ScalingPlanResources": [], even though the member's documentation declares 1 the only valid value and the operation models ValidationException.
