Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# UpdateQueue replaces a reserved queue's reservation plan but leaves ConcurrentJobs at the old slot count

_Filed: 2026-10-25T04:07:54Z_
_Operation: UpdateQueue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a RESERVED queue the model says the concurrency the queue reports comes from its reservation plan, and `CreateQueue` implements exactly that: when the request omits `ConcurrentJobs`, the stored `concurrent_jobs` is taken from `ReservationPlanSettings.ReservedSlots`.

`UpdateQueue` replaces the whole reservation plan (`record.reservation_plan = Some(reservation_plan(settings, now))`) when the request carries `ReservationPlanSettings`, but it only ever writes `concurrent_jobs` from the request's own `ConcurrentJobs` member. So adding capacity to a reserved queue — the case the member documents ("When you add capacity to your queue by increasing the number of RTS, you extend the term of your commitment") — leaves the queue reporting the PREVIOUS slot count as its `ConcurrentJobs`, while `ReservationPlan.ReservedSlots` reports the new one. The two members of the same projected `Queue` then contradict each other, and they stay contradictory for every later `GetQueue`/`ListQueues` read because the stale value is persisted. `ListQueues`' `UnallocatedConcurrentJobs` arithmetic is unaffected (it sums ON_DEMAND queues only), so the only channel that exposes the real reserved concurrency is the one left stale.

Note this is NOT the already-open `UpdateQueue` validation issue (decreasing ReservedSlots): the defect here is present on a perfectly legal INCREASE that the handler accepts and applies.

## Evidence

Smithy model, `CreateQueueRequest$ConcurrentJobs`: "Specify the maximum number of jobs your queue can process concurrently. ... For reserved queues, specify the number of jobs you can process concurrently in your reservation plan instead." and `UpdateQueueRequest$ConcurrentJobs`: "... For reserved queues, update your reservation plan instead in order to increase your yearly commitment." `Queue$ConcurrentJobs`: "The maximum number of jobs your queue can process concurrently."

Sibling handler applies the rule — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_queue.rs:227-240:
```
// "For reserved queues, specify the number of jobs you can
// process concurrently in your reservation plan instead" — so a reserved
// queue's concurrency is the slot count it reserved, and an on-demand one's
// is the account default.
concurrent_jobs: input.concurrent_jobs.unwrap_or_else(|| {
    input.reservation_plan_settings.as_ref()
        .map_or(crate::state::DEFAULT_CONCURRENT_JOBS, |settings| settings.reserved_slots)
}),
```

This handler skips it — src/ops/update_queue.rs:213-225:
```
if let Some(settings) = input.reservation_plan_settings.as_ref() {
    ...
    record.reservation_plan = Some(crate::state::reservation_plan(settings, now));
}
```
and the only write of the concurrency, src/ops/update_queue.rs:246-248:
```
if let Some(concurrent_jobs) = input.concurrent_jobs {
    record.concurrent_jobs = concurrent_jobs;
}
```
`crate::state::reservation_plan` (src/state.rs:1615-1627) builds the new plan from `settings.reserved_slots` and touches nothing else on the record.

Test coverage: tests/all/queue_update.rs:401-461 `a_reservation_plan_applies_only_to_a_reserved_queue` updates a reserved queue created with `reservedSlots: 2` to `reservedSlots: 4` and asserts ONLY `/queue/reservationPlan/{commitment,renewalType,reservedSlots}` — nothing asserts `/queue/concurrentJobs`, so no existing test pins the stale value.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_queue(Name='rq', PricingPlan='RESERVED', ReservationPlanSettings={'Commitment':'ONE_YEAR','RenewalType':'EXPIRE','ReservedSlots':2}) — the response reports Queue.ConcurrentJobs = 2.
3. update_queue(Name='rq', ReservationPlanSettings={'Commitment':'ONE_YEAR','RenewalType':'EXPIRE','ReservedSlots':8}) — an increase, which the handler accepts with HTTP 200.
4. Read the response (and then get_queue(Name='rq')): Queue.ReservationPlan.ReservedSlots is 8 while Queue.ConcurrentJobs is still 2.
5. Expected: the reserved queue's ConcurrentJobs tracks the reservation plan it was just given (8), as CreateQueue already derives it from ReservedSlots.
