Issue-Source: trace-sweep-divergence
Issue-Trace: logsrd-get-log-events-end-of-stream-token
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logsrd-get-log-events-end-of-stream-token logsval-get-log-events-limit-above-maximum logsval-get-log-events-limit-below-minimum logsval-get-log-events-name-and-identifier-exclusive logsval-get-log-events-name-or-identifier-required
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|GetLogEvents|2xx->4xx|$.events
Issue-Key: logs|GetLogEvents|2xx->4xx|$.nextBackwardToken
Issue-Key: logs|GetLogEvents|2xx->4xx|$.nextForwardToken
Issue-Key: logs|GetLogEvents|2xx->4xx|status
Issue-Related: 20261022T220153Z-logs-putlogevents-accepts-out-of-window-log-events-without.md
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# GetLogEvents pagination tokens omit the service direction-prefix envelope

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLogEvents

## Description
CloudWatch Logs GetLogEvents returns two opaque cursors, nextForwardToken and nextBackwardToken, in a direction-prefixed envelope: a forward cursor is `f/<body>` and a backward cursor is `b/<body>`, optionally with a trailing `/s` sentinel at the end of the stream. The emulator emits the bare body with no prefix, and it then refuses a request carrying a service-shaped token with 400 instead of answering 200 with an empty page. Paging a log stream is therefore impossible either way: the token a client receives is the wrong shape, and a correctly shaped one is rejected. This bundle has 3 causes; all 3 must be closed.

Cause 1 of 3 -- both tokens are emitted without their direction prefix.
AWS: Smithy -- GetLogEvents output nextForwardToken and nextBackwardToken target NextToken @length min 1; the operation is @paginated with inputToken nextToken and outputToken nextForwardToken. Recorded behavior only (the model does not state the token grammar): every recorded GetLogEvents cursor carries a leading direction prefix -- `f/` for the forward cursor and `b/` for the backward cursor -- and the fixture asserts the token's shape rather than its body.
Emulator: both members are present but carry only the hex-encoded cursor body, with no `f/` or `b/` prefix, so neither token matches the service's cursor shape.
Why: a client that round-trips the token through anything that validates or discriminates the cursor's direction (an SDK paginator, a cursor cache, a proxy that distinguishes forward from backward scans) cannot use it, and a client that stores a cursor and later classifies it has no direction to read. This cluster spans five fixture steps across four fixtures.
Fix: the GetLogEvents response serializer / token encoder -- wrap the cursor body in the service's direction-prefixed envelope.
Traces: logsrd-get-log-events-end-of-stream-token#task#3 (also logsval-get-log-events-limit-above-maximum#task#3, logsval-get-log-events-limit-below-minimum#task#3, logsval-get-log-events-name-and-identifier-exclusive#task#3, logsval-get-log-events-name-or-identifier-required#task#3).

Cause 2 of 3 -- the backward token is emitted with the forward cursor's body and still no prefix.
AWS: as cause 1. Recorded: nextBackwardToken is `b/<body>` where the body encodes the backward scan position.
Emulator: nextBackwardToken is a bare body, and the body it carries is not the one the recording pairs with the backward cursor -- the emulator's forward and backward cursors are not distinguished in the way the recorded pair is.
Why: paging backwards from a cursor returns the wrong window (or the same window forever), so a reader walking a stream towards older events never terminates correctly.
Fix: the GetLogEvents token encoder -- emit the backward cursor in the `b/` envelope with the backward scan position.
Traces: logsrd-get-log-events-end-of-stream-token#task#4 (also the same fixture's task#5).

Cause 3 of 3 -- a service-shaped token is refused with 400 instead of returning an empty end-of-stream page.
AWS: as cause 1; GetLogEvents models InvalidParameterException, ResourceNotFoundException and ServiceUnavailableException. Recorded: re-reading at the end of the stream answers 200 with events = [] and both cursors echoed back in the end-of-stream form `b/<zeros>/s` and `f/<zeros>/s` -- an exhausted cursor is a successful empty page, not an error.
Emulator: 400; events, nextForwardToken and nextBackwardToken all absent.
Why: this is the terminal case of every paging loop. A client that follows nextForwardToken to the end gets a 400 instead of an empty page, so the loop raises instead of finishing -- and because the emulator's own tokens (causes 1-2) are the wrong shape, there is no token value that both sides accept.
Fix: the GetLogEvents handler's nextToken parsing -- accept the service's direction-prefixed and end-of-stream (`/s`) token forms and answer 200 with an empty events list when the cursor is exhausted.
Traces: logsrd-get-log-events-end-of-stream-token#task#6.

## Evidence
Cause 1 (logsrd-get-log-events-end-of-stream-token#task#3, 5 affected steps, 200 on both sides): $.nextBackwardToken and $.nextForwardToken both expected '<shaped-not-asserted>' (shape asserted, body not) and both got a bare hex body with no `b/` or `f/` prefix -- the shape assertion fails.
Cause 2 (same fixture task#4, 2 affected steps, 200 on both sides): $.nextBackwardToken expected 'b/<backward body>', got '<a bare body with no prefix>'.
Cause 3 (same fixture task#6): status expected 200, got 400; $.events expected [], got None; $.nextBackwardToken expected 'b/00000000000000000000000000000000000000000000000000000000/s', got None; $.nextForwardToken expected 'f/00000000000000000000000000000000000000000000000000000000/s', got None.
Smithy citations: GetLogEvents output nextForwardToken / nextBackwardToken -> NextToken @length min 1; @paginated inputToken nextToken, outputToken nextForwardToken, items events, pageSize limit; errors [InvalidParameterException, ResourceNotFoundException, ServiceUnavailableException].
The token bodies are opaque and environment-dependent; the requirement is the `f/` / `b/` envelope (with the `/s` end-of-stream form) and that an exhausted cursor yields 200 with an empty events list.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logsrd-get-log-events-end-of-stream-token step task#3 and inspect nextForwardToken / nextBackwardToken -- both are bare hex with no `f/` or `b/` prefix.
3. Replay the same fixture's task#4 -- nextBackwardToken has no `b/` prefix and does not carry the recorded backward cursor body.
4. Replay the same fixture's task#6 (GetLogEvents at the end of the stream, recorded 200 with events=[] and `b/<zeros>/s` / `f/<zeros>/s` tokens) -- the emulator answers 400 with no events and no tokens.
