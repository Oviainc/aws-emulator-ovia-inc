Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.start-live-tail-session-idle-stays-open
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|StartLiveTail|InvalidParameterException
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# logs: StartLiveTail refuses a valid logEventFilterPattern, rejecting any unquoted term that contains a hyphen with InvalidParameterException "Invalid character(s) in term '-'"

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartLiveTail

## Description
Cause 1 of 1: StartLiveTail's filter-pattern validation rejects hyphens inside a term.
AWS: Smithy (cloudwatchlogs) StartLiveTail.logEventFilterPattern targets FilterPattern, a plain string with @length min 0 / max 1024 and NO @pattern constraint; its documentation says only that the pattern is "A symbolic description of how CloudWatch Logs should interpret the data in each log event". In the CloudWatch Logs filter-pattern grammar a bare unquoted term is matched as a substring/term of the message and a hyphen is an ordinary character inside that term (the hyphen is special only as the leading exclusion marker of '-term'). A request whose logEventFilterPattern is a single hyphenated word is therefore accepted and simply matches nothing, and StartLiveTail answers 200 with an event stream (sessionStart, then periodic sessionUpdate frames; an idle session stays open for the fixed 3h session limit). InvalidParameterException is modelled for StartLiveTail (client error, HTTP 400), but not for this input.
Emulator: StartLiveTail with logEventFilterPattern='sim2real-behav-pattern-that-never-matches-anything' is refused: botocore.errorfactory.InvalidParameterException — "An error occurred (InvalidParameterException) when calling the StartLiveTail operation: Invalid character(s) in term '-'". The session is never opened, so no sessionStart/sessionUpdate frame can be observed. The message's fixed text "Invalid character(s) in term " plus the quoted character is emulator-authored for an input AWS accepts; its variable part here is the hyphen the validator extracted from the term.
Why: a caller that live-tails with any realistic filter term — a hyphenated service name, a resource id, a uuid, a dashed marker — is refused outright instead of getting a stream, and the refusal names a character AWS treats as ordinary, so there is no corrected pattern the caller can derive from it. Code that opens a tail with a non-matching filter to watch for future events (the documented idle-session behaviour) cannot run at all.
Fix: the StartLiveTail handler's logEventFilterPattern validation — the filter-pattern term lexer must accept '-' (and the other ordinary characters) inside a term and reserve the hyphen's special meaning for the leading exclusion form, keeping only the modelled @length 0..1024 check. Note: open issue 20261104T143156Z-logs-cloudwatch-logs-startlivetail-s-event-stream-omits-the.md covers the stream's missing initial-response frame and open issue 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md covers 501s; neither covers this input-validation refusal, which happens before any stream exists.
Traces: logs.start-live-tail-session-idle-stays-open.

## Evidence
Expected: StartLiveTail(logGroupIdentifiers=[<group arn without ':*'>], logEventFilterPattern='sim2real-behav-pattern-that-never-matches-anything') returns 200 and an event stream that yields a sessionStart frame and then periodic sessionUpdate heartbeats while idle.
Actual: HTTP 400 InvalidParameterException — "An error occurred (InvalidParameterException) when calling the StartLiveTail operation: Invalid character(s) in term '-'" raised from client.py _make_api_call; no frame is ever delivered.

## Steps to Reproduce
1. logs.create_log_group(logGroupName='sim2real-behav-lg-<id>').
2. Read its ARN from describe_log_groups and strip a trailing ':*'.
3. Call logs.start_live_tail(logGroupIdentifiers=[<arn>], logEventFilterPattern='sim2real-behav-pattern-that-never-matches-anything').
4. Observe InvalidParameterException "Invalid character(s) in term '-'" instead of an open event stream; the same call with a single non-hyphenated term is accepted.
