Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: f7ae49092a7217fe38189780203a00baa32a4bc7

# Minted invocation ARN carries a 33-char hex id instead of the UUID resource id AWS mints

_Filed: 2026-10-24T20:37:15Z_
_Operation: InvokeDataAutomationAsync | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f7ae49092a7217fe38189780203a00baa32a4bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The resource portion of the `invocationArn` this operation returns is built as `{nanos-since-epoch:016x}-{mint-count:016x}` (e.g. `data-automation-invocation/17c0f3a9b8d2e4f1-0000000000000001`). The real Bedrock Data Automation Runtime mints a UUID for the job and the returned ARN's resource segment is that UUID in canonical 8-4-4-4-12 lowercase-hex form (e.g. `data-automation-invocation/9f4e8c7a-1c2b-4d6e-9a01-3b5c7d9e1f24`); the same id is what appears in the job's output prefix under the submitted `outputConfiguration.s3Uri`. Expected: the minted resource id is a canonical UUID, so a client (or a fixture) that parses the id out of the ARN as a UUID, or derives the job's output prefix from it, sees the same shape it sees against AWS. Actual: a 33-character two-field hex string that satisfies the modelled `[a-zA-Z0-9-_]+` pattern but matches no AWS-shaped id, so any id-shape assertion or UUID parse of the returned ARN fails. No state or lifecycle behaviour is wrong — this is purely the format of the identifier the response hands back, which is why it is LOW.

## Evidence

Spec: `InvocationArn` (OPERATIONS.md line 55, smithy_query `InvokeDataAutomationAsyncResponse$invocationArn`) is `@required`, `@length(1..128)`, pattern `^arn:aws(...)+:bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$` — the pattern permits the emulator's id but does not pin it, so the id FORMAT is a fidelity question answered by AWS's own wire, where BDA job ids are UUIDs (the published BDA flow reads the job output/metadata under `<output-prefix>/<invocation-uuid>/`). Handler/state: `src/state.rs` `ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::mint_invocation_id` (lines ~456-463) returns `format!("{since_epoch:016x}-{:016x}", self.minted_invocations)`, and `src/ops/invoke_data_automation_async.rs` lines 240-241 feed that straight into `crate::state::invocation_arn`. Repo precedent for treating AWS id shape as part of fidelity: `services/service-com-amazonaws-cloudwatchevents-awsevents/tests/all/rule_lifecycle.rs` line ~1006 asserts "Each id is a UUID: 8-4-4-4-12 lowercase hex, as EventBridge answers". Test coverage: `tests/all/data_automation_invocations.rs` asserts only that the ARN is the stored record's own and satisfies the modelled pattern; no test asserts the id shape, so nothing pins the current format as intentional.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/project/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices"}}
2. Read `invocationArn` from the 200 response and split off the segment after `data-automation-invocation/`
3. Observe a 33-character `<16 hex>-<16 hex>` string; attempting to parse it as a UUID fails. Real BDA returns a canonical 8-4-4-4-12 lowercase-hex UUID there
4. Submit a second request and observe that the two ids differ only in the trailing counter field, unlike the independent UUIDs AWS returns
