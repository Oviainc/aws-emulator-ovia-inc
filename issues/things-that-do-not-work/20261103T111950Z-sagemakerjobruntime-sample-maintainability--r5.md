Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: abe0322016058f5d6b4da132f622185913313fb8

# Hoist the sampling quota-vs-validation error mapping out of both sampling handlers

_Filed: 2026-11-03T11:19:50Z_
_Operation: Sample | Class: maintainability_

> _Note: this issue was identified against the code base at commit `abe0322016058f5d6b4da132f622185913313fb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a `SamplingRefusal` that is a quota becomes `ServiceQuotaExceededException`, anything else becomes `ValidationException`" is written out twice — once in `Sample::handle` and once, character for character apart from the error enum name, in `SampleWithResponseStream::handle`. `crate::state` already owns the DECISION (the `SamplingRefusal::quota` flag plus `is_quota()`), and its own documentation claims the handler does nothing but name an enum: "so the decision lives here and only the error enum is named at the handler". The code contradicts that: each handler also re-states the `if refusal.is_quota() { … } else { … }` branch, so the two copies can drift (one handler could be changed to report a non-quota refusal as something else) while `state.rs` still claims one place decides it. The branch belongs with the flag — e.g. a `SamplingRefusal` consumer in `state.rs` handed the two per-operation error constructors — leaving each handler naming only its own enum, exactly as the doc says. No response bytes and no existing test change: both handlers already answer the same status, code and sentence for every row of `both_sampling_paths_answer_one_request_the_same_way`.

## Evidence

Copy 1 — `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample.rs:207`:
```
        .map_err(|refusal| {
            if refusal.is_quota() {
                SampleErrorModeled::ServiceQuotaExceededException(ServiceQuotaExceededException {
                    message: refusal.into_message(),
                })
                .into()
            } else {
                validation(refusal.into_message())
            }
        })?;
```
Copy 2 — `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample_with_response_stream.rs:207`:
```
        .map_err(|refusal| {
            if refusal.is_quota() {
                SampleWithResponseStreamErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: refusal.into_message(),
                    },
                )
                .into()
            } else {
                validation(refusal.into_message())
            }
        })?;
```
The single-place claim it breaks — `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:694`:
```
/// Each operation renders this as one of its OWN modelled errors — a quota as
/// its `ServiceQuotaExceededException`, anything else as its
/// `ValidationException` — so the decision lives here and only the error enum
/// is named at the handler.
```
Existing coverage that must stay green and unchanged: `tests/all/sampling_path_is_shared.rs:334` `both_sampling_paths_answer_one_request_the_same_way` — rows include `"a trajectory already holding the turn bound"` expecting `status: 402, code: "ServiceQuotaExceededException"` (`tests/all/sampling_path_is_shared.rs:243`) and `"a payload larger than one captured turn may retain"` expecting `status: 400, code: "ValidationException"` (`tests/all/sampling_path_is_shared.rs:228`), each asserted on BOTH `/sample` and `/sample-with-response-stream`; plus `tests/all/trajectory_lifecycle.rs:1136` `the_turn_bound_is_reported_as_a_service_quota`.

## Steps to Reproduce

1. Open `src/ops/sample.rs` at line 207 and `src/ops/sample_with_response_stream.rs` at line 207 side by side: the same two-branch mapping from `SamplingRefusal` to a modelled error appears in both, differing only in the error enum name.
2. Open `src/state.rs` at line 690 and read the `SamplingRefusal` doc comment, which states the decision lives in `state` and that "only the error enum is named at the handler" — the branch itself is nonetheless re-stated at each handler.
3. Edit only one of the two handlers (e.g. make the non-quota arm of `sample.rs` report a different modelled error) to see that nothing in `state.rs` constrains the pair: the divergence is caught only by the separate wire-level suite `tests/all/sampling_path_is_shared.rs`, not by the module that claims to own the decision.
