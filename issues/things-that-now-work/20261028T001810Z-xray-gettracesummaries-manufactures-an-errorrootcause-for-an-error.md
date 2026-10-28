Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.fault-error-throttle-flags-and-root-causes-derived-from-segment-document
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.fault-error-throttle-flags-and-root-causes-derived-from-segment-document|run#35|AssertionError|assert errored.get("ErrorRootCauses") == [], (
Issue-Commit: 9b4e6b1ff6c9db4163689c6acf2e0383e4d25f93

# xray: GetTraceSummaries manufactures an ErrorRootCause for an error-flagged segment that carries no cause block

> _Note: this issue was identified against the code base at commit `9b4e6b1ff6c9db4163689c6acf2e0383e4d25f93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
ONE net-new behavioral root cause in the X-Ray service (the failing check carries SERVICES = ["xray"]). The xray service is bootstrapped and answering (105/126 xray checks pass in this run), so this is a fidelity defect, not a bootstrap gap. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

RELATED, NOT COVERING: 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md covers the OPPOSITE direction for a DIFFERENT member — ResponseTimeRootCauses is never populated (its Cause 2) — and the per-trace GetServiceGraph projection and IAM authorization; it says nothing about ErrorRootCauses being populated when nothing was submitted to derive one from. 20261022T142349Z-xray-x-ray-group-and-trace-segment-destination-responses.md and 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md are account-config/group surfaces, not the trace summary projection.

Cause 1 of 1: GetTraceSummaries fabricates a non-empty ErrorRootCauses list for a trace whose only segment is flagged `error: true` and carries NO `cause` block, and the fabricated root cause contains an empty Exceptions list.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models TraceSummary.ErrorRootCauses -> ErrorRootCauses (list of ErrorRootCause{Services -> ErrorRootCauseServices, ClientImpacting}), documented as "A collection of ErrorRootCause structures corresponding to the trace segments" — i.e. the list is DERIVED from what the segment documents contain, each ErrorRootCause being "the root cause of a trace summary error". The model does not state the derivation rule, so for the rule itself: recorded behavior only (the check records a real-AWS replay, us-west-2 2026-09-11) — a fault/error-flagged segment carrying a segment-level `cause` block yields a root cause whose Services[0].EntityPath[0].Exceptions[0].Name is the submitted exception `type`, while an error-flagged segment carrying NO `cause` block yields `ErrorRootCauses == []`: the service does not manufacture a root cause it has no exception to attribute. In the same recording the flag derivation itself is correct in the emulator (HasFault/HasError/HasThrottle independence and the FaultRootCauses derived from a `cause` block all pass in this run), which isolates the defect to the no-cause-block case.
Emulator: for a trace whose single segment is `{"name": "<svc>-error", "error": true, "http": {"response": {"status": 400}}}` with no `cause` member, GetTraceSummaries returns ErrorRootCauses = [{'Services': [{'Name': '<svc>-error', 'Names': ['<svc>-error'], 'EntityPath': [{'Name': '<svc>-error', 'Exceptions': [], 'Remote': False}], 'Inferred': False}], 'ClientImpacting': True}] — a root cause synthesized from the segment's own identity, with an EMPTY Exceptions list, instead of the empty list AWS returns. The service name and the `-error` suffix above are values taken from the check's request (its generated segment `name`); the requirement is the empty LIST, not those strings. No status code or error code diverges here: the call is a 200 whose payload carries a member AWS leaves empty.
Why: ErrorRootCauses is how a client (console view, latency/error triage, or an automated diagnoser) learns WHICH entity and WHICH exception caused a trace's error. An emulator that emits a root cause for every error-flagged trace makes the member carry no information: a client cannot distinguish "X-Ray attributed this error to an entity, here is the exception" from "nothing was submitted to attribute it to", and code that reads Services[0].EntityPath[0].Exceptions[0] to report the failing exception type gets an IndexError / empty attribution against the emulator for traces where AWS would simply have reported no root cause. It is the false-positive direction: callers see a diagnosis AWS never produced.
Fix: in the trace-summary projection, derive ErrorRootCauses (and the FaultRootCauses path that shares it) ONLY from a segment/subsegment `cause` block — emit no ErrorRootCause when no segment in the trace carries one, rather than synthesizing one from the segment's own name with an empty Exceptions list. Belongs in the GetTraceSummaries handler and the summary-building state it reads (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs and the trace/segment index that builds TraceSummary).
Traces: xray.fault-error-throttle-flags-and-root-causes-derived-from-segment-document.

## Evidence
xray.fault-error-throttle-flags-and-root-causes-derived-from-segment-document — suite verdict `mismatch`.

Failure site (check line 232):
  assert errored.get("ErrorRootCauses") == [], (
      "an error-flagged trace with no cause block must not manufacture a root cause, got ...")
  AssertionError: an error-flagged trace with no cause block must not manufacture a root cause, got [{'Services': [{'Name': 'sim2real-behav-xray-flags-5fb1a0-error', 'Names': ['sim2real-behav-xray-flags-5fb1a0-error'], 'EntityPath': [{'Name': 'sim2real-behav-xray-flags-5fb1a0-error', 'Exceptions': [], 'Remote': False}], 'Inferred': False}], 'ClientImpacting': True}]

Expected (real-AWS, per the check's recorded verification): ErrorRootCauses == [] for that trace.
Observed: a one-element list as quoted above. ('sim2real-behav-xray-flags-5fb1a0' is the check's per-run generated segment name — a request value filling the template, not part of the requirement.)

Scope of the divergence is narrow and well-bounded by what the SAME check already passed before reaching line 232: the four flagged segments were all accepted (UnprocessedTraceSegments == []); HasFault/HasError/HasThrottle came back independent, including the throttle+error segment reclassifying as HasThrottle with HasError False; and the fault segment's FaultRootCauses was derived correctly from its `cause` block — naming the submitted exception type, ClientImpacting True, EntityPath == [the submitted segment name], Remote False, Inferred False. Only the no-cause-block case diverges: the emulator populates the list where AWS leaves it empty.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); confirm bootstrap_signal is false and the xray service answers (105/126 xray checks pass).
2. now = time.time(). Call xray.put_trace_segments(TraceSegmentDocuments=[json.dumps({"id": "<16 hex>", "name": "probe-error", "trace_id": "1-<8 hex>-<24 hex>", "start_time": now-1.0, "end_time": now, "error": True, "http": {"request": {"method": "GET", "url": "https://example.invalid/bad"}, "response": {"status": 400}}})]) — note there is NO `cause` member. Observe UnprocessedTraceSegments == [].
3. Poll xray.get_trace_summaries(StartTime=now-300s, EndTime=now+60s), following NextToken, until the submitted trace id appears, and read that summary's ErrorRootCauses.
4. Observe a one-element list whose Services[0].EntityPath[0] is {'Name': 'probe-error', 'Exceptions': [], 'Remote': False}. Expected: ErrorRootCauses == [].
5. Control (should keep working, and does today): submit a second segment flagged "fault": true WITH a `cause` block {"exceptions": [{"id": "<16 hex>", "type": "MyProbeError", "message": "probe fault"}]} and confirm its FaultRootCauses names MyProbeError — so only the no-cause-block derivation needs to change.
6. Re-replay xray.fault-error-throttle-flags-and-root-causes-derived-from-segment-document to confirm the fix (it continues past line 232 to assert errored.FaultRootCauses == [] and throttled.FaultRootCauses == [], which must also be empty).
