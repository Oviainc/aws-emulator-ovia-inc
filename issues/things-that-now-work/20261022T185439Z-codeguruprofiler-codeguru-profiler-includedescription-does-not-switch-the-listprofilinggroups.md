Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.list-profiling-groups-include-description-toggles-response-shape codeguruprofiler.post-agent-profile-invalid-content-type-rejected codeguruprofiler.post-agent-profile-unsupported-content-type-rejected codeguruprofiler.get-notification-configuration-nonexistent-group-not-found-rejected codeguruprofiler.list-profile-times-start-after-end-rejected codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.list-profiling-groups-include-description-toggles-response-shape|run#9|AssertionError|assert "profilingGroupNames" not in page, (
Issue-Key: codeguruprofiler.post-agent-profile-invalid-content-type-rejected|_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: codeguruprofiler.post-agent-profile-unsupported-content-type-rejected|_error#7|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: codeguruprofiler.get-notification-configuration-nonexistent-group-not-found-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codeguruprofiler.list-profile-times-start-after-end-rejected|run#10|AssertionError|assert err["Code"] == "ValidationException", (
Issue-Key: codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# CodeGuru Profiler: includeDescription does not switch the ListProfilingGroups response shape, PostAgentProfile accepts payloads AWS refuses, and three operations answer NotImplemented instead of their modeled errors (3 causes)

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListProfilingGroups, PostAgentProfile, GetNotificationConfiguration, ListProfileTimes, UpdateProfilingGroup

## Description
Three distinct root causes in the codeguruprofiler emulator, all grounded in failing behavioral checks. Each must be closed before this issue is resubmitted.

=== Cause 1 of 3: ListProfilingGroups returns profilingGroupNames even when includeDescription=true ===
AWS: Smithy ListProfilingGroups output has `profilingGroupNames` (ProfilingGroupNames, required) and `profilingGroups` (ProfilingGroupDescriptions). The model documentation for ListProfilingGroupsRequest.includeDescription states: "If true, then a list of ProfilingGroupDescription objects that contain detailed information about profiling groups is returned. If false, then a list of profiling group names is returned." The member docs repeat this per-member: profilingGroupNames is "returned only if includeDescription is false" and profilingGroups "only if includeDescription is true". So the flag TOGGLES the response shape: with includeDescription=true the names list is omitted from the body entirely.
Emulator: both keys are always present. The replay saw body keys ['profilingGroupNames', 'profilingGroups'] for includeDescription=true. The handler builds the response with `profiling_group_names: page.to_vec()` unconditionally and only makes `profiling_groups` conditional.
Why: a client that selects the branch by key presence (e.g. `"profilingGroups" in page else profilingGroupNames`, or code asserting the two shapes are mutually exclusive) sees an ambiguous response; pagination helpers that iterate whichever key is present can double-count a page.
Fix: the ListProfilingGroups handler's response construction — make profilingGroupNames conditional on includeDescription being false, mirroring the existing conditional for profilingGroups.
Traces: codeguruprofiler.list-profiling-groups-include-description-toggles-response-shape; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/list_profiling_groups.rs:198-213.

=== Cause 2 of 3: PostAgentProfile stores any payload and answers 204 where AWS refuses with 400 ValidationException ===
AWS: Smithy PostAgentProfile models ValidationException (HTTP 400) and success status 204, with required members agentProfile (blob, @httpPayload) and contentType (Content-Type header). Recorded behavior: AWS PARSES the submitted profile body regardless of the Content-Type header and refuses a body that is not a valid agent profile with 400 ValidationException — a bare symbol payload under Content-Type text/plain is refused (recorded example message "Invalid agent profile: Unexpected type for profile root. Expected STRUCT, got SYMBOL..") and an empty JSON object `{}` under application/json is refused for the missing start field (recorded example message "Invalid agent profile: Profile must contain a start field.."). The rejected submission must leave profilingStatus.latestAgentProfileReportedAt unset.
Emulator: both calls SUCCEED with HTTP 204 (replay captured 'HTTPStatusCode': 204 for the bare-symbol payload). The handler passes the bytes to the store verbatim with no payload validation at all, so the rejected profile is also recorded as reported.
Why: an agent (or test) that relies on the service refusing a malformed profile gets silent acceptance; the group's profiling status then claims a profile was reported, and GetProfile/ListProfileTimes expose data AWS would never have accepted. This is an overly-permissive data-plane divergence: a 400 class becomes a 204 success.
Fix: PostAgentProfile's handler/validation — parse the agentProfile payload (JSON and Amazon Ion) and return the modeled ValidationException (400) for a root that is not a struct and for a profile missing the required start field, without mutating state. Status 400 + code ValidationException are the requirements; the recorded message texts above are examples.
Traces: codeguruprofiler.post-agent-profile-invalid-content-type-rejected, codeguruprofiler.post-agent-profile-unsupported-content-type-rejected; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/post_agent_profile.rs:142-184.

=== Cause 3 of 3: NotImplemented is returned where the operation's modeled client error is required ===
AWS: GetNotificationConfiguration, ListProfileTimes and UpdateProfilingGroup all model ResourceNotFoundException (404) and ValidationException (400) in this Smithy model. Recorded behavior: GetNotificationConfiguration on a never-created profiling group is 404 ResourceNotFoundException; UpdateProfilingGroup on a nonexistent group is 404 ResourceNotFoundException; ListProfileTimes with startTime after endTime is 400 ValidationException (a cross-field check AWS performs before touching the resource).
Emulator: all three answer the scaffold's `NotImplemented` (replay: "expected ResourceNotFoundException for a never-created profiling group, got 'NotImplemented'"; "a startTime after endTime was rejected with 'NotImplemented', expected ValidationException"; "expected ResourceNotFoundException, got NotImplemented"). Note the emulator's shared pre-dispatch validation DOES fire for single-member constraints on these same operations (pattern/length/enum violations come back as ValidationException), so only the cross-field and resource-existence refusals fall through to the stub.
Why: a client's error handling keys off the error code — NotImplemented is neither retryable nor a modeled member of these operations, so `except ResourceNotFoundException` / `except ValidationException` paths never run and a nonexistent group is indistinguishable from an emulator gap. Error-class fidelity for these refusals is independent of the happy path.
Related already-open work: the scaffold-stub issues filed for GetNotificationConfiguration, ListProfileTimes and UpdateProfilingGroup in this same service cover implementing the success path of those operations; this cause is their refusal classes, and these three checks were NOT marked as covered by those issues in the replay report.
Fix: the three operations' handlers and their validation — implement the resource lookup and the startTime<=endTime cross-field check and return the modeled errors (404 ResourceNotFoundException / 400 ValidationException) instead of the Scaffolded::NotImplemented channel.
Traces: codeguruprofiler.get-notification-configuration-nonexistent-group-not-found-rejected, codeguruprofiler.list-profile-times-start-after-end-rejected, codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected; handlers under services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/{get_notification_configuration.rs,list_profile_times.rs,update_profiling_group.rs}.

## Evidence
Cause 1 — codeguruprofiler.list-profiling-groups-include-description-toggles-response-shape:
  expected: includeDescription=True omits profilingGroupNames (model doc: true => ProfilingGroupDescription objects, false => names)
  actual:   AssertionError: includeDescription=True must omit profilingGroupNames, got keys ['ResponseMetadata', 'profilingGroupNames', 'profilingGroups']

Cause 2 — codeguruprofiler.post-agent-profile-invalid-content-type-rejected / -unsupported-content-type-rejected:
  expected: 400 ValidationException for agentProfile=b'not-a-profile' with contentType='text/plain' (recorded message example "Invalid agent profile: Unexpected type for profile root. Expected STRUCT, got SYMBOL..") and for agentProfile=b'{}' with contentType='application/json' (recorded message example "Invalid agent profile: Profile must contain a start field..")
  actual:   AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 204, ...}}

Cause 3 — modeled error vs NotImplemented:
  get-notification-configuration-nonexistent-group-not-found-rejected: expected ResourceNotFoundException, got 'NotImplemented'
  list-profile-times-start-after-end-rejected: "a startTime after endTime was rejected with 'NotImplemented', expected \"ValidationException\""
  update-profiling-group-nonexistent-not-found-rejected: "expected ResourceNotFoundException, got NotImplemented"

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguruprofiler client at it.
2. Cause 1: create two profiling groups, then call ListProfilingGroups(includeDescription=True) and inspect the response keys — profilingGroupNames is present alongside profilingGroups.
3. Cause 2: CreateProfilingGroup, then PostAgentProfile(profilingGroupName=name, agentProfile=b"not-a-profile", contentType="text/plain") — returns 204 instead of 400 ValidationException; repeat with agentProfile=b"{}", contentType="application/json" — also 204. DescribeProfilingGroup then shows profilingStatus.latestAgentProfileReportedAt set from a payload AWS would have refused.
4. Cause 3: GetNotificationConfiguration(profilingGroupName="never-created"); ListProfileTimes(profilingGroupName=<existing>, startTime=now, endTime=now-1h, period="PT5M"); UpdateProfilingGroup(profilingGroupName="never-created", agentOrchestrationConfig={"profilingEnabled": True}) — each returns NotImplemented instead of ResourceNotFoundException / ValidationException.
5. Or replay the checks listed in trace_ids.
