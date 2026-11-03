Issue-Source: trace-sweep-divergence
Issue-Trace: xray-alarm-firing-trace-window-and-filter-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-alarm-firing-trace-window-and-filter-mistakes xray-trace-window-unit-mistakes xray-group-scoped-service-graph-and-statistics xray-trace-summaries-range-type-and-sampling-options xray-latency-filter-expression-mistakes xray-trace-summaries-hourly-pagination
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSummaries|4xx->2xx|$.Message
Issue-Key: xray|GetTraceSummaries|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|GetTraceSummaries|4xx->2xx|status
Issue-Key: xray|GetServiceGraph|4xx->2xx|$.Message
Issue-Key: xray|GetServiceGraph|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|GetServiceGraph|4xx->2xx|status
Issue-Key: xray|GetTimeSeriesServiceStatistics|4xx->2xx|$.Message
Issue-Key: xray|GetTimeSeriesServiceStatistics|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|GetTimeSeriesServiceStatistics|4xx->2xx|status
Issue-Key: xray|GetTraceSummaries|4xx->4xx|$.Message
Issue-Key: xray|GetTraceSummaries|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: xray|GetTraceSummaries|4xx->4xx|$.message
Issue-Key: xray|GetServiceGraph|4xx->4xx|$.Message
Issue-Key: xray|GetTraceSummaries|2xx->4xx|$.ApproximateTime
Issue-Key: xray|GetTraceSummaries|2xx->4xx|$.TraceSummaries
Issue-Key: xray|GetTraceSummaries|2xx->4xx|$.TracesProcessedCount
Issue-Key: xray|GetTraceSummaries|2xx->4xx|status
Issue-Commit: 80501f644d2883f30c53d819479800a81248de03

# xray trace-query window, period and group validation diverge from AWS (8 causes)

> _Note: this issue was identified against the code base at commit `80501f644d2883f30c53d819479800a81248de03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
EIGHT distinct net-new root causes on the X-Ray trace-query read surface (GetTraceSummaries, GetServiceGraph, GetTimeSeriesServiceStatistics), all found by wire replay. Four are requests the emulator answers 200 that AWS refuses 400; four are refusals where the status is right but the error shape or message text diverges. Smithy model consulted: services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json.

RELATED, NOT COVERING: the open behavioral issue 20261102T085952Z-xray-5-net-new-behavioral-root-causes-createsamplingrule.md names the missing 6 h GetServiceGraph and 24 h GetTraceSummaries window bounds (its Causes 3 and 4) but carries behavioral check ids only, so no wire fixture step is bound to it — Causes 1 and 2 below are filed so the wire steps are re-replayed. 20261029T190848Z-xray-gettracesummaries-filter-expression-grammar-rejects-two-documented.md covers the annotation[key] and ! productions of the filter parser, not group scoping (Cause 7) nor the type of error raised for a group reference. 20261029T190848Z-xray-refusal-messages-diverge-from-aws-on-four.md covers sampling-rule and encryption-config messages only. None of the eight clusters below was marked stub_path or owned_by in the replay report.

Cause 1 of 8: GetTraceSummaries does not enforce its 24-hour time-range bound.
AWS: Smithy GetTraceSummaries = POST /TraceSummaries, success 200, required StartTime/EndTime (Timestamp), errors InvalidRequestException (400, member Message) and ThrottledException (429). Recorded behavior: a window longer than 24 h is refused 400 with body $.Message = "Time range cannot be longer than 24 hours" (entirely fixed text — nothing is taken from the request) and an x-amzn-errortype response header naming the error shape.
Emulator: answers HTTP 200 with a normal trace-summaries body; no Message member and no x-amzn-errortype header are emitted at all.
Why: a client that must split a long range into <=24 h slices (every dashboard or exporter that scans a day of traces) sees the un-split query succeed against the emulator and fail in production; the split path is never exercised.
Fix: the GetTraceSummaries handler's input validation in the xray crate — reject EndTime - StartTime > 24 h with InvalidRequestException before any projection work.
Traces: xray-alarm-firing-trace-window-and-filter-mistakes#task#1 (cluster also covers xray-trace-window-unit-mistakes#task#1 and #task#4).

Cause 2 of 8: GetServiceGraph does not enforce its 6-hour time-range bound — a different handler and a different bound from Cause 1.
AWS: Smithy GetServiceGraph = POST /ServiceGraph, required StartTime/EndTime, errors InvalidRequestException (400) / ThrottledException (429). Recorded behavior: a window longer than 6 h is refused 400 with $.Message = "Time range cannot be longer than 6 hours" (all fixed text) plus the x-amzn-errortype header.
Emulator: HTTP 200 with a graph payload, no Message, no x-amzn-errortype.
Why: the 6 h/24 h asymmetry between the graph and the summaries is a real API constraint clients have to encode; against the emulator neither bound exists, so service-map consumers ship without window splitting.
Fix: the GetServiceGraph handler's input validation in the xray crate.
Traces: xray-alarm-firing-trace-window-and-filter-mistakes#task#6.

Cause 3 of 8: GetTimeSeriesServiceStatistics accepts a Period outside the supported set.
AWS: Smithy GetTimeSeriesServiceStatistics = POST /TimeSeriesServiceStatistics, Period targets NullableInteger with NO modelled range, so the constraint is server-side; errors InvalidRequestException (400) / ThrottledException (429). Recorded behavior: a Period that is neither 60 nor 300 is refused 400 with $.Message = "Time period must be one of [60, 300]" — entirely fixed text, the bracketed pair being the allowed set and not a value echoed from the request — plus the x-amzn-errortype header.
Emulator: HTTP 200 with a statistics body, no refusal.
Why: a caller that picks an arbitrary granularity (e.g. 30 s or 900 s) gets plausible series from the emulator and a hard 400 in AWS, and never learns that only the one-minute and five-minute aggregations exist.
Fix: the GetTimeSeriesServiceStatistics handler's input validation in the xray crate.
Traces: xray-group-scoped-service-graph-and-statistics#task#6.

Cause 4 of 8: an unparsable StartTime/EndTime is refused with the wrong error shape and the wrong message.
AWS: Smithy declares InvalidRequestException (400) with a single member `Message` (capital M) on GetTraceSummaries; there is no ValidationException shape in the X-Ray model at all. Recorded behavior: a time member whose value cannot be converted to an epoch is refused 400 carrying $.Message = "<the raw value as submitted> can not be converted to milliseconds since epoch". Fixed text: " can not be converted to milliseconds since epoch"; the single variable part is the offending value copied verbatim from the request (the recording's value is the literal string the fixture submitted — it is the request's value, not a constant of the message).
Emulator: the status 400 is right, but the response is the generic framework validation envelope — a lowercase $.message body key with no $.Message member, and x-amzn-errortype: ValidationException, an error shape this service does not model.
Why: every SDK maps the error shape from x-amzn-errortype, so a client catching InvalidRequestException (the only 400 X-Ray declares here) sees an unmodelled error instead and falls through to a generic failure; and because the message moved to a different body key, nothing that reads Message sees any explanation.
Fix: the time-member deserialization/validation on the GetTraceSummaries handler in the xray crate — raise InvalidRequestException with AWS's template instead of the generic validation envelope.
Traces: xray-trace-window-unit-mistakes#task#2.

Cause 5 of 8: the enum-constraint validation message renders the allowed value set in the wrong order.
AWS: Smithy TimeRangeType is an enum whose values are declared TraceId, Event, Service; GetTraceSummaries.TimeRangeType targets it. Recorded behavior for an unknown value: 400 with body $.message (lowercase, the framework validation envelope) = "1 validation error detected: Value '<rejected value>' at 'timeRangeType' failed to satisfy constraint: Member must satisfy enum value set: [TraceId, Service, Event]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts: the rejected value and the lowerCamel member path, both from the request, and the allowed set rendered in the service's own order — recorded as [TraceId, Service, Event].
Emulator: emits the same template with the set sorted alphabetically — [Event, Service, TraceId]. Status, body key, member path and rejected value are all correct; only the ordering of the set diverges.
Why: the enumerated set in the message is what a caller (or a human reading a log, or a test asserting AWS's text) reads back to learn the legal values; a sorted rendering never matches AWS's string, so any equality match on the refusal text fails.
Fix: the enum-constraint renderer in the xray crate's shared input validation — emit the model's value order, not a sort.
Traces: xray-alarm-firing-trace-window-and-filter-mistakes#task#3 (cluster also covers xray-trace-summaries-range-type-and-sampling-options#task#1).

Cause 6 of 8: GetServiceGraph's unknown-group refusal message is missing its trailing period.
AWS: Smithy GetServiceGraph takes GroupName (@length 1..32) / GroupARN (@length 1..400) and declares InvalidRequestException (400). Recorded behavior: naming a group that does not exist is refused 400 with $.Message = "Group not found." — entirely fixed text, including the final period; nothing from the request is interpolated.
Emulator: "Group not found" (no period).
Why: the message is the only thing distinguishing "no such group" from the other InvalidRequestException causes of this call, since the code is shared; a client or assertion matching AWS's exact text never matches.
Fix: the group-resolution guard shared by the GetServiceGraph / GetTimeSeriesServiceStatistics handlers in the xray crate.
Traces: xray-alarm-firing-trace-window-and-filter-mistakes#task#5 (cluster also covers xray-group-scoped-service-graph-and-statistics#task#2 and #task#4).

Cause 7 of 8: a group-scoped GetTraceSummaries query is answered with a filter-expression parse error instead of resolving the group.
AWS: recorded behavior only (GetTraceSummaries's group scoping is not a modelled member; the request carries it alongside FilterExpression). A query scoped to a group that does not exist is refused 400 with $.Message = "Group not found." — the same fixed text as Cause 6, so the two surfaces agree.
Emulator: refuses 400 but with the parser's own message, $.Message = "Error Location: line 1, position 1 : Invalid input symbol: 'group.'" — the group reference is fed into the filter-expression grammar instead of being resolved against group state, so position 1 is the start of the group term. Fixed text of the emulator's message is "Error Location: line 1, position " / " : Invalid input symbol: "; the position and quoted symbol come from the input.
Why: the group reference is never resolved, so (a) a nonexistent group is reported as a syntax error the caller did not make, and (b) a caller cannot tell a bad group name from a malformed filter — both are 400 with the same code. A group-scoped query against a group that DOES exist cannot be scoped at all by this path.
Fix: the GetTraceSummaries handler in the xray crate — resolve the group scope before the filter-expression parser runs, and refuse an unknown group with InvalidRequestException and AWS's fixed text.
Traces: xray-group-scoped-service-graph-and-statistics#task#5 (cluster also covers xray-latency-filter-expression-mistakes#task#2, xray-trace-summaries-hourly-pagination#task#3, xray-trace-summaries-range-type-and-sampling-options#task#2 and #task#3).

Cause 8 of 8: GetTraceSummaries refuses with 400 a query AWS answers 200 with an empty page.
AWS: Smithy GetTraceSummaries returns 200 with TraceSummaries (list), ApproximateTime ("The start time of this page of results") and TracesProcessedCount. Recorded behavior: for these queries AWS answers 200 with TraceSummaries = [], TracesProcessedCount = 0 and ApproximateTime set to the page start inside the requested window — an accepted query that simply matched nothing.
Emulator: answers 400, emitting none of TraceSummaries / ApproximateTime / TracesProcessedCount. The same three fixtures reach this step after the group-scoped and filter-expression steps of Causes 5 and 7, so the emulator is rejecting scoping/filter forms AWS accepts rather than returning an empty result set. Hypothesis: the refusal is the same parser/group path as Cause 7 reached with a form that AWS parses.
Why: "no matching traces" and "your request is invalid" are opposite outcomes. A client that treats a 400 as a bug in its own query (backing off, alerting, or aborting a scan) breaks on a perfectly legal empty window, and pagination over a sparse window cannot run at all.
Fix: the GetTraceSummaries handler's filter-expression/group acceptance in the xray crate — accept the forms AWS accepts and answer 200 with an empty page (TraceSummaries [], TracesProcessedCount 0, ApproximateTime = page start).
Traces: xray-group-scoped-service-graph-and-statistics#task#8 (cluster also covers xray-latency-filter-expression-mistakes#task#6 and xray-trace-summaries-range-type-and-sampling-options#task#5).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 73/123 steps passed): 8 net-new divergence clusters, service=xray, none carrying stub_path or owned_by.

Cause 1 — xray-alarm-firing-trace-window-and-filter-mistakes#task#1, GetTraceSummaries (3 steps in cluster):
  status: expected 400, got 200
  $.Message: expected 'Time range cannot be longer than 24 hours', got None
  $headers.x-amzn-errortype: expected (shape-asserted), got None

Cause 2 — xray-alarm-firing-trace-window-and-filter-mistakes#task#6, GetServiceGraph:
  status: expected 400, got 200
  $.Message: expected 'Time range cannot be longer than 6 hours', got None
  $headers.x-amzn-errortype: expected (shape-asserted), got None

Cause 3 — xray-group-scoped-service-graph-and-statistics#task#6, GetTimeSeriesServiceStatistics:
  status: expected 400, got 200
  $.Message: expected 'Time period must be one of [60, 300]', got None
  $headers.x-amzn-errortype: expected (shape-asserted), got None

Cause 4 — xray-trace-window-unit-mistakes#task#2, GetTraceSummaries (status 400 on both sides):
  $.Message: expected '<submitted value> can not be converted to milliseconds since epoch' (recorded with the literal value the fixture submitted), got None
  $headers.x-amzn-errortype: shape_mismatch — got 'ValidationException', a shape the X-Ray model does not declare; the explanation was emitted under the lowercase $.message key instead of $.Message

Cause 5 — xray-alarm-firing-trace-window-and-filter-mistakes#task#3, GetTraceSummaries (2 steps in cluster; status 400 on both sides):
  $.message expected: "1 validation error detected: Value 'Events' at 'timeRangeType' failed to satisfy constraint: Member must satisfy enum value set: [TraceId, Service, Event]"
  $.message actual:   "1 validation error detected: Value 'Events' at 'timeRangeType' failed to satisfy constraint: Member must satisfy enum value set: [Event, Service, TraceId]"
  (only the order of the set differs; 'Events' and 'timeRangeType' are the request's value and member path)

Cause 6 — xray-alarm-firing-trace-window-and-filter-mistakes#task#5, GetServiceGraph (3 steps in cluster; status 400 on both sides):
  $.Message: expected 'Group not found.', got 'Group not found'

Cause 7 — xray-group-scoped-service-graph-and-statistics#task#5, GetTraceSummaries (5 steps in cluster; status 400 on both sides):
  $.Message: expected 'Group not found.', got "Error Location: line 1, position 1 : Invalid input symbol: 'group.'"

Cause 8 — xray-group-scoped-service-graph-and-statistics#task#8, GetTraceSummaries (3 steps in cluster):
  status: expected 200, got 400
  $.ApproximateTime: expected the page-start timestamp, got None
  $.TraceSummaries: expected [], got None
  $.TracesProcessedCount: expected 0, got None

## Steps to Reproduce
Prerequisite: git_clone, then replay_wire_corpus (it builds and boots the emulator from the checkout).

Cause 1: POST /TraceSummaries with StartTime/EndTime more than 24 h apart -> must be 400 InvalidRequestException with Message 'Time range cannot be longer than 24 hours' and an x-amzn-errortype header; currently 200. Replay xray-alarm-firing-trace-window-and-filter-mistakes#task#1.
Cause 2: POST /ServiceGraph with StartTime/EndTime more than 6 h apart -> must be 400 with Message 'Time range cannot be longer than 6 hours'; currently 200. Replay xray-alarm-firing-trace-window-and-filter-mistakes#task#6.
Cause 3: POST /TimeSeriesServiceStatistics with Period not in {60, 300} -> must be 400 with Message 'Time period must be one of [60, 300]'; currently 200. Replay xray-group-scoped-service-graph-and-statistics#task#6.
Cause 4: POST /TraceSummaries with a StartTime/EndTime value that is not convertible to an epoch -> must be 400 InvalidRequestException (x-amzn-errortype naming that shape) with $.Message '<the submitted value> can not be converted to milliseconds since epoch'; currently a ValidationException envelope under lowercase $.message. Replay xray-trace-window-unit-mistakes#task#2.
Cause 5: POST /TraceSummaries with TimeRangeType set to an unmodelled value -> the enum value set in the message must read [TraceId, Service, Event]; currently sorted. Replay xray-alarm-firing-trace-window-and-filter-mistakes#task#3.
Cause 6: POST /ServiceGraph with GroupName naming no existing group -> Message must be exactly 'Group not found.' (trailing period); currently 'Group not found'. Replay xray-alarm-firing-trace-window-and-filter-mistakes#task#5.
Cause 7: POST /TraceSummaries scoped to a group that does not exist -> Message must be exactly 'Group not found.'; currently a filter-expression parser error "Invalid input symbol: 'group.'". Replay xray-group-scoped-service-graph-and-statistics#task#5.
Cause 8: POST /TraceSummaries with the scoping/filter form used in the step -> must be 200 with TraceSummaries [], TracesProcessedCount 0 and ApproximateTime at the page start; currently 400. Replay xray-group-scoped-service-graph-and-statistics#task#8.

Re-replay all eight bindings (issue_trace/issue_step plus trace_ids) to confirm every cause is closed.
