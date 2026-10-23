Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.start-live-tail-streams-matching-events logs.start-live-tail-streams-new-events logs.start-live-tail-session-idle-stays-open logs.start-live-tail-nonexistent-log-group-rejected logs.start-live-tail-mutually-exclusive-stream-filters-rejected logs.start-live-tail-invalid-log-group-rejected logs.live-tail-session-behavior logs.live-tail-session-closes-after-max-duration-ttl logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups logs.get-log-object-retrieves-event-payload logs.get-log-object-returns-raw-payload logs.get-log-object-download-url-ttl logs.get-log-object-download-url-rejected-after-expiry logs.get-log-object-unauthorized-rejected logs.get-integration-nonexistent-fails logs.delete-integration-unknown-name-rejected logs.untag-resource-nonexistent-arn-fails logs.disassociate-source-not-associated-fails logs.associate-kms-key-nonexistent-group-fails logs.associate-kms-key-nonexistent-log-group-fails logs.disassociate-kms-key-nonexistent-log-group-fails logs.bearer-token-authentication-lifecycle logs.bearer-token-authentication-nonexistent-log-group-rejected logs.put-bearer-token-authentication-invalid-config-rejected logs.put-bearer-token-authentication-invalid-operation-state logs.put-bearer-token-authentication-invalid-operation-state-rejected logs.create-log-group-invalid-name-rejected logs.create-log-group-name-exceeds-length-limit-rejected logs.create-log-group-name-exceeds-max-length-rejected logs.create-log-stream-invalid-name-rejected logs.invalid-parameter-validation-on-create logs.list-validation-error-semantics logs.describe-log-streams-prefix-and-orderby-conflict-fails logs.op-contract-describe-log-streams logs.agentcore-log-group-prefix-and-pattern-lookup-scope logs.op-contract-describe-log-groups-mutually-exclusive-filters-rejected logs.limit-exceeded-triggers logs.op-contract-putResourcePolicy logs.put-subscription-filter-exceeds-quota-rejected logs.put-bearer-token-authentication-enforced-server-side
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|StartLiveTail|NotImplemented
Issue-Key: logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups|run#7|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.get-log-object-retrieves-event-payload|run#4|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.get-log-object-returns-raw-payload|run#4|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.get-log-object-download-url-ttl|run#8|AssertionError|assert code_malformed == "InvalidParameterException", (
Issue-Key: logs.get-log-object-download-url-rejected-after-expiry|run#6|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.get-log-object-unauthorized-rejected|run#10|AssertionError|assert positive_code == "InvalidParameterException", (
Issue-Key: logs.get-integration-nonexistent-fails|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: logs.delete-integration-unknown-name-rejected|run#4|AssertionError|assert get_code == "ResourceNotFoundException", (
Issue-Key: logs.untag-resource-nonexistent-arn-fails|run#7|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: logs.disassociate-source-not-associated-fails|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: logs.associate-kms-key-nonexistent-group-fails|run#26|AssertionError|assert message == expected_message, (
Issue-Key: logs.associate-kms-key-nonexistent-log-group-fails|run#26|AssertionError|assert message == expected_message, (
Issue-Key: logs.disassociate-kms-key-nonexistent-log-group-fails|run#9|AssertionError|assert message == "The specified log group does not exist.", (
Issue-Key: logs.bearer-token-authentication-lifecycle|run#15|AssertionError|assert message == expected_message, (
Issue-Key: logs.bearer-token-authentication-nonexistent-log-group-rejected|run#6|AssertionError|assert message == expected_message, (
Issue-Key: logs.put-bearer-token-authentication-invalid-config-rejected|run#11|AssertionError|assert err == ("ResourceNotFoundException", "The specified log group does not exist."), (
Issue-Key: logs.put-bearer-token-authentication-invalid-operation-state|run#12|AssertionError|assert err == ("ResourceNotFoundException", "The specified log group does not exist."), f"got {err!r}"
Issue-Key: logs.put-bearer-token-authentication-invalid-operation-state-rejected|run#6|AssertionError|assert err_enable == ("ResourceNotFoundException", "The specified log group does not exist."), (
Issue-Key: logs.create-log-group-invalid-name-rejected|run#5|AssertionError|assert code == (
Issue-Key: logs.create-log-group-name-exceeds-length-limit-rejected|run#6|AssertionError|assert code == (
Issue-Key: logs.create-log-group-name-exceeds-max-length-rejected|run#6|AssertionError|assert code == (
Issue-Key: logs.create-log-stream-invalid-name-rejected|run#8|AssertionError|assert code == (
Issue-Key: logs.invalid-parameter-validation-on-create|run#7|AssertionError|assert msg_group == expected, f"expected message {expected!r}, got {msg_group!r}"
Issue-Key: logs.list-validation-error-semantics|run#9|AssertionError|assert msg_pattern == expected, f"expected message {expected!r}, got {msg_pattern!r}"
Issue-Key: logs.describe-log-streams-prefix-and-orderby-conflict-fails|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: logs.op-contract-describe-log-streams|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: logs.agentcore-log-group-prefix-and-pattern-lookup-scope|run#13|AssertionError|assert got == expected, f"prefix+pattern together returned {got!r}, expected {expected!r}"
Issue-Key: logs.op-contract-describe-log-groups-mutually-exclusive-filters-rejected|run#15|AssertionError|assert message == (
Issue-Key: logs.limit-exceeded-triggers|run#13|AssertionError|assert code_msg == "Resource limit exceeded.", (
Issue-Key: logs.op-contract-putResourcePolicy|run#14|AssertionError|assert message == (
Issue-Key: logs.put-subscription-filter-exceeds-quota-rejected|run#15|AssertionError|assert overflow_message == "Resource limit exceeded.", f"GOT={overflow_message!r}"
Issue-Key: logs.put-bearer-token-authentication-enforced-server-side|run#21|AssertionError|assert status == 403, f"expected 403 before auth is enabled, got {status}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# CloudWatch Logs: StartLiveTail and GetLogObject answer 501, three more operations answer NotImplemented on their not-found paths, and error messages diverge from AWS's fixed texts (6 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartLiveTail

## Description
Six net-new root causes in the CloudWatch Logs emulator: unimplemented data-plane operations and error-shape fidelity. ALL SIX must be closed before submitting.

Cause 1 of 6: StartLiveTail is not implemented.
AWS: StartLiveTail opens an event-stream session over the chosen log groups and streams sessionStart/sessionUpdate frames; an unknown log group is refused with ResourceNotFoundException and logStreamNames spanning two log groups with InvalidParameterException. Smithy (cloudwatchlogs): StartLiveTail is a streaming operation whose modeled errors are AccessDeniedException, InvalidParameterException, LimitExceededException, ResourceNotFoundException, ServiceUnavailableException - NotImplemented is not modeled.
Emulator: ClientError code 'NotImplemented', message 'operation StartLiveTail is not implemented in this emulator', even for an existing log group. NOTE: the open issue 20261022T220153Z-logs-startlivetail-answers-501-notimplemented-instead-of-resolving-the.md describes this operation; it does not carry these check ids, so this cause is recorded here with them - consolidate if it is the same work.
Why: the live-tail surface is unusable and its validation semantics (which group / which streams) cannot be exercised.
Fix: implement the StartLiveTail handler and its event-stream serializer in the logs service crate.
Traces: logs.start-live-tail-streams-matching-events, logs.start-live-tail-streams-new-events, logs.start-live-tail-session-idle-stays-open, logs.start-live-tail-nonexistent-log-group-rejected, logs.start-live-tail-mutually-exclusive-stream-filters-rejected, logs.start-live-tail-invalid-log-group-rejected, logs.live-tail-session-behavior, logs.live-tail-session-closes-after-max-duration-ttl, logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups.

Cause 2 of 6: GetLogObject is not implemented.
AWS: GetLogObject resolves a logObjectPointer previously issued by the service and streams the raw event payload; a client-constructed, expired or unrecognized pointer is refused with InvalidParameterException (400).
Emulator: ClientError code 'NotImplemented' for every pointer, so neither the success nor the validation path exists.
Why: callers cannot fetch a log object at all, and a malformed pointer is indistinguishable from an unimplemented service.
Fix: implement the GetLogObject handler (pointer issuance + resolution) in the logs service crate.
Traces: logs.get-log-object-retrieves-event-payload, logs.get-log-object-returns-raw-payload, logs.get-log-object-download-url-ttl, logs.get-log-object-download-url-rejected-after-expiry, logs.get-log-object-unauthorized-rejected.

Cause 3 of 6: GetIntegration, UntagResource and DisassociateSource answer NotImplemented where AWS answers ResourceNotFoundException.
AWS: each of those operations resolves its target first and refuses an unknown integration name / resource ARN / never-associated source with 400 ResourceNotFoundException (the shape is modeled on the operation).
Emulator: ClientError code 'NotImplemented' - the operation is a scaffold stub, so the not-found contract cannot be observed.
Why: a client's error handling for 'this resource does not exist' never runs, and a retry loop keyed on ResourceNotFoundException sees an unknown code.
Fix: implement those handlers in the logs service crate's ops modules, including the lookup-miss path.
Traces: logs.get-integration-nonexistent-fails, logs.delete-integration-unknown-name-rejected, logs.untag-resource-nonexistent-arn-fails, logs.disassociate-source-not-associated-fails.

Cause 4 of 6: the ResourceNotFoundException message for a missing log group drops the trailing period.
AWS: the message is the fixed text 'The specified log group does not exist.' (constant per operation, not derived from the request).
Emulator: 'The specified log group does not exist' (no period) on AssociateKmsKey, DisassociateKmsKey and PutBearerTokenAuthentication. Status 400 and code ResourceNotFoundException are correct.
Why: clients and test suites that match the documented message string fail against the emulator.
Fix: the error-construction helper for the not-found path in the logs service crate.
Traces: logs.associate-kms-key-nonexistent-group-fails, logs.associate-kms-key-nonexistent-log-group-fails, logs.disassociate-kms-key-nonexistent-log-group-fails, logs.bearer-token-authentication-lifecycle, logs.bearer-token-authentication-nonexistent-log-group-rejected, logs.put-bearer-token-authentication-invalid-config-rejected, logs.put-bearer-token-authentication-invalid-operation-state, logs.put-bearer-token-authentication-invalid-operation-state-rejected.

Cause 5 of 6: constraint-violation messages do not follow AWS's validation template.
AWS: the message is built from the request by a known rule - "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where <pattern> is rendered WITHOUT the surrounding ^...$ anchors; for logGroupNamePattern AWS renders a different pattern altogether ((\^?[\.\-_\/#A-Za-z0-9]{3,512})|(\^?[...]{3,24})(\|\^?[...]{3,24}){0,4}). Smithy (cloudwatchlogs): LogGroupName @length 1..512 @pattern ^[\.\-_/#A-Za-z0-9]+$; LogStreamName @pattern ^[^:*]*$. Codes/status (400 InvalidParameterException) already match.
Emulator: emits the anchored model pattern verbatim, and uses its own prose for other fixed AWS texts: DescribeLogStreams orderBy conflict ('logStreamNamePrefix cannot be specified when orderBy is LastEventTime' vs AWS 'Cannot order by LastEventTime with a logStreamNamePrefix.'), DescribeLogGroups prefix+pattern ('logGroupNamePrefix and logGroupNamePattern are mutually exclusive; specify only one' vs AWS 'LogGroup name prefix and LogGroup name pattern are mutually exclusive parameters.'), resource-policy limit ('an account can have a maximum of 10 resource policies without a resourceArn' vs AWS 'Resource limit exceeded.'), and PutResourcePolicy document validation.
Why: clients that parse the validation message to learn which member failed, and suites pinned to the documented text, diverge.
Fix: the validation-error formatter in the logs service crate (strip pattern anchors; use the modeled/fixed AWS texts).
Traces: logs.create-log-group-invalid-name-rejected, logs.create-log-group-name-exceeds-length-limit-rejected, logs.create-log-group-name-exceeds-max-length-rejected, logs.create-log-stream-invalid-name-rejected, logs.invalid-parameter-validation-on-create, logs.list-validation-error-semantics, logs.describe-log-streams-prefix-and-orderby-conflict-fails, logs.op-contract-describe-log-streams, logs.agentcore-log-group-prefix-and-pattern-lookup-scope, logs.op-contract-describe-log-groups-mutually-exclusive-filters-rejected, logs.limit-exceeded-triggers, logs.op-contract-putResourcePolicy, logs.put-subscription-filter-exceeds-quota-rejected.

Cause 6 of 6: a log-group endpoint protected by bearer-token authentication answers 404 instead of 403 before auth is enabled.
AWS: recorded behavior - the request is refused with HTTP 403 while bearer-token authentication is not enabled on the group.
Emulator: answers HTTP 404.
Why: a client cannot distinguish 'not authorized' from 'no such endpoint' while rolling bearer-token auth out.
Fix: the bearer-token authentication gate for the log-group data endpoint in the logs service crate.
Traces: logs.put-bearer-token-authentication-enforced-server-side.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartLiveTail operation: operation StartLiveTail is not implemented in this emulator; AssertionError: positive control: StartLiveTail on a real, existing log group was rejected: NotImplemented; AssertionError: expected InvalidParameterException for logStreamNames over two groups, got 'NotImplemented'. Expected: a live-tail event stream, ResourceNotFoundException, InvalidParameterException.
Cause 2: AssertionError: expected GetLogObject to reject a client-constructed logObjectPointer with InvalidParameterException (confirmed against real AWS), got 'NotImplemented'. Expected: 400 InvalidParameterException / the raw payload.
Cause 3: AssertionError: expected ResourceNotFoundException from GetIntegration on an unknown name, got 'NotImplemented'; 'expected ResourceNotFoundException untagging a non-existent resource, got NotImplemented'; 'expected ResourceNotFoundException disassociating a never-associated source, got NotImplemented'.
Cause 4: AssertionError: expected the exact message 'The specified log group does not exist.', got 'The specified log group does not exist'.
Cause 5: AssertionError: expected message "... Member must satisfy regular expression pattern: [\\.\\-_/#A-Za-z0-9]+", got "... Member must satisfy regular expression pattern: ^[\\.\\-_/#A-Za-z0-9]+$"; AssertionError: expected message 'Cannot order by LastEventTime with a logStreamNamePrefix.', got 'logStreamNamePrefix cannot be specified when orderBy is LastEventTime'; AssertionError: expected message 'Resource limit exceeded.', got 'an account can have a maximum of 10 resource policies without a resourceArn'.
Cause 6: AssertionError: expected 403 before auth is enabled, got 404.

## Steps to Reproduce
Cause 1: CreateLogGroup, then logs.start_live_tail(logGroupIdentifiers=[arn]) -> NotImplemented.
Cause 2: logs.get_log_object(logObjectPointer='<made-up>') -> NotImplemented instead of InvalidParameterException.
Cause 3: logs.get_integration(integrationName='nope') / logs.untag_resource(resourceArn='<absent>') -> NotImplemented instead of ResourceNotFoundException.
Cause 4: logs.associate_kms_key(logGroupName='absent', kmsKeyId=<arn>) -> message without the trailing period.
Cause 5: logs.create_log_group(logGroupName='bad name!') -> message quotes the anchored pattern; logs.describe_log_streams(logGroupName=g, orderBy='LastEventTime', logStreamNamePrefix='p') -> emulator-specific text.
Cause 6: hit the log-group data endpoint before enabling bearer-token auth -> 404 instead of 403.
