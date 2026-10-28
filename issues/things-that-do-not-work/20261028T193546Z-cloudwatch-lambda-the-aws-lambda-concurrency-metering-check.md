Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity
Issue-Key: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity|_poll_until#26|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# cloudwatch__lambda: the AWS/Lambda concurrency-metering check is cut short before its data plane — ConcurrentExecutions/Invocations/Throttles and the account-level UnreservedConcurrentExecutions/ClaimedAccountConcurrency series are never published

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 1: the AWS/Lambda concurrency metering a reserved burst must publish does not exist in this emulator, and the concurrency-metering check is cut short in its pre-burst setup, so not one of the five series it reads is ever observable.

RELATED OPEN ISSUE (named, not a duplicate claim): issues/things-that-do-not-work/20261027T120000Z-lambda-cloudwatch-aws-lambda-concurrency-metering-is-never-published.md carries this same check id in its Issue-Trace-Ids, but its Issue-Key records the OLD failure position (`_poll_until#24|Timeout|time.sleep(interval_s)` — a harness timeout inside the readiness poll). This replay no longer reproduces that key: the check now fails with an AssertionError raised by `_poll_until` at the wall-clock wait that precedes the burst, so the replay report did not mark this check OWNED and its replay key is not carried by any open issue. This report re-grounds the cause on the key that is actually reproducible today.

AWS: recorded behavior only — the Smithy models describe neither metric publication nor allocation-state timing. Recorded AWS behavior: PutProvisionedConcurrencyConfig on an alias is accepted 202 with Status IN_PROGRESS and GetProvisionedConcurrencyConfig moves to READY for a valid, in-budget allocation (never stuck IN_PROGRESS, never FAILED). Lambda then publishes, off the response path, the AWS/Lambda series this check reads with cloudwatch GetMetricData at Period 60: ConcurrentExecutions, Invocations and Throttles dimensioned by FunctionName, plus the DIMENSIONLESS account-level series UnreservedConcurrentExecutions and ClaimedAccountConcurrency. Two recorded properties are the substance of it: a reserved function's concurrent executions are charged to its reservation and NOT to the unreserved pool (so UnreservedConcurrentExecutions tracks only the unreserved function's burst), and at one timestamp ClaimedAccountConcurrency minus UnreservedConcurrentExecutions equals the account's allocated concurrency as Lambda itself lists it (each function's reserved concurrency, or for a function with none the sum of its provisioned allocations). A RequestResponse Invoke of a python3.12 deployment package runs the package and returns its JSON result with StatusCode 200 and no FunctionError.

Emulator: the check never reaches a single metric assertion. It fails in `run` at the wall-clock wait that must be satisfied before the burst, with `AssertionError: the wall clock entering the first minute wholly after the allocations did not happen within <remaining-budget>s` — the budget granted to that wait is the runner's remaining time less 10 s, so by the time the emulator-facing setup (GetAccountSettings, iam CreateRole/PutRolePolicy, three CreateFunction + function_active_v2 waits, PutFunctionConcurrency/GetFunctionConcurrency, PublishVersion, CreateAlias, PutProvisionedConcurrencyConfig, the GetProvisionedConcurrencyConfig readiness poll, and the ListFunctions/GetFunctionConcurrency/ListProvisionedConcurrencyConfigs sweep that lists allocated concurrency) has run, less wall clock is left than the up-to-62 s the next whole minute boundary costs. Everything downstream of that point is therefore unexercised, and independently broken: the 4+3-way RequestResponse burst cannot run at all, because Invoke refuses every real deployment package with HTTP 501 NotImplemented, and no lambda handler or state path writes into the CloudWatch metric store at all — nothing in the lambda crate names AWS/Lambda, ConcurrentExecutions, UnreservedConcurrentExecutions or ClaimedAccountConcurrency — so GetMetricData answers every one of the five queries with an empty Timestamps/Values pair. The account-level half (ClaimedAccountConcurrency and UnreservedConcurrentExecutions) is computable from the lambda store today, with no runtime behind it, and is still published nowhere. Hypothesis (neither the model nor the recording states this): the wall-clock shortfall is the pre-burst setup consuming the check's allowance, with the fixed 10 s poll interval of the provisioned-concurrency readiness wait and the per-function allocated-concurrency sweep as the two largest terms; the emulator's own PROVISIONED_CONCURRENCY_ALLOCATION_DELAY is only 500 ms (services/service-com-amazonaws-lambda-awsgirapiservice/src/state.rs), so the allocation itself is not the span.

Why: a client relying on AWS's behavior gets no concurrency observability whatsoever. An operator cannot alarm on account concurrency headroom (UnreservedConcurrentExecutions), cannot see what the account has committed (ClaimedAccountConcurrency), and cannot distinguish a function that is being throttled from one that is idle (ConcurrentExecutions/Throttles under a reservation). Every capacity-planning dashboard and every deployment gate built on those series reads an empty series forever rather than failing loudly, so automation concludes there is unlimited headroom and no throttling.

Fix: the lambda service. The concurrency metering belongs in the lambda crate's state/metering path (services/service-com-amazonaws-lambda-awsgirapiservice/src/state.rs and the concurrency handlers put_function_concurrency.rs / put_provisioned_concurrency_config.rs), which must publish the AWS/Lambda namespace into the CloudWatch metric store so cloudwatch GetMetricData can read it: ConcurrentExecutions/Invocations/Throttles dimensioned by FunctionName, charging a reserved function's executions to its reservation and not to the unreserved pool, and the dimensionless UnreservedConcurrentExecutions and ClaimedAccountConcurrency, with ClaimedAccountConcurrency including reserved plus provisioned allocations. The function-level half additionally depends on Invoke actually running a deployment package (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs), tracked by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md — metering published with no run behind it would be a datapoint for work nothing performed. The account-level half does not wait on that and can be published from the store as it stands.

Traces: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity (verdict: mismatch).

## Evidence
Failing check: cloudwatch__lambda.obs-lambda-1-reserved-burst-meters-function-concurrency-and-claimed-account-identity — verdict [mismatch].

Replay output, verbatim (the numeric budget and the datetime are per-run values, not requirements; the fixed text is the description string and the `did not happen within ...s; last read ...` frame):

    File "check_cloudwatch__lambda_obs_lambda_1_reserved_burst_meters_function_concurrency_and_claimed_account_identity.py", line 128, in run
      _poll_until(
    File "...", line 420, in _poll_until
      raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
    AssertionError: the wall clock entering the first minute wholly after the allocations did not happen within 33.67259131900005s; last read datetime.datetime(2026, 10, 28, 19, 18, 47, 616668, tzinfo=datetime.timezone.utc)

Expected (AWS): the setup completes, the burst of 4 RequestResponse invokes of the reserved function (reservation 6) and 3 of the unreserved function overlap, and cloudwatch GetMetricData at Period 60 over the burst minute returns, for AWS/Lambda:
  - ConcurrentExecutions Maximum, Dimensions FunctionName=<reserved fn> == 4 exactly
  - Invocations Sum, Dimensions FunctionName=<reserved fn> == 4
  - Throttles Sum, Dimensions FunctionName=<reserved fn> — no datapoint (4 invokes under a reservation of 6)
  - UnreservedConcurrentExecutions Maximum (no dimensions) >= 3 (only the unreserved function's burst is charged to the pool)
  - ClaimedAccountConcurrency Maximum (no dimensions) minus UnreservedConcurrentExecutions at that same timestamp == the allocated concurrency Lambda itself lists (>= reserved 6 + provisioned 1)

Actual (emulator):
  - the check is cut short in its pre-burst setup and never issues the burst or the GetMetricData calls;
  - the burst itself cannot succeed: lambda Invoke refuses a real python3.12 deployment package with HTTP 501 `NotImplemented` (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs), where AWS answers 200 with the handler's JSON result;
  - all five AWS/Lambda series are empty — the lambda crate publishes nothing into the CloudWatch metric store, so GetMetricData returns empty Timestamps/Values for each MetricDataQuery instead of the datapoints above.

Supporting emulator facts read from the checkout: PutProvisionedConcurrencyConfig stores the allocation IN_PROGRESS with a 500 ms due instant (state.rs: PROVISIONED_CONCURRENCY_ALLOCATION_DELAY = 500 ms) and GetProvisionedConcurrencyConfig materialises READY, so the readiness poll does terminate — the stall is not the allocation state.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. lambda: GetAccountSettings — note AccountLimit.UnreservedConcurrentExecutions.
3. iam: CreateRole trusting lambda.amazonaws.com, with an inline policy allowing only cloudwatch:PutDashboard and cloudwatch:ListDashboards.
4. lambda: CreateFunction three times (python3.12, handler.handler, 128 MB, Timeout 60) with a handler that registers a dashboard, waits for the whole wave to register, then sleeps for a payload-supplied sleep_ms; wait function_active_v2 on each. Call them A (reserved), B (unreserved), C (provisioned).
5. lambda: PutFunctionConcurrency(FunctionName=A, ReservedConcurrentExecutions=6); GetFunctionConcurrency(A) must read back 6.
6. lambda: PublishVersion(C), CreateAlias(C, 'warm', that version), PutProvisionedConcurrencyConfig(FunctionName=C, Qualifier='warm', ProvisionedConcurrentExecutions=1); poll GetProvisionedConcurrencyConfig(C,'warm') until Status is READY or FAILED. Expected READY — this does happen today.
7. Wait for the next whole minute boundary, then invoke A 4 times and B 3 times concurrently with sleep_ms=10000 so the bursts fully overlap.
   Expected: every invoke returns StatusCode 200 with no FunctionError and the handler's JSON body.
   Observed: each RequestResponse invocation is refused HTTP 501 NotImplemented, so nothing runs and nothing is metered. (In the replayed run the check does not even reach this step: the wall-clock wait for the minute boundary is granted less time than it needs and raises `AssertionError: the wall clock entering the first minute wholly after the allocations did not happen within <n>s`.)
8. cloudwatch: GetMetricData at Period 60 over that minute, namespace AWS/Lambda, for ConcurrentExecutions Maximum / Invocations Sum / Throttles Sum dimensioned FunctionName=A, and for the DIMENSIONLESS UnreservedConcurrentExecutions Maximum and ClaimedAccountConcurrency Maximum.
   Expected: ConcurrentExecutions Maximum == 4, Invocations Sum == 4, no Throttles datapoint, UnreservedConcurrentExecutions Maximum >= 3, and ClaimedAccountConcurrency - UnreservedConcurrentExecutions at the same timestamp == the allocated concurrency listed from lambda (>= 7).
   Observed: every one of those five results comes back with empty Timestamps/Values — nothing publishes AWS/Lambda metrics.
