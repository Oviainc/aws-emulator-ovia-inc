Issue-Source: trace-sweep-divergence
Issue-Trace: logs-live-tail-error-path
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-live-tail-error-path
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|StartLiveTail|4xx->5xx|$.__type
Issue-Key: logs|StartLiveTail|4xx->5xx|$.message
Issue-Key: logs|StartLiveTail|4xx->5xx|status
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# StartLiveTail answers 501 NotImplemented instead of resolving the log group

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartLiveTail

## Description
StartLiveTail is not served by the emulator: it answers HTTP 501 with __type NotImplemented, an error that is neither in the operation's modeled error set nor a status the service can return. The replay report attributes no stub_path and no owning open issue to this cluster, and no open issue mentions live tail, so it is net-new. This issue has 1 cause.

Cause 1 of 1 -- StartLiveTail is unserved, so even its negative path cannot be exercised.
AWS: Smithy -- StartLiveTail takes logGroupIdentifiers (required, StartLiveTailLogGroupIdentifiers, @length 1..10), optional logStreamNames / logStreamNamePrefixes (@length 1..100) and logEventFilterPattern (FilterPattern @length 0..1024); it returns responseStream (StartLiveTailResponseStream, @streaming) and is bound to the `stream-` host prefix. Its modeled errors are AccessDeniedException, InvalidOperationException, InvalidParameterException, LimitExceededException and ResourceNotFoundException -- all client errors at httpError 400. Recorded: a StartLiveTail naming a log group that does not exist is refused with 400 and __type ResourceNotFoundException.
Emulator: HTTP 501 with __type NotImplemented and a message stating the operation is not implemented in this emulator.
Why: 501 is a server-side "this server cannot do that" status, so an SDK classifies it as a transport/server fault and retries it rather than surfacing a client error. A client that tails a log group gets retry storms and an unrecognisable error code instead of the modeled ResourceNotFoundException, and every live-tail path -- happy and negative alike -- is unreachable. Because the operation returns an event stream, nothing downstream of it can be exercised either.
Fix: implement the StartLiveTail handler in the cloudwatchlogs service -- at minimum resolve logGroupIdentifiers against log-group state and refuse an unknown log group with the modeled ResourceNotFoundException at 400, and serve the modeled StartLiveTailResponseStream for a resolvable one.
Traces: logs-live-tail-error-path#task#1.

## Evidence
logs-live-tail-error-path#task#1 (StartLiveTail against a nonexistent log group): status expected 400, got 501; $.__type expected 'ResourceNotFoundException', got 'NotImplemented'; $.message expected "Provided LogGroup doesn't exist." (an example of the not-found text), got 'operation StartLiveTail is not implemented in this emulator'.
The 400 status and the ResourceNotFoundException error code are the requirements; the message text is an example.
Smithy citations: StartLiveTail input {logGroupIdentifiers required @length 1..10, logStreamNames @length 1..100, logStreamNamePrefixes @length 1..100, logEventFilterPattern @length 0..1024}, output {responseStream -> StartLiveTailResponseStream @streaming}, endpoint hostPrefix 'stream-', errors [AccessDeniedException, InvalidOperationException, InvalidParameterException, LimitExceededException, ResourceNotFoundException] (every one a client error at 400; NotImplemented and 501 are not modeled anywhere on this operation).
Replay linkage: this cluster reports stub_path = "" and owned_by = [], so no auto-filed stub issue and no open issue carries its replay key; list_existing_issues finds no issue mentioning live tail.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-live-tail-error-path step task#1: StartLiveTail with logGroupIdentifiers naming a log group that does not exist.
3. The emulator answers HTTP 501 with {'__type': 'NotImplemented', 'message': 'operation StartLiveTail is not implemented in this emulator'}.
4. The recording answers HTTP 400 with __type 'ResourceNotFoundException'.
