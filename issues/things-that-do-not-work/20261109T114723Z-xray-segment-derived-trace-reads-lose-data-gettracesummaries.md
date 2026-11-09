Issue-Source: trace-sweep-divergence
Issue-Trace: xray-put-segment-is-found-by-trace-summaries-and-batch-get
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-put-segment-is-found-by-trace-summaries-and-batch-get
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].Annotations.order_id[*].ServiceIds
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].AvailabilityZones
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].InstanceIds
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].MatchedEventTime
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].ResourceARNs
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].Revision
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].ServiceIds[*]
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TraceSummaries[*].Users
Issue-Key: xray|BatchGetTraces|2xx->2xx|$.Traces[*].Segments[*].Document
Issue-Related: 20261103T034719Z-xray-gettracesummaries-hourly-paging-missing-and-getservicegraph-echoes.md
Issue-Commit: a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6

# xray segment-derived trace reads lose data: GetTraceSummaries omits modeled TraceSummary members and BatchGetTraces rewrites the stored segment document (2 causes)

> _Note: this issue was identified against the code base at commit `a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
Two net-new root causes in how the xray emulator projects a segment accepted by PutTraceSegments back out through the trace-read operations. Both are 200/200 content divergences: the call succeeds but the body a client reads back is not the body X-Ray returns. Neither is covered by an open issue; the nearest open xray issues (20261103T034719Z-xray-gettracesummaries-hourly-paging-missing-and-getservicegraph-echoes.md for GetTraceSummaries paging/ApproximateTime bucketing, and 20261103T034719Z-xray-batchgettraces-gettracegraph-trace-id-validation-diverges-from.md for trace-id validation) are about paging/window echo and about input validation, not about the shape or the preservation of the returned trace data.

Cause 1 of 2: GetTraceSummaries builds a partial TraceSummary for an ingested segment.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model) GetTraceSummaries -> GetTraceSummariesResult.TraceSummaries: list<TraceSummary>. TraceSummary models Users: TraceUsers (list), ResourceARNs: TraceResourceARNs (list), InstanceIds: TraceInstanceIds (list), AvailabilityZones: TraceAvailabilityZones (list), MatchedEventTime: Timestamp, Revision: Integer (@default 0), ServiceIds: list<ServiceId>, and Annotations: Annotations, a map of annotation key -> list<ValueWithServiceIds> whose members are AnnotationValue plus ServiceIds. The recording shows AWS, for a single trace built from one submitted segment, returning the four list members present-and-empty ([]), a MatchedEventTime, Revision=1 (the revision is incremented as the trace is assembled, not left at the shape default), and ServiceIds containing TWO entries for the submitted service — the segment's own service and a second one with Type "client" — while leaving ValueWithServiceIds.ServiceIds inside Annotations unset.
Emulator: the same summary comes back with Users, ResourceARNs, InstanceIds and AvailabilityZones absent entirely (null rather than []), MatchedEventTime absent, Revision=0, only ONE ServiceIds entry (the second, Type "client", entry is missing), and Annotations.<key>[].ServiceIds populated with the service entry AWS leaves null. (The same step also reports ApproximateTime differing from the recording; that field is a clock value per run and is NOT part of this finding.)
Why: a client that reads TraceSummary.ResourceARNs/InstanceIds/AvailabilityZones/Users by iterating a list gets a null-dereference or a "field missing" path instead of an empty iteration; a client that uses Revision to detect trace updates sees a trace frozen at revision 0; a client that enumerates ServiceIds to discover which services participated in a trace sees the caller/client side of the call disappear; and a client reading Annotations sees per-annotation ServiceIds that AWS does not emit.
Fix: the GetTraceSummaries handler's TraceSummary projection in the xray service crate — the code that derives a summary from stored segment documents must emit the empty collection members as empty lists rather than omitting them, set MatchedEventTime and the trace revision, derive the client-side ServiceId in addition to the segment's own, and stop attaching ServiceIds to annotation values.
Traces: xray-put-segment-is-found-by-trace-summaries-and-batch-get#task#2.

Cause 2 of 2: BatchGetTraces does not return the segment document as submitted.
AWS: Smithy BatchGetTraces -> BatchGetTracesResult.Traces: list<Trace>; Trace.Segments: list<Segment>; Segment.Document: SegmentDocument (@length min 1), documented as "The segment document" — the document uploaded with PutTraceSegments, whose schema is the X-Ray segment-document schema. X-Ray stores and returns the submitted document; it does not author its own timestamps for a segment the caller timed. In the recording the document returned for the trace carries exactly the submitted start_time/end_time (serialized as the exponential-form epoch seconds the caller sent) and the caller's field order.
Emulator: the returned Document is a re-serialization of the emulator's own parsed copy: the member order is rewritten, and start_time/end_time are replaced with the emulator's ingest-time epoch seconds instead of the values the caller submitted in PutTraceSegments. The submitted segment's timing is therefore unrecoverable from the trace read.
Why: the segment document is the payload clients parse for timing, http, annotations and subsegment structure. Replacing the submitted start_time/end_time with ingest time makes every latency, duration and ordering computation over a replayed/backdated segment wrong, and a client that compares the document it submitted against the document it reads back (a round-trip or idempotency check) never matches.
Fix: the xray service's segment state plus the BatchGetTraces serializer — store the submitted segment document and return it unmodified, rather than reconstructing it from parsed fields and substituting ingest-time timestamps.
Traces: xray-put-segment-is-found-by-trace-summaries-and-batch-get#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture xray-put-segment-is-found-by-trace-summaries-and-batch-get (both steps returned HTTP 200, as expected — the divergence is in the body).

Cause 1 — step task#2, GetTraceSummaries, expected 200 / actual 200:
  $.TraceSummaries[0].AvailabilityZones: expected [], got None (member absent)
  $.TraceSummaries[0].InstanceIds:       expected [], got None (member absent)
  $.TraceSummaries[0].ResourceARNs:      expected [], got None (member absent)
  $.TraceSummaries[0].Users:             expected [], got None (member absent)
  $.TraceSummaries[0].MatchedEventTime:  expected a timestamp, got None (member absent)
  $.TraceSummaries[0].Revision:          expected 1, got 0
  $.TraceSummaries[0].ServiceIds[1]:     expected {"AccountId": null, "Name": "<segment service name>", "Names": ["<segment service name>"], "Type": "client"}, got None (only one ServiceIds entry emitted)
  $.TraceSummaries[0].Annotations.<annotation-key>[0].ServiceIds: expected None, got [{"Name": "<segment service name>", "Names": ["<segment service name>"]}]
  $.ApproximateTime: differs by run clock only — excluded from this finding.

Cause 2 — step task#3, BatchGetTraces, expected 200 / actual 200:
  $.Traces[0].Segments[0].Document: value_mismatch. The recorded document preserves the submitted segment verbatim, including the submitted start_time/end_time in the exponential epoch-second form the caller sent (e.g. start_time 1.793915525E9, end_time 1.793915527E9) and the caller's member order (id, name, start_time, trace_id, end_time, http, annotations). The emulator returned a re-serialized object with a different member order (trace_id, id, name, start_time, end_time, http, annotations) and with start_time/end_time replaced by its own ingest-time epoch seconds — i.e. the submitted timestamps were discarded, not just reformatted.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay fixture xray-put-segment-is-found-by-trace-summaries-and-batch-get from trace set simparity-wire-aws-v1.
   - Setup submits one segment document via PutTraceSegments, with explicit start_time/end_time, an http block and an annotation (order_id).
3. Cause 1: at step task#2 the fixture calls GetTraceSummaries over a window covering the segment. Inspect TraceSummaries[0]: AvailabilityZones/InstanceIds/ResourceARNs/Users are absent instead of empty lists, MatchedEventTime is absent, Revision is 0 instead of 1, ServiceIds has one entry instead of two (no Type="client" entry), and Annotations.order_id[0].ServiceIds is populated where AWS returns null.
4. Cause 2: at step task#3 the fixture calls BatchGetTraces with the trace id from step task#2. Compare Traces[0].Segments[0].Document against the document submitted in step 2: the member order is rewritten and start_time/end_time carry the emulator's ingest time rather than the submitted values.
