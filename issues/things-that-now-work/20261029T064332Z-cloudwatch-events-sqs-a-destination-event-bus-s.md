Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied
Issue-Key: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied|run#48|AssertionError|assert leaked == [], (
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# cloudwatch__events__sqs: a destination event bus's resource-policy Deny is not enforced on a bus-to-bus forward — the forwarded event is delivered to the destination bus's target anyway (previously the forward delivered nothing at all)

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED by an open issue, so no existing work item carries it.

Status change worth stating plainly for the fixer: open issue 20261029T014025Z-cloudwatch-events-sqs-a-bus-to-bus-forward.md carries this same check id, but it records the EARLIER failure mode — the forward delivered nothing at all, so the check timed out at 60 s while merely establishing the delivery path, and its replay key names the `_drain#34|Timeout` frame. That part now works: the forward is established and events DO reach the destination bus's consumer. The check now progresses past establishment and fails on a different, later assertion, which is why its key no longer joins that issue. The remaining defect described below is the authorization half of the same contract and is net-new as an observation.

=== Cause 1 of 1: the destination bus's resource policy is never evaluated on a forwarded PutEvents, so an explicit Deny of the forwarding role still delivers the event ===

AWS:      Smithy: PutTargets.Targets -> Target{Arn required, RoleArn, DeadLetterConfig}; an event-bus target ARN is forwarded under the target's RoleArn, and the forward is itself a `PutEvents` against the destination bus. PutPermission/the bus resource policy is the authorization input for that call. The model says nothing about the outcome of a denied forward, so the outcome is recorded behavior only (AWS documents it under eb-event-bus-perms / eb-rule-dlq / eb-monitoring). Behavior: when the DESTINATION bus's resource policy denies the forwarding role `events:PutEvents`, the forward FAILS — the event does not appear on the destination bus and the destination bus's own rule therefore never matches and never delivers to its target. AWS records the refusal instead: the event is written to the forwarding target's DeadLetterConfig queue with ERROR_CODE = NO_PERMISSIONS; the source rule's AWS/Events TriggeredRules datapoint still counts the match (the rule DID trigger); and AWS/Events FailedInvocations for that rule stays 0 — a permissions refusal is not a failed invocation, and FailedInvocations is emitted only when non-zero, so "no datapoints" and "a zero datapoint" are distinct observations. A twin control rule on the SOURCE bus with an undeniable SQS target receives the same event at the same instant, so an absence at the destination is attributable to the Deny and not to rule warm-up.

Emulator: the Deny is accepted and stored but never consulted at forward time. After PutPermission denies the forwarding role `events:PutEvents` on the destination bus, a PutEvents on the source bus is still forwarded, still matched by the destination bus's rule, and still delivered to that rule's SQS target: the check's absence hold read 1 message from the destination consumer during a 10 s window (after the control confirmed the same event was live), and the message body is the probe event itself (`{"version":"0","id":"...","detail-type":"BusForwardProbe","source":...}`). Hypothesis: the bus-to-bus forwarding path publishes straight into the destination bus's event stream without running the destination bus's resource-policy evaluation that a direct PutEvents would run — the same authorization gap the sibling open issue 20261029T014025Z-events-sqs-an-explicit-deny-in-an-event.md reports for a DIRECT PutEvents (where a stored explicit Deny is likewise never enforced), here reached through a rule's event-bus target rather than from a client. That sibling issue is scoped to the `events__sqs` direct-call path and does not carry this check.

Why:      the destination bus's resource policy is the only control a bus owner has over who may forward into it. A client that scopes it deliberately — allowing one partner account's or one role's forwards and denying others — gets no enforcement at all: denied events land on the bus and fan out to every rule on it, which is a cross-tenant data leak, not merely a fidelity gap. The monitoring contract built on top is unobservable too: because the forward "succeeds", there is no NO_PERMISSIONS dead letter and no withheld FailedInvocations, so the exact signal AWS gives an operator to distinguish "the destination POLICY is wrong" from "the destination is down" never appears, and a mis-scoped policy is invisible in both the data path and the telemetry.

Fix:      the rule target-delivery path of the eventbridge service, where an event-bus target ARN is forwarded: the forward must be authorized against the DESTINATION bus's resource policy under the target's RoleArn, exactly as a direct PutEvents to that bus would be, and a refused forward must not reach the destination bus's event stream. The refusal accounting belongs with it — write the refused event to the target's DeadLetterConfig queue with ERROR_CODE NO_PERMISSIONS, publish the source rule's AWS/Events TriggeredRules datapoint, and withhold FailedInvocations (emit no datapoint) for a permissions refusal.

Traces:   cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied

## Evidence
cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied — verdict [mismatch], and this is the one of the three listed cloudwatch__events__sqs failures the suite marked neither ALREADY FILED nor OWNED.

Failure site: check_cloudwatch__events__sqs_failed_invocations_records_the_bus_forwarding_the_destination_policy_denied.py line 417, `assert leaked == [], (...)`.

Reported AssertionError, verbatim:
  "the destination bus's consumer received 1 message(s) during a 10s hold (after the control delivered) even though its bus policy denies events:PutEvents for arn:aws:iam::000000000000:role/sim2real-nd-fwd-role-b4f7f7648a: ['{\"version\":\"0\",\"id\":\"18e2ec8f-2c1f-8c52-0000-000000000006\",\"detail-type\":\"BusForwardProbe\",\"source\":']"

Expected vs actual:
  Expected (AWS): during the 10 s absence hold after the Deny, the destination bus's consumer queue receives ZERO messages (leaked == []); the DeadLetterConfig queue receives the event with ERROR_CODE = NO_PERMISSIONS; AWS/Events TriggeredRules for the forwarding rule has a non-zero Sum; AWS/Events FailedInvocations for that rule sums to 0 and emits no datapoint.
  Actual (emulator): the destination consumer received 1 message, whose body is the forwarded probe event itself.

Why this is attributable to the Deny and not to timing: the hold is entered only AFTER the check's twin control rule on the source bus (undeniable SQS target) confirmed the same event was delivered, so the delivery path was demonstrably live and the read is of the post-Deny state. The role named in the message is the forwarding role the destination bus policy denies — the identity whose forward must have been refused. The role ARN, the queue names and the event id in that line are per-run values; the requirement is that the count be 0 for ANY role the destination bus policy denies.

Progress relative to the earlier filing (20261029T014025Z-cloudwatch-events-sqs-a-bus-to-bus-forward.md, Issue-Key ...|_drain#34|Timeout|resp = sqs.receive_message(): that run could not get past `_emit_until_delivered()` establishing the forward, and reported only "Error: Check timed out after 60.0s". This run reaches line 417, which means PutRule/PutTargets on both buses, the event-bus target forward, the destination rule's SQS delivery and the control delivery all now work. The residual failure is purely the unenforced destination-bus Deny.

The three downstream observations (NO_PERMISSIONS dead letter, TriggeredRules > 0, FailedInvocations summing to 0 with no datapoint) are still unverified: the leak assertion fails first, so they were not reached. They are part of the same cause and must be closed with it.

## Steps to Reproduce
1. CreateEventBus <src>; CreateEventBus <dst> (note <dst>'s EventBusArn).
2. Create three SQS queues (control, destination consumer, DLQ), each with a policy allowing Principal {Service: events.amazonaws.com} sqs:SendMessage with Condition ArnLike aws:SourceArn = arn:*:events:<region>:<account>:rule/*.
3. On <dst>: PutRule with pattern {"source": ["sim2real.nd.busdeny"], "detail-type": ["BusForwardProbe"]}, State=ENABLED; PutTargets the destination consumer queue.
4. On <src>: PutRule with the same pattern; PutTargets the control queue (the twin control).
5. Create an IAM role trusting events.amazonaws.com with an inline policy allowing events:PutEvents on <dst>'s ARN. On <src>: PutRule with the same pattern; PutTargets {Arn: <dst>'s bus ARN, RoleArn: that role, DeadLetterConfig: {Arn: the DLQ ARN}}.
6. PutEvents on <src> with that source/detail-type, re-emitting a fresh nonce until one reaches the destination consumer queue — this establishes the forward (a fresh rule drops events until live and a drop is never retried). This step now succeeds against the emulator.
7. Call PutPermission on <dst> with an explicit Deny of events:PutEvents for the forwarding role created in step 5.
8. PutEvents on <src> again with the same source/detail-type, then for 10 s repeatedly ReceiveMessage from the destination consumer queue.
   Observed: the destination consumer receives the forwarded event (1 message, body = the event envelope).
   Expected (AWS): the destination consumer receives NOTHING; the control queue on <src> still receives the event (proving the emission was live); the DLQ receives the event with ERROR_CODE = NO_PERMISSIONS; GetMetricStatistics on AWS/Events TriggeredRules for the forwarding rule shows a non-zero Sum; GetMetricStatistics on AWS/Events FailedInvocations for that rule sums to 0 and returns no datapoints.

Replay shortcut: re-run the check named in trace_ids.
