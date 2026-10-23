Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.get-policy-absent-before-any-permission codeguruprofiler.get-policy-no-policy-not-found codeguruprofiler.put-permission-invalid-action-group-rejected codeguruprofiler.delete-profiling-group-removes-visibility codeguruprofiler.delete-profiling-group-then-resource-not-found codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected codeguruprofiler.get-profile-no-aggregated-data-yet codeguruprofiler.get-profile-without-data-not-found codeguruprofiler.list-profile-times-start-after-end-rejected codeguruprofiler.tag-nonexistent-resource-not-found codeguruprofiler.tag-resource-empty-tags-validation-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler|GetPolicy|ResourceNotFoundException
Issue-Key: codeguruprofiler.delete-profiling-group-removes-visibility|run#12|AssertionError|assert policy_message == expected_policy_message, (policy_message, expected_policy_message)
Issue-Key: codeguruprofiler.delete-profiling-group-then-resource-not-found|run#14|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected|run#8|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.get-profile-no-aggregated-data-yet|_error_code#16|AssertionError|raise AssertionError("expected a rejection, but the call SUCCEEDED")
Issue-Key: codeguruprofiler.get-profile-without-data-not-found|_error_code#19|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codeguruprofiler.list-profile-times-start-after-end-rejected|run#12|AssertionError|assert err["Message"] == expected, (
Issue-Key: codeguruprofiler.tag-nonexistent-resource-not-found|run#13|AssertionError|assert err["Code"] == "NotFoundException", err
Issue-Key: codeguruprofiler.tag-resource-empty-tags-validation-rejected|run#7|AssertionError|assert err["Message"] == "Invalid request input, expected non-empty tags map", err["Message"]
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# CodeGuru Profiler: GetPolicy 404s on a policy-less group, GetProfile serves data AWS has not aggregated, and four not-found/validation refusals use the wrong code or a generic message (6 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy, GetProfile, UpdateProfilingGroup, ListProfileTimes, TagResource

## Description
Six distinct root causes in the codeguruprofiler emulator, each grounded in failing behavioral checks from the simparity-behavioral-aws-v1 replay. ALL SIX must be closed before this issue is resubmitted. Related already-open work (does NOT cover these): 20261022T185439Z-codeguruprofiler-codeguru-profiler-includedescription-does-not-switch-the-listprofilinggroups.md carries ListProfileTimes/UpdateProfilingGroup checks for an earlier failure point (NotImplemented); those two operations now DO return the modeled error code, and the two checks fail further along, on the message rule described in Causes 2 and 4 here. The PutPermission / AddNotificationChannels / GetNotificationConfiguration scaffold-stub issues cover standing those operations up, not the GetPolicy representation in Cause 1.

=== Cause 1 of 6: GetPolicy returns 404 ResourceNotFoundException for an EXISTING profiling group that simply has no resource-based policy ===
AWS: Smithy GetPolicy GET /profilingGroups/{profilingGroupName}/policy, success code 200, errors ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). The model marks output members policy and revisionId required, but recorded behavior only: on a freshly created group that has never had PutPermission called, GetPolicy SUCCEEDS with 200 and the body carries NEITHER policy NOR revisionId; 404 ResourceNotFoundException is reserved for the case where the profiling group itself does not exist. The two cases are distinct classes.
Emulator: GetPolicy on an existing, policy-less group raises 404 ResourceNotFoundException (replay message example: "no resource-based policy is attached to profiling group <name>"). The emulator conflates "no policy attached" with "group not found".
Why: a client cannot distinguish a group that exists but is unshared from a group that was deleted. Code that reads the current policy, mutates it and writes it back (the standard read-modify-write on a resource policy) aborts on a 404 for a group that is perfectly healthy; an "is this group shared?" probe reports the group missing.
Fix: the GetPolicy handler and its serializer — when the profiling group exists, answer 200 and omit policy/revisionId entirely when no policy is attached; keep 404 ResourceNotFoundException only for a missing group.
Traces: codeguruprofiler.get-policy-absent-before-any-permission, codeguruprofiler.get-policy-no-policy-not-found, codeguruprofiler.put-permission-invalid-action-group-rejected (this third check gets past PutPermission's enum rejection and then fails on the same GetPolicy 404 when asserting no policy was written); services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_policy.rs.

=== Cause 2 of 6: one generic not-found message is used for every operation, instead of the per-operation message built from accountId + profilingGroupName ===
AWS: ResourceNotFoundException (404, member message required) is modeled for DescribeProfilingGroup, GetPolicy and UpdateProfilingGroup alike, but recorded behavior only shows AWS emits a DIFFERENT message per operation, each built from the request: DescribeProfilingGroup -> "CodeGuru Profiler can\u2019t complete the operation because {name} was not found."; GetPolicy -> "No profiling group with accountId: {accountId}; name: {name} was found."; UpdateProfilingGroup -> "Unable to find profiling group with accountId: {accountId}; profilingGroupName: {name} during update". The requirement is the per-operation rule (which operation, plus the caller's account id and the requested group name), not the recorded literal text.
Emulator: every operation returns the DescribeProfilingGroup form. Replay: GetPolicy on a deleted group returned "CodeGuru Profiler can\u2019t complete the operation because <name> was not found." where "No profiling group with accountId: 000000000000; name: <name> was found." is required; UpdateProfilingGroup on a never-created group returned the same Describe-shaped text where "Unable to find profiling group with accountId: 000000000000; profilingGroupName: <name> during update" is required. Status 404 and code ResourceNotFoundException are correct in all three.
Why: clients and operators that key on the message to tell WHICH lookup failed (common in CodeGuru Profiler onboarding tooling, which distinguishes a missing group from a missing policy on the same name) read the wrong cause, and the account id that scopes the lookup is dropped from the diagnostic entirely.
Fix: the not-found construction shared by these handlers — each operation's handler should raise its own message, formatted from the request's profilingGroupName and the caller account, rather than calling one common helper.
Traces: codeguruprofiler.delete-profiling-group-removes-visibility, codeguruprofiler.delete-profiling-group-then-resource-not-found, codeguruprofiler.update-profiling-group-nonexistent-not-found-rejected; handlers under services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/{get_policy.rs,update_profiling_group.rs}.

=== Cause 3 of 6: GetProfile serves a 200 profile for a group whose submitted data has not been aggregated ===
AWS: Smithy GetProfile GET /profilingGroups/{profilingGroupName}/profile, success 200 with required profile (@httpPayload blob) and Content-Type header, errors ResourceNotFoundException (404) and ValidationException (400). Recorded behavior only: aggregation is asynchronous and lags well past a single request, so GetProfile over a window with no AGGREGATED profile returns 404 ResourceNotFoundException with message "No profiles found for the requested time range!" — including immediately after a PostAgentProfile that itself succeeded and moved DescribeProfilingGroup's profilingStatus.latestAgentProfileReportedAt (the checks record this as unresolved after 200s of polling against real AWS). The not-found class here is about the absence of an aggregated profile, not the absence of the group (DescribeProfilingGroup still succeeds).
Emulator: GetProfile returns 200 with a profile body as soon as a profile has been posted (replay: "expected a rejection, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ... 'contentType': 'application/json', 'profile': <StreamingBody>}", content-length 115). It treats a single posted agent profile as an immediately available aggregate, for both the period=PT5M and the startTime/endTime forms of the request.
Why: this is an overly-permissive data-plane divergence — a 404 class becomes a 200. A client written against AWS retries or back-fills while aggregation is pending; against the emulator it receives a synthetic aggregate that AWS would never return, so consumers of the profile payload (flame-graph renderers, regression gates) are exercised on data with no aggregation applied and the pending-aggregation code path is never covered.
Fix: GetProfile's handler and the state that backs it — keep posted agent profiles separate from aggregated profiles and return 404 ResourceNotFoundException (message example "No profiles found for the requested time range!") while no aggregated profile covers the requested window.
Traces: codeguruprofiler.get-profile-no-aggregated-data-yet, codeguruprofiler.get-profile-without-data-not-found; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_profile.rs.

=== Cause 4 of 6: ListProfileTimes' startTime-after-endTime ValidationException does not name the interval it rejected ===
AWS: Smithy ListProfileTimes requires startTime, endTime and period (AggregationPeriod enum PT5M|PT1H|P1D) and models ValidationException (400). Recorded behavior only: a range whose startTime is after its endTime is refused with 400 ValidationException whose message is built from the request, echoing both bounds in ISO-8601 millisecond form: "Interval start time (<startTime>) must be before end time (<endTime>)". The rule — a validation message naming the offending members and their values — is the requirement; the literal timestamps are request-derived.
Emulator: returns 400 ValidationException (correct class) with the fixed string "endTime must not be before startTime", which names no values. Replay: "a startTime after endTime was rejected with message 'endTime must not be before startTime', expected 'Interval start time (2026-10-23T13:28:12.123Z) must be before end time (2026-10-23T11:28:12.123Z)'".
Why: a caller paginating many windows cannot tell WHICH window it mis-ordered from the error, and tooling that parses the bounds out of the message to self-correct gets nothing.
Fix: ListProfileTimes' validation — format the cross-field message from the request's startTime and endTime (ISO-8601 with milliseconds, as the request supplied them).
Traces: codeguruprofiler.list-profile-times-start-after-end-rejected; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/list_profile_times.rs.

=== Cause 5 of 6: TagResource on a nonexistent profiling group ARN returns the wrong error CODE ===
AWS: Smithy TagResource POST /tags/{resourceArn}, success 204, with resourceArn (@httpLabel, ProfilingGroupArn) and a required tags map; the model lists ResourceNotFoundException/ValidationException/InternalServerException. Recorded behavior only: a well-formed ARN naming a profiling group that does not exist is refused by the tagging surface with code NotFoundException and message "Resource was not found" — a tagging-specific code, NOT the ResourceNotFoundException used by the profiling-group operations.
Emulator: returns code ResourceNotFoundException with the Describe-shaped text. Replay: {'Message': 'CodeGuru Profiler can\u2019t complete the operation because <name> was not found.', 'Code': 'ResourceNotFoundException'}, where NotFoundException / "Resource was not found" is required.
Why: an error CODE is a client contract — `except cgp.exceptions.NotFoundException` around a tag reconciliation loop never fires, so a tagger that is supposed to skip deleted resources crashes or retries instead. Cross-service tagging helpers that branch on the tagging error code mis-classify the failure.
Fix: TagResource's handler/error mapping (and the matching path in UntagResource/ListTagsForResource if it shares the mapping) — emit the tagging surface's NotFoundException code for an ARN whose resource does not exist.
Traces: codeguruprofiler.tag-nonexistent-resource-not-found; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/tag_resource.rs.

=== Cause 6 of 6: TagResource with an empty tags map is rejected by generic body parsing, not by tags validation ===
AWS: Smithy TagResource's tags member is a required TagsMap with no modeled length bound, so this is recorded behavior only: an empty map is refused with BadRequestException and the message "Invalid request input, expected non-empty tags map" — a validation error naming the offending member and the constraint it violated — and the resource's existing tag set is left unchanged.
Emulator: returns BadRequestException (correct class) with "Invalid request body", a generic deserialization message that names neither the member nor the constraint. Replay: AssertionError on the message, "Invalid request body". This reads as the empty map being refused by the body parser rather than by TagResource's own tags validation.
Why: the caller cannot tell a malformed request body from a well-formed request carrying an empty tags map, so an SDK user who passes tags={} by accident gets no actionable diagnosis; and a rejection raised by the parser rather than the operation leaves it unclear whether the validation ordering (reject before any state mutation) is actually implemented.
Fix: TagResource's validation — check the deserialized tags map for emptiness in the operation's own validation and raise BadRequestException naming the member/constraint (message example above), before any tag-state mutation.
Traces: codeguruprofiler.tag-resource-empty-tags-validation-rejected; services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/tag_resource.rs.

## Evidence
Cause 1 — GetPolicy on an existing policy-less group:
  expected: HTTP 200, body with neither 'policy' nor 'revisionId'
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPolicy operation: no resource-based policy is attached to profiling group sim2real-cgp-43309e748298
  (same 404 also aborts put-permission-invalid-action-group-rejected at its "effect absent" GetPolicy step)

Cause 2 — per-operation not-found message (404 ResourceNotFoundException in both cases):
  GetPolicy after delete: actual 'CodeGuru Profiler can\u2019t complete the operation because sim2real-behav-cgp-28de27fbf74d was not found.' / required form 'No profiling group with accountId: 000000000000; name: sim2real-behav-cgp-28de27fbf74d was found.'
  UpdateProfilingGroup on missing group: actual 'CodeGuru Profiler can\u2019t complete the operation because sim2real-cgp-missing-25336558e5b6 was not found.' / required form 'Unable to find profiling group with accountId: 000000000000; profilingGroupName: sim2real-cgp-missing-25336558e5b6 during update'

Cause 3 — GetProfile with no aggregated profile:
  expected: 404 ResourceNotFoundException, message example 'No profiles found for the requested time range!'
  actual:   AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '115', ...}}, 'contentType': 'application/json', 'profile': <botocore.response.StreamingBody object>}
  (reproduced via both period='PT5M' and startTime/endTime forms, immediately after a successful PostAgentProfile)

Cause 4 — ListProfileTimes startTime after endTime (code ValidationException matches):
  expected: 'Interval start time (2026-10-23T13:28:12.123Z) must be before end time (2026-10-23T11:28:12.123Z)'
  actual:   'endTime must not be before startTime'

Cause 5 — TagResource on a nonexistent profiling group ARN:
  expected: Code 'NotFoundException', Message 'Resource was not found'
  actual:   {'Message': 'CodeGuru Profiler can\u2019t complete the operation because sim2real-cgp-missing-b8cbde19a906 was not found.', 'Code': 'ResourceNotFoundException'}

Cause 6 — TagResource with tags={} (code BadRequestException matches):
  expected: 'Invalid request input, expected non-empty tags map'
  actual:   'Invalid request body'

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguruprofiler client at it (region/account as configured; the replay used account 000000000000).
2. Cause 1: CreateProfilingGroup(profilingGroupName=N); GetPolicy(profilingGroupName=N) -> 404 ResourceNotFoundException instead of 200 with no policy/revisionId.
3. Cause 2: CreateProfilingGroup(profilingGroupName=N); DeleteProfilingGroup(profilingGroupName=N); GetPolicy(profilingGroupName=N) and UpdateProfilingGroup(profilingGroupName="never-created", agentOrchestrationConfig={"profilingEnabled": False}) -> both carry the DescribeProfilingGroup-shaped message instead of their own per-operation message naming accountId and the group name.
4. Cause 3: CreateProfilingGroup(profilingGroupName=N, agentOrchestrationConfig={"profilingEnabled": True}); PostAgentProfile(profilingGroupName=N, agentProfile=json of {start,end,callgraph}, contentType="application/json"); then GetProfile(profilingGroupName=N, period="PT5M") and GetProfile(profilingGroupName=N, startTime=now-5m, endTime=now) -> HTTP 200 with a profile body instead of 404 ResourceNotFoundException.
5. Cause 4: ListProfileTimes(profilingGroupName=N, startTime=now+1h, endTime=now-1h, period="PT5M") -> ValidationException whose message omits the interval bounds.
6. Cause 5: TagResource(resourceArn="arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/does-not-exist", tags={"a": "b"}) -> Code ResourceNotFoundException instead of NotFoundException.
7. Cause 6: CreateProfilingGroup(profilingGroupName=N, tags={"seed": "1"}); TagResource(resourceArn=<its arn>, tags={}) -> BadRequestException with 'Invalid request body'.
8. Or replay the checks listed in trace_ids.
