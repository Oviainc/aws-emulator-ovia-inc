Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.set-log-delivery-configuration-rejected-requests-leave-configuration-unchanged
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.set-log-delivery-configuration-rejected-requests-leave-configuration-unchanged|run#21|AssertionError|assert (oversized_code, oversized_message) == ("InvalidParameterException", oversized_expected), (
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# cognito-idp: SetLogDeliveryConfiguration's over-the-limit LogConfigurations rejection renders the offending value as '3 items' instead of the member list AWS echoes

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetLogDeliveryConfiguration

## Description
ONE root cause (Cause 1 of 1).

Cause 1 of 1: the constraint-violation message for SetLogDeliveryConfiguration.LogConfigurations substitutes an item COUNT for the offending value, instead of rendering the submitted list over the service's model types the way AWS does.

AWS: Smithy (cognito-identity-provider) SetLogDeliveryConfiguration.LogConfigurations is required, target LogConfigurationListType with @length min 0 / max 2; the operation models InvalidParameterException (errorKind client, httpError 400). A list of 3 otherwise well-formed entries is refused with HTTP 400, code InvalidParameterException and the standard AWS constraint message, whose FIXED text is "1 validation error detected: Value '" + <value> + "' at 'logConfigurations' failed to satisfy constraint: Member must have length less than or equal to " + <max>. The VARIABLE parts come from the request and the model: <value> is the submitted list rendered over the service's own model types — '[' + a comma-separated rendering of each entry as LogConfigurationType(logLevel=<LogLevel>, eventSource=<EventSource>, cloudWatchLogsConfiguration=CloudWatchLogsConfigurationType(logGroupArn=<LogGroupArn>), s3Configuration=null, firehoseConfiguration=null) + ']' (unset destinations render as null) — the member path is 'logConfigurations', and <max> is the model's @length max, 2.
Emulator: status 400 and code InvalidParameterException are right, but the message is "1 validation error detected: Value '3 items' at 'logConfigurations' failed to satisfy constraint: Member must have length less than or equal to 2" — the offending value is replaced by a synthesized count ('N items') instead of the rendered members. The sibling rejection in the same check (a LogConfiguration with no destination, message "Request validation Failed. Following event sources in request have no destination: [userNotification].") matched, and the positive single-entry Set was never reached because the assertion fired first.
Why: AWS's constraint messages are self-describing — a client (or an operator reading a log) sees exactly which values were rejected. With a count substituted, the diagnostic content is gone, and any client or test that matches AWS's message text for list-constraint violations on this service sees a different message than the real service produces. Because the same renderer is used for every list/length constraint violation, the divergence is not confined to this one member.
Fix: the cognito-idp input-validation layer for SetLogDeliveryConfiguration and the shared constraint-violation message renderer it calls — the renderer must format the rejected collection value over the service's model types rather than emitting an item count, keeping the member path and the model-derived limit it already gets right.
Traces: cognito-idp.set-log-delivery-configuration-rejected-requests-leave-configuration-unchanged.
Related: open issue 20261023T113357Z-cognito-identity-provider-setlogdeliveryconfiguration-stub.md was filed when this operation was an unimplemented scaffold stub; the operation now answers, and this is a message-fidelity defect in that answer, not the stub.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, verdict mismatch, check cognito-idp.set-log-delivery-configuration-rejected-requests-leave-configuration-unchanged:

  AssertionError: expected InvalidParameterException "1 validation error detected: Value '[LogConfigurationType(logLevel=ERROR, eventSource=userNotification, cloudWatchLogsConfiguration=CloudWatchLogsConfigurationType(logGroupArn=<log-group-arn>), s3Configuration=null, firehoseConfiguration=null), <same entry>, <same entry>]' at 'logConfigurations' failed to satisfy constraint: Member must have length less than or equal to 2" for a 3-entry LogConfigurations list, got 'InvalidParameterException': "1 validation error detected: Value '3 items' at 'logConfigurations' failed to satisfy constraint: Member must have length less than or equal to 2"

(The check builds the expected value from the ARN of the log group it just created, so the ARN in the rendered entries is request-derived, not part of the template; the error code, the member path 'logConfigurations' and the limit 2 agree.)

Model: smithy_query operation=SetLogDeliveryConfiguration (cognito-identity-provider) — LogConfigurations required, length {min 0, max 2}; errors include InvalidParameterException (client, httpError 400).

## Steps to Reproduce
1. Boot the emulator and replay cognito-idp.set-log-delivery-configuration-rejected-requests-leave-configuration-unchanged.
2. Manual repro: CreateUserPool; CreateLogGroup in CloudWatch Logs and take its bare log-group ARN (strip a trailing ':*').
3. Call SetLogDeliveryConfiguration(UserPoolId=<pool>, LogConfigurations=[e, e, e]) where e = {LogLevel: ERROR, EventSource: userNotification, CloudWatchLogsConfiguration: {LogGroupArn: <arn>}}.
4. Expect HTTP 400 InvalidParameterException whose message renders the three submitted entries as LogConfigurationType(...) values inside the quoted Value; observe the message carrying 'Value \'3 items\'' instead.
5. GetLogDeliveryConfiguration must still report LogConfigurations == [] (that part held).
