Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3eaf755d8cc11ae1fb08c6ffaeacb042be4bf7e2

# A sample count stored as an Ion integer beyond i64 is dropped from the tally instead of counted, against sample_count's own stated rule

_Filed: 2026-10-29T10:27:22Z_
_Operation: BatchGetFrameMetricData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3eaf755d8cc11ae1fb08c6ffaeacb042be4bf7e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`sample_count` documents exactly when it refuses to read a value as a sample count: "when it is no number, or no number a count of samples can be (negative, infinite or not a number)". The Ion integer branch breaks that rule: it reads the integer through `expect_i64()` and propagates `None` with `?` the moment the value does not fit an `i64`. Such a value IS a number and IS a count a profile can record (positive, finite), so by the function's own rule it must be counted.

The consequence is not a refusal but a silently wrong value. `tally_into` treats `None` from `sample_count` as "this member is not a count, descend into it" (`None => tally_into(value, ...)`), and the value is a scalar, so the walk's `_ => {}` arm adds nothing: the count lands in neither `counts.0` (the frames the metric names) nor `counts.1` (the whole profile). The reported `frameMetricData` value for that step is therefore computed as if the frame had recorded nothing in that thread state — a ratio over partial data, reported as a complete answer.

The same crate reads the same Ion type the other way one file over: `get_profile`'s `json_from_ion` falls back to `json_number(integer.to_string())` when `expect_i64()` fails, i.e. it carries the out-of-`i64` integer over instead of dropping it. Only the `Decimal` branch of `sample_count` has an equivalent fallback (it goes through the decimal's own text), so one Ion spelling of a huge count is counted and the other is not.

## Evidence

The rule the function states — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/batch_get_frame_metric_data.rs:403-405:
```
/// `element` as a sample COUNT — the number a frame carries for one thread state — or
/// `None` when it is no number, or no number a count of samples can be (negative,
/// infinite or not a number).
```
The code under it — src/ops/batch_get_frame_metric_data.rs:406-419:
```
fn sample_count(element: &Element) -> Option<f64> {
    let count = match element.value() {
        IonValue::Int(integer) => integer.expect_i64().ok().map(|value| value as f64)?,
        IonValue::Float(float) => *float,
        // Ion writes a decimal exponent with `d`, where Rust writes it with `e`.
        IonValue::Decimal(decimal) => decimal
            .to_string()
            .replace(['d', 'D'], "e")
            .parse::<f64>()
            .ok()?,
        _ => return None,
    };
    (count.is_finite() && count >= 0.0).then_some(count)
}
```
What a `None` then does to the tally — src/ops/batch_get_frame_metric_data.rs:370-388:
```
                match sample_count(value) {
                    Some(count) if thread_states.contains(name) => {
                        counts.1 += count;
                        if inside {
                            counts.0 += count;
                        }
                    }
                    // A number keyed by anything else is a value the frame carries, not a
                    // count in a thread state this metric asks about.
                    Some(_) => {}
                    None => tally_into(
```
The sibling reader of the same Ion type, which does NOT drop it — src/ops/get_profile.rs:690-694:
```
        IonValue::Int(integer) => integer
            .expect_i64()
            .ok()
            .map(::serde_json::Value::from)
            .unwrap_or_else(|| json_number(integer.to_string())),
```
Test coverage of the tally today: tests/all/frame_metric_data.rs::a_frame_metric_is_the_frames_counts_over_the_profiles_counts (value_cases at tests/all/frame_metric_data.rs:474) uses counts inside `i64`, so no case exercises the dropped branch.

## Steps to Reproduce

1. Create a profiling group, then POST an Ion profile whose root struct carries `start` and a frame carrying a thread-state count larger than `i64::MAX` (e.g. `{ start: 1600000000000, A: { WAITING: 18446744073709551616 } }`).
2. Wait for the aggregation of that profile to fall due (crate::state::aggregated_at) and call BatchGetFrameMetricData for frameName `A`, type `AggregatedRelativeTotalTime`, threadStates `[WAITING]` over a period covering the profile.
3. Observe the reported value is 0: src/ops/batch_get_frame_metric_data.rs:408 returns `None` for that integer, so src/ops/batch_get_frame_metric_data.rs:380 descends into the scalar and adds the count to neither `counts.0` nor `counts.1`.
4. Post the same count spelled as an Ion decimal (`1.8446744073709551616d19`) and repeat: the decimal branch (lines 411-415) reads it, so the same count is now included — the two spellings of one count disagree.
