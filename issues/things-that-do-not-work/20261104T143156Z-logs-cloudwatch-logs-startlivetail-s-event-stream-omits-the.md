Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.start-live-tail-streams-new-events logs.start-live-tail-streams-matching-events logs.start-live-tail-session-idle-stays-open logs.start-live-tail-nonexistent-log-group-rejected logs.start-live-tail-mutually-exclusive-stream-filters-rejected logs.start-live-tail-invalid-log-group-rejected logs.live-tail-session-behavior logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups logs.deep-cwlogs-semantics-filter-log-events-stream-names-with-prefix-rejected logs.filter-log-events-bare-exists-selector-rejected logs.filter-log-events-json-selector-existence-null-boolean-and-array-operators logs.syslog-configuration-put-list-delete-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.start-live-tail-streams-new-events|run#9|ResponseParserError|resp = logs.start_live_tail(logGroupIdentifiers=[log_group_arn])
Issue-Key: logs.start-live-tail-streams-matching-events|run#11|ResponseParserError|resp = logs.start_live_tail(
Issue-Key: logs.start-live-tail-session-idle-stays-open|run#7|ResponseParserError|resp = logs.start_live_tail(
Issue-Key: logs.start-live-tail-nonexistent-log-group-rejected|run#8|ResponseParserError|positive_stream = logs.start_live_tail(logGroupIdentifiers=[real_arn])["responseStream"]
Issue-Key: logs.start-live-tail-mutually-exclusive-stream-filters-rejected|run#10|ResponseParserError|control_resp = logs.start_live_tail(logGroupIdentifiers=[group_arn], logStreamNames=["stream-a"])
Issue-Key: logs.start-live-tail-invalid-log-group-rejected|run#9|ResponseParserError|control_stream = logs.start_live_tail(logGroupIdentifiers=[real_arn])["responseStream"]
Issue-Key: logs.live-tail-session-behavior|run#7|ResponseParserError|resp = svc.start_live_tail(logGroupIdentifiers=[group_arn])
Issue-Key: logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter|run#16|ResponseParserError|response_stream = logs.start_live_tail(
Issue-Key: logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream|run#15|ResponseParserError|logs.start_live_tail(logGroupIdentifiers=[group_arn], logStreamNames=[stream_a])
Issue-Key: logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: logs.deep-cwlogs-semantics-filter-log-events-stream-names-with-prefix-rejected|run#14|AssertionError|assert message == (
Issue-Key: logs.filter-log-events-bare-exists-selector-rejected|run#22|AssertionError|assert (code, msg) == ("InvalidParameterException", "Invalid filter pattern"), (
Issue-Key: cloudwatch-logs|FilterLogEvents|InvalidParameterException
Issue-Key: logs.syslog-configuration-put-list-delete-lifecycle|_find_syslog_vpc_endpoint_service_name#16|AssertionError|raise AssertionError(
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# CloudWatch Logs: StartLiveTail's event stream omits the initial-response frame (no SDK client can read a session), and five more logs data-plane divergences (6 causes)

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartLiveTail

## Description
Six net-new root causes in the CloudWatch Logs (`logs`) surface, grounded in 14 failing behavioral checks listed under `### logs` in the replay report, none of which is marked ALREADY FILED or OWNED. ALL SIX must be closed before this issue is submitted.

RELATED OPEN ISSUES (named for consolidation; neither carries these checks' replay keys, so they do NOT cover the causes below). (a) 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md lists the start-live-tail/live-tail check ids in its Issue-Trace-Ids, but its cause is 'StartLiveTail answers NotImplemented/501'; its replay key is `logs|StartLiveTail|NotImplemented`. That is no longer what happens: StartLiveTail now answers 200 with an event stream, and the stream itself is malformed, which is a different defect in a different component (the event-stream serializer, not a missing handler). (b) 20261027T074309Z-logs-syslog-configurations-are-ungradeable-ec2-describevpcendpointservices-omits.md describes exactly cause 6 but carries only three of the four syslog check ids; the fourth is recorded here.

=== Cause 1 of 6: the StartLiveTail event stream does not begin with an `initial-response` event, so every SDK client fails to parse the response before any session frame is delivered ===
AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models StartLiveTail with output member `responseStream` targeting the streaming union `StartLiveTailResponseStream` (members sessionStart -> LiveTailSessionStart, sessionUpdate -> LiveTailSessionUpdate, SessionTimeoutException, SessionStreamingException), hostPrefix `stream-`, errors AccessDeniedException / InvalidOperationException / InvalidParameterException / LimitExceededException / ResourceNotFoundException. For an awsJson1.1 operation whose output carries an event stream, the wire contract is that the vnd.amazon.eventstream body opens with a frame whose `:event-type` header is `initial-response` (carrying the output's non-stream members) before any modeled event frame; real AWS emits it, which is why botocore's event-stream parser calls get_initial_response() first and only then yields sessionStart/sessionUpdate.
Emulator: StartLiveTail returns 200 with an event-stream body whose FIRST frame is not `initial-response`, so botocore aborts in parsers.py `_handle_event_stream` with `botocore.parsers.ResponseParserError: First event was not of type initial-response` before a single session frame is read. This happens identically on the happy paths and on the refusal paths (unknown log group, invalid identifier, mutually exclusive stream filters), so even the modeled errors are unobservable.
Why: Live Tail is completely unusable from any AWS SDK or the CLI: the client raises a protocol parse error instead of a session, so a caller can neither start a tail, nor read sessionStart (sessionId/requestId/the echoed filters), nor receive sessionUpdate events, nor see ResourceNotFoundException / InvalidParameterException for a bad request. A client cannot even distinguish 'bad request' from 'broken service'.
Fix: the StartLiveTail event-stream serializer in the logs service crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328) must emit the leading `initial-response` frame for the awsJson1.1 event stream before the first `sessionStart` frame, and must frame the modeled in-stream exceptions the same way.
Traces: logs.start-live-tail-streams-new-events, logs.start-live-tail-streams-matching-events, logs.start-live-tail-session-idle-stays-open, logs.start-live-tail-nonexistent-log-group-rejected, logs.start-live-tail-mutually-exclusive-stream-filters-rejected, logs.start-live-tail-invalid-log-group-rejected, logs.live-tail-session-behavior, logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter, logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream.

=== Cause 2 of 6: StartLiveTail's refusal of logStreamNames spanning more than one log group uses the emulator's own wording ===
AWS: 400 InvalidParameterException with the fixed text 'LogStreamNames provided with more than one LogGroupIdentifier. LogStreamNames is disabled if more than one LogGroupIdentifier is provided.' (recorded behavior; the message is constant for this error class and contains no request-derived values). Smithy models logGroupIdentifiers @length 1..10 and logStreamNames @length 1..100 without expressing this cross-member rule, so the rule lives in the handler.
Emulator: the code InvalidParameterException and status 400 are right, but the message is 'logStreamNames and logStreamNamePrefixes can be specified only with a single log group in logGroupIdentifiers.'
Why: clients and suites that match the documented refusal text (and operators reading it) see a message AWS never produces.
Fix: the StartLiveTail request-validation path in the logs service crate must emit AWS's fixed text for this rule.
Traces: logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups.

=== Cause 3 of 6: FilterLogEvents' refusal of logStreamNames together with logStreamNamePrefix uses the emulator's own wording ===
AWS: 400 InvalidParameterException with the fixed text 'Cannot call FilterLogEvents with both logStreamNames and logStreamNamePrefix' (recorded behavior; fully fixed text, no request values). Smithy (FilterLogEvents) models logStreamNames and logStreamNamePrefix as independent optional members and models InvalidParameterException on the operation, so the mutual exclusion is a handler rule.
Emulator: status 400 and code InvalidParameterException are right; the message is 'logStreamNames and logStreamNamePrefix are mutually exclusive parameters.'
Why: same message-contract break as cause 2, on the most-used read operation of the service.
Fix: the FilterLogEvents request-validation path in the logs service crate.
Traces: logs.deep-cwlogs-semantics-filter-log-events-stream-names-with-prefix-rejected.

=== Cause 4 of 6: the FilterLogEvents 'bad filter pattern' refusal appends an emulator-specific explanation to AWS's fixed message ===
AWS: a filterPattern that does not parse (here a bare existence selector '{$.flag EXISTS}') is refused 400 InvalidParameterException with the fixed message 'Invalid filter pattern' — exactly that, with no trailing detail (recorded behavior). Smithy: filterPattern targets FilterPattern @length 0..1024 and the operation models InvalidParameterException; the model says nothing about the grammar, so the message text is the only observable contract.
Emulator: 'Invalid filter pattern: the condition does not parse' — AWS's text plus a suffix AWS never emits.
Why: a client that keys on the exact message (or compares it against the documented one) treats a correctly-classified refusal as an unknown error.
Fix: the filter-pattern parser's error construction in the logs service crate must use the bare fixed text.
Traces: logs.filter-log-events-bare-exists-selector-rejected.

=== Cause 5 of 6: the JSON filter-pattern engine does not support array-index selectors, so valid patterns are refused instead of selecting events ===
AWS: CloudWatch Logs' JSON filter syntax supports indexing into an array member — `$.arr[0]`, `$.arr[1]`, `$.arr[*]` — in FilterLogEvents and TestMetricFilter, selecting the events whose indexed element matches (recorded behavior; the model only constrains filterPattern @length 0..1024). A pattern such as '{$.arr[0] = "red"}' or '{$.arr[0] = "blue" || $.n = 4}' is a successful 200 returning the matching events, not an error.
Emulator: FilterLogEvents refuses the pattern with 400 InvalidParameterException and the message 'Invalid filter pattern: <selector-prefix> selects no field' (this run: "'$.arr[' selects no field") — the parser stops at the '[' and reports the truncated selector, so the whole array-operator family (and the `||` alternations that contain one) is unusable; the check never gets to compare FilterLogEvents against TestMetricFilter for the remaining operators.
Why: a client filtering JSON log events by an array element gets a hard validation error for a documented, valid pattern; the same pattern in a metric filter cannot be tested either.
Fix: the JSON filter-pattern parser/evaluator shared by FilterLogEvents and TestMetricFilter in the logs service crate must accept `[N]` and `[*]` element selectors (and keep accepting them inside `||` alternations).
Traces: logs.filter-log-events-json-selector-existence-null-boolean-and-array-operators.

=== Cause 6 of 6: the syslog surface stays unreachable because the EC2 endpoint-service catalog publishes no CloudWatch Logs syslog-ingestion PrivateLink service ===
AWS: PutSyslogConfiguration / ListSyslogConfigurations / DeleteSyslogConfiguration take a vpcEndpointId (Smithy @pattern ^vpce-[0-9a-f]{1,64}$) that must name an interface endpoint to the DEDICATED regional syslog-ingestion service, which AWS publishes in DescribeVpcEndpointServices as `com.amazonaws.<region>.syslog-logs`, distinct from the plain API service `com.amazonaws.<region>.logs` (recorded behavior; the name is derived from the request's Region and cannot be guessed — a synthesized name is refused with InvalidServiceName).
Emulator: for the signing Region the only logs-related entry published is `com.amazonaws.<region>.logs` (this run: ['com.amazonaws.us-west-2.logs']), so the check aborts in its own provisioning helper with 'no dedicated syslog-ingestion VPC endpoint service is available in this account/region for CloudWatch Logs' and PutSyslogConfiguration / ListSyslogConfigurations / DeleteSyslogConfiguration are never reached.
Why: a client provisioning syslog ingestion must discover the regional service name before it can create the endpoint; with no catalog entry the entire feature is unreachable and none of its contracts (a Put becoming visible in ListSyslogConfigurations with the group's logGroupArn, a Delete removing exactly that (logGroupIdentifier, vpcEndpointId) pair, a Delete of an unconfigured pair refused 400 InvalidParameterException) can be observed.
Fix: the EC2 Region endpoint-service catalog used by DescribeVpcEndpointServices must include the Region's syslog-ingestion Interface service, named as AWS publishes it and distinct from the plain logs service; then the three syslog handlers in the logs crate must key configurations on (logGroupIdentifier, vpcEndpointId). This is the same cause as open issue 20261027T074309Z-logs-syslog-configurations-are-ungradeable-ec2-describevpcendpointservices-omits.md — consolidate; only this fourth check id is new.
Traces: logs.syslog-configuration-put-list-delete-lifecycle.

## Evidence
Cause 1 (9 checks, identical failure): botocore.parsers.ResponseParserError: First event was not of type initial-response — raised in parsers.py line 1105 from _handle_event_stream, i.e. before any frame of the session is delivered. Expected: a parseable event stream opening with an `initial-response` frame, then `sessionStart` (LiveTailSessionStart: requestId, sessionId, logGroupIdentifiers, echoed logStreamNames/logStreamNamePrefixes/logEventFilterPattern) and `sessionUpdate` frames; on the negative paths, ResourceNotFoundException / InvalidParameterException.

Cause 2: AssertionError: expected message 'LogStreamNames provided with more than one LogGroupIdentifier. LogStreamNames is disabled if more than one LogGroupIdentifier is provided.', got 'logStreamNames and logStreamNamePrefixes can be specified only with a single log group in logGroupIdentifiers.'

Cause 3: AssertionError: unexpected FilterLogEvents refusal message for names plus prefix: 'logStreamNames and logStreamNamePrefix are mutually exclusive parameters.' (expected 'Cannot call FilterLogEvents with both logStreamNames and logStreamNamePrefix'; code InvalidParameterException already matches, and each selector ALONE returns the right events).

Cause 4: AssertionError: FilterLogEvents('{$.flag EXISTS}') was refused with 'InvalidParameterException' / 'Invalid filter pattern: the condition does not parse' (expected ('InvalidParameterException', 'Invalid filter pattern')).

Cause 5: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the FilterLogEvents operation: Invalid filter pattern: '$.arr[' selects no field — raised for the pattern '{$.arr[0] = "red"}' while iterating a table of documented operators; expected 200 with the events whose arr[0] == "red".

Cause 6: AssertionError: no dedicated syslog-ingestion VPC endpoint service is available in this account/region for CloudWatch Logs (logs-related endpoint services seen: ['com.amazonaws.us-west-2.logs']) — raised at check line 97, before any logs syslog call. (The Region and the generated names above are this run's values filling the templates; the requirement is a Region-derived syslog service name distinct from the plain logs service.)

## Steps to Reproduce
Cause 1: logs.create_log_group(logGroupName=G); then logs.start_live_tail(logGroupIdentifiers=[<G's arn>]) and iterate the response's 'responseStream' -> botocore raises ResponseParserError('First event was not of type initial-response'). Repeat with a nonexistent log group and with logStreamNames+logStreamNamePrefixes set: the same parse error replaces the modeled refusal.
Cause 2: logs.start_live_tail(logGroupIdentifiers=[arnA, arnB], logStreamNames=['s']) -> InvalidParameterException with the emulator's wording instead of AWS's fixed text.
Cause 3: create a group with streams 'svc-a'/'svc-b' and one event each; logs.filter_log_events(logGroupName=G, logStreamNames=['svc-a'], logStreamNamePrefix='svc-', startTime=..., endTime=...) -> 400 InvalidParameterException with the wrong message (each selector alone returns its events).
Cause 4: logs.filter_log_events(logGroupName=G, filterPattern='{$.flag EXISTS}') -> message carries the extra ': the condition does not parse'.
Cause 5: put JSON events containing an array member, e.g. {"m":"t-true","arr":["red","blue"]}; logs.filter_log_events(logGroupName=G, filterPattern='{$.arr[0] = "red"}') -> InvalidParameterException instead of the matching event; same for '{$.arr[*] = "red"}' and for logs.test_metric_filter with those patterns.
Cause 6: ec2.describe_vpc_endpoint_services(Filters=[{'Name':'service-type','Values':['Interface']}]) paged to exhaustion -> no ServiceName contains 'syslog'; expect the Region's syslog-ingestion service alongside com.amazonaws.<region>.logs so that CreateVpcEndpoint and then PutSyslogConfiguration/ListSyslogConfigurations/DeleteSyslogConfiguration can run.
