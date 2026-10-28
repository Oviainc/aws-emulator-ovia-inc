Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# A reserved queue's ConcurrentJobs overrides its reservation plan, so ConcurrentJobs and ReservationPlan.ReservedSlots disagree

_Filed: 2026-10-28T16:07:15Z_
_Operation: CreateQueue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQueue` (and `UpdateQueue`, through the same helper) lets a caller-stated `ConcurrentJobs` win over the `ReservedSlots` of the `ReservationPlanSettings` stored in the SAME request, on a queue whose `PricingPlan` is `RESERVED`.

The model says the member does not apply to a reserved queue — the concurrency of a reserved queue is bought by its plan — and the crate's own helper documents exactly that, claiming the two projected members "can never disagree". The code breaks its own claim: `concurrent_jobs_for` gives the request's `ConcurrentJobs` priority over the plan's `ReservedSlots`, and nothing else re-reconciles them.

Concrete effect: `CreateQueue{PricingPlan:RESERVED, ReservationPlanSettings:{ReservedSlots:2,...}, ConcurrentJobs:5}` answers 201 with `queue.concurrentJobs == 5` and `queue.reservationPlan.reservedSlots == 2`, and `GetQueue`/`ListQueues` keep reporting that contradiction until some later call stores a new plan. Worse, the value is never measured against anything for a reserved queue — `check_concurrent_jobs` applies only its negative test when the pricing plan is `RESERVED` (state.rs:839) — so a reserved queue can be stored reporting a concurrency orders of magnitude above both its plan and the Region's pool. `UpdateQueue` has the same hole in its no-plan form: `UpdateQueue{ConcurrentJobs:999999}` on a reserved queue stores 999999 beside an unchanged plan.

## Evidence

Model (`smithy_query(query="doc",shape="CreateQueueRequest",member="ConcurrentJobs")`):
> "Specify the maximum number of jobs your queue can process concurrently. For on-demand queues, the value you enter is constrained by your service quotas ... **For reserved queues, specify the number of jobs you can process concurrently in your reservation plan instead.**"

and for the update (`shape="UpdateQueueRequest",member="ConcurrentJobs"`):
> "... **For reserved queues, update your reservation plan instead in order to increase your yearly commitment.**"

The crate states the same rule and asserts the invariant it then breaks —
services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:851-859:
> "`Queue$ConcurrentJobs` is \"the maximum number of jobs your queue can process concurrently\", and for a RESERVED queue the model points the caller at the plan rather than at the member ... So the slot count a plan reserves IS the concurrency of the queue it is stored on, which makes the rule one rule for every call that stores a plan: a call that stores one and states no `ConcurrentJobs` takes the queue's concurrency from that plan's `ReservedSlots`, **so the two members of one projected `Queue` can never disagree**."

But the code prefers the request's member unconditionally —
services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:864-872:
```rust
pub fn concurrent_jobs_for(
    requested: Option<i32>,
    settings: Option<&ReservationPlanSettings>,
    stored: i32,
) -> i32 {
    requested
        .or_else(|| settings.map(|settings| settings.reserved_slots))
        .unwrap_or(stored)
}
```

and both doors reach it with a plan beside a stated member. `CreateQueue` only refuses the (reserved, no-plan) and (on-demand, plan) pairings, never (reserved, plan, ConcurrentJobs) —
services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_queue.rs:204-218, then :299-310:
```rust
            concurrent_jobs: crate::state::concurrent_jobs_for(
                input.concurrent_jobs,
                input.reservation_plan_settings.as_ref(),
                crate::state::DEFAULT_CONCURRENT_JOBS,
            ),
            ...
            reservation_plan: input
                .reservation_plan_settings
                .as_ref()
                .map(|settings| crate::state::reservation_plan(settings, now)),
```
services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/update_queue.rs:333-337 is the same call with `record.concurrent_jobs` as the fallback.

The pool check cannot catch it either — services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:836-845:
```rust
    if requested < 0 { return Err(ConcurrencyRefusal::Negative(requested)); }
    if pricing_plan == PRICING_PLAN_ON_DEMAND && requested > available { ... }
```

Test coverage: the invariant is asserted, but only for requests that omit `ConcurrentJobs`.
- services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/queue_reserved_concurrency.rs:236-243 asserts "a reserved queue's concurrency must be the slot count its plan reserves" — but every row asserts only AFTER an `UpdateQueue` that stores a plan, and the one row that states both (:129-138) uses `concurrentJobs: 7` with `plan("EXPIRE", 7)`, so the two agree by coincidence.
- The create-time row that does state both, :122-128 `reserved_create(2, json!({ "concurrentJobs": 5 }))`, is never asserted at the create; only after the plan-9 update.
- services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/resource_projection_defaults.rs:169-184 quotes the model sentence ("For reserved queues, specify the number of jobs you can process concurrently in your reservation plan instead.") and asserts `concurrent_jobs: 8` for a plan of 8 slots — again with `ConcurrentJobs` omitted.
So no existing test pins the current result for the both-stated case; making the plan win leaves every quoted assertion satisfied.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. Call CreateQueue with Name="rq", PricingPlan="RESERVED", ReservationPlanSettings={Commitment:"ONE_YEAR", RenewalType:"EXPIRE", ReservedSlots:2} and ConcurrentJobs=5.
3. Observe HTTP 201 with queue.concurrentJobs == 5 while queue.reservationPlan.reservedSlots == 2 — the two members of one projected Queue disagree, which state.rs:859 says can never happen.
4. Call GetQueue(Name="rq") and ListQueues: both keep reporting the same contradiction.
5. Repeat with ConcurrentJobs=999999: the create still succeeds, because check_concurrent_jobs (state.rs:839) applies the pool test only to ON_DEMAND queues, so a reserved queue is stored reporting a concurrency far above both its plan and TOTAL_CONCURRENT_JOBS.
6. Same via update: CreateQueue a reserved queue with a 2-slot plan, then UpdateQueue(Name="rq", ConcurrentJobs=999999) with no reservationPlanSettings — concurrentJobs becomes 999999 while reservationPlan.reservedSlots stays 2.
