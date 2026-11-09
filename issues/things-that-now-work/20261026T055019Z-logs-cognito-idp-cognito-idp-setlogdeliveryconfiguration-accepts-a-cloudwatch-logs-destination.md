Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__logs.obs-cognito-idp-1-log-delivery-config
Issue-Key: cognito-idp__logs.obs-cognito-idp-1-log-delivery-config|_error#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# cognito-idp SetLogDeliveryConfiguration accepts a CloudWatch Logs destination that does not resolve instead of refusing with InvalidParameterException

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetLogDeliveryConfiguration

## Description
Cause 1 of 1: SetLogDeliveryConfiguration performs no cross-service resolution of the CloudWatchLogsConfiguration.LogGroupArn it is handed, so a destination log group that does not exist in CloudWatch Logs is stored and echoed back with HTTP 200 instead of being refused. Failing check: logs__cognito-idp.obs-cognito-idp-1-log-delivery-config.

AWS: Cognito resolves the log-group ARN against CloudWatch Logs at set time and refuses an ARN that does not resolve. Smithy (com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice): SetLogDeliveryConfiguration input is {UserPoolId (required, pattern ^[\w-]+_[0-9a-zA-Z]+$), LogConfigurations (required, list, @length 0..2)} and the operation models InvalidParameterException (client, httpError 400) among its errors. Recorded behavior (measured against real AWS 2026-10-01, as stated in the check): a well-formed log-group ARN in the caller's own account and region whose log group was NEVER created is refused with HTTP 400, error code InvalidParameterException and message "Invalid destination in the input."; the same refusal applies to an ARN that was accepted earlier and whose log group has since been removed by DeleteLogGroup, and the refused call must leave the previously stored configuration untouched. The message is entirely fixed text — "Invalid destination in the input." — with no variable part interpolated from the request; the variable parts of the request are the UserPoolId and the LogGroupArn, and neither appears in it.

Emulator: the call SUCCEEDS. For a never-created log group the emulator answered HTTP 200 with LogDeliveryConfiguration = {UserPoolId: <pool id>, LogConfigurations: [{LogLevel: INFO, EventSource: userAuthEvents, CloudWatchLogsConfiguration: {LogGroupArn: <the non-existent group's ARN>}}]} (content-type application/x-amz-json-1.1), and the bogus destination is then readable back through GetLogDeliveryConfiguration. No InvalidParameterException is ever raised, so the later leg of the same scenario — offering the ARN of a log group that CloudWatch Logs has just deleted, which must also be refused while the stored configuration keeps naming the other group — cannot be reached either. Note the still-open issue 20261023T113357Z-cognito-identity-provider-setlogdeliveryconfiguration-stub.md describes this operation as an unimplemented scaffold stub; it now answers 200, so that issue does not cover this missing destination validation.

Why: a client that configures user-pool log delivery gets a success for a configuration real Cognito rejects. The pool is then believed to be delivering userAuthEvents to a log group that does not exist, so no records are ever produced and the typo/ordering bug AWS surfaces immediately (create the log group BEFORE setting delivery) is silently swallowed. Code that asserts on the InvalidParameterException path — including teardown ordering, where deleting the log group must make a re-point fail — takes the wrong branch, and negative tests pass against the emulator and fail against AWS.

Fix: the SetLogDeliveryConfiguration handler's validation in the cognito-idp service (service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice) must resolve each LogConfigurations[].CloudWatchLogsConfiguration.LogGroupArn against the CloudWatch Logs state (existence of the named log group in the ARN's account/region) before persisting, and raise InvalidParameterException (HTTP 400, message "Invalid destination in the input.") when it does not resolve, leaving any previously stored LogDeliveryConfiguration unchanged.

Traces: logs__cognito-idp.obs-cognito-idp-1-log-delivery-config (fails at the first negative leg: "expected a rejection, but the call SUCCEEDED and returned {'LogDeliveryConfiguration': ...}").

## Evidence
Expected (real AWS, per the check's recorded behavior and the Smithy model): SetLogDeliveryConfiguration with LogConfigurations=[{EventSource: userAuthEvents, LogLevel: INFO, CloudWatchLogsConfiguration: {LogGroupArn: <ARN of a log group that was never created>}}] -> HTTP 400, Error.Code = InvalidParameterException, Error.Message = "Invalid destination in the input." (fixed text, no request values interpolated); GetLogDeliveryConfiguration still shows no log configurations. The same refusal after DeleteLogGroup removes a once-valid destination, with the stored configuration still naming the other group.

Actual (emulator): HTTP 200 with body {"LogDeliveryConfiguration": {"UserPoolId": "<pool id>", "LogConfigurations": [{"LogLevel": "INFO", "EventSource": "userAuthEvents", "CloudWatchLogsConfiguration": {"LogGroupArn": "arn:aws:logs:<region>:<account>:log-group:/<never-created-group>"}}]}} — the check aborts with AssertionError: expected a rejection, but the call SUCCEEDED.

## Steps to Reproduce
1. Create a PLUS-tier Cognito user pool (CreateUserPool with UserPoolTier=PLUS) and confirm DescribeUserPool reports UserPoolTier=PLUS.
2. Build a well-formed CloudWatch Logs log-group ARN in the same account and region for a log group that was never created.
3. Call cognito-idp SetLogDeliveryConfiguration with UserPoolId=<pool> and LogConfigurations=[{EventSource: userAuthEvents, LogLevel: INFO, CloudWatchLogsConfiguration: {LogGroupArn: <that ARN>}}].
   Expected: HTTP 400 InvalidParameterException with message "Invalid destination in the input." and GetLogDeliveryConfiguration showing no configuration. Observed: HTTP 200 echoing the configuration.
4. (Second, unreachable leg) Create log group A, set delivery to A (accepted), re-point to log group B, then DeleteLogGroup A and call SetLogDeliveryConfiguration with A's ARN again: AWS refuses with the same InvalidParameterException and the stored configuration still names B.

Replay: run_behavioral_suite and re-replay check logs__cognito-idp.obs-cognito-idp-1-log-delivery-config.
