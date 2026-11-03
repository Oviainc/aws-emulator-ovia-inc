Issue-Source: trace-sweep-divergence
Issue-Trace: xray-trace-summaries-hourly-pagination
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-trace-summaries-hourly-pagination xray-trace-window-unit-mistakes xray-latency-filter-expression-mistakes xray-alarm-firing-trace-window-and-filter-mistakes xray-group-scoped-service-graph-and-statistics
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.ApproximateTime
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.NextToken
Issue-Key: xray|GetServiceGraph|2xx->2xx|$.EndTime
Issue-Key: xray|GetServiceGraph|2xx->2xx|$.StartTime
Issue-Commit: 80501f644d2883f30c53d819479800a81248de03

# xray GetTraceSummaries hourly paging missing and GetServiceGraph echoes an unrequested window (2 causes)

> _Note: this issue was identified against the code base at commit `80501f644d2883f30c53d819479800a81248de03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
TWO net-new root causes in the SUCCESS bodies of the trace-query reads — both calls answer 200 with the right status, and the response envelope is wrong. Smithy model consulted: services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json. No open issue names these clusters, and neither was marked stub_path or owned_by in the replay report (the open issue 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md concerns graph CONTENT and ResponseTimeRootCauses, not the paging envelope).

Cause 1 of 2: GetTraceSummaries does not page the requested window — ApproximateTime is the server's wall clock instead of the page start, and NextToken is never emitted.
AWS: Smithy GetTraceSummariesResult documents ApproximateTime as "The start time of this page of results" and NextToken as "If the requested time frame contained more than one page of results, you can use this token to retrieve the next page. The first page contains the most recent results, closest to the end of the time frame"; the operation is @paginated with inputToken/outputToken NextToken and items TraceSummaries. Recorded behavior: X-Ray pages a multi-hour window one page per hour — the first page's ApproximateTime is that page's start boundary, an INTEGER epoch-second timestamp lying INSIDE the requested window and aligned to the hour (e.g. the top of the hour that begins the page), and NextToken is present whenever the window spans more than one page. The requirement is those two properties, not any one run's epoch value.
Emulator: ApproximateTime is the current wall-clock time at the moment of the call, carrying sub-second precision (a fractional epoch) and sitting OUTSIDE the requested window; NextToken is absent even for a window that spans several hours, so the response claims to be the only page.
Why: the whole result set beyond the first page is unreachable — a client paging a day of traces stops after one call and silently under-reports, with no error to notice. ApproximateTime is also the cursor clients use to know which slice of the window they just received, so using the wall clock makes a page unplaceable (and non-deterministic between two identical calls); a consumer that advances its own cursor by ApproximateTime either re-reads or skips.
Fix: the GetTraceSummaries handler's paging/response projection in the xray crate — derive ApproximateTime from the page's start boundary (hour-aligned, integer seconds, within the requested window) and issue a NextToken while pages remain, honouring it on input.
Traces: xray-trace-summaries-hourly-pagination#task#1 (ApproximateTime and NextToken together; cluster also covers xray-trace-window-unit-mistakes#task#5), xray-latency-filter-expression-mistakes#task#5 (ApproximateTime alone; cluster also covers xray-trace-summaries-hourly-pagination#task#4 and xray-trace-window-unit-mistakes#task#6), xray-alarm-firing-trace-window-and-filter-mistakes#task#4 (NextToken alone).

Cause 2 of 2: GetServiceGraph emits StartTime/EndTime on a response where AWS emits neither.
AWS: recorded behavior only — StartTime and EndTime are modelled as optional members of GetServiceGraphResult, and for the group-scoped call in this step AWS's 200 response carries NEITHER member (the recording asserts both absent). Hypothesis: X-Ray omits the echoed window when the group-scoped graph resolves to no services.
Emulator: always populates both members, echoing the requested window back as epoch-second timestamps.
Why: a client that uses the PRESENCE of StartTime/EndTime to decide whether the service actually resolved and materialised a graph window (as opposed to echoing the request) reads a graph where AWS reported none; optional members that are always present also let a consumer build on a field AWS may omit, which then NPEs or defaults wrongly against the real service.
Fix: the GetServiceGraph handler's response projection in the xray crate — only emit StartTime/EndTime where the real service does, rather than unconditionally echoing the request's window.
Traces: xray-group-scoped-service-graph-and-statistics#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false): 4 net-new divergence clusters, service=xray, none carrying stub_path or owned_by.

Cause 1 — xray-trace-summaries-hourly-pagination#task#1, GetTraceSummaries, 200/200 (2 steps in cluster):
  $.ApproximateTime: expected the page-start timestamp inside the requested window (recorded as an hour-aligned integer epoch second), got the call-time wall clock with sub-second precision (recorded actual 1793677331.5854514, i.e. a fractional epoch outside the window)
  $.NextToken: expected present (shape-asserted), got None
Cause 1 — xray-latency-filter-expression-mistakes#task#5, GetTraceSummaries, 200/200 (3 steps in cluster):
  $.ApproximateTime: expected the page-start timestamp (recorded hour-aligned integer), got the wall clock as a fractional epoch (recorded actual 1793677325.2898533)
Cause 1 — xray-alarm-firing-trace-window-and-filter-mistakes#task#4, GetTraceSummaries, 200/200:
  $.NextToken: expected present (shape-asserted), got None
  (the specific epochs above are per-run values, not requirements; the requirements are hour-aligned integer seconds inside the requested window, and a token while pages remain)

Cause 2 — xray-group-scoped-service-graph-and-statistics#task#7, GetServiceGraph, 200/200:
  $.StartTime: type_mismatch — expected absent, got an epoch-second value (recorded actual 1793671200)
  $.EndTime:   type_mismatch — expected absent, got an epoch-second value (recorded actual 1793673720)

## Steps to Reproduce
Prerequisite: git_clone, then replay_wire_corpus (it builds and boots the emulator from the checkout).

Cause 1:
1. POST /TraceSummaries over a window spanning more than one hour -> the 200 body's ApproximateTime must be the hour-aligned start of the returned page, as integer epoch seconds inside the requested window (not the call time, and not fractional), and NextToken must be present because further pages remain. Currently ApproximateTime is the server's wall clock with sub-second precision and NextToken is absent.
2. Feed that NextToken back as GetTraceSummaries.NextToken -> the next page must be served with its own earlier ApproximateTime, until the window is exhausted.
3. Replay xray-trace-summaries-hourly-pagination#task#1, xray-latency-filter-expression-mistakes#task#5 and xray-alarm-firing-trace-window-and-filter-mistakes#task#4.

Cause 2:
4. POST /ServiceGraph for the group-scoped window of the step -> the 200 body must carry neither StartTime nor EndTime; currently both are emitted, echoing the requested window. Replay xray-group-scoped-service-graph-and-statistics#task#7.

Re-replay all four bindings (issue_trace/issue_step plus trace_ids) to confirm both causes are closed.
