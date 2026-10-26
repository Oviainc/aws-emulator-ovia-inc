Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse
Issue-Key: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse|run#27|AssertionError|assert overlap >= OVERLAP_FLOOR_S, (
Issue-Commit: 8fdd90c15790d38aa3c5d0088f99b03c79823121

# cloudformation__sns__sqs: stack provisioning has no timeline — every per-resource stack event carries one identical timestamp, so independent branches show zero overlap and reverse-order teardown is unobservable

> _Note: this issue was identified against the code base at commit `8fdd90c15790d38aa3c5d0088f99b03c79823121`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: a stack's per-resource events are all stamped at a single instant, so the provisioning timeline the dependency graph implies is not represented.

AWS: CloudFormation walks a stack's dependency graph and reports that walk through `DescribeStackEvents` (Smithy: `DescribeStackEvents` output `StackEvents`, paginated by `NextToken`). Each resource gets a `CREATE_IN_PROGRESS` event when it is SCHEDULED and a `CREATE_COMPLETE` event when it FINISHES, each with its own `Timestamp`, so the event log shows: two resources with no dependency between them in flight at the same time (their [scheduled, completed] intervals overlap), a `DependsOn`/`Ref` child scheduled only after its parent completed, a join scheduled only after ALL its predecessors completed, and on `DeleteStack` the reverse topological order (a dependent's `DELETE_COMPLETE` precedes the start of its dependency's delete). Resources that genuinely take time — e.g. an `AWS::CloudFormation::WaitCondition` held unsignalled — have a correspondingly long interval. The per-event `Timestamp` semantics are documented behavior, not a modeled constraint (recorded behavior only for the exact ordering guarantees).
Emulator: every per-resource event of one stack operation carries the same instant, so no interval has any duration and nothing overlaps. In the failing check the two independent gate resources, each held 20 seconds before being signalled, report scheduled times `{'GateA': 1792949528.959734, 'GateB': 1792949528.959737}` and completion times `{'GateA': 1792949528.959737, 'GateB': 1792949528.959738}` — microseconds apart — giving an observed overlap of 0.000s against a floor of 5.0s. The events exist and are per-resource (so this is NOT the already-filed 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md / 20261025T050651Z-cloudformation-records-no-per-resource-stack-events-a.md); what is missing is that each resource is recorded as starting and finishing at distinct times as it is actually provisioned.
Why: a client that reads `DescribeStackEvents` to see what a deploy is doing, how long a resource took, which resource is blocking, or in what order a teardown proceeded gets a degenerate log in which everything happened at once. Deployment dashboards, timing/SLO tooling and any test that asserts a resource waited for its dependency (or that a WaitCondition held the stack open) cannot work against it.
Fix: the event recording in the CloudFormation provisioning path — `services/service-com-amazonaws-cloudformation-cloudformation/src/state.rs` (the `provisioning` module and the pending-status clock; `STATUS_TRANSITION_DELAY` is defined at line 204 and the whole deployment settles at one instant) — must stamp each resource's `CREATE_IN_PROGRESS` when that resource is scheduled and its `CREATE_COMPLETE`/`DELETE_COMPLETE` when it finishes, in dependency order, rather than writing the whole resource set with the stack's single settle timestamp. `ops/describe_stack_events.rs` then reports them unchanged.
Traces: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse

## Evidence
Suite: simparity-behavioral-ga-prod-v1. Check `cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse` [mismatch]:

  AssertionError: GateA and GateB overlapped for only 0.000s (floor 5.0s, held 20.0s before signalling): sched={'GateA': 1792949528.959734, 'GateB': 1792949528.959737} done={'GateA': 1792949528.959737, 'GateB': 1792949528.959738}

Expected (from `DescribeStackEvents`): for the diamond template's two independent branch heads, `min(done) - max(sched) >= 5.0s` because each gate is held 20s before being signalled; each `DependsOn` child scheduled at or after its parent's completion; the join scheduled at or after the last of its predecessors; on delete, the join's `DELETE_COMPLETE` at or before the start of either branch's delete and the two branch deletes overlapping.
Actual: all four reported timestamps for the two gates fall inside a 4-microsecond window, i.e. every resource's `CREATE_IN_PROGRESS` and `CREATE_COMPLETE` event carries the stack's single settle instant, so overlap is 0.000s and no ordering or duration assertion over the event log can hold. The earlier assertions in the check confirm the events themselves are present and per-resource (both gates were observed `CREATE_IN_PROGRESS` in one poll) — only their timing is degenerate.

## Steps to Reproduce
Boot the emulator and replay `cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse` from suite simparity-behavioral-ga-prod-v1.

By hand: `CreateStack` with a diamond template — two independent `AWS::CloudFormation::WaitCondition` gates (GateA, GateB), an `AWS::SQS::Queue` under each via `DependsOn`, two SNS subscriptions, and a join resource that `DependsOn` both subtrees — hold ~20s before `SignalResource` on each gate, then read `DescribeStackEvents` and compare each resource's `CREATE_IN_PROGRESS` vs `CREATE_COMPLETE` `Timestamp`. All of them are the same instant. Repeat for `DeleteStack` and compare the `DELETE_*` timestamps.

## Resolution

A stack operation's per-resource events are no longer written at the instant the operation
happened to take the store lock. The walk a stack operation makes over the template's
dependency relation is now a planned TIMELINE
(`services/service-com-amazonaws-cloudformation-cloudformation/src/state/timeline.rs`): each
resource's `*_IN_PROGRESS` event falls due when the walk SCHEDULED it — once everything it
depends on has finished — and its terminal event when it FINISHED, and the stack's `settle`
records each of them at that instant. So the event log shows independent resources in flight
together, a `DependsOn`/`Ref` child only after its parent completed, a join only after the
last of its predecessors, and a deletion walking the same relation backwards. A wait
condition finishes when the signal that satisfies its `Count` arrives, so a gate held open
has an interval as long as the wait and nothing behind it is scheduled — or reported — until
then. The walk is laid out inside the span the stack's own clock already owed, so no
operation takes longer than it did. `ops/describe_stack_events.rs` reports the log unchanged.

Held to by `tests/all/stack_event_timeline.rs`.

A deployment an owning service REFUSES ends when it is refused, which is sooner than the
span a walk is laid out across, so the walk the refusal cut short is scaled into the span
the operation really took (`Timeline::compress_into`): it is reported in the order it
happened, inside the operation's own events, and never past the failure that ended it.

Still open, and NOT resolved by this change: the recorded check goes on past its overlap
assertion and then waits for a message published to one of the stack's SNS topics to arrive
in a subscribed SQS queue. `AWS::SNS::Subscription` and `AWS::SQS::QueuePolicy` are resource
types this emulator provisions nowhere, so the stack records them from the template alone and
no subscription exists to deliver through — a provisioning-coverage gap in
`src/state/provisioner.rs`, not a timeline one.
