Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly
Issue-Key: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly|run#14|AssertionError|assert reason_lines[:4] == expected_head, (
Issue-Commit: a6bd8162e254cd53a71309df843cd006b7af8737

# cloudformation__lambda__ssm: a custom_resources.Provider onEvent handler that raises on Create is never run — the resource's CREATE_FAILED reason is a ResourceConflict/Pending refusal instead of the handler's own FAILED response text

> _Note: this issue was identified against the code base at commit `a6bd8162e254cd53a71309df843cd006b7af8737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One net-new behavioral root cause of this service group, grounded in one failing behavioral check of this replay.

Cause 1 of 1: when a CDK `custom_resources.Provider` onEvent handler raises on Create, the resource's CREATE_FAILED reason must be the custom resource's own FAILED response, but the emulator never invokes the handler at all and instead reports a provider-invocation ResourceConflict, so the failure reason is wrong and the handler's text never appears.
AWS: recorded behavior only. A custom resource whose provider answers FAILED gets a CREATE_FAILED event whose ResourceStatusReason is the fixed text `Received response status [FAILED] from custom resource. Message returned: ` followed by the handler's own message, then a blank line, then `Logs: ` and the provider function's log-group name, then a blank line — the handler message and the log-group name are the variable parts, taken from the provider's response and the deployed function. The stack then ends ROLLBACK_COMPLETE and the SSM parameter the handler would have written is never created.
Emulator: the reason is a single line, `Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)` — the provisioner invoked the provider function while the emulator still reports it in state `Pending`, the call was refused, and the handler's own raise never happened.
Why: the failure reason is the only channel carrying a custom resource's diagnostic back to the operator. A client (or CDK user) debugging a failed deploy is shown a transient Lambda state conflict instead of the handler's exception text and the log group to read, and cannot tell a provider rejection apart from an unavailable provider. The positive arm of the same app is blocked for the same reason, so a Provider-backed stack cannot be deployed at all.
Fix: the custom-resource provisioner in cloudformation must wait for (and retry past) a provider function still in Pending rather than terminating the resource on that refusal, and must then build the CREATE_FAILED ResourceStatusReason from the provider's FAILED response (`Received response status [FAILED] from custom resource. Message returned: …` plus the `Logs: <log group>` block); lambda's function-state transition out of Pending is the other half. The open issue 20261103T061702Z-cloudformation-lambda-ssm-3-net-new-root-causes.md works on neighbouring custom-resource causes in this same group but does not carry this check's replay key.
Traces: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly.

## Evidence
cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly [mismatch]:
  AssertionError: Widget's CREATE_FAILED reason opened with ['Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)'], expected ['Received response status [FAILED] from custom resource. Message returned: Error: <handler message>', '', 'Logs: /aws/lambda/<stack>-OnEvent…', '']
  Fixed text required: 'Received response status [FAILED] from custom resource. Message returned: ' + the handler's message, then an empty line, then 'Logs: ' + the provider function's log-group name, then an empty line. Variable parts: the handler message (from the provider's FAILED response) and the log-group name (from the deployed function). The marker string and generated names in the recording are per-run values.
  Observed fixed text: 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: …, HandlerErrorCode: ResourceConflict)'.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier, then `cdk deploy` an app whose stack has a `custom_resources.Provider` custom resource (logical id Widget) writing an SSM parameter, with its onEvent handler forced to raise on Create.
2. Expect a non-zero exit, the Widget resource CREATE_FAILED with a reason opening 'Received response status [FAILED] from custom resource. Message returned: <handler message>' followed by a blank line, 'Logs: /aws/lambda/<provider onEvent function>' and a blank line, the stack ROLLBACK_COMPLETE, and ssm GetParameter for the parameter answering not-found.
3. Observe the reason is instead the single line 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (… HandlerErrorCode: ResourceConflict)'.
4. Deploy the same app unforced — it must succeed and write the SSM parameter.
5. Re-replay: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly.
