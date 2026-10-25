Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# GetInstanceMetricData's inverted-window refusal prints whole seconds, the form its two sibling metric handlers document as wrong

_Filed: 2026-10-25T09:20:00Z_
_Operation: GetInstanceMetricData | Class: error-response_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three unit-bearing metric handlers in this crate refuse a window whose `endTime` does not follow its `startTime` with the same sentence, "startTime must be earlier than endTime, got startTime {} and endTime {} (seconds since the epoch)". `GetBucketMetricData` and `GetDistributionMetricData` interpolate `input.start_time.as_secs_f64()` / `input.end_time.as_secs_f64()` and carry a comment stating exactly why: the sub-second part must be included "so a window inverted inside one second — or one that lies before the epoch — is reported as what it is rather than as two equal whole seconds". `GetInstanceMetricData` interpolates `start.0` / `end.0`, the whole-seconds component of the `timestamp_key` tuple — producing precisely the "two equal whole seconds" message the siblings call out as misreporting. A caller who sends startTime=1700000000.900 and endTime=1700000000.100 is correctly refused, but is told "got startTime 1700000000 and endTime 1700000000", which states no reason for the refusal.

## Evidence

The handler that prints whole seconds — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_instance_metric_data.rs:222-229:
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
and `timestamp_key`'s `.0` is only the seconds, src/state.rs:11770-11772:
```
pub fn timestamp_key(value: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
}
```

The two siblings state the rule this breaks — src/ops/get_distribution_metric_data.rs:271-281:
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

Test coverage: tests/all/instance_metric_data.rs, tests/all/distribution_metric_data.rs and tests/all/bucket_metric_data.rs each exercise the inverted-window refusal; none asserts the interpolated numbers, so no test pins the whole-second form.

## Steps to Reproduce

1. Call GetInstanceMetricData for an existing instance with metricName="CPUUtilization", unit="Percent", period=300, statistics=["Average"], startTime=1700000000.9 and endTime=1700000000.1.
2. Observe HTTP 400 InvalidInputException with message "startTime must be earlier than endTime, got startTime 1700000000 and endTime 1700000000 (seconds since the epoch)" — two identical numbers that do not state the refusal's cause.
3. Call GetDistributionMetricData (or GetBucketMetricData) with the same inverted sub-second window and observe the message carries 1700000000.9 and 1700000000.1, the form the sibling's own comment (get_distribution_metric_data.rs:275-278) says is required.
