Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# Deduplicate the sampling pre-flight and refusal-mapping logic shared by Sample and SampleWithResponseStream

_Filed: 2026-10-25T15:48:19Z_
_Operation: Sample | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two sampling handlers carry the same three rules twice, line for line, differing only in the error enum name, the `streamed` flag, the body renderer and the Content-Type constant:

1. the empty-`Body` refusal,
2. the `job_exists` job-resolution refusal (and its message), and
3. the `AppendRefusal` -> modelled-error mapping (`is_quota()` -> `ServiceQuotaExceededException`, everything else -> `ValidationException`, with the comment explaining why a sealed trajectory is not a `ConflictException`).

Because each rule exists in two places, the copies can drift: a change to how a sealed trajectory or a quota refusal is reported, or to the order of the pre-flight checks, has to be made twice, and a one-sided edit silently makes `/sample` and `/sample-with-response-stream` answer differently for the same state. The service's own doc already states the two paths are meant to be identical ("The SAME state path as `Sample`, so one trajectory can mix turns served by either operation; only the framing of the response body differs." — sample_with_response_stream.rs:201-202), which is the invariant the duplication puts at risk.

The shared part can live once in the crate (e.g. next to `append_turn` in `src/state.rs`, answering either the `ServedTurn` or the `(is_quota, message)` pair, with each handler keeping its own error enum and its own body renderer). Every response body, header, status and message stays byte-identical, so no existing test changes.

## Evidence

Both copies, same crate:

`src/ops/sample.rs:165-172`:
```
        // `Body` is `@required` and `@httpPayload`, so a request without one is
        // already refused at the boundary; a request WITH an empty payload gets
        // this far and carries no inference request to capture.
        if input.body.is_empty() {
            return Err(validation(
                "Body must carry the inference request payload; it was empty.".to_string(),
            ));
        }
```
`src/ops/sample_with_response_stream.rs:167-174`: the identical block, same comment, same sentence.

`src/ops/sample.rs:182-186`:
```
        if !crate::state::job_exists(&*self.engine, &input.job_arn) {
            return Err(validation(crate::state::job_not_found_message(
                &input.job_arn,
            )));
        }
```
`src/ops/sample_with_response_stream.rs:184-188`: identical, preceded by the same 8-line comment.

`src/ops/sample.rs:223-236`:
```
            .map_err(|refusal| {
                let message = refusal.message(&trajectory_id);
                if refusal.is_quota() {
                    SampleErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException { message },
                    )
                    .into()
                } else {
                    // `Sample` models no `ConflictException`, so a request that
                    // conflicts with the trajectory's sealed state is refused with
                    // the client error it does model.
                    validation(message)
                }
            })?;
```
`src/ops/sample_with_response_stream.rs:227-240`: the same branch with `SampleWithResponseStreamErrorModeled` substituted.

The handler itself states the two paths must stay the same — `src/ops/sample_with_response_stream.rs:201-202`: "// The SAME state path as `Sample`, so one trajectory can mix turns served // by either operation; only the framing of the response body differs."

Tests that already pin every response the shared code produces (so a refactor is verifiable without new tests):
- `tests/all/trajectory_lifecycle.rs:315` `every_sampling_call_captures_a_turn_and_serves_what_it_recorded`, `:540` `complete_rollout_seals_the_trajectory_and_refuses_a_different_status` (the sealed-trajectory 400 on both paths, lines 643-659), `:1041` `the_turn_bound_is_reported_as_a_service_quota`;
- `tests/all/job_resolution.rs:189` `a_sampling_call_for_a_job_that_was_never_created_is_refused` and `:291` `a_sampling_call_for_a_created_job_is_served_however_the_arn_addresses_it` (both iterate `[SAMPLE_PATH, SAMPLE_STREAM_PATH]`);
- `tests/all/job_arn_is_the_callers_job.rs:203`/`:275`;
- `tests/all/modelled_rejections.rs:409` `a_payload_too_large_to_retain_is_refused_and_captured_bytes_are_counted`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample.rs and read `handle` (lines 153-242).
2. Open services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample_with_response_stream.rs and read `handle` (lines 153-246): the empty-body refusal, the `job_exists` refusal and the `AppendRefusal` -> error mapping are the same code twice, differing only in the error enum name, the `streamed` flag, the body renderer and the Content-Type constant.
3. Change one copy only (for example reword or re-order the empty-`Body` refusal in sample.rs) and observe that `/sample` and `/sample-with-response-stream` now answer differently for the same input, with nothing in the crate holding the two in step.
4. Confirm the behaviour is already pinned on both paths by `tests/all/job_resolution.rs` (rows loop over `[SAMPLE_PATH, SAMPLE_STREAM_PATH]`) and `tests/all/trajectory_lifecycle.rs`, so extracting the shared logic must leave every response and every test unchanged.
