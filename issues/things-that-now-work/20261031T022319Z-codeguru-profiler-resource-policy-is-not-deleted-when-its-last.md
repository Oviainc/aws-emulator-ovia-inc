Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-resource-policy-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|RemovePermission|2xx->2xx|$
Issue-Key: codeguru-profiler|GetPolicy|2xx->2xx|$
Issue-Commit: 26a6b9756b64727e017084c2f7bc8019f98b8844

# Resource policy is not deleted when its last statement is removed

> _Note: this issue was identified against the code base at commit `26a6b9756b64727e017084c2f7bc8019f98b8844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemovePermission

## Description
Two causes, one shared root: the emulator keeps a resource-policy record for a profiling group after the last permission statement has been removed, and serves that residual record (a policy document with an empty Statement array, plus the pre-removal revisionId) from both RemovePermission and the following GetPolicy. On AWS, removing the last action group leaves the profiling group with NO resource policy, and both responses carry no policy/revisionId members at all (empty JSON body).

Cause 1 of 2: RemovePermission returns a synthetic empty policy instead of an empty body once the last statement is gone.
  AWS: recorded behavior only for the empty-policy case. Smithy (codeguruprofiler) models RemovePermission as DELETE /profilingGroups/{profilingGroupName}/policy/{actionGroup} with http code 200 and output RemovePermissionResponse{policy: String (required), revisionId: RevisionId (required, pattern [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})}; the model says nothing about the state in which the group has no policy left. The recording shows AWS answering HTTP 200 with an empty JSON object — i.e. neither policy nor revisionId is serialized — because the removal deleted the policy rather than leaving it with zero statements. Hypothesis: the service drops the policy resource entirely when its last action group is removed, so there is no revision left to report.
  Emulator: HTTP 200 (status matches) with a body carrying policy = '{\n  "Version" : "2012-10-17",\n  "Statement" : [ ]\n}' and revisionId = the revision in effect before the removal. Replay verdict: mismatch at $ — value_mismatch, expected {}, got {policy, revisionId}.
  Why: a client that treats the presence of `policy` as "a resource policy still exists" concludes the profiling group is still shared after it tore the sharing down, and a client that feeds the returned revisionId into the next PutPermission/RemovePermission sends a revision for a policy AWS no longer has (on AWS that call must omit revisionId, or it fails with ConflictException). Teardown/idempotency loops that poll until the policy is gone never terminate.
  Fix: the RemovePermission handler and the resource-policy state for profiling groups in the codeguruprofiler service crate — when the removal empties the statement list, delete the policy record (and its revision) instead of storing an empty-statement document, and have the serializer omit policy/revisionId when no policy exists.
  Traces: codeguruprofiler-resource-policy-lifecycle#task#8.

Cause 2 of 2: GetPolicy on a profiling group with no resource policy serves the residual empty policy instead of an empty body.
  AWS: recorded behavior only. Smithy models GetPolicy as readonly GET /profilingGroups/{profilingGroupName}/policy, http code 200, output GetPolicyResponse{policy: String (required), revisionId: RevisionId (required)}, errors ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). The recording shows that after the policy has been removed AWS still answers 200 (not 404 — the profiling group itself exists) but with an empty JSON object, carrying no policy and no revisionId.
  Emulator: HTTP 200 with policy = '{\n  "Version" : "2012-10-17",\n  "Statement" : [ ]\n}' and the same stale revisionId it returned from the preceding RemovePermission. Replay verdict: mismatch at $ — value_mismatch, expected {}, got {policy, revisionId}.
  Why: GetPolicy is the read path clients use to decide whether a profiling group is shared and to obtain the revisionId for the next optimistic-concurrency write. Reporting a policy that AWS says does not exist makes an unshared group look shared, and hands out a revisionId that AWS would reject; policy-diffing tooling also sees a spurious empty-Statement document where AWS reports none.
  Fix: the GetPolicy handler/serializer in the codeguruprofiler service crate — read the policy state and omit policy/revisionId entirely when the group has no policy, rather than materializing a default document.
  Traces: codeguruprofiler-resource-policy-lifecycle#task#9.

Note: the other codeguru-profiler divergences in this replay are already owned by the open issues 'PostAgentProfile accepts an invalid agent profile with 204...', 'BatchGetFrameMetricData reports one time step too many...' and 'Client-error refusals diverge from AWS...'; none of those covers the resource-policy lifecycle.

## Evidence
Fixture codeguruprofiler-resource-policy-lifecycle, replayed against the emulator (trace set simparity-wire-aws-v1; emulator_up=true, 62/68 steps passed).

Step task#8 — RemovePermission (DELETE /profilingGroups/{name}/policy/{actionGroup}?revisionId=...):
  expected status 200, actual status 200
  expected body: {}
  actual body:   {"policy": "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}", "revisionId": "<pre-removal revision uuid>"}
  replay detail: $: value_mismatch (expected {}, got {'policy': '{\n  "Version" : "2012-10-17",\n  "Statement" : [ ]\n}', 'revisionId': '032d1f82-2b7f-ddd2-87a6-9170257cc2f1'})

Step task#9 — GetPolicy (GET /profilingGroups/{name}/policy):
  expected status 200, actual status 200
  expected body: {}
  actual body:   the same {"policy": "...Statement: [ ]...", "revisionId": "<same revision uuid>"}
  replay detail: $: value_mismatch (expected {}, got {'policy': '{\n  "Version" : "2012-10-17",\n  "Statement" : [ ]\n}', 'revisionId': '032d1f82-2b7f-ddd2-87a6-9170257cc2f1'})

The revision uuid shown above is one run's generated value and is NOT part of the requirement; the requirement is that neither member is serialized at all. Smithy references: RemovePermission output RemovePermissionResponse{policy, revisionId}; GetPolicy output GetPolicyResponse{policy, revisionId}; GetPolicy errors ResourceNotFoundException(404)/ThrottlingException(429)/InternalServerException(500).

## Steps to Reproduce
1. Replay fixture codeguruprofiler-resource-policy-lifecycle against the emulator (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. The fixture creates a profiling group, PUTs a resource-policy action group via PutPermission, then at step task#8 calls RemovePermission for that action group with the current revisionId (DELETE /profilingGroups/{profilingGroupName}/policy/{actionGroup}?revisionId=...). Observe the 200 response body: the emulator returns policy (a document whose Statement array is empty) and revisionId, where AWS returns an empty JSON object.
3. At step task#9 the fixture calls GetPolicy on the same profiling group (GET /profilingGroups/{profilingGroupName}/policy). Observe the 200 response: the emulator again returns the empty-statement policy document and the same revisionId, where AWS returns an empty JSON object.
4. Both steps are reported by the replay as verdict=mismatch with key $ (value_mismatch, expected {}).
