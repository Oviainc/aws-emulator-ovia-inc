Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods|_sleep_until#14|Timeout|time.sleep(min(remaining, 5.0))
Issue-Commit: d566282976cd3ecbd638dcfb7eabea66136f946a

# xray: GetTimeSeriesServiceStatistics period bucketing is ungradable because the account trace-segment-destination singleton never settles to ACTIVE (1 cause)

> _Note: this issue was identified against the code base at commit `d566282976cd3ecbd638dcfb7eabea66136f946a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSegmentDestination

## Description
Cause 1 of 1: the account-wide trace-segment-destination singleton never reaches its settled status, so every caller that gates on it blocks forever — and that is exactly where the Period=60 bucketing check for GetTimeSeriesServiceStatistics is stopped, before it ever submits a segment. Failing check: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods.

AWS: GetTraceSegmentDestination (Smithy xray, POST /GetTraceSegmentDestination -> 200 {Destination: TraceSegmentDestination, Status: TraceSegmentDestinationStatus}; TraceSegmentDestinationStatus enum = [PENDING, ACTIVE]) reports Status=ACTIVE for an account that is settled on its destination. PENDING is the transitional value of an in-flight UpdateTraceSegmentDestination only; an account that has not been switched, or whose switch has completed, reports ACTIVE. That is the only terminal state a client can wait on — AWS exposes no other completion signal for the switch, which is why the documented enablement flow is "call UpdateTraceSegmentDestination, then poll GetTraceSegmentDestination until Status==ACTIVE". Downstream, GetTimeSeriesServiceStatistics (Smithy: POST /TimeSeriesServiceStatistics, required StartTime/EndTime, optional Period/EntitySelectorExpression @length 1..500 -> TimeSeriesServiceStatistics[]{Timestamp, ServiceSummaryStatistics: ServiceStatistics{TotalCount, OkCount, ErrorStatistics, FaultStatistics, TotalResponseTime}}) buckets submitted segments by Period: two PutTraceSegments calls for one service name made more than Period seconds apart in wall-clock time must occupy two distinct datapoints, each with ServiceSummaryStatistics.TotalCount=1 and OkCount=1 (clean 2xx segment, no error/fault member) and ErrorStatistics.TotalCount=FaultStatistics.TotalCount=0, with datapoint Timestamps inside the requested window and about as far apart as the two submissions; an EntitySelectorExpression naming a service that was never submitted returns no datapoint carrying ServiceSummaryStatistics (recorded behavior: periods with unrelated account traffic may return placeholder datapoints with no ServiceSummaryStatistics member).

Emulator: the check is killed with 'Check timed out after 60.0s' without reaching a single assertion. Its first action is the account-singleton pin, which calls GetTraceSegmentDestination and loops (10 s apart, up to 600 s) until Status==ACTIVE. The emulator's settled account answers Status='PENDING' instead of 'ACTIVE' — the same response already recorded on the wire corpus for this operation (see Cause 5 of open issue 20261022T142349Z-xray-x-ray-group-and-trace-segment-destination-responses.md: `$.Status: value_mismatch (expected 'ACTIVE', got 'PENDING')`) — so the predicate is never satisfiable and the helper spins until the harness cap fires. Consequently nothing about the bucketing surface is exercised: no PutTraceSegments, no GetTimeSeriesServiceStatistics call, no datapoint, no EntitySelectorExpression filtering. A second timing-out check in this suite, xray.telemetry-only-host-absent-from-service-graph, opens with the same pin and dies the same way (it is reported OWNED by another issue for its own assertion, so it is not bundled here). Note for the fixer: this check also spaces its two submissions 120 s apart by design, which is longer than the harness's 60 s cap, so after the PENDING->ACTIVE defect is fixed the check still cannot run to completion under that cap — the bucketing assertions have to be re-confirmed by driving the sequence manually (steps below).

Why: a client cannot complete the Transaction Search / trace-destination enablement flow at all. Every automation that polls GetTraceSegmentDestination for Status==ACTIVE before instrumenting — SDK waiters, CDK/CFN custom resources, the console's enablement wizard — deadlocks instead of proceeding, and because PENDING is advertised as transitional the client cannot tell this apart from a switch that is genuinely still in flight and will therefore keep waiting rather than erroring. Secondarily, the whole GetTimeSeriesServiceStatistics aggregation surface (period bucketing, per-bucket Ok/Error/Fault counts, entity-selector scoping) is unverifiable behind this block, so a client building latency/error dashboards on it gets no signal that it works or not.

Fix: the trace-segment-destination state machine behind GetTraceSegmentDestination in the X-Ray service's account-config state — the steady-state default must be reported ACTIVE, and an accepted UpdateTraceSegmentDestination must transition PENDING -> ACTIVE once applied rather than leaving the singleton in the transitional value. Once that unblocks, the GetTimeSeriesServiceStatistics handler and its aggregation state need to bucket accepted segments by submission time into Period-wide datapoints carrying ServiceSummaryStatistics and to honour EntitySelectorExpression. Related open issue 20261022T142349Z-xray-x-ray-group-and-trace-segment-destination-responses.md records the same PENDING response from wire replay but carries no behavioral replay key for this check, so the two should be reconciled.

Traces: xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods.

## Evidence
Replay output:

[mismatch] xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods
  Error: Check timed out after 60.0s

Expected: GetTraceSegmentDestination on a settled account answers 200 with Status='ACTIVE' (enum TraceSegmentDestinationStatus = [PENDING, ACTIVE]), letting the check proceed; two PutTraceSegments calls ~120 s apart for one unique service name then yield exactly two GetTimeSeriesServiceStatistics datapoints with Period=60, each ServiceSummaryStatistics {TotalCount: 1, OkCount: 1, ErrorStatistics.TotalCount: 0, FaultStatistics.TotalCount: 0}, Timestamps inside the requested window and ~120 s apart, while the same window narrowed with EntitySelectorExpression service(id(name: "<never-submitted-name>")) returns no datapoint carrying ServiceSummaryStatistics.

Actual: no assertion is reached. The emulator answers GetTraceSegmentDestination with Status='PENDING' for the settled account — corroborated verbatim by wire replay in open issue 20261022T142349Z (Cause 5: `$.Status: value_mismatch (expected 'ACTIVE', got 'PENDING')`) — and the check's settle-poll (predicate Status==ACTIVE, 10 s interval, 600 s budget) therefore never exits; the harness kills the check at 60 s. Same opening helper, same timeout, in xray.telemetry-only-host-absent-from-service-graph.

Requirement parts: 'ACTIVE' and 'PENDING' are the fixed enum values from the model; the Destination value ('XRay'/'CloudWatchLogs'), service names, trace ids, timestamps and the marker suffix in the check are per-run values from the request and state, not part of the requirement.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (bootstrap_signal false, emulator serving).
2. Call xray.get_trace_segment_destination() on a freshly booted emulator that has had no UpdateTraceSegmentDestination applied. Expected: 200 with {Destination: 'XRay', Status: 'ACTIVE'}. Observed: Status 'PENDING'.
3. Poll that call for Status=='ACTIVE' as the enablement flow does (10 s interval). Observe it never settles, so any caller gated on it blocks indefinitely.
4. With the destination pinned (or the ACTIVE gate removed), call xray.put_trace_segments once with a clean segment {id, name: <unique service>, trace_id, start_time, end_time, http.response.status 200}, wait > 60 s, then submit a second such segment under the same name and a new trace id; assert UnprocessedTraceSegments == [] both times.
5. Call xray.get_time_series_service_statistics(StartTime=<before first submit>, EndTime=<after second submit>, Period=60, EntitySelectorExpression='service(id(name: "<unique service>")) ') and page NextToken to exhaustion. Expected two datapoints carrying ServiceSummaryStatistics with TotalCount=1 and OkCount=1 each, zero errors and faults, Timestamps ~the submission gap apart.
6. Repeat step 5 with a service name that was never submitted; expected no datapoint carrying ServiceSummaryStatistics.
7. Or replay the check id in trace_ids.
