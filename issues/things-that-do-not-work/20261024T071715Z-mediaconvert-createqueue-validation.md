Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateQueue stores any ConcurrentJobs value, including a negative one and one far beyond the account's concurrent-job pool

_Filed: 2026-10-24T07:17:15Z_
_Operation: CreateQueue | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ConcurrentJobs` is a bare `__integer` in the model (no @range), so the framework's boundary validator accepts any i32 and the handler applies no check of its own: ops/create_queue.rs writes `concurrent_jobs: input.concurrent_jobs.unwrap_or(...)` straight into the QueueRecord, and ops/update_queue.rs does the same on update.

Expected: the member's own documentation constrains it — 'For on-demand queues, the value you enter is constrained by your service quotas for Maximum concurrent jobs, per on-demand queue and Maximum concurrent jobs, per account' — so a request asking for a negative allocation, or for more concurrent jobs than the Region's unallocated pool (`ListQueues$UnallocatedConcurrentJobs`, 'the remaining number of concurrent jobs that are not associated with a queue and are available to allocate to a queue') has a problem in the request and is refused with BadRequestException (400) / ServiceQuotaExceededException (402), leaving no queue stored.

Actual: `create_queue(Name='q', ConcurrentJobs=-5)` answers 201 and GetQueue/ListQueues then report `concurrentJobs: -5`; `create_queue(Name='q2', ConcurrentJobs=1000000)` answers 201 and ListQueues reports `unallocatedConcurrentJobs: 0` (state::concurrency_allocation clamps the subtraction instead of refusing the allocation), so the account's pool accounting silently goes inconsistent and a client validating its capacity planning against the emulator sees a success AWS would refuse.

## Evidence

Spec: smithy_query(operation="CreateQueue") — `ConcurrentJobs` is `__integer` with no range, documented as constrained by the account/per-queue concurrent-job quotas; `ListQueuesResponse$TotalConcurrentJobs` / `$UnallocatedConcurrentJobs` document the pool this allocation draws from, and CreateQueue models both BadRequestException (400) and ServiceQuotaExceededException (402). Handler: services/.../src/ops/create_queue.rs (no bound check on `concurrent_jobs`; only name emptiness and the pricing-plan/reservation pairing are validated) and src/ops/update_queue.rs (`record.concurrent_jobs = concurrent_jobs` unconditionally). State: src/state.rs `concurrency_allocation()` clamps `(ceiling - allocated)` into `0..=ceiling`, i.e. the over-allocation is absorbed rather than refused. Test coverage: tests/all/queue_update.rs and resource_projection_defaults.rs only exercise positive in-range values (1, 3, 7, 150); no test asserts the acceptance of a negative or pool-exceeding value, so nothing enshrines the current behaviour.

## Steps to Reproduce

1. Boot the emulator and point a botocore mediaconvert client at it.
2. create_queue(Name='q-negative', ConcurrentJobs=-5) -> expect BadRequestException (400) and no queue stored; observe HTTP 201.
3. get_queue(Name='q-negative') -> observe Queue.ConcurrentJobs == -5, a value AWS would never report.
4. create_queue(Name='q-huge', ConcurrentJobs=1000000) -> expect a refusal (400/402) because the allocation exceeds the Region's unallocated concurrent jobs; observe HTTP 201.
5. list_queues() -> observe unallocatedConcurrentJobs == 0 while the sum of the queues' concurrentJobs vastly exceeds totalConcurrentJobs.
