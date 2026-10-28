Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-model-invocation-logging-config-clears-server-state bedrock.delete-model-invocation-logging-config-without-permission-denied bedrock.put-model-invocation-logging-config-enforced-on-invoke
Issue-Key: bedrock.delete-model-invocation-logging-config-clears-server-state|run#22|AssertionError|raise AssertionError(
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|AccessDeniedException
Issue-Commit: 92d05357548446f4b279c345bbe5fce7cc943ba5

# PutModelInvocationLoggingConfiguration denies EVERY cloudWatchConfig as "cross-account pass role" without reading the role ARN's account, so no CloudWatch Logs destination can ever be installed (1 cause)

> _Note: this issue was identified against the code base at commit `92d05357548446f4b279c345bbe5fce7cc943ba5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
REOPENS 20261025T105655Z-bedrock-logs-putmodelinvocationloggingconfiguration-accepts-a-cloudwatchconfig-that-aws-refuses-with.md: its fix left the call refused.

=== Cause 1 of 1: the PassRole decision for loggingConfig.cloudWatchConfig.roleArn is taken unconditionally on the mere PRESENCE of cloudWatchConfig, never on the role's account, so a same-account role is refused 403 AccessDeniedException "Cross-account pass role is not allowed." and a CloudWatch Logs model-invocation-logging destination can never be installed ===

AWS: Smithy (com.amazonaws.bedrock) models PutModelInvocationLoggingConfiguration as PUT /logging/modelinvocations, success code 200, one required input member loggingConfig (LoggingConfig), errors AccessDeniedException (@httpError 403), ValidationException (400), ThrottlingException (429), InternalServerException (500); the output shape has no members. LoggingConfig.cloudWatchConfig targets CloudWatchConfig, whose logGroupName (@length 1..512) and roleArn (RoleArn, @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$) are both required. GetModelInvocationLoggingConfiguration (GET, same URI) returns loggingConfig; DeleteModelInvocationLoggingConfiguration (DELETE, same URI) clears it.

The model does not state the outcome of the PassRole step, so for the outcome: recorded behavior only. What the recordings behind these three checks show is that a CloudWatch Logs destination whose roleArn names a role IN THE CALLER'S OWN ACCOUNT, trusted by Principal Service bedrock.amazonaws.com, is INSTALLABLE: the put answers 200 with an empty body and a subsequent GetModelInvocationLoggingConfiguration reports that cloudWatchConfig back. All three checks use that put only as a BASELINE and then assert something else (that Delete clears server state; that an unauthorized Delete is denied and leaves the baseline intact; that an account-level config is enforced on InvokeModel) — none of them asserts the put's own success as its subject, and every one of them is stopped dead at the baseline. The cross-account denial is a real refusal class, but its requirement is conditional on the role's account differing from the account the role is passed to: its fixed text is "Cross-account pass role is not allowed." carried in AccessDeniedException.message at HTTP 403, with no variable part, and it is owed ONLY when the roleArn's embedded 12-digit account id is not the caller's account.

Emulator: every put carrying a cloudWatchConfig is refused, whatever the role. services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs defines pass_role_denial() as `if config.cloud_watch_config.is_some() { return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED) } None` — it never parses the roleArn, never extracts its account id, and never compares it to the caller's account; the doc comment above it states outright that "the two sides differ for every value the member can carry". put_model_invocation_logging_configuration.rs calls it before any store and returns AccessDeniedException{message} with HTTP 403. Replay shows botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the PutModelInvocationLoggingConfiguration operation: Cross-account pass role is not allowed. — for role arn:aws:iam::000000000000:role/sim2real-bedlog-delivery-<suffix>, i.e. a role in the emulator's OWN account 000000000000, labelled cross-account. Because the refusal is total, GetModelInvocationLoggingConfiguration can never report a cloudWatchConfig, Delete has nothing to clear, and account-level logging can never be exercised on the data plane.

Why: a client relying on AWS's behavior cannot turn model-invocation logging on at all against this emulator. An operator wiring audit/compliance logging to CloudWatch Logs with the documented same-account bedrock.amazonaws.com delivery role gets a hard 403 where AWS returns 200, so the whole logging feature — Put, the Get that reads it back, the Delete that clears it, and the server-side enforcement of it on InvokeModel/Converse — is unreachable. The previous fix replaced an over-permissive accept-everything with an over-restrictive deny-everything: the correct gate is conditional on the role ARN's account, and collapsing it to "cloudWatchConfig present" makes a legitimate configuration permanently un-installable and makes any negative test of the genuine cross-account case indistinguishable from the positive case.

Fix: crate::state::pass_role_denial and the cloudWatchConfig branch of PutModelInvocationLoggingConfiguration's handler — parse the 12-digit account id out of cloudWatchConfig.roleArn and deny with the modelled AccessDeniedException (HTTP 403, message "Cross-account pass role is not allowed.") only when that account differs from the calling account; when the accounts match, store the configuration and answer 200 with the empty output so Get reports it back, Delete clears it, and the data plane can honour it. The denial must stay effect-free (nothing installed) in the case it still applies.

Traces: bedrock.delete-model-invocation-logging-config-clears-server-state, bedrock.delete-model-invocation-logging-config-without-permission-denied, bedrock.put-model-invocation-logging-config-enforced-on-invoke. Code: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs lines 2623-2658 (CROSS_ACCOUNT_PASS_ROLE_DENIED, pass_role_denial); services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs lines 139-150.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), service bedrock 50/84 passed. Three failing checks, one cause.

1) bedrock.put-model-invocation-logging-config-enforced-on-invoke [error] — the diverging call, unguarded:
   expected: HTTP 200, empty body, for loggingConfig = {cloudWatchConfig: {logGroupName: <log group created moments earlier>, roleArn: <same-account role trusted by bedrock.amazonaws.com and scoped to that log group>}, textDataDeliveryEnabled: true, ...}
   actual:   HTTP 403, error code AccessDeniedException, message "Cross-account pass role is not allowed."
             botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the PutModelInvocationLoggingConfiguration operation: Cross-account pass role is not allowed.
   Message anatomy: "Cross-account pass role is not allowed." is entirely FIXED text with no variable part — it is the wrong message here not because of its wording but because it is owed only when the roleArn's account differs from the caller's; the request's roleArn account equals the caller's account.

2) bedrock.delete-model-invocation-logging-config-without-permission-denied [mismatch] — blocked at its baseline by the same refusal, and the role it names proves the account comparison is not being made:
   AssertionError: could not establish a baseline CloudWatch logging config for role 'arn:aws:iam::000000000000:role/sim2real-bedlog-delivery-e179a92b2110' (same-account bedrock.amazonaws.com trust): AccessDeniedException: Cross-account pass role is not allowed.
   (role account 000000000000 == the emulator's caller account; the emulator still calls the pass cross-account)

3) bedrock.delete-model-invocation-logging-config-clears-server-state [mismatch] — same refusal at the same baseline step:
   AssertionError: ... rejects Bedrock's role assumption / bucket validation path independent of configuration correctness: AccessDeniedException: Cross-account pass role is not allowed.

Code facts: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2653 `pub fn pass_role_denial(config: &LoggingConfig) -> Option<String> { if config.cloud_watch_config.is_some() { return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string()); } None }` — no ARN parse, no account comparison. state.rs:2630 `pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";`. ops/put_model_invocation_logging_configuration.rs:141 returns AccessDeniedException from that denial before any state write.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): PutModelInvocationLoggingConfiguration PUT /logging/modelinvocations code 200, @idempotent, input {loggingConfig required}, output {} (no members); errors AccessDeniedException httpError 403, ValidationException 400, ThrottlingException 429, InternalServerException 500. GetModelInvocationLoggingConfiguration GET same URI, output {loggingConfig}. DeleteModelInvocationLoggingConfiguration DELETE same URI, output {}.

Relation to prior work: 20261025T105655Z-bedrock-logs-putmodelinvocationloggingconfiguration-accepts-a-cloudwatchconfig-that-aws-refuses-with.md (status fixed) asked for the PassRole decision to be evaluated for cloudWatchConfig.roleArn; the delivered fix denies on the presence of cloudWatchConfig rather than on the role's account, which is what leaves these three calls refused.

## Steps to Reproduce
Boot the emulator from this checkout and replay, in suite simparity-behavioral-ga-prod-v1: bedrock.put-model-invocation-logging-config-enforced-on-invoke, bedrock.delete-model-invocation-logging-config-without-permission-denied, bedrock.delete-model-invocation-logging-config-clears-server-state.

Minimal reproduction of the single cause:
1. logs:CreateLogGroup a uniquely named group; DescribeLogGroups(logGroupNamePrefix=<that name>) to take its arn.
2. sts:GetCallerIdentity to take the calling account id A.
3. iam:CreateRole in account A with trust policy {Effect: Allow, Principal: {Service: bedrock.amazonaws.com}, Action: sts:AssumeRole, Condition: {StringEquals: {aws:SourceAccount: A}}}; iam:PutRolePolicy granting logs:CreateLogStream, logs:PutLogEvents, logs:DescribeLogStreams on <log group arn> and <log group arn>:*. The resulting ARN is arn:aws:iam::<A>:role/<name> — SAME account as the caller.
4. Call PutModelInvocationLoggingConfiguration with loggingConfig = {cloudWatchConfig: {logGroupName: <name>, roleArn: <role arn from step 3>}, textDataDeliveryEnabled: true, imageDataDeliveryEnabled: false, embeddingDataDeliveryEnabled: false}.
   Expect: HTTP 200 with an empty body. Observe: HTTP 403, AccessDeniedException, "Cross-account pass role is not allowed."
5. GetModelInvocationLoggingConfiguration — expect loggingConfig to report back the cloudWatchConfig stored in step 4; observe it reports no cloudWatchConfig, because nothing was ever stored.
6. Negative control that must still refuse: repeat step 4 with a roleArn whose embedded account id is NOT A (e.g. arn:aws:iam::999999999999:role/<name>) — that one is owed HTTP 403 AccessDeniedException with exactly the message "Cross-account pass role is not allowed." Today steps 4 and 6 are indistinguishable.
