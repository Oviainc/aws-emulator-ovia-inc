Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch
Issue-Key: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch|_poll_until_denied#15|Timeout|time.sleep(3.0)
Issue-Commit: ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063

# events__sqs: an explicit Deny in an event-bus resource policy is stored but never enforced — PutEvents is accepted where AWS refuses the WHOLE request with AccessDeniedException

> _Note: this issue was identified against the code base at commit `ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED, so no open issue carries it.

=== Cause 1 of 1: a bus resource policy with Effect Deny on events:PutEvents, scoped by an events:source condition, is not evaluated at PutEvents time ===
AWS: Smithy: PutEvents.Entries is required @length(1..10); the model declares only InternalException on PutEvents, so the authorization refusal is recorded behavior only. PutPermission attaches the statement and DescribeEventBus returns it in Policy (the emulator does this part correctly). After the policy settles, PutEvents carrying an entry whose Source matches the Deny condition is refused at REQUEST level: HTTP 400, error code AccessDeniedException, message "User: <caller arn> is not authorized to perform: events:PutEvents on resource: <bus arn> with an explicit deny in a resource-based policy". The fixed text is "User: ", " is not authorized to perform: events:PutEvents on resource: " and " with an explicit deny in a resource-based policy"; the variable parts are the caller's ARN (the signing identity, as GetCallerIdentity reports it) and the ARN of the bus named by EventBusName. Crucially the refusal is NOT a per-entry validation failure: a two-entry batch mixing one denied source with one allowed source is refused in its entirety with the same code and the same message, and NEITHER entry is published — not even the entry whose source the Deny does not name. The allowed source on its own is accepted (FailedEntryCount 0, an EventId per entry) and really arrives at the rule's target. RemovePermission(StatementId=<sid>) restores the whole request: the identical two-entry batch is then fully accepted and BOTH entries are delivered.
Emulator: the Deny is stored and echoed by DescribeEventBus, but never enforced. The check polls PutEvents on the denied source for up to 180 s waiting for the call to be refused; it is accepted every time, so the check hangs to the harness cap ("Error: Check timed out after 60.0s") without ever producing an error code at all. The two downstream contracts — all-or-nothing refusal of the mixed batch, and non-delivery of both of its entries — are therefore also unmet, since an accepted PutEvents publishes both.
Why: this is over-permissive in the worst direction for a security control. A client that uses an event-bus resource policy as the boundary that keeps a source off a bus (a per-tenant or per-publisher Deny) gets events published anyway, with a 200 and an EventId telling it the Deny worked. There is no error code to key on, and because the emulator never refuses, the downstream guarantee AWS gives — a refused batch publishes NOTHING — can be violated silently with half a batch landing on the bus.
Fix: the PutEvents handler in the eventbridge service must evaluate the target bus's resource policy (including events:source and the other events: condition keys) before accepting any entry, and refuse the whole request with HTTP 400 AccessDeniedException and AWS's explicit-deny message rather than treating a resource-policy condition as a per-entry filter or ignoring it. The sibling open issue 20261025T002357Z-events-eventbridge-performs-no-authorization-event-bus-resource-policies.md reports the same absent-authorization family for checks under the 'events' token; it does not carry this check (the suite marked it neither owned nor filed), and specifically does not cover the all-or-nothing batch semantics or the RemovePermission restore leg graded here.
Traces: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch

## Evidence
From the behavioral suite replay (168/319 passed; events__sqs 10/12).

events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch [mismatch]:
  Error: Check timed out after 60.0s
  Sequence up to the hang: CreateEventBus -> PutRule {"source": [denied, allowed]} + SQS target -> delivery path warmed to two consecutive round trips (so the path WAS live) -> PutPermission with {Sid: Sim2realDenyOneSource, Effect: Deny, Principal: "*", Action: events:PutEvents, Resource: <bus arn>, Condition: {StringEquals: {"events:source": <denied source>}}} -> DescribeEventBus confirms the statement is in Policy -> _poll_until_denied() calls PutEvents on the denied source every 3 s for up to 180 s expecting a ClientError and getting an acceptance each time. Its timeout message states the defect exactly: "the explicit Deny on bus <bus> for source <source> never took effect ... PutEvents was still accepted ... The policy is stored (DescribeEventBus returns it) but not enforced, which is the over-permissive failure this check exists to catch." The expected steady state is code AccessDeniedException with message "User: <caller arn> is not authorized to perform: events:PutEvents on resource: <bus arn> with an explicit deny in a resource-based policy" (the arns are per-run values filling the template).

## Steps to Reproduce
1. CreateEventBus Name=<bus>; note EventBusArn.
2. Create an SQS queue, allow events.amazonaws.com sqs:SendMessage scoped to the rule ARN; PutRule EventPattern {"source": ["<denied>", "<allowed>"]} on <bus>; PutTargets the queue.
3. PutEvents on "<allowed>" until two consecutive events round-trip to the queue (prove the path live), then drain it.
4. PutPermission EventBusName=<bus> Policy = {"Version":"2012-10-17","Statement":[{"Sid":"Sim2realDenyOneSource","Effect":"Deny","Principal":"*","Action":"events:PutEvents","Resource":"<bus arn>","Condition":{"StringEquals":{"events:source":"<denied>"}}}]}. DescribeEventBus must show it.
5. PutEvents a single entry with Source="<denied>": expect HTTP 400 AccessDeniedException with message "User: <caller arn> is not authorized to perform: events:PutEvents on resource: <bus arn> with an explicit deny in a resource-based policy".
6. PutEvents a two-entry batch [Source="<denied>", Source="<allowed>"]: expect the SAME request-level AccessDeniedException and the same message, and verify NEITHER entry reaches the queue (use a separate positive control on "<allowed>" alone, which must be accepted with FailedEntryCount 0 and arrive, to prove the absence is suppression).
7. RemovePermission EventBusName=<bus> StatementId=Sim2realDenyOneSource, then re-issue the identical two-entry batch: expect FailedEntryCount 0 and BOTH entries delivered to the queue.
