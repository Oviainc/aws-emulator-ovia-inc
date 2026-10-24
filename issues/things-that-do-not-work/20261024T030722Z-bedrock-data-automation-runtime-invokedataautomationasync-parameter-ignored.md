Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# inputConfiguration.assetProcessingConfiguration is accepted and silently discarded

_Filed: 2026-10-24T03:07:22Z_
_Operation: InvokeDataAutomationAsync | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InputConfiguration` has two members: `s3Uri` and `assetProcessingConfiguration` (which carries `video.segmentConfiguration`, a `TimestampSegment` with `startTimeMillis` / `endTimeMillis` delimiting the part of the asset to process). The handler copies only `input_configuration.s3_uri` onto the invocation record; the `Invocation` struct has no field for the asset-processing configuration at all, so a submission that delimits a video segment is indistinguishable in stored state from one that asks for the whole asset. Nothing the job later does (status, duration, or the output the service is supposed to produce) can reflect the requested segment, and the member cannot be checked for the cross-member consistency the real service enforces (an `endTimeMillis` that does not follow `startTimeMillis`).

Expected: the submitted segment configuration is retained as part of the accepted job so the job's processing scope — and any refusal of an inconsistent segment — derives from it. Actual: dropped at the handler boundary.

## Evidence

Spec: `InputConfiguration.assetProcessingConfiguration` -> `AssetProcessingConfiguration.video` -> `VideoAssetProcessingConfiguration.segmentConfiguration` -> `VideoSegmentConfiguration.timestampSegment` -> `TimestampSegment{startTimeMillis (required, min 0), endTimeMillis (required, min 300000)}` (smithy_query shapes). Handler: `src/ops/invoke_data_automation_async.rs` line 246 passes `input.input_configuration.s3_uri.clone()` into `Invocation::new` and never touches `input_configuration.asset_processing_configuration`; `src/state.rs`'s `Invocation` declares `input_s3_uri` and no asset-processing field. Tests: the string `assetProcessing` does not occur anywhere in `tests/`, so no test covers the member.

## Steps to Reproduce

1. Call `InvokeDataAutomationAsync` with `inputConfiguration = {"s3Uri": "s3://bucket/video.mp4", "assetProcessingConfiguration": {"video": {"segmentConfiguration": {"timestampSegment": {"startTimeMillis": 600000, "endTimeMillis": 300000}}}}}` plus the other required members
2. Observe the submission is accepted (HTTP 200 with an `invocationArn`) and that nothing in the stored record or any later read carries the requested segment
3. Inspect `Invocation` in `src/state.rs`: there is no field for the asset-processing configuration, so the member is discarded rather than persisted
