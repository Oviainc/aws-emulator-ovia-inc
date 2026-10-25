Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching
Issue-Key: events__sqs|StartReplay|NotImplemented
Issue-Key: events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns|_collect#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered|_collect#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts|_receive#21|Timeout|resp = sqs.receive_message(
Issue-Key: events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching|_drain_all#1|Timeout|return {k: f.result() for k, f in futs.items()}
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# events__sqs: archive replay is unreachable (StartReplay is a stub), api-destination retry bounds are never exercised, and a malformed-pattern update check cannot complete (3 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
Three independent root causes in the events__sqs suite. None of the six checks is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 3: StartReplay is a scaffold stub, so the whole archive-replay data plane is unreachable.
AWS: Smithy (com-amazonaws-eventbridge-awsevents) models StartReplay with required {ReplayName @length 1..64 @pattern ^[\.\-_A-Za-z0-9]+$, EventSourceArn (an archive ARN), EventStartTime, EventEndTime, Destination} and output {ReplayArn, State, StateReason, ReplayStartTime}; errors InternalException, InvalidEventPatternException, LimitExceededException, ResourceAlreadyExistsException, ResourceNotFoundException. A valid call answers HTTP 200 and EventBridge then re-emits the archived events whose `time` falls inside [EventStartTime, EventEndTime] through the rules as they exist AT REPLAY TIME, honouring Destination.FilterArns, and stamps each redelivered envelope with `replay-name` (a field event patterns can match, so a rule requiring `replay-name: [{exists: false}]` must NOT receive the replay) while keeping the ORIGINAL event `time`.
Emulator: `An error occurred (NotImplemented) when calling the StartReplay operation: operation StartReplay is scaffolded but not implemented in this emulator`. The two checks that reach StartReplay fail with that error; the other two (which first wait for archive ingestion before replaying) hang to the 60 s harness cap and never get a replay either.
Why: replay is how a consumer re-processes history after a bug or an outage. With StartReplay refused, no replay-driven recovery can be built or tested, and the `replay-name` opt-out that stops a consumer re-processing history is unreachable.
Fix: the StartReplay handler in the eventbridge crate plus the replay re-emission path — window selection from the archive, FilterArns scoping, re-evaluation against current rules, and the `replay-name` envelope field being present at MATCHING time (not stamped after routing).
Traces: events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived, events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window, events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns, events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered.

Cause 2 of 3: an api-destination target's retry bound is never exercised because no attempt is made.
AWS: recorded behavior only. AWS documents a 5 s client execution timeout for API destination endpoints and retries timed-out requests up to the target's RetryPolicy.MaximumRetryAttempts (https://repost.aws/knowledge-center/eventbridge-api-destination-target-failure); the retried event finally lands in the DLQ. The observable requirement is that a target with N extra retries makes measurably more attempts, and takes measurably longer, than an otherwise identical target with zero retries, and that both end in the DLQ.
Emulator: the check hangs to the 60 s harness cap; no request reaches the endpoint and no dead letter arrives for either target, so the two configurations are indistinguishable.
Why: a client cannot bound how long EventBridge will keep hammering a slow endpoint, and cannot tell a retry-exhausted failure from a never-attempted one.
Fix: the api-destination branch of the eventbridge target dispatcher and its per-attempt timeout/retry accounting.
Traces: events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts.

Cause 3 of 3: the malformed-pattern-update suite cannot complete, so neither the nine refusals nor the live rule's continued matching is observable.
AWS: Smithy models PutRule's InvalidEventPatternException (400) and TestEventPattern's InvalidEventPatternException (400). AWS refuses nine distinct malformed pattern fragments on PutRule with InvalidEventPatternException and a `Event pattern is not valid. Reason: <reason>` message (recorded example reasons: `Unrecognized numeric range operator: !=`, `Value of > must be numeric`, `Only one key allowed in match expression`, `exists match pattern must be either true or false.`, `Consecutive wildcard characters at pos 2`, `IPv4 mask bits must be < 32`, `Empty arrays are not allowed`, `Unrecognized match type detail`, `Unrecognized match type startswith`), the refused update leaves the live rule's prior pattern in place, and the live rule keeps matching only what that prior pattern matches.
Emulator: the check hangs to the 60 s harness cap — the live-rule delivery leg that proves the rule kept matching never produces a message, so the effect-preserved half of the assertion can never be reached.
Why: a client cannot confirm that a rejected PutRule update is atomic (prior pattern retained and still matching), which is the property that makes a failed deploy safe.
Fix: the eventbridge rule-target delivery path (the SQS target branch) so the live control delivers, and the PutRule pattern-validation refusals it is paired against.
Traces: events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching.

## Evidence
Cause 1:
- events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived [error]: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartReplay operation: operation StartReplay is scaffolded but not implemented in this emulator. Expected: HTTP 200 {ReplayArn, State, ReplayStartTime} and the replay delivered to a rule created AFTER archival, carrying `replay-name` and the original `time`.
- events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window [error]: same NotImplemented on StartReplay. Expected: exactly the two in-window events of a three-event PutEvents redelivered, each carrying `replay-name`.
- events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns [mismatch]: `Check timed out after 60.0s` — no replay ever reaches rule A, so rule B's required silence is ungradeable.
- events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered [mismatch]: `Check timed out after 60.0s` — no replay reaches rule Y, so the `replay-name exists:false` opt-out on rule X is ungradeable.

Cause 2 — events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts [mismatch]: `Check timed out after 60.0s`. Two identical api-destination targets differing only in RetryPolicy.MaximumRetryAttempts (0 vs N) against a non-responding endpoint. Expected: the N-retry target makes at least N more attempts and takes at least N*5 s longer, and both events land in the DLQ. Actual: no attempt recorded and no dead letter for either target.

Cause 3 — events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching [mismatch]: `Check timed out after 60.0s`. Expected: each of the nine malformed PutRule updates refused InvalidEventPatternException with its `Event pattern is not valid. Reason: ...` message, and the live rule (pattern {"detail":{"k":[{"prefix":"ok"}]}}) still delivering only the "ok" event to its queue and never the "zz" event. Actual: the live rule's queue receives nothing, so the check never completes.

## Steps to Reproduce
Cause 1:
1. create_event_bus(Name=B); create_archive(ArchiveName=A, EventSourceArn=<bus arn>).
2. put_events with explicit per-entry Time values (e.g. one 2 h old, two 1 h old) on bus B; wait for the archive's EventCount to move.
3. put_rule on B with an SQS target; start_replay(ReplayName=R, EventSourceArn=<archive arn>, EventStartTime/EventEndTime = a 10 min band around the 1 h mark, Destination={'Arn': <bus arn>, 'FilterArns': [<rule arn>]}) -> NotImplemented.

Cause 2:
1. Create two api destinations on one connection pointing at an endpoint that never answers; create a bus, a rule and two targets differing only in RetryPolicy.MaximumRetryAttempts (0 and N), both with the same DeadLetterConfig.
2. put_events one matching event; poll the DLQ and time the two arrivals -> no arrivals.

Cause 3:
1. create_event_bus; put_rule(Name='live', EventPattern='{"detail":{"k":[{"prefix":"ok"}]}}') with an SQS target.
2. put_events the "ok" event -> expect it in the queue (this is the leg that never delivers).
3. For each malformed fragment (e.g. '{"detail":{"n":[{"numeric":["!=",1]}]}}'), put_rule on the SAME rule name -> expect InvalidEventPatternException, then confirm the live rule still delivers only "ok" and never "zz".

Replay the six check ids in trace_ids to confirm.
