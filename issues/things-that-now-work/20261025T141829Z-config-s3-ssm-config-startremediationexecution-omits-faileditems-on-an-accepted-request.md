Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched
Issue-Key: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched|run#34|KeyError|assert control_start["FailedItems"] == [], (
Issue-Commit: c7b32a251ff44cc6c28a05b1a335573ec2c90dc2

# config StartRemediationExecution omits FailedItems on an accepted request, so a client cannot read the per-resource acceptance result (1 cause)

> _Note: this issue was identified against the code base at commit `c7b32a251ff44cc6c28a05b1a335573ec2c90dc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRemediationExecution

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: when every resource key in the request starts successfully, StartRemediationExecution answers 200 with the `FailedItems` member omitted entirely instead of reporting it as an empty list.

AWS: Smithy (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): StartRemediationExecution -> StartRemediationExecutionResponse{FailureMessage: String, FailedItems: ResourceKeys (@length min 1 max 100)}; errors InsufficientPermissionsException / InvalidParameterValueException / NoSuchRemediationConfigurationException, all client/HTTP 400. `FailedItems` is documented "For resources that have failed to start execution, the API returns a resource key object." Recorded behavior: an accepted on-demand remediation (the rule has a remediation configuration and the named resource keys are startable) answers HTTP 200 carrying `FailedItems` as an EMPTY list — the member is present and empty, which is what distinguishes "all keys accepted" from a partial acceptance that lists the keys that failed. The check asserts exactly that: `control_start["FailedItems"] == []`.

Emulator: `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_remediation_execution.rs:284-287` returns `failed_items: (!failed.is_empty()).then_some(failed)`, i.e. the member is set only when at least one key failed and is dropped from the response otherwise. The replay reaches this on the control leg — the preceding refusal leg passes (NoSuchRemediationConfigurationException with the message `There is no applicable RemediationConfiguration setup for AWS Config rule <ConfigRuleName>`, fixed text with the rule name taken from the request, and the bucket is correctly left unmodified) — and then fails with `KeyError: 'FailedItems'` on `assert control_start["FailedItems"] == []`. Status and error code are not in question here: the divergence is purely the success response's shape.

Why: a client that reads the result of an on-demand remediation to decide whether every resource key it submitted was accepted has to index `FailedItems`. Against AWS that is always safe and yields `[]` on full acceptance; against the emulator it raises KeyError / returns absent, so the client either crashes or cannot distinguish "all accepted" from "the field was not reported" and must guess. Any batch remediation driver that reconciles submitted keys against failed keys is broken by the omission.

Fix: the response construction in StartRemediationExecution's handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_remediation_execution.rs, the `Ok(StartRemediationExecutionOutput { .. })` at the end of `handle`), which must report `FailedItems` as an empty list rather than omitting the member when nothing failed; `FailureMessage` stays absent when there is no failure.

Traces: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; bootstrap_signal false; 177/749 checks passed).

[error] config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched
  Error: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_config__s3__ssm_config_remediation_execution_without_configuration_rejected_and_resource_untouched.py", line 827, in run
    assert control_start["FailedItems"] == [], (
  KeyError: 'FailedItems'

Expected (AWS): HTTP 200 with `FailedItems: []` (member present, empty) for a request whose every ResourceKey started.
Actual (emulator): HTTP 200 with no `FailedItems` member at all — botocore raises KeyError on the lookup.

What the check establishes before it fails (from the replayed source), all of which the emulator gets right:
  - puts a remediation configuration for the rule, then deletes it and polls DescribeRemediationConfigurations until the rule reports none;
  - StartRemediationExecution is then refused with code `NoSuchRemediationConfigurationException` and message exactly `There is no applicable RemediationConfiguration setup for AWS Config rule {rule}` (fixed text; `{rule}` is the ConfigRuleName from the request) — this assertion PASSES;
  - GetBucketVersioning confirms the bucket was not modified by the refused call — PASSES;
  - re-arms the identical remediation configuration, polls until it is reported again, and re-issues the identical StartRemediationExecution; the call is accepted, and the assertion on the accepted response's `FailedItems` is where it dies.

Code grounding in the checkout:
  - services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_remediation_execution.rs:284-287 — `failed_items: (!failed.is_empty()).then_some(failed)`.

Model grounding: StartRemediationExecutionResponse members `FailureMessage` (String) and `FailedItems` (ResourceKeys, @length 1..100), doc "For resources that have failed to start execution, the API returns a resource key object."

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Re-replay config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched.
3. Manually, against the emulator:
   a. Create an S3 bucket <bucket> and a Config rule <rule> that evaluates AWS::S3::Bucket.
   b. aws configservice put-remediation-configurations with a configuration for <rule> and ResourceType AWS::S3::Bucket (e.g. an SSM document target that enables bucket versioning).
   c. aws configservice start-remediation-execution --config-rule-name <rule> --resource-keys resourceType=AWS::S3::Bucket,resourceId=<bucket>
4. Observe the response JSON: it is `{}` (no FailedItems key). AWS answers `{"FailedItems": []}` for the same accepted request.
