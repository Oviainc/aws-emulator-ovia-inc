Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.put-model-invocation-logging-config-enforced-on-invoke
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|AccessDeniedException
Issue-Commit: 92d05357548446f4b279c345bbe5fce7cc943ba5

# REOPENS bedrock PutModelInvocationLoggingConfiguration: a same-account cloudWatchConfig role is now refused 403 AccessDeniedException "Cross-account pass role is not allowed.", so account-level invocation logging can never be enabled (1 cause)

> _Note: this issue was identified against the code base at commit `92d05357548446f4b279c345bbe5fce7cc943ba5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
REOPENS 20261025T105655Z-bedrock-logs-putmodelinvocationloggingconfiguration-accepts-a-cloudwatchconfig-that-aws-refuses-with.md: its fix left the call refused.

=== Cause 1 of 1: PutModelInvocationLoggingConfiguration unconditionally denies every cloudWatchConfig.roleArn as a cross-account PassRole, so no caller can ever turn model-invocation logging on ===

AWS: Smithy (com.amazonaws.bedrock) models PutModelInvocationLoggingConfiguration as PUT /logging/modelinvocations with success code 200, one required input member `loggingConfig` (LoggingConfig), and errors AccessDeniedException (httpError 403), ValidationException (400), ThrottlingException (429), InternalServerException (500). LoggingConfig.cloudWatchConfig targets CloudWatchConfig, whose members logGroupName and roleArn are both required; roleArn is pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$. The model does not state the outcome of the PassRole step, so for the outcome: recorded behavior only. The grounding recording establishes the SUCCESS path: a cloudWatchConfig whose logGroupName names a log group created moments earlier in the SAME account, and whose roleArn names a role created moments earlier in the SAME account with a trust policy allowing sts:AssumeRole for Principal Service bedrock.amazonaws.com and an inline policy granting logs:CreateLogStream / logs:PutLogEvents / logs:DescribeLogStreams on exactly that log group's ARN, is ACCEPTED with HTTP 200 and an empty body, and a subsequent GetModelInvocationLoggingConfiguration returns the loggingConfig that was just stored (cloudWatchConfig.logGroupName equal to the submitted name). The recording's only tolerated transient is a ValidationException whose message contains "Failed to validate permissions" while IAM propagates; it is retried, never treated as terminal. AccessDeniedException is reserved for a role the caller may not pass — a role in another account, or one whose trust policy does not admit bedrock.amazonaws.com — not for the account's own freshly trusted logging role.

Emulator: the PUT is REFUSED on the first attempt with HTTP 403, error code AccessDeniedException and message "Cross-account pass role is not allowed." for a roleArn in the caller's own account whose trust policy names bedrock.amazonaws.com. The message is wholly fixed text with no variable part — it names neither the roleArn it rejected nor the account it believed the role belonged to — which is itself the tell that no PassRole/account comparison is actually being performed: the denial is unconditional for the cloudWatchConfig branch. Because the message is not the retried "Failed to validate permissions" ValidationException, the check's propagation retry does not absorb it and the error surfaces terminally.

This is the mirror of the earlier defect, not a new one. The fixed issue named above was grounded in a recording where EVERY cloudWatchConfig came back 403 with this same message, and its fix hard-wired that refusal onto the branch. The constant refusal is as wrong as the constant acceptance it replaced: AWS decides per role, and a same-account role that trusts bedrock.amazonaws.com and is scoped to the target log group is passable. The emulator now has no input at all for which cloudWatchConfig succeeds.

Why: a client relying on AWS's behavior cannot enable Bedrock model-invocation logging to CloudWatch Logs at all. Every operator path that configures audit/compliance logging — console-equivalent wiring, Terraform/CloudFormation convergence, a CI bootstrap that calls Put then Get — fails with a 403 that reads as an IAM misconfiguration in the caller's own account, sending users to debug a trust policy that is already correct. Everything downstream of the configuration is unreachable as a consequence: the account-singleton Get can never report a cloudWatchConfig, and the server-side delivery of invocation records into the configured log group (the leg this check exists to prove: logging is enforced from the account configuration, with no per-call opt-in on InvokeModel) is never exercised. Restore/teardown paths that re-Put a previously captured loggingConfig also fail, so a test that saves and restores the singleton cannot put the account back.

Fix: the cloudWatchConfig branch of PutModelInvocationLoggingConfiguration's handler/validation. The PassRole decision must be an actual decision over the submitted roleArn rather than a constant denial: parse the account id out of cloudWatchConfig.roleArn and compare it to the caller's account, and consult the referenced role's trust policy for a statement allowing sts:AssumeRole to Principal Service bedrock.amazonaws.com. Deny with the modelled AccessDeniedException (HTTP 403) only when that decision is actually negative — a role in a different account, or one whose trust policy does not admit the Bedrock service principal — and in that case leave the account-singleton configuration untouched. On the positive decision, return 200 with an empty body and commit the loggingConfig so an independent GetModelInvocationLoggingConfiguration returns exactly what was submitted. The transient-while-IAM-propagates answer, if one is modelled at all, belongs on ValidationException with the "Failed to validate permissions" wording, not on AccessDeniedException.

Traces: bedrock.put-model-invocation-logging-config-enforced-on-invoke. Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), service bedrock 3/18 passed.

Check bedrock.put-model-invocation-logging-config-enforced-on-invoke [error]:
  request: PutModelInvocationLoggingConfiguration with loggingConfig = {cloudWatchConfig: {logGroupName: <log group created seconds earlier via logs:CreateLogGroup in the caller's own account>, roleArn: <role created seconds earlier via iam:CreateRole in the caller's own account, trust policy Principal Service bedrock.amazonaws.com / Action sts:AssumeRole, inline policy logs:CreateLogStream+PutLogEvents+DescribeLogStreams on that group's ARN and <arn>:*>}, textDataDeliveryEnabled: true}
  expected: HTTP 200, empty body; then GetModelInvocationLoggingConfiguration().loggingConfig.cloudWatchConfig.logGroupName == the submitted log group name
  actual:   HTTP 403, error code AccessDeniedException. Replay output:
            botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the PutModelInvocationLoggingConfiguration operation: Cross-account pass role is not allowed.

  Message anatomy: "Cross-account pass role is not allowed." is 100% fixed text — there is NO variable part. The roleArn, its account id and the caller's account id, all of which are request/state values and all of which any genuine cross-account denial would have to be derived from, appear nowhere. The call is refused for a role that is not cross-account.

  The check's own retry loop proves the refusal is terminal and not propagation noise: it retries ONLY a ClientError whose Error.Message contains "Failed to validate permissions" (for up to 60 s); an AccessDeniedException is re-raised immediately.

  Unreached as a consequence (all after the failing Put): Get reflecting the stored log group; InvokeModel with no client-side logging option; polling logs:FilterLogEvents for an invocation record carrying both the per-run marker and the modelId; and the registered restore that re-Puts the account's original loggingConfig.

Regression linkage: the fixed issue 20261025T105655Z-bedrock-logs-putmodelinvocationloggingconfiguration-accepts-a-cloudwatchconfig-that-aws-refuses-with.md asked for this 403 to be produced for a cloudWatchConfig, and quoted "Cross-account pass role is not allowed" explicitly as "a recorded example, an example only". The delivered fix returns it for every cloudWatchConfig, so the branch now has no accepting input.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): PutModelInvocationLoggingConfiguration http PUT /logging/modelinvocations code 200, idempotent, required input member loggingConfig -> LoggingConfig, empty output shape; errors AccessDeniedException httpError 403, ValidationException 400, ThrottlingException 429, InternalServerException 500. GetModelInvocationLoggingConfiguration http GET /logging/modelinvocations code 200, readonly, output member loggingConfig -> LoggingConfig. CloudWatchConfig.logGroupName and CloudWatchConfig.roleArn both required; roleArn @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock.put-model-invocation-logging-config-enforced-on-invoke (suite simparity-behavioral-ga-prod-v1).

1. logs:CreateLogGroup a uniquely named group; logs:DescribeLogGroups(logGroupNamePrefix=<that name>) to take its arn.
2. iam:CreateRole with trust policy {Version: 2012-10-17, Statement: [{Effect: Allow, Principal: {Service: bedrock.amazonaws.com}, Action: sts:AssumeRole}]}; iam:PutRolePolicy granting logs:CreateLogStream, logs:PutLogEvents, logs:DescribeLogStreams on <log group arn> and <log group arn>:*. The role is in the CALLER'S OWN account.
3. GetModelInvocationLoggingConfiguration and stash loggingConfig so it can be restored.
4. Call PutModelInvocationLoggingConfiguration with loggingConfig = {cloudWatchConfig: {logGroupName: <name from step 1>, roleArn: <arn from step 2>}, textDataDeliveryEnabled: true}, retrying only while the error message contains "Failed to validate permissions".
   Expect: HTTP 200 with an empty body.
   Observe: HTTP 403 AccessDeniedException "Cross-account pass role is not allowed." on the first attempt, not retried, check aborts here.
5. (Unreached) GetModelInvocationLoggingConfiguration must return loggingConfig.cloudWatchConfig.logGroupName == the name from step 1.
6. (Unreached) InvokeModel with no per-call logging option, then poll logs:FilterLogEvents(logGroupName=<name>) until an event message contains both the per-run marker and the modelId — proving logging is enforced from the account configuration alone.
7. (Unreached) Restore: re-Put the loggingConfig stashed in step 3, or DeleteModelInvocationLoggingConfiguration if there was none.
