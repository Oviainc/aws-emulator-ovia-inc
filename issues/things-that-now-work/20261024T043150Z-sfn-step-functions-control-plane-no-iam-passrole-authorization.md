Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.create-state-machine-insufficient-role-permissions-denied stepfunctions.sfn-update-state-machine-with-an-unpassable-role-is-denied sfn.update-state-machine-cross-account-role-denied kms__stepfunctions.xsvc-sfn-3-execution-role-without-kms-generate-data-key-start-execution-denied stepfunctions.sfn-payload-above-the-256-kilobyte-limit-is-rejected-and-a-just-under-payload-succeeds stepfunctions.stop-execution-on-express-unsupported stepfunctions.list-executions-redrive-filter-unsupported-for-express stepfunctions.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged stepfunctions.choice-no-match-without-default-rejected lambda__stepfunctions.definition-with-unreachable-transition-target-rejected stepfunctions.express-sync-failure-returns-a-result-not-an-error-with-billing-details stepfunctions.execution-metrics-reach-cloudwatch-under-the-state-machine-dimension stepfunctions.log-level-selects-which-history-events-reach-cloudwatch-logs cloudwatch__stepfunctions.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none stepfunctions.describe-state-machine-kms-invalid-state
Issue-Key: stepfunctions.create-state-machine-insufficient-role-permissions-denied|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: stepfunctions.sfn-update-state-machine-with-an-unpassable-role-is-denied|_error_when_settled#12|AssertionError|raise AssertionError(
Issue-Key: sfn.update-state-machine-cross-account-role-denied|_error_when_settled#31|AssertionError|raise AssertionError(
Issue-Key: kms__stepfunctions.xsvc-sfn-3-execution-role-without-kms-generate-data-key-start-execution-denied|run#13|AssertionError|assert codes == ["KmsAccessDeniedException"] * 3, (
Issue-Key: stepfunctions.sfn-payload-above-the-256-kilobyte-limit-is-rejected-and-a-just-under-payload-succeeds|run#12|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.stop-execution-on-express-unsupported|run#14|AssertionError|assert code == "InvalidArn", f"expected InvalidArn stopping an EXPRESS execution, got {code}: {message}"
Issue-Key: stepfunctions.list-executions-redrive-filter-unsupported-for-express|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: sfn|UpdateStateMachine|MissingRequiredParameter
Issue-Key: stepfunctions.choice-no-match-without-default-rejected|run#9|AssertionError|assert bad.get("error") == _RUNTIME_ERROR, (
Issue-Key: lambda__stepfunctions.definition-with-unreachable-transition-target-rejected|run#11|AssertionError|assert message == (
Issue-Key: stepfunctions.express-sync-failure-returns-a-result-not-an-error-with-billing-details|run#20|AssertionError|assert code == "InvalidArn", (
Issue-Key: stepfunctions.execution-metrics-reach-cloudwatch-under-the-state-machine-dimension|_await_metric_sum#13|Timeout|time.sleep(_METRIC_INTERVAL_S)
Issue-Key: stepfunctions.log-level-selects-which-history-events-reach-cloudwatch-logs|run#4|Timeout|time.sleep(_DELIVERY_SETTLE_S)
Issue-Key: cloudwatch__stepfunctions.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none|_poll_until_stable#16|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.describe-state-machine-kms-invalid-state|run#11|Timeout|time.sleep(3)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# Step Functions control plane: no IAM/PassRole authorization, wrong error codes for Express-only restrictions, oversized-input message echoes the payload, logging-config updates refused, no CloudWatch metrics or log delivery (10 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStateMachine

## Description
This issue bundles TEN distinct root causes in the Step Functions control plane, its validation and its observability side effects. All ten must be closed before submitting. Re-replay every check id in trace_ids.

Cause 1 of 10: no authorization is evaluated on the control plane, so role-permission and iam:PassRole restrictions have no effect.
AWS: CreateStateMachine/UpdateStateMachine require the caller to hold `iam:PassRole` for the `roleArn` (including cross-account role ARNs) and refuse otherwise with `AccessDeniedException` (HTTP 400); StartExecution against a customer-managed-key state machine whose execution role lacks `kms:GenerateDataKey` is refused with `KmsAccessDeniedException` (Smithy: StartExecution errors include KmsAccessDeniedException; CreateStateMachine/UpdateStateMachine errors include KmsAccessDeniedException and ValidationException).
Emulator: all of these calls SUCCEED. UpdateStateMachine even replaces the state machine's role with one the caller may not pass.
Why: a client's least-privilege tests pass against the emulator and fail in production; a denied privilege escalation is silently performed.
Fix: authorization evaluation in the sfn control-plane handlers (CreateStateMachine, UpdateStateMachine, StartExecution) against the request's signing identity. Related open issue: 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md, which carries the replay key for a cloudtrail check only.
Traces: sfn.create-state-machine-insufficient-role-permissions-denied, sfn.sfn-update-state-machine-with-an-unpassable-role-is-denied, sfn.update-state-machine-cross-account-role-denied, kms__sfn.xsvc-sfn-3-execution-role-without-kms-generate-data-key-start-execution-denied.

Cause 2 of 10: the oversized-`input` validation message echoes the whole payload and states the wrong constraint.
AWS: Smithy: `StartExecution.input` is `SensitiveData` with `@length(min: 0, max: 262144)` and `@sensitive`. The rule that builds a constraint-violation message is known: AWS names the member and its constraint WITHOUT echoing the sensitive value — `1 validation error detected: Value at 'input' failed to satisfy constraint: Member must have size less than or equal to 262144 bytes in UTF-8 encoding`.
Emulator: returns `1 validation error detected: Value '<the entire 263168-byte input>' at 'input' failed to satisfy constraint: Member must have length less than or equal to 262144` — it interpolates the `@sensitive` member's value and reports a character-length constraint instead of a UTF-8 byte-size constraint.
Why: a sensitive payload is reflected back in an error (a disclosure AWS specifically avoids), the error body is ~263 KB instead of ~150 bytes, and the constraint wording a client matches on is wrong.
Fix: the StartExecution input validation/serialization path in the sfn crate — suppress `@sensitive` member values in constraint messages and measure the limit in UTF-8 bytes.
Traces: sfn.sfn-payload-above-the-256-kilobyte-limit-is-rejected-and-a-just-under-payload-succeeds.

Cause 3 of 10: StopExecution on an EXPRESS execution returns the wrong error code.
AWS: Smithy: StopExecution errors include `InvalidArn` (HTTP 400). Stopping an execution of an EXPRESS state machine is refused with `InvalidArn`.
Emulator: returns `ValidationException` (HTTP 400) with the message `StopExecution does not support execution <arn>, which belongs to an EXPRESS state machine`.
Why: a client branching on `InvalidArn` (or on botocore's `exceptions.InvalidArn`) does not catch the refusal.
Fix: the StopExecution handler's state-machine-type check in the sfn crate.
Traces: sfn.stop-execution-on-express-unsupported.

Cause 4 of 10: ListExecutions accepts `redriveFilter` against an EXPRESS state machine.
AWS: Smithy: ListExecutions errors include `StateMachineTypeNotSupported` and `ValidationException` (both HTTP 400); redrive is a Standard-only feature and the request is refused.
Emulator: answers HTTP 200 with `{'executions': []}`.
Why: a client that relies on the refusal to detect an unsupported configuration instead sees a successful empty page and concludes there are no executions.
Fix: the ListExecutions handler's parameter validation in the sfn crate (reject `redriveFilter` for EXPRESS state machines).
Traces: sfn.list-executions-redrive-filter-unsupported-for-express.

Cause 5 of 10: UpdateStateMachine refuses a loggingConfiguration-only update and never validates the log-group ARN.
AWS: Smithy: on `UpdateStateMachineInput` only `stateMachineArn` is `@required`; `definition`, `roleArn`, `loggingConfiguration`, `tracingConfiguration` and `encryptionConfiguration` are all optional, and `InvalidLoggingConfiguration` (HTTP 400) is a modeled error. Updating only `loggingConfiguration` is legal, and a `logGroupArn` that is not the qualified `...:log-group:<name>:*` form is refused with `InvalidLoggingConfiguration`.
Emulator: returns `MissingRequiredParameter` with the message `either definition or roleArn must be given to update a state machine`, so the logging configuration is never evaluated at all and the invalid ARN is never diagnosed.
Why: a client cannot change logging or tracing configuration without also resubmitting the definition or role, and a malformed log-group ARN is accepted on paths that do reach the state, silently disabling delivery.
Fix: the UpdateStateMachine handler's required-parameter check and its loggingConfiguration validation in the sfn crate.
Traces: sfn.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged.

Cause 6 of 10: a Choice state with no matching rule and no `Default` reports the wrong error name.
AWS: recorded behavior only — the recording shows the execution terminating with `error='States.Runtime'`.
Emulator: terminates with `error='States.NoChoiceMatched'`, cause="no choice rule matched the state's input and the state declares no 'Default'".
Why: a `Catch` whose `ErrorEquals` lists the name AWS produces does not match, so the execution fails uncaught where AWS would route it to the catcher.
Fix: the Choice-state no-match error classification in the sfn execution interpreter.
Traces: sfn.choice-no-match-without-default-rejected.

Cause 7 of 10: definition validation reports only the first problem, omitting the terminal-state finding.
AWS: Smithy: CreateStateMachine errors include `InvalidDefinition` (HTTP 400). The recorded message enumerates every validation problem the definition has — both the dangling transition target AND `Workflow has no terminal state at null` — so the message is built from the definition by a known rule.
Emulator: returns only `Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: state 'InvokeProcessingLambda' transitions to 'AStateThatIsNotDefined', which 'States' does not declare'`; the terminal-state problem is not detected or reported.
Why: a client fixing the reported problem re-submits and is refused again for a problem AWS would have reported in the first response; and a definition with no terminal state is not diagnosed as such.
Fix: the definition validator in the sfn crate — collect all findings (unreachable/dangling transition targets AND absence of a terminal state) and report them together.
Traces: lambda__stepfunctions.definition-with-unreachable-transition-target-rejected.

Cause 8 of 10: GetExecutionHistory does not reject an EXPRESS execution ARN.
AWS: Smithy: GetExecutionHistory errors are ExecutionDoesNotExist, `InvalidArn`, InvalidToken and the Kms* family — there is no `NotImplemented`. Express execution history lives only in CloudWatch Logs, and the control plane refuses an Express execution ARN with `InvalidArn` (HTTP 400), not `ExecutionDoesNotExist`.
Emulator: answers `NotImplemented` (the operation is a scaffold stub; see open issue 20261023T114649Z-sfn-getexecutionhistory-stub.md, which does not state this Express-ARN requirement).
Why: a client distinguishing "wrong ARN kind" from "no such execution" gets neither, and the stub's error is not a modeled error for this operation at all.
Fix: the GetExecutionHistory handler in the sfn crate must validate the ARN's state-machine type before any lookup.
Traces: sfn.express-sync-failure-returns-a-result-not-an-error-with-billing-details.

Cause 9 of 10: executions produce no CloudWatch metrics and no execution-history log delivery.
AWS: a state machine publishes `AWS/States` metrics (ExecutionsStarted/Succeeded/Failed/Time under the `StateMachineArn` dimension, and for EXPRESS workflows the billed-metric series that sums to the execution's `billingDetails`), and when `loggingConfiguration.level` is ALL/ERROR/FATAL the selected history events are delivered to the configured log group. Recorded behavior: the checks poll CloudWatch/CloudWatch Logs for those series and events.
Emulator: nothing ever appears, so the checks make no progress and are killed with `Check timed out after 60.0s`.
Why: any monitoring, alarming or log-based assertion over Step Functions is unobservable, and `level` has no observable effect.
Fix: the observability side effects of the sfn execution engine — metric emission into the CloudWatch state and history-event delivery into the CloudWatch Logs state.
Traces: sfn.execution-metrics-reach-cloudwatch-under-the-state-machine-dimension, sfn.log-level-selects-which-history-events-reach-cloudwatch-logs, cloudwatch__sfn.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none.

Cause 10 of 10: DescribeStateMachine never answers for a state machine whose customer-managed key is in an invalid state.
AWS: Smithy: the Kms* errors (`KmsInvalidStateException`, HTTP 400) are modeled on the execution-reading operations; a describe against a state machine whose KMS key has been disabled/scheduled for deletion returns a modeled error rather than hanging.
Emulator: the call never returns a response the check can act on — it is killed with `Check timed out after 60.0s`.
Why: a client blocks indefinitely instead of receiving a 400 it can retry or surface.
Fix: the KMS-key state check in the sfn DescribeStateMachine handler (and the shared key-resolution path it uses) must terminate with the modeled error instead of blocking.
Traces: sfn.describe-state-machine-kms-invalid-state.

## Evidence
Cause 1: 'expected a ClientError, but the call succeeded: None' (CreateStateMachine with an insufficient role); 'UpdateStateMachine SUCCEEDED with a roleArn the caller is denied iam:PassRole on; real AWS refuses it, and the state machine's role has now been changed'; "expected 'AccessDeniedException' but the call SUCCEEDED -- the iam:PassRole restriction it depends on is not in effect"; 'StartExecution whose execution role lacks kms:GenerateDataKey must be refused with KmsAccessDeniedException on every attempt while the twin succeeds; got ["<call succeeded>", "<call succeeded>", "<call succeeded>"]'.
Cause 2: expected message "1 validation error detected: Value at 'input' failed to satisfy constraint: Member must have size less than or equal to 262144 bytes in UTF-8 encoding"; got "1 validation error detected: Value '{\"items\":[\"sim2real-item-00000\",...] [263168 bytes]' at 'input' failed to satisfy constraint: Member must have length less than or equal to 262144".
Cause 3: expected InvalidArn stopping an EXPRESS execution; got ValidationException.
Cause 4: expected a ClientError; got HTTP 200 {'executions': []}.
Cause 5: expected InvalidLoggingConfiguration; got 'MissingRequiredParameter': 'either definition or roleArn must be given to update a state machine'.
Cause 6: expected error 'States.Runtime'; got 'States.NoChoiceMatched'.
Cause 7: expected an InvalidDefinition message that also contains "Workflow has no terminal state at null"; got only "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: state 'InvokeProcessingLambda' transitions to 'AStateThatIsNotDefined', which 'States' does not declare'".
Cause 8: expected GetExecutionHistory to reject the Express execution ARN with InvalidArn; got 'NotImplemented'.
Cause 9: expected the AWS/States metric series / delivered log events; got nothing within the harness cap ('Check timed out after 60.0s').
Cause 10: expected a modeled KmsInvalidStateException-class response from DescribeStateMachine; got no response ('Check timed out after 60.0s').

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: call CreateStateMachine/UpdateStateMachine with a roleArn the signing identity cannot pass (including a cross-account ARN), and StartExecution on a CMK-encrypted state machine whose execution role lacks kms:GenerateDataKey — all succeed.
3. Cause 2: StartExecution with an 'input' larger than 262144 bytes and read the error message.
4. Cause 3: create an EXPRESS state machine, StartSyncExecution, then StopExecution on the resulting execution ARN.
5. Cause 4: ListExecutions(stateMachineArn=<EXPRESS arn>, redriveFilter='REDRIVEN').
6. Cause 5: UpdateStateMachine passing only stateMachineArn and loggingConfiguration (with an unqualified logGroupArn).
7. Cause 6: run a Choice state whose rules do not match and which declares no Default; read DescribeExecution.error.
8. Cause 7: CreateStateMachine with a state transitioning to an undeclared state and no terminal state; read the InvalidDefinition message.
9. Cause 8: GetExecutionHistory with an EXPRESS execution ARN.
10. Cause 9: run executions on a state machine with loggingConfiguration level ALL, then poll CloudWatch GetMetricData for AWS/States and CloudWatch Logs for the log group.
11. Cause 10: create a state machine with a customer-managed KMS key, disable the key, then call DescribeStateMachine.
