Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.start-live-tail-streams-new-events logs.start-live-tail-streams-matching-events logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups logs.start-live-tail-mutually-exclusive-stream-filters-rejected logs.syslog-configuration-delete-nonexistent-rejected logs.syslog-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.start-live-tail-streams-new-events|_drain_until#27|Timeout|event = events_queue.get(timeout=max(0.1, remaining))
Issue-Key: logs.start-live-tail-streams-matching-events|run#25|Timeout|time.sleep(0.5)
Issue-Key: logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter|run#23|Timeout|time.sleep(0.5)
Issue-Key: logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream|run#16|Timeout|time.sleep(4.0)
Issue-Key: logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups|_tail_until#8|Timeout|time.sleep(4.0)
Issue-Key: logs.start-live-tail-mutually-exclusive-stream-filters-rejected|run#16|AssertionError|assert message == (
Issue-Key: logs.syslog-configuration-delete-nonexistent-rejected|run#12|AssertionError|assert message == not_associated, f"unexpected never-configured delete message: {message!r}"
Issue-Key: logs.syslog-configuration-lifecycle|run#14|AssertionError|assert error_message == (
Issue-Commit: f23784b0aff92740251c6928f623341ae96cfe61

# logs StartLiveTail never streams LiveTailSessionUpdate frames; two refusal messages are emulator-authored (3 causes)

> _Note: this issue was identified against the code base at commit `f23784b0aff92740251c6928f623341ae96cfe61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartLiveTail

## Description
Three net-new root causes on the CloudWatch Logs live-tail and syslog-configuration surfaces. Cause count: 3 — all three must be closed before submitting.

=== Cause 1 of 3: StartLiveTail opens a session and then delivers nothing ===
AWS: StartLiveTail is a managed streaming session. Smithy: StartLiveTail input logGroupIdentifiers (required, @length min 1 max 10), optional logStreamNames / logStreamNamePrefixes (@length min 1 max 100) and logEventFilterPattern (@length max 1024); output responseStream targets StartLiveTailResponseStream with @streaming, and the operation carries the endpoint hostPrefix "stream-". AWS emits exactly one LiveTailSessionStart frame (sessionId plus the resolved logGroupIdentifiers), then one LiveTailSessionUpdate frame per batch of newly ingested matching events, each sessionResults entry carrying logStreamName, logGroupIdentifier, message and timestamp. Only events ingested AFTER the session opened are delivered; events already present in the group when the session opened are not. Scope members (logStreamNames, logStreamNamePrefixes, logEventFilterPattern) narrow which ingested events appear.
Emulator: the sessionStart frame IS delivered — the positive control inside logs.start-live-tail-mutually-exclusive-stream-filters-rejected receives a frame containing `sessionStart` — but no LiveTailSessionUpdate frame ever follows. Events written with PutLogEvents after the session opened never appear on the response stream, and no terminal frame (SessionTimeoutException / SessionStreamingException) is sent either. Every check that drains the stream for a sessionUpdate blocks until the harness kills it: `Error: Check timed out after 60.0s`.
Why: a client that tails a log group live gets neither data nor an error — it blocks forever instead of either receiving its events or failing. Any monitoring/tailing workflow (`aws logs tail --follow`, console Live Tail) silently never produces output, and a scoped session can never be verified.
Fix: the StartLiveTail handler's streaming response in the logs service — it must publish LiveTailSessionUpdate frames off the log-group ingest path (PutLogEvents) for the sessions subscribed to that group, filtered by the session's logStreamNames / logStreamNamePrefixes / logEventFilterPattern scope, and excluding events ingested before the session opened.
Traces (check ids): logs.start-live-tail-streams-new-events, logs.start-live-tail-streams-matching-events, logs.live-tail-excludes-pre-subscription-events-and-honours-the-filter, logs.log-cwl-plumbing-live-tail-stream-names-scope-delivers-only-the-named-stream, logs.log-cwl-plumbing-live-tail-stream-scope-rejected-across-two-log-groups.

=== Cause 2 of 3: StartLiveTail's mutually-exclusive stream-filter refusal message is emulator-authored ===
AWS: supplying both logStreamNames and logStreamNamePrefixes on one StartLiveTail call is refused with HTTP 400 InvalidParameterException (Smithy: StartLiveTail binds InvalidParameterException, httpError 400) whose message is the fixed sentence pair, with NO variable parts: "LogStreamNames provided with LogStreamPrefixes. LogStreamPrefixes is disabled if log stream names is provided."
Emulator: the status (400) and the error code (InvalidParameterException) are correct, but the message is "logStreamNames and logStreamNamePrefixes are mutually exclusive parameters." — entirely different wording.
Why: the message is part of the error contract; a client or test that matches AWS's sentence does not recognise the refusal, and operators reading it see text AWS never emits.
Fix: the message string in the mutual-exclusion validation branch of the StartLiveTail handler (its input validation), not the code or status, which are already right.
Traces (check ids): logs.start-live-tail-mutually-exclusive-stream-filters-rejected.

=== Cause 3 of 3: DeleteSyslogConfiguration's not-associated refusal message is emulator-authored ===
AWS: DeleteSyslogConfiguration for a (log group, vpcEndpointId) pair that has no syslog configuration — whether it was never created or was just deleted — is refused with HTTP 400 InvalidParameterException (Smithy: DeleteSyslogConfiguration binds InvalidParameterException, httpError 400; vpcEndpointId @pattern ^vpce-[0-9a-f]{1,64}$) whose message is "VPC endpoint <vpcEndpointId> is not associated with log group <logGroupName>". The fixed text is "VPC endpoint " ... " is not associated with log group "; the two variable parts are the vpcEndpointId from the request and the log group named by the request's logGroupIdentifier.
Emulator: the status (400) and the code (InvalidParameterException) are correct, but the message is "No syslog configuration for vpcEndpointId [<vpcEndpointId>] exists on this log group." — different wording, the endpoint id is bracketed, and the log group is never named, only referred to as "this log group".
Why: a client matching AWS's sentence does not recognise the refusal and cannot recover the log group name from it, so a teardown or reconciliation loop that keys off the message mis-handles the already-detached case.
Fix: the message string in the association-check branch of the DeleteSyslogConfiguration handler in the logs service.
Note: the open WIRE issue 20261029T222444Z-logs-syslog-configuration-operations-skip-vpc-endpoint-validation.md records this same wording defect as its Cause 4, but these two behavioral checks are not on that issue's replay keys, so they are recorded here; closing that cause closes this one.
Traces (check ids): logs.syslog-configuration-delete-nonexistent-rejected, logs.syslog-configuration-lifecycle.

## Evidence
Cause 1: five checks fail with `Error: Check timed out after 60.0s`. logs.start-live-tail-streams-new-events reaches its assertion only after a 90 s loop that repeatedly PutLogEvents a fresh marker and drains the response stream for a `sessionUpdate` whose sessionResults contain that marker; the drain never returns, so the check is killed at the suite's 60 s cap. The sessionStart frame itself IS served — the positive control in logs.start-live-tail-mutually-exclusive-stream-filters-rejected asserts `"sessionStart" in control_event` and passes — which localises the gap to the sessionUpdate publication, not to session setup.

Cause 2: AssertionError: unexpected mutually-exclusive filter message: 'logStreamNames and logStreamNamePrefixes are mutually exclusive parameters.' — the check asserts code == 'InvalidParameterException' (passes) and then message == 'LogStreamNames provided with LogStreamPrefixes. LogStreamPrefixes is disabled if log stream names is provided.' (fails).

Cause 3: AssertionError: unexpected never-configured delete message: 'No syslog configuration for vpcEndpointId [vpce-0000000000000008e] exists on this log group.'; expected 'VPC endpoint <vpcEndpointId> is not associated with log group <logGroupName>'. The same mismatch fires on the already-deleted path in logs.syslog-configuration-lifecycle: AssertionError: unexpected already-deleted syslog configuration message: 'No syslog configuration for vpcEndpointId [vpce-0000000000000008e] exists on this log group.' The vpce id and group name shown are this run's values and fill the template; the requirement is the sentence shape with the request's endpoint id and the request's log group name.

## Steps to Reproduce
Cause 1: CreateLogGroup G, CreateLogStream S in G, take G's ARN from DescribeLogGroups and strip the trailing ':*', then StartLiveTail(logGroupIdentifiers=[arn]). Consume the responseStream on a background thread: a LiveTailSessionStart arrives. Now PutLogEvents(G, S, [{timestamp: now, message: 'probe <marker>'}]) and keep draining — AWS delivers a LiveTailSessionUpdate whose sessionResults contain 'probe <marker>' with that stream, group identifier, message and timestamp; the emulator delivers nothing. Repeat with logStreamNames=[S] (only S's lines must appear), with logEventFilterPattern set, and with events written before the session opened (those must NOT appear).

Cause 2: with G, S above, call StartLiveTail(logGroupIdentifiers=[arn], logStreamNames=['stream-a'], logStreamNamePrefixes=['stream-']) and read the first frame — expect HTTP 400 InvalidParameterException with message exactly 'LogStreamNames provided with LogStreamPrefixes. LogStreamPrefixes is disabled if log stream names is provided.'

Cause 3: CreateLogGroup N, then DeleteSyslogConfiguration(logGroupIdentifier=N, vpcEndpointId=<a real interface endpoint id>) with no configuration present — expect HTTP 400 InvalidParameterException 'VPC endpoint <id> is not associated with log group N'. Then PutSyslogConfiguration(N, <id>), DeleteSyslogConfiguration(N, <id>) (succeeds), and DeleteSyslogConfiguration(N, <id>) again — expect the identical refusal and message.
