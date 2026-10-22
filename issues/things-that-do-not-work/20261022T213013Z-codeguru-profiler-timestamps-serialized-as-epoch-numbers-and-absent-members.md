Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-error-paths codeguruprofiler-notification-channels-lifecycle codeguruprofiler-profiling-group-lifecycle codeguruprofiler-read-only-analysis-paths codeguruprofiler-resource-policy-lifecycle codeguruprofiler-submit-feedback-error codeguruprofiler-tagging-workflow
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|CreateProfilingGroup|2xx->2xx|$.createdAt
Issue-Key: codeguru-profiler|CreateProfilingGroup|2xx->2xx|$.profilingStatus
Issue-Key: codeguru-profiler|CreateProfilingGroup|2xx->2xx|$.updatedAt
Issue-Key: codeguru-profiler|DescribeProfilingGroup|2xx->2xx|$.createdAt
Issue-Key: codeguru-profiler|DescribeProfilingGroup|2xx->2xx|$.updatedAt
Issue-Key: codeguru-profiler|ListProfilingGroups|2xx->2xx|$.profilingGroupNames
Issue-Key: codeguru-profiler|ListProfilingGroups|2xx->2xx|$.profilingGroups[arn="arn
Issue-Key: codeguru-profiler|UpdateProfilingGroup|2xx->2xx|$.createdAt
Issue-Key: codeguru-profiler|UpdateProfilingGroup|2xx->2xx|$.updatedAt
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.anomalies
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.profileEndTime
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.profileStartTime
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.recommendations
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Timestamps serialized as epoch numbers and absent members emitted across codeguru-profiler responses

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfilingGroup, DescribeProfilingGroup, UpdateProfilingGroup, ListProfilingGroups, GetRecommendations

## Description
Five wire-replay causes in how the codeguru-profiler emulator SERIALIZES successful response bodies. Two faults recur across them: (a) every modeled `timestamp` member is written as a JSON NUMBER of epoch seconds instead of an ISO-8601 date-time STRING, and (b) members AWS omits from the body are emitted as empty containers. All five causes must be closed before this issue is resubmitted.

=== Cause 1 of 5: CreateProfilingGroup — createdAt/updatedAt epoch numbers, and profilingStatus emitted empty ===
AWS: Smithy CreateProfilingGroup -> 201, output member `profilingGroup` (@httpPayload) targets ProfilingGroupDescription, whose `createdAt` and `updatedAt` both target Timestamp with `timestampFormat: date-time`; the member docs state "Specify using the ISO 8601 format. For example, 2020-06-01T13:15:02.001Z". So both are ISO-8601 date-time strings on the wire. `profilingStatus` (ProfilingStatus) is NOT required; recorded behavior shows AWS omits the key entirely for a newly created group, since no agent has yet reported.
Emulator: writes `createdAt`/`updatedAt` as bare epoch-second numbers, and emits `profilingStatus` as an empty object `{}`.
Why: an SDK parses a date-time member by its modeled format, so a JSON number where a date-time string is modeled is either a parse error or a wildly wrong instant; and a client that branches on the presence of `profilingStatus` (the documented way to tell whether any profile has ever been reported) is told a status object exists when the group has no profiling activity at all.
Fix: the CreateProfilingGroup response serializer for ProfilingGroupDescription — render date-time members with the modeled format and omit `profilingStatus` when there is no reported activity.
Traces: codeguruprofiler-error-paths#task#1 (cluster also covers codeguruprofiler-notification-channels-lifecycle#task#1, codeguruprofiler-profiling-group-lifecycle#setup#1, codeguruprofiler-read-only-analysis-paths#setup#1, codeguruprofiler-resource-policy-lifecycle#setup#2, codeguruprofiler-submit-feedback-error#setup#1, codeguruprofiler-tagging-workflow#task#1 — 7 steps).

=== Cause 2 of 5: DescribeProfilingGroup — createdAt/updatedAt epoch numbers ===
AWS: DescribeProfilingGroup -> 200 with the same ProfilingGroupDescription payload; `createdAt`/`updatedAt` carry `timestampFormat: date-time`. The fixture asserts only the SHAPE of these members (an ISO-8601 date-time string), not their value.
Emulator: both members come back as epoch-second numbers.
Why: the read path of a profiling group is where clients learn the group's age; a numeric value fails the modeled date-time shape, so an SDK-level deserialize of the description breaks on the happy path.
Fix: the shared ProfilingGroupDescription serializer used by the read handler.
Traces: codeguruprofiler-profiling-group-lifecycle#task#2 (cluster also covers codeguruprofiler-profiling-group-lifecycle#task#6).

=== Cause 3 of 5: UpdateProfilingGroup — createdAt/updatedAt epoch numbers ===
AWS: UpdateProfilingGroup -> 200, same ProfilingGroupDescription payload with both date-time members.
Emulator: epoch-second numbers again (and the two differ from each other, confirming the update did bump updatedAt — only the encoding is wrong).
Why: same as Cause 2, on the mutate path; a client reading back the updated description cannot deserialize it.
Fix: the UpdateProfilingGroup handler's response serialization (same shared description serializer).
Traces: codeguruprofiler-profiling-group-lifecycle#task#5.

=== Cause 4 of 5: ListProfilingGroups — profilingGroupNames emitted alongside descriptions, plus epoch timestamps in each description ===
AWS: Smithy ListProfilingGroups output has `profilingGroupNames` (required) and `profilingGroups`; the model documentation for `includeDescription` states "If true, then a list of ProfilingGroupDescription objects ... is returned. If false, then a list of profiling group names is returned", and the member docs add that `profilingGroupNames` is "returned only if includeDescription is false". So the flag TOGGLES the body: with includeDescription=true the names key is absent. Each returned description carries the same date-time `createdAt`/`updatedAt`.
Emulator: emits `profilingGroupNames` even when descriptions were requested, and writes each description's `createdAt`/`updatedAt` as epoch numbers.
Why: a client that picks a branch by key presence, or a paginator that iterates whichever key is present, sees an ambiguous response and can double-count a page; the numeric timestamps break deserialization of every element.
Fix: the ListProfilingGroups handler — make `profilingGroupNames` conditional on includeDescription being false (mirroring the existing conditional for `profilingGroups`), and use the corrected description serializer. Related already-open behavioral issue: "CodeGuru Profiler: includeDescription does not switch the ListProfilingGroups response shape..." (20261022T185439Z-codeguruprofiler-codeguru-profiler-includedescription-does-not-switch-the-listprofilinggroups) describes the same toggle fault from the behavioral corpus, but it does NOT carry this wire fixture's replay key, so this cause is filed here with the key that exercises it.
Traces: codeguruprofiler-profiling-group-lifecycle#task#4.

=== Cause 5 of 5: GetRecommendations — profileStartTime/profileEndTime epoch numbers, and empty recommendations/anomalies lists emitted ===
AWS: Smithy GetRecommendations -> 200; output `profileStartTime` and `profileEndTime` both target Timestamp with `timestampFormat: date-time`. `recommendations` (Recommendations) and `anomalies` (Anomalies) are marked @required in the model, yet the recorded AWS response for a group with an aggregated profile but nothing to report OMITS both keys — recorded behavior only for that part. Hypothesis: the service suppresses the two analysis lists rather than returning them empty, so the model's @required is not honoured on the wire for this operation.
Emulator: writes both timestamps as epoch-second numbers and emits `recommendations: []` and `anomalies: []`.
Why: the two timestamps are the window the recommendations apply to, so a numeric encoding breaks deserialization of the whole analysis response; and a client distinguishing "no analysis available" from "analysis ran and found nothing" by key presence is given the wrong answer.
Fix: the GetRecommendations handler's serializer — render the two date-time members with the modeled format and omit the two analysis lists when they are empty.
Traces: codeguruprofiler-read-only-analysis-paths#task#4.

## Evidence
All five are 2xx responses whose BODY diverges; statuses matched. Expected = recorded AWS, actual = emulator, from the replay output.

Cause 1 — codeguruprofiler-error-paths#task#1, CreateProfilingGroup, status 201==201:
  $.createdAt: expected '2026-09-06T03:30:03.407Z' (ISO-8601 date-time string), got 1792704389.034473 (number)
  $.updatedAt: expected '2026-09-06T03:30:03.407Z', got 1792704389.034473
  $.profilingStatus: type_mismatch — expected key ABSENT, got {}
  (affected_count 7: same cluster across 7 fixture steps)

Cause 2 — codeguruprofiler-profiling-group-lifecycle#task#2, DescribeProfilingGroup, status 200==200:
  $.createdAt: expected '<shaped-not-asserted>' (date-time string shape), got 1792704391.1457057
  $.updatedAt: expected '<shaped-not-asserted>', got 1792704391.1457057
  (affected_count 2: also #task#6)

Cause 3 — codeguruprofiler-profiling-group-lifecycle#task#5, UpdateProfilingGroup, status 200==200:
  $.createdAt: expected '<shaped-not-asserted>', got 1792704391.1457057
  $.updatedAt: expected '<shaped-not-asserted>', got 1792704391.201994

Cause 4 — codeguruprofiler-profiling-group-lifecycle#task#4, ListProfilingGroups, status 200==200:
  $.profilingGroupNames: type_mismatch — expected key ABSENT, got ['sim2real-probe-codeguruprofiler-pg-lifecycle-c242a374']
  $.profilingGroups[arn=...].createdAt: expected '<shaped-not-asserted>', got 1792704391.1457057
  $.profilingGroups[arn=...].updatedAt: expected '<shaped-not-asserted>', got 1792704391.1457057

Cause 5 — codeguruprofiler-read-only-analysis-paths#task#4, GetRecommendations, status 200==200:
  $.profileStartTime: expected '2026-09-06T02:29:48.981Z', got 1792700791.789
  $.profileEndTime: expected '2026-09-06T03:29:48.981Z', got 1792704391.789
  $.recommendations: type_mismatch — expected key ABSENT, got []
  $.anomalies: type_mismatch — expected key ABSENT, got []

The recorded ISO values above are examples from one recording; the requirement is the ENCODING (ISO-8601 date-time string per the model's timestampFormat), not the instant.

## Steps to Reproduce
Boot the emulator and point a codeguru-profiler client at it.
1. Cause 1: CreateProfilingGroup(profilingGroupName=<new>, clientToken=uuid4()) and read the raw 201 body — createdAt/updatedAt are JSON numbers and profilingStatus is {}.
2. Cause 2: DescribeProfilingGroup(profilingGroupName=<that group>) — same two members are numbers in the 200 body.
3. Cause 3: UpdateProfilingGroup(profilingGroupName=<that group>, agentOrchestrationConfig={'profilingEnabled': True}) — same two members are numbers.
4. Cause 4: ListProfilingGroups(includeDescription=True) — body contains profilingGroupNames alongside profilingGroups, and each description's timestamps are numbers.
5. Cause 5: PostAgentProfile a valid profile, then GetRecommendations(profilingGroupName=<group>, startTime=.., endTime=..) — profileStartTime/profileEndTime are numbers and recommendations/anomalies are present as [].
6. Or replay fixture codeguruprofiler-error-paths step task#1 plus the fixture#step bindings in trace_ids.
