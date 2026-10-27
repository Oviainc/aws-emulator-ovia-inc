Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs
Issue-Key: bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs|run#14|AssertionError|assert exc is not None, (
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# PutModelInvocationLoggingConfiguration accepts a cloudWatchConfig that AWS refuses with 403 AccessDeniedException, silently enabling a CloudWatch Logs destination

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
=== Cause 1 of 1: PutModelInvocationLoggingConfiguration does not evaluate the cloudWatchConfig.roleArn PassRole step, so a CloudWatch Logs destination AWS refuses with 403 AccessDeniedException is accepted ===

AWS: Smithy (com.amazonaws.bedrock) models PutModelInvocationLoggingConfiguration as PUT /logging/modelinvocations with success code 200, a single required input member loggingConfig (LoggingConfig) and errors AccessDeniedException (httpError 403), InternalServerException (500), ThrottlingException (429), ValidationException (400). LoggingConfig.cloudWatchConfig targets CloudWatchConfig, whose members logGroupName (@length 1..512) and roleArn (RoleArn, @length max 2048, @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$) are BOTH required — i.e. the CloudWatch destination is the one destination that forces Bedrock to assume a caller-supplied role, where s3Config has a role-free path. AccessDeniedException carries a single `message` member, documented "The request is denied because of missing access permissions."

The model does not state the outcome of that PassRole step, so for the outcome: recorded behavior only. Measured against real AWS, every cloudWatchConfig is refused on the very FIRST call with HTTP 403 / error code AccessDeniedException whose message is the cross-account-pass-role denial (recorded example, an example only: "Cross-account pass role is not allowed"), against a freshly created role whose trust policy allows sts:AssumeRole for Principal Service bedrock.amazonaws.com and whose inline policy grants logs:CreateLogStream/logs:PutLogEvents/logs:DescribeLogStreams on exactly the target log group's ARN — so the refusal is not a malformed-role artifact. The recording is corroborated by two other recordings of the same message against different accounts, and no recording of a cloudWatchConfig ever being accepted exists. The refusal is also effect-free: a GetModelInvocationLoggingConfiguration taken afterwards returns exactly the loggingConfig that was in place before the refused call, with no partial cloudWatchConfig left behind. The same recording establishes a positive control first — GetFoundationModel(amazon.nova-lite-v1:0) returns modelDetails with modelLifecycle.status ACTIVE — so the 403 is attributable to the CloudWatch destination's role, not to the caller being denied Bedrock outright.

Emulator: the PUT is ACCEPTED. With loggingConfig = {cloudWatchConfig: {logGroupName: <a log group created moments earlier via logs:CreateLogGroup>, roleArn: <the freshly created role>}, textDataDeliveryEnabled: true, imageDataDeliveryEnabled: false, embeddingDataDeliveryEnabled: false} the call returns success on the first attempt and raises no ClientError, so no status, error code or message was produced to compare. The replay records: AssertionError: PutModelInvocationLoggingConfiguration accepted a CloudWatch Logs destination in this environment. No roleArn PassRole decision is taken anywhere on this path.

Why: a client relying on AWS's behavior is told its model-invocation logs are now being delivered to CloudWatch Logs when, against AWS, the configuration was refused and nothing is being delivered. An operator wiring up audit/compliance logging gets a false-positive "logging is on" from the emulator and builds dashboards, alarms and retention policy around a destination AWS never accepted; conversely a negative-authorization test that asserts the 403 (and asserts that the refused call left the account-singleton configuration untouched) passes in production and fails only here, so the emulator cannot be used to validate which roles Bedrock may assume. The divergence is in the status class and the error code — 200 versus 403/AccessDeniedException — not merely in message text.

Fix: the cloudWatchConfig branch of PutModelInvocationLoggingConfiguration's handler/validation — evaluate the PassRole decision for cloudWatchConfig.roleArn (the role Bedrock must assume to write into the named log group) before committing state, return the modelled AccessDeniedException with HTTP 403 when it is denied, and leave the account-singleton logging configuration exactly as it was so an independent GetModelInvocationLoggingConfiguration shows no partial cloudWatchConfig.

Related open issues, neither of which covers this: 20261023T232022Z-bedrock-catalogued-foundation-model-refused-as-invalid-responses.md names a cross-region logging BUCKET (s3Config) being accepted, not the cloudWatchConfig role; 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md names the missing CALLER-identity authorization gate on DeleteModelInvocationLoggingConfiguration and CreateCustomModel, not the service-role PassRole step inside this Put's cloudWatchConfig.

Service attribution note: the grounding check id is bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs (it spans bedrock + logs: CloudWatch Logs supplies the log group and its ARN, Bedrock answers the diverging call), so this is filed under the check id's leading token bedrock__logs. The diverging operation itself is Bedrock's PutModelInvocationLoggingConfiguration.

Traces: bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs. Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), service bedrock__logs 0/1 passed.

Check bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs [mismatch]:
  expected: HTTP 403, error code AccessDeniedException, message being the specific cross-account-pass-role refusal, for loggingConfig.cloudWatchConfig = {logGroupName: <freshly created log group>, roleArn: <freshly created, correctly trusted and log-group-scoped role>}; then GetModelInvocationLoggingConfiguration().loggingConfig unchanged from its pre-call value
  actual:   the PUT succeeded on the first attempt (no ClientError), so no status/code/message was returned. AssertionError at `assert exc is not None`: "PutModelInvocationLoggingConfiguration accepted a CloudWatch Logs destination in this environment; the withdrawal this check asserts no longer holds..."
  (positive control in the same check passed first: GetFoundationModel('amazon.nova-lite-v1:0') returned modelDetails with modelId == 'amazon.nova-lite-v1:0' and modelLifecycle.status == 'ACTIVE'; CreateLogGroup + DescribeLogGroups matched exactly one group, and the role was created with a bedrock.amazonaws.com trust policy plus logs:CreateLogStream/PutLogEvents/DescribeLogStreams on that group's ARN)

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): PutModelInvocationLoggingConfiguration http PUT /logging/modelinvocations code 200; errors AccessDeniedException httpError 403, ValidationException 400, ThrottlingException 429, InternalServerException 500; CloudWatchConfig.logGroupName and CloudWatchConfig.roleArn both required, roleArn pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock__logs.model-invocation-log-event-shape-at-cloudwatch-logs (suite simparity-behavioral-ga-prod-v1).

1. Read and stash the account singleton: GetModelInvocationLoggingConfiguration (tolerating ResourceNotFoundException as "no config"), and register a restore.
2. Positive control: GetFoundationModel(modelIdentifier='amazon.nova-lite-v1:0') must return modelDetails with modelLifecycle.status == 'ACTIVE'.
3. logs:CreateLogGroup a uniquely named group; DescribeLogGroups(logGroupNamePrefix=<that name>) to take its arn.
4. iam:CreateRole with trust policy {Effect: Allow, Principal: {Service: bedrock.amazonaws.com}, Action: sts:AssumeRole}; iam:PutRolePolicy granting logs:CreateLogStream, logs:PutLogEvents, logs:DescribeLogStreams on <log group arn> and <log group arn>:*.
5. Call PutModelInvocationLoggingConfiguration with loggingConfig = {cloudWatchConfig: {logGroupName: <name>, roleArn: <role arn>}, textDataDeliveryEnabled: true, imageDataDeliveryEnabled: false, embeddingDataDeliveryEnabled: false} (retrying only on ValidationException, to absorb IAM propagation).
   Expect: HTTP 403 with error code AccessDeniedException and the cross-account-pass-role message. Observe: the call succeeds.
6. Effect-absent leg: GetModelInvocationLoggingConfiguration().loggingConfig must equal the value stashed in step 1 — against the emulator it instead now carries the cloudWatchConfig that AWS refused.
