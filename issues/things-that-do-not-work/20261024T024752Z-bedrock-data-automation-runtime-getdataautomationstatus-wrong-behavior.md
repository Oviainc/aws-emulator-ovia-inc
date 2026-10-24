Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# GetDataAutomationStatus echoes the submitted output prefix and can never report a failed job

_Filed: 2026-10-24T02:47:52Z_
_Operation: GetDataAutomationStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two divergences in the same reported record.

(1) `outputConfiguration.s3Uri` is echoed back verbatim from the submission. For a real Bedrock Data Automation job, the `outputConfiguration.s3Uri` that `GetDataAutomationStatus` reports once the job is terminal is the job's OWN metadata location under the submitted prefix (`<submitted-prefix>/<invocationId>/job_metadata.json`) — that is the value AWS's own BDA sample flow feeds straight into S3 `GetObject` to discover the per-asset `standard_output`/`custom_output` result files. The emulator instead returns the bare prefix the caller supplied (`s3://out-bucket/project/`), so a client that follows the documented "poll status, then read the metadata JSON at the returned s3Uri" flow is pointed at a prefix, not at a job-specific metadata object. Nothing under that prefix is materialised either, so the whole output side of the workflow is inert.

(2) A job's lifecycle only ever runs `Created -> InProgress -> Success` (`crate::state::Invocation::advance_to`). `AutomationJobStatus::ClientError` / `ServiceError` are modelled and the output carries `errorType` and `errorMessage`, but no code path ever assigns `error_type` / `error_message` or sets a failure status, so a submission that can only fail in the real service — e.g. an `inputConfiguration.s3Uri` naming an object that does not exist, or an asset type the named project/blueprint cannot handle — is reported as `Success` with no error fields. A client that branches on `status == "ClientError"` or reads `errorMessage` can never exercise that branch against the emulator.

## Evidence

Spec: `smithy_query` shape `AutomationJobStatus` declares `ClientError | Created | InProgress | ServiceError | Success`, and `GetDataAutomationStatusResponse` declares `errorType`, `errorMessage` and `outputConfiguration` (OutputConfiguration, required member `s3Uri`). Handler: `src/ops/get_data_automation_status.rs` lines 200-212 build `output_configuration: Some(OutputConfiguration { s3_uri: invocation.output_s3_uri.clone() })` — the value stored verbatim at submit time (`src/ops/invoke_data_automation_async.rs` line 247, `input.output_configuration.s3_uri.clone()`) — and `error_type` / `error_message`, which `crate::state::Invocation::new` (state.rs ~line 339) sets to `None` and which no other code assigns. Lifecycle: `Invocation::advance_to` (state.rs lines 365-378) transitions only to `InProgress` then `Success`. Test coverage: `tests/all/data_automation_invocations.rs` lines 444-449 assert "the reported outputConfiguration must be the one submitted", i.e. a test currently PINS the echo; `grep errorType|ClientError` over `tests/` finds only the enum-value list (line 70), so no test asserts a failure path. Cross-service grounding for the metadata object: this repo already performs in-process cross-service reads/writes (e.g. `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs`).

## Steps to Reproduce

1. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/project/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices"}} and keep the returned invocationArn.
2. Poll `GetDataAutomationStatus` with that invocationArn until `status` is `Success`, then read `outputConfiguration.s3Uri`: the emulator returns `s3://out-bucket/project/` (the submitted prefix). Real BDA returns the job's metadata object location under that prefix (`.../<invocationId>/job_metadata.json`), which a client then fetches from S3.
3. Submit a second job whose `inputConfiguration.s3Uri` names an object that does not exist (e.g. "s3://in-bucket/assets/missing.pdf"), then poll `GetDataAutomationStatus`: the emulator reports `status: Success` with no `errorType`/`errorMessage`. Real BDA terminates such a job in a failure status (`ClientError`) with `errorType`/`errorMessage` populated.
4. Grep the crate for assignments to `error_type` / `error_message` or to a failure `AutomationJobStatus`: there are none, so no status other than Created/InProgress/Success is reachable.
