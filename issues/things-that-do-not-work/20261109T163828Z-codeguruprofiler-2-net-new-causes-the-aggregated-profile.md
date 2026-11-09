Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile codeguruprofiler.post-agent-profile-nonexistent-group-not-found-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile|run#22|AssertionError|assert profile["start"] == expected_start, (
Issue-Key: codeguruprofiler.post-agent-profile-nonexistent-group-not-found-rejected|run#6|AssertionError|assert err["Message"] == expected_message, (err["Message"], expected_message)
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# codeguruprofiler: 2 net-new causes — the aggregated profile GetProfile serves carries epoch-millisecond window bounds instead of AWS's ISO-8601 document, and PostAgentProfile's not-found message is re-worded

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfile, PostAgentProfile

## Description
Two distinct net-new root causes in the codeguruprofiler emulator, each grounded in a failing behavioral check of the simparity-behavioral-aws-v1 replay. BOTH must be closed before this issue is resubmitted. Already-filed work that does NOT cover either cause: the fixed issue 20261023T123333Z-codeguruprofiler-codeguru-profiler-getpolicy-404s-on-a-policy-less.md re-worded the not-found messages of GetPolicy and UpdateProfilingGroup only — PostAgentProfile (Cause 2) was not in its scope, and its Cause 3 concerned WHEN GetProfile answers 404, not WHAT the served document looks like (Cause 1). The open issue 20261029T105339Z-codeguruprofiler-listprofiletimes-wrong-behavior--r2.md concerns which windows ListProfileTimes announces, not the profile body.

=== Cause 1 of 2: the document GetProfile serves as the aggregated profile carries the window bounds as epoch-millisecond NUMBERS, not AWS's ISO-8601 instants ===
AWS: Smithy GetProfile GET /profilingGroups/{profilingGroupName}/profile, 200 with required `profile` (@httpPayload blob, target AggregatedProfile) and the required Content-Type header; the AggregatedProfile shape is an unconstrained blob with no prose doc, so the document's own schema is recorded behavior only. The recording shows CodeGuru Profiler does not hand back the agent's submission: it serves an AGGREGATED profile document whose `start` and `end` are ISO-8601 UTC instants with millisecond precision in the form `YYYY-MM-DDTHH:MM:SS.mmmZ` (the aligned aggregation window's bounds — derived from the requested period and the group's data, never from one run's clock), and which carries an `aggregatedMetadata` object with `numReportedAgents` counting the distinct agents that posted into that window. The posted agent document, by contrast, is the one that expresses `start`/`end` as epoch milliseconds.
Emulator: after CreateProfilingGroup + ConfigureAgent + PostAgentProfile for a closed 5-minute window, GetProfile(period='PT5M', accept='application/json') answers 200 and the JSON body's `start` is the integer `1794241500000` where the ISO-8601 string `'2026-11-09T16:25:00.000Z'` (that same instant) is required. Hypothesis: the stored agent-profile document is streamed back verbatim rather than an aggregated document being derived from it — in which case `end` is the same raw millisecond number and `aggregatedMetadata`/`numReportedAgents` is absent entirely; the check's first assertion is on `start`, so the replay stops there and the rest of the document is unverified by this run.
Why: the profile payload is the whole point of the data plane. Every consumer of it (flame-graph renderers, the CodeGuru console format, regression gates, the agent-side round-trip checks) parses `start`/`end` as ISO-8601 strings and reads `aggregatedMetadata.numReportedAgents`; against the emulator those reads raise a type error or KeyError, so no aggregation-consuming code path can be exercised at all, and a client cannot tell which window the bytes it received describe.
Fix: GetProfile's handler/serializer and the state that backs the aggregation — build the aggregated document for the requested window (ISO-8601 millisecond `start`/`end` from the aligned window, plus `aggregatedMetadata` with the distinct reported-agent count) instead of returning the posted agent document's own fields.
Traces: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile.

=== Cause 2 of 2: PostAgentProfile's ResourceNotFoundException message is emulator-authored, not CodeGuru Profiler's PostAgentProfile wording ===
AWS: Smithy PostAgentProfile POST /profilingGroups/{profilingGroupName}/agentProfile, success 204, models ResourceNotFoundException (404) with a required `message`. Recorded behavior only for the text: a submission naming a profiling group that was never created is refused 404 ResourceNotFoundException whose message is the fixed sentence `No profiling group found in account ` + account + ` with name ` + name + `.` — i.e. fixed text 'No profiling group found in account', 'with name' and the terminating period, with the caller's account id and the request's profilingGroupName as its only variable parts.
Emulator: returns the correct status and code but the message 'Profiling group for <account>/<name> does not exist!' (replay: 'Profiling group for 000000000000/sim2real-behav-cgp-missing-e037e43dcb6b does not exist!' where 'No profiling group found in account 000000000000 with name sim2real-behav-cgp-missing-e037e43dcb6b.' is required). Account id and group name are the request-derived parts in both; the fixed text and its shape are wholly different.
Why: an error message is part of the contract. Agents and onboarding tooling that key on this sentence (or parse the account/name out of it) to distinguish 'group not yet created' from any other 404 on the same path mis-classify the failure, and operators grepping logs for AWS's wording find nothing.
Fix: PostAgentProfile's handler / its not-found construction — raise this operation's own message, formatted from the caller's account and the request's profilingGroupName, rather than a shared or hand-written sentence.
Traces: codeguruprofiler.post-agent-profile-nonexistent-group-not-found-rejected.

## Evidence
Cause 1 — GetProfile aggregated document after a PostAgentProfile for a closed PT5M window (status 200, contentType application/json both correct):
  expected: profile['start'] == '2026-11-09T16:25:00.000Z'  (ISO-8601 UTC, millisecond precision, the aligned window start)
  actual:   profile['start'] == 1794241500000            (epoch-millisecond integer)
  AssertionError: expected the aggregated profile to start at '2026-11-09T16:25:00.000Z', got 1794241500000
  (the same window's `end` and the document's aggregatedMetadata.numReportedAgents == 1 are asserted immediately after and were not reached)

Cause 2 — PostAgentProfile against a never-created profiling group (HTTP 404 and Code 'ResourceNotFoundException' both correct):
  expected Message: 'No profiling group found in account 000000000000 with name sim2real-behav-cgp-missing-e037e43dcb6b.'
  actual   Message: 'Profiling group for 000000000000/sim2real-behav-cgp-missing-e037e43dcb6b does not exist!'
  (fixed text = 'No profiling group found in account ... with name ....'; variable parts = the caller's account id and the request's profilingGroupName)

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguruprofiler client at it (the replay ran with account 000000000000).
2. Cause 1: CreateProfilingGroup(profilingGroupName=N, agentOrchestrationConfig={'profilingEnabled': True}); ConfigureAgent(profilingGroupName=N, fleetInstanceId=F); compute the last CLOSED 5-minute window [S,E) aligned to UTC; PostAgentProfile(profilingGroupName=N, agentProfile=json.dumps({'start': S_ms, 'end': E_ms, 'callgraph': {'frameId':'ALL','runnableCount':1000,'children':{}}}).encode(), contentType='application/json'); poll GetProfile(profilingGroupName=N, period='PT5M', accept='application/json') until it answers 200 and json.loads the body: `start` is the integer S_ms instead of the ISO-8601 string for S, and the document carries no aggregatedMetadata.numReportedAgents.
3. Cause 2: PostAgentProfile(profilingGroupName='<a name never created>', agentProfile=<any valid profile document>, contentType='application/json') -> 404 ResourceNotFoundException whose Message is 'Profiling group for <account>/<name> does not exist!' instead of 'No profiling group found in account <account> with name <name>.'
4. Or replay the checks listed in trace_ids.
