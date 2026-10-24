Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# UpdateQueue lets a reserved queue DECREASE its reserved transcode slots (and accepts any ConcurrentJobs value)

_Filed: 2026-10-24T13:28:46Z_
_Operation: UpdateQueue | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ReservationPlanSettings$ReservedSlots` documents a one-way rule: "Specifies the number of reserved transcode slots (RTS) for this queue... You can't decrease the number of RTS in your reserved queue. You can increase the number of RTS by extending your existing commitment with a new 12-month commitment for the larger number."

The emulator's UpdateQueue applies whatever ReservedSlots the request carries, in either direction, and restarts the 12-month term as it goes: a RESERVED queue created with reservedSlots=4 can be updated to reservedSlots=1 and answers HTTP 200 with reservationPlan.reservedSlots=1 and a fresh expiresAt. Expected: a decrease is refused with BadRequestException (400) and the stored plan is left exactly as it was; only an increase is applied (restarting the commitment, which the handler already does correctly).

The same handler also writes `ConcurrentJobs` and `MaximumConcurrentFeeds` straight onto the record with no bounds check, so an update can park a queue at a negative or pool-exceeding concurrency — the same over-permissiveness already filed against CreateQueue, which a fix on CreateQueue alone would leave reachable through this operation.

## Evidence

Smithy model: `ReservationPlanSettings$ReservedSlots` doc ("You can't decrease the number of RTS in your reserved queue..."); UpdateQueue declares BadRequestException (httpError 400) and takes ReservationPlanSettings / ConcurrentJobs / MaximumConcurrentFeeds. Handler: src/ops/update_queue.rs — the ONLY reservation check is `if record.pricing_plan != PRICING_PLAN_RESERVED { ...400... }`, after which it unconditionally does `record.reservation_plan = Some(crate::state::reservation_plan(settings, now))` (src/state.rs `reservation_plan` ignores the previous plan entirely); `record.concurrent_jobs = concurrent_jobs` and `record.maximum_concurrent_feeds = ...` are likewise unconditional. Test coverage: tests/all/queue_update.rs `a_reservation_plan_applies_only_to_a_reserved_queue` ENSHRINES the divergence — it creates a reserved queue with reservedSlots=2 and asserts that an UpdateQueue to reservedSlots=1 returns HTTP 200 and that GetQueue reports reservationPlan.reservedSlots=1, so that case must be revised with the fix.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_queue(Name='rq', PricingPlan='RESERVED', ReservationPlanSettings={'Commitment':'ONE_YEAR','RenewalType':'EXPIRE','ReservedSlots':4}).
3. update_queue(Name='rq', ReservationPlanSettings={'Commitment':'ONE_YEAR','RenewalType':'EXPIRE','ReservedSlots':1}).
4. Expect BadRequestException (400) and get_queue(Name='rq')['Queue']['ReservationPlan']['ReservedSlots'] still 4; observe HTTP 200 with ReservedSlots 1 and a refreshed ExpiresAt.
5. Positive control: the same call with ReservedSlots=8 must still succeed and restart the commitment.
6. Secondary: update_queue(Name=<an on-demand queue>, ConcurrentJobs=-5) — expect a 400, observe 200 with the negative value persisted and reported by ListQueues' UnallocatedConcurrentJobs arithmetic.
