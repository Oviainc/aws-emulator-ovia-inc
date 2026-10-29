Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity
Issue-Key: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# lambda/cloudwatch: the AWS/Lambda concurrency metering a reserved burst should publish is never produced, so the concurrency-metering check never reaches an assertion

> _Split out of `issues/things-that-now-work/20261025T192750Z-cloudwatch-2-net-new-root-causes-putmanagedinsightrules-accepts.md` (its cause 2 of 2), whose cause 1 — `PutManagedInsightRules` accepting an unresolvable `TemplateName` — is fixed. This report carries the cause that is NOT fixed, so it stays filed under the work that does not work yet. The text below is that cause as it was filed, plus what is now verified about it._

## Operation
GetMetricData (cloudwatch), PutProvisionedConcurrencyConfig / GetProvisionedConcurrencyConfig / Invoke (lambda)

## Description
The AWS/Lambda concurrency-metering check never reaches an assertion — it is killed at the suite's 60 s cap while still inside a pre-invoke poll, so neither the provisioned-concurrency allocation nor the account-level concurrency series can be observed.

Failing check id: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity.

AWS: recorded behavior only — the Smithy models do not describe metric publication or allocation-state timing. Recorded: PutProvisionedConcurrencyConfig on an alias leaves GetProvisionedConcurrencyConfig reporting Status IN_PROGRESS and then READY (never stuck IN_PROGRESS, never FAILED for a valid, in-budget allocation). Lambda itself then publishes, off the response path, the AWS/Lambda series this check reads with GetMetricData at Period 60: ConcurrentExecutions dimensioned by FunctionName, Invocations and Throttles dimensioned by FunctionName, and the DIMENSIONLESS account-level series UnreservedConcurrentExecutions and ClaimedAccountConcurrency. Two recorded properties are the substance of the check: a reserved function's concurrent executions are charged to its reservation and NOT to the unreserved pool (so UnreservedConcurrentExecutions tracks only the unreserved function's burst), and at the same timestamp ClaimedAccountConcurrency minus UnreservedConcurrentExecutions equals the account's allocated concurrency as Lambda itself lists it (each function's reserved concurrency, or for a function with none the sum of its provisioned allocations) — which includes this check's reserved 6 plus provisioned 1.

Emulator: the check is killed with "Error: Check timed out after 60.0s" before any assertion in the metric phase is reached, so none of the above is observable. The calls it makes before that first assertion, in order, are lambda GetAccountSettings, iam CreateRole, three CreateFunction + function_active_v2 waits, PutFunctionConcurrency/GetFunctionConcurrency, PublishVersion, CreateAlias, PutProvisionedConcurrencyConfig, and then a 420 s-budget poll of GetProvisionedConcurrencyConfig for Status in (READY, FAILED). The check's own declared ceilings are _PC_READY_BUDGET_S=420.0 for that readiness poll, _ROLE_PROPAGATION_BUDGET_S=120.0 for CreateFunction, and _METRIC_BUDGET_S=300.0 for the GetMetricData poll, so it was killed inside the first of those it could not satisfy; every assertion in the metric phase is downstream of it.

Why: an operator cannot tell when a provisioned-concurrency allocation is actually serving, and cannot alarm on account concurrency headroom at all. Every capacity-planning dashboard built on UnreservedConcurrentExecutions / ClaimedAccountConcurrency, and every deployment gate that waits for a provisioned-concurrency config to report READY before shifting traffic, blocks or reads an empty series forever.

Fix: the component is the lambda service: the concurrency metering that publishes the AWS/Lambda ConcurrentExecutions, UnreservedConcurrentExecutions and ClaimedAccountConcurrency series must charge reserved executions to the reservation and include reserved plus provisioned allocations in ClaimedAccountConcurrency, so cloudwatch GetMetricData can read them.

## Current state, verified 2026-10-27

- The stall this cause hypothesised — "the allocation never leaves IN_PROGRESS" — is NOT the stall any more. `PutProvisionedConcurrencyConfig` stores the allocation IN_PROGRESS with a due instant and `GetProvisionedConcurrencyConfig` materialises the move to READY (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/put_provisioned_concurrency_config.rs`, `.../get_provisioned_concurrency_config.rs`), covered by that crate's `tests/all/provisioned_concurrency.rs` and `tests/all/provisioned_concurrency_resolution.rs`. The readiness poll terminates.
- What blocks the rest: every observable the metric phase reads — `ConcurrentExecutions` Maximum for the reserved function across its 4-way burst, `Invocations` Sum, the absence of a `Throttles` datapoint under a reservation of 6, and `UnreservedConcurrentExecutions` tracking only the unreserved function's 3-way burst — exists only if the three python3.12 deployment packages actually run, concurrently, for the ten seconds the check's payload asks for. `lambda` `Invoke(RequestResponse)` cannot run a deployment package in this emulator: `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs:521` refuses it with `NotImplemented`, because that crate carries no in-process interpreter (its `src/` holds only `lib.rs`, `scaffolded.rs`, `service.rs`, `state.rs`). That refusal is tracked by `issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`, and this cause unblocks with it: metering published with no run behind it would be a datapoint for work nothing performed.
- The account-level half (ClaimedAccountConcurrency = each function's reserved concurrency, or the sum of its provisioned allocations for a function with none) is computable from the lambda store today and is the part of this cause that does not wait on the runtime, but it is published nowhere: no lambda handler or state path writes into the CloudWatch metric store (nothing in that crate names AWS/Lambda, ConcurrentExecutions or ClaimedAccountConcurrency).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. iam: CreateRole trusting lambda.amazonaws.com. lambda: CreateFunction three times (python3.12, 128 MB, timeout 30) with a handler that sleeps for a payload-supplied sleep_ms; wait function_active_v2 on each.
3. lambda: PutFunctionConcurrency(FunctionName=<A>, ReservedConcurrentExecutions=6); GetFunctionConcurrency(<A>) must read back 6.
4. lambda: PublishVersion(<C>), CreateAlias(<C>, 'warm', that version), PutProvisionedConcurrencyConfig(FunctionName=<C>, Qualifier='warm', ProvisionedConcurrentExecutions=1).
5. Poll lambda GetProvisionedConcurrencyConfig(<C>, 'warm') until Status is READY or FAILED. Expected: it reaches READY — which it now does.
6. Wait for the next whole minute boundary, then invoke <A> 4 times and the unreserved <B> 3 times concurrently with sleep_ms=10000 so the bursts fully overlap. Observed: each RequestResponse invocation is refused HTTP 501 NotImplemented, so nothing runs and nothing is metered.
7. cloudwatch: GetMetricData at Period 60 over that minute for AWS/Lambda ConcurrentExecutions Maximum and Invocations Sum and Throttles Sum dimensioned FunctionName=<A>, and for the DIMENSIONLESS UnreservedConcurrentExecutions Maximum and ClaimedAccountConcurrency Maximum.
   Expected: ConcurrentExecutions Maximum for <A> == 4 exactly, Invocations Sum == 4, no Throttles datapoint; UnreservedConcurrentExecutions Maximum >= 3 (only <B>'s burst is charged to the unreserved pool); and ClaimedAccountConcurrency - UnreservedConcurrentExecutions at the same timestamp == the allocated concurrency listed from lambda (>= reserved 6 + provisioned 1).
   Observed: every one of those series is empty — nothing publishes AWS/Lambda metrics.
