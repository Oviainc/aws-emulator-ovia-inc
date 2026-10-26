Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# GetContainerServiceMetricData's inverted-window refusal prints whole seconds, the form its sibling metric handlers document as wrong

_Filed: 2026-10-26T05:32:57Z_
_Operation: GetContainerServiceMetricData | Class: error-response_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four metric handlers in this crate refuse a window whose `endTime` does not follow its `startTime` with the same sentence, "startTime must be earlier than endTime, got startTime {} and endTime {} (seconds since the epoch)". GetBucketMetricData and GetDistributionMetricData interpolate `input.start_time.as_secs_f64()` / `input.end_time.as_secs_f64()` and carry a comment saying exactly why: the sub-second part must be included "so a window inverted inside one second — or one that lies before the epoch — is reported as what it is rather than as two equal whole seconds". GetContainerServiceMetricData interpolates `start.0` / `end.0`, the whole-seconds component of the `timestamp_key` tuple — producing precisely the "two equal whole seconds" message the siblings call out as misreporting. A caller sending startTime=1700000000.900 and endTime=1700000000.100 is correctly refused but told "got startTime 1700000000 and endTime 1700000000", which states no reason for the refusal.

(The same defect on GetInstanceMetricData is already filed separately; this is the second copy of it, on a different operation.)

## Evidence

The handler that prints whole seconds — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_container_service_metric_data.rs:211-219:
```
        let start = crate::state::timestamp_key(input.start_time);
        let end = crate::state::timestamp_key(input.end_time);
        if start >= end {
            return Err(invalid(format!(
                "startTime must be earlier than endTime, got startTime {} and endTime {} \
                 (seconds since the epoch)",
                start.0, end.0
            )));
        }
```
and `timestamp_key`'s `.0` is only the seconds — src/state.rs:12686:
```
pub fn timestamp_key(value: ::aws_smithy_types::DateTime) -> (i64, u32) {
```

The siblings state the rule this breaks — src/ops/get_distribution_metric_data.rs:271-282:
```
        if start >= end {
            return Err(invalid(format!(
                "startTime must be earlier than endTime, got startTime {} and endTime {} \
                 (seconds since the epoch)",
                // The INSTANT each member names, sub-second part included and
                // signed, so a window inverted inside one second — or one that
                // lies before the epoch — is reported as what it is rather than
                // as two equal whole seconds.
                input.start_time.as_secs_f64(),
                input.end_time.as_secs_f64()
            )));
        }
```
and the identical block at src/ops/get_bucket_metric_data.rs:279-290.

Test coverage: tests/all/container_service_metric_data.rs:449-541 exercises the inverted-window refusal ("a window that ends one second before it starts", "a window that ends a day before it starts") but asserts only that `message` is non-empty (lines 535-540), so no test pins the whole-second form, and no case uses a sub-second window.

## Steps to Reproduce

1. Create a container service in us-east-1 (CreateContainerService serviceName="metric-svc", power="nano", scale=1).
2. Call GetContainerServiceMetricData with serviceName="metric-svc", metricName="CPUUtilization", period=300, statistics=["Average"], startTime=1700000000.9, endTime=1700000000.1.
3. Observe HTTP 400 InvalidInputException with message "startTime must be earlier than endTime, got startTime 1700000000 and endTime 1700000000 (seconds since the epoch)" — two identical numbers that do not state the cause.
4. Call GetDistributionMetricData (or GetBucketMetricData) with the same inverted sub-second window and observe 1700000000.9 / 1700000000.1, the form the sibling's own comment (get_distribution_metric_data.rs:275-278) says is required.
