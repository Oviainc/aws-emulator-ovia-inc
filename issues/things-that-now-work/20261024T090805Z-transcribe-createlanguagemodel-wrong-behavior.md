Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Every custom language model settles in FAILED and never reaches COMPLETED, so the family can never be used

_Filed: 2026-10-24T09:08:05Z_
_Operation: CreateLanguageModel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLanguageModel` records the model (good) but the single clock-owed transition it schedules is hard-wired to failure: `LanguageModelRecord::advance_clock` always moves `IN_PROGRESS -> FAILED` and stamps a `FailureReason` saying "this emulator reads no object store and makes no outbound request, so it cannot train a custom language model from data held there". Consequently EVERY custom language model this emulator holds ends `FAILED`: `DescribeLanguageModel` can never report `ModelStatus: COMPLETED`, and no `StartTranscriptionJob` that names the model in `ModelSettings.LanguageModelName` can reference a usable model.

Expected (real service): a create whose `InputDataConfig.S3Uri` names training data that exists answers 200 with `ModelStatus: IN_PROGRESS`, and a later `DescribeLanguageModel` reports `ModelStatus: COMPLETED` with no `FailureReason` ("When the status displays as COMPLETED, your model is ready to use" — model doc for `LanguageModel$ModelStatus`); only an unreadable/absent S3 location or an invalid data layout ends `FAILED` with a `FailureReason`. Actual: `IN_PROGRESS` then unconditionally `FAILED`, for every request, including one whose training data is present in the companion object store.

This is the same hard-wired-failure shape already filed for `CreateMedicalVocabulary` (open issue 20261024T063214Z-transcribe-createmedicalvocabulary-wrong-behavior) but a distinct operation and a distinct record type, and it is not a platform limitation: other service crates in this workspace resolve S3 objects in-process through the shared emulator (e.g. `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs::read_plugin_file`), which would let this family succeed when the named location exists and fail only when it does not.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateLanguageModel")` — `InputDataConfig` is `@required`, and the output `ModelStatus` targets `ModelStatus` whose values are `COMPLETED | FAILED | IN_PROGRESS`; `LanguageModel$ModelStatus` is documented "When the status displays as COMPLETED, your model is ready to use". Handler/state: `src/ops/create_language_model.rs` lines 252-262 build the record via `LanguageModelRecord::in_progress(..)` with `failed_due: Some(now + MODEL_TRANSITION_DELAY)`; `src/state.rs` `LanguageModelRecord::advance_clock` (lines ~994-1008) unconditionally sets `MODEL_STATUS_FAILED` plus `language_model_failure_reason(&self.input_data_config.s3_uri)`, and the constant `MODEL_STATUS_COMPLETED` carries the comment "No model this emulator records reaches it". Tests: `tests/all/language_models.rs` header states "These tests assert exactly that, and never a COMPLETED model" and line ~304 asserts `FAILED`, so the current divergence is encoded by the suite — no test asserts the AWS outcome.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.CreateLanguageModel` and body {"ModelName":"clm-1","LanguageCode":"en-US","BaseModelName":"NarrowBand","InputDataConfig":{"S3Uri":"s3://DOC-EXAMPLE-BUCKET/training/","DataAccessRoleArn":"arn:aws:iam::000000000000:role/TranscribeRole"}} (with that prefix present in the companion S3 emulator) — 200 with ModelStatus IN_PROGRESS
2. Wait past the status transition (~500 ms) and call `Transcribe.DescribeLanguageModel` with {"ModelName":"clm-1"}
3. Emulator answers LanguageModel.ModelStatus FAILED with a FailureReason naming the S3Uri it did not read; AWS answers ModelStatus COMPLETED with no FailureReason
4. Call `Transcribe.StartTranscriptionJob` with {"ModelSettings":{"LanguageModelName":"clm-1"}} — against AWS this references a usable model; against the emulator the model it references can only ever be FAILED
