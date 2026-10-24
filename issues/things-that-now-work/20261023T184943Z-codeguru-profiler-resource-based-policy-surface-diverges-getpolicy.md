Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-get-policy-no-policy-not-found
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-get-policy-no-policy-not-found codeguruprofiler-resource-policy-lifecycle codeguruprofiler-error-paths
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|GetPolicy|2xx->4xx|$
Issue-Key: codeguru-profiler|GetPolicy|2xx->4xx|status
Issue-Key: codeguru-profiler|RemovePermission|2xx->2xx|$
Issue-Key: codeguru-profiler|PutPermission|2xx->2xx|$.policy
Issue-Key: codeguru-profiler|GetPolicy|2xx->2xx|$.policy
Issue-Key: codeguru-profiler|RemovePermission|4xx->4xx|$.message
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# CodeGuru Profiler resource-based policy surface diverges: GetPolicy 404s without a policy, emptied-policy responses, document rendering, and two policy-error messages (6 causes)

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy, PutPermission, RemovePermission

## Description
SIX distinct root causes in the codeguruprofiler emulator's resource-based-policy surface (GetPolicy / PutPermission / RemovePermission), every one grounded in a failing step of the simparity-wire-aws-v1 wire replay. ALL SIX must be closed before this issue is resubmitted; each carries its own fixture#step below and in trace_ids, so each must be re-replayed individually.

Related already-open work that does NOT cover these: 20261023T123333Z-codeguruprofiler-codeguru-profiler-getpolicy-404s-on-a-policy-less.md is a BEHAVIORAL issue whose Cause 1 describes the same GetPolicy representation as Cause 1 here, but it carries only behavioral check ids (codeguruprofiler.get-policy-*) and none of the wire replay keys in this issue, so the wire steps below are not gated by it; its Cause 2 covers the per-operation not-found message for a MISSING group, which is a different case from Causes 5 and 6 here (policy-scoped errors on a group that exists). 20261022T213013Z-codeguru-profiler-postagentprofile-accepts-an-invalid-agent-profile-with-204.md owns the PostAgentProfile/GetProfile steps of this corpus and is unrelated to the policy surface.

=== Cause 1 of 6: GetPolicy returns 404 ResourceNotFoundException for an existing profiling group that simply has no resource-based policy ===
AWS: Smithy GetPolicy, GET /profilingGroups/{profilingGroupName}/policy, success code 200; errors ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). Output members policy and revisionId are both @required in the model, but recorded behavior only: for a profiling group that EXISTS and has no policy attached — one that has never had PutPermission called, or whose last statement was removed — AWS answers 200 with an EMPTY body, carrying neither policy nor revisionId, and reserves 404 ResourceNotFoundException for the case where the profiling group itself does not exist. The two conditions are distinct classes.
Emulator: answers 404 ResourceNotFoundException (recorded example message: 'no resource-based policy is attached to profiling group <name>'), conflating 'no policy attached' with 'group not found'.
Why: a client cannot tell an existing-but-unshared group from a deleted one. The standard read-modify-write on a resource policy (GetPolicy, mutate, PutPermission with the revisionId) aborts on a 404 for a perfectly healthy group, and an 'is this group shared?' probe reports the group missing. It is also self-inconsistent with Cause 2, where the emulator itself hands back an emptied policy document for the very state it then 404s on.
Fix: the GetPolicy handler and its serializer (services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_policy.rs) plus the absence path of the store's resource_policy accessor — when the group resolves, answer 200 and omit policy/revisionId; keep 404 ResourceNotFoundException only for a group that does not exist.
Traces: codeguruprofiler-get-policy-no-policy-not-found#task#3 (cluster also covers codeguruprofiler-resource-policy-lifecycle#task#3 and #task#9, i.e. before any PutPermission and after the last RemovePermission).

=== Cause 2 of 6: RemovePermission of the last statement answers with an emptied policy document and a minted revisionId, where AWS answers with an empty body ===
AWS: Smithy RemovePermission, DELETE /profilingGroups/{profilingGroupName}/policy/{actionGroup} with required revisionId (@httpQuery), success code 200; output policy and revisionId @required. Recorded behavior only: removing the only/last action-group statement leaves the group with NO resource-based policy, and the 200 response body is EMPTY — no policy, no revisionId — consistent with what GetPolicy then answers for that group (Cause 1).
Emulator: 200 with policy = '{"Version":"2012-10-17","Statement":[]}' and a freshly minted revisionId.
Why: a client that reads a non-empty policy field as 'this group still has a policy' concludes the group is still shared after it has been fully unshared, and reconciliation loops never converge. Worse, it is handed a revisionId that the store does not keep (the handler mints it and then clears the policy), so the next optimistic-locking write against that revision is refused — the caller holds a revision token for a policy that does not exist.
Fix: the RemovePermission handler/serializer (src/ops/remove_permission.rs, the PolicyWrite::Written arm) and the store (src/state.rs:818-860, which renders the emptied document and mints a revision before clearing the policy) — when the removal leaves no statements, report 'policy gone' and serialize an empty body.
Traces: codeguruprofiler-resource-policy-lifecycle#task#8.

=== Cause 3 of 6: the policy document PutPermission returns has the wrong Sid and the wrong Principal.AWS shape ===
AWS: PutPermission's output policy is 'The JSON-formatted resource-based policy on the profiling group that includes the added permissions' (model doc); the document's content is recorded behavior only. The recording shows, for a single agentPermissions grant to one role: Sid = 'agentPermissions-statement' (the action group name with a '-statement' suffix), Principal.AWS = the single principal ARN as a STRING scalar (not a one-element array), Action = ['codeguru-profiler:ConfigureAgent','codeguru-profiler:PostAgentProfile'], Resource = the profiling group ARN, and the whole document pretty-printed (2-space indent, ' : ' between key and value).
Emulator: Sid = the bare action group 'agentPermissions', Principal.AWS = always a JSON ARRAY, and the document serialized compactly with no whitespace (src/state.rs:277-290 builds it with serde_json::json! using "Sid": action_group and "Principal": {"AWS": principals}).
Why: the Sid and the Principal shape are the policy's semantic content, not presentation. A client that locates its statement by Sid ('agentPermissions-statement') never finds it, so it re-grants instead of updating, or reports the group unshared. Code that reads Principal.AWS in its single-principal string form (the form every AWS policy reader accepts, and the form the recording returns) gets a list and either crashes or stringifies the array. The indentation is recorded formatting rather than a semantic requirement, but it is part of the document AWS returns and any byte-comparison of the policy string also diverges, so render with the same pretty printer.
Fix: the policy document renderer in the store (src/state.rs, ProfilingGroupPolicy::document at 277-290) — suffix the Sid, collapse a single-element principal list to a scalar, and pretty-print.
Traces: codeguruprofiler-resource-policy-lifecycle#task#4 (cluster also covers #task#6, the second PutPermission in the lifecycle).

=== Cause 4 of 6: GetPolicy serves the same mis-rendered policy document on the read path ===
AWS: GetPolicy's output policy member carries the same JSON-formatted document PutPermission returned for that revision — recorded behavior only, and the recording shows the identical pretty-printed document with Sid 'agentPermissions-statement' and Principal.AWS as a string scalar.
Emulator: GetPolicy returns the compact document with Sid 'agentPermissions' and Principal.AWS as an array, i.e. the write path's rendering defect is observable on the read path too.
Why: this is the path a read-modify-write actually reads, so a client that fetches the current policy to amend it sees the wrong Sid and Principal shape even when it never inspected PutPermission's own response. It is listed as its own cause because it is a separate operation and a separate replay step: fixing the renderer for the write path without re-checking GetPolicy leaves this step failing.
Fix: same renderer as Cause 3 (src/state.rs:277-290) reached through src/ops/get_policy.rs — verify both operations after the change.
Traces: codeguruprofiler-resource-policy-lifecycle#task#5.

=== Cause 5 of 6: RemovePermission's no-policy 404 message does not follow AWS's rule (it drops the account scope) ===
AWS: 404 ResourceNotFoundException when the named profiling group exists but has no resource-based policy to remove from. Status (404) and code match the emulator; the message is a requirement here because it is built from the request plus the caller's account — recorded form: 'No policy attached to profiling group name: <profilingGroupName> in accountId: <accountId>; '. The rule (name the profiling group from the request and the account the lookup was scoped to) is the requirement, not the recorded literal text.
Emulator: 'profiling group <name> has no resource-based policy granting <actionGroup>' (src/ops/remove_permission.rs:215-225) — it names the action group instead of the account and does not follow the documented-by-recording form, so the account that scoped the lookup is absent from the diagnostic.
Why: a caller operating across accounts cannot tell from the error WHICH account's view of the group had no policy, which is precisely what this message exists to say; tooling that parses the group name and account out of the message gets neither in the expected positions.
Fix: RemovePermission's PolicyWrite::NoPolicy arm (src/ops/remove_permission.rs) — format the message from the request's profilingGroupName and the caller account, as the store's other group-scoped messages do.
Traces: codeguruprofiler-error-paths#task#6.

=== Cause 6 of 6: RemovePermission's revision-conflict 409 message withholds both revision ids ===
AWS: 409 ConflictException ('The requested operation would cause a conflict with the current state of a service resource associated with the request') when the request's revisionId is not the policy's current revision. Status (409) and code match the emulator; the message is a requirement because it is built from the request's revisionId and the resource's current one — recorded form: 'Policy has been updated since the revision: <requestRevisionId>, new RevisionId: <currentRevisionId>'. The rule (echo the stale revision the caller sent and hand back the revision to retry against) is the requirement, not the literal uuids.
Emulator: 'the revisionId does not match the current revision of the resource-based policy of profiling group <name>; read it with GetPolicy and write against that revision' — src/state.rs:1009ff deliberately withholds the stored revision, which is the opposite of what the recording shows AWS doing.
Why: AWS's conflict is self-healing — the client retries immediately against the new RevisionId carried in the message. A client written against that behavior finds no revision to retry with and either gives up or is forced into an extra GetPolicy round-trip per conflict; under contention that turns a single retry into a livelock-prone read loop. It also hides which stale revision the caller sent, so concurrent writers cannot tell whose write lost.
Fix: the conflict message builder (src/state.rs, revision_conflict_message at 1009ff) and its callers in src/ops/{put_permission.rs,remove_permission.rs} — pass through the request's revisionId and the policy's current revisionId and format both into the message.
Traces: codeguruprofiler-resource-policy-lifecycle#task#7.

## Evidence
All from the simparity-wire-aws-v1 replay (emulator up, health 200; 68 steps, 48 passed, 20 mismatched, 0 errored, 0 blocked, state_reset_failures 0, so none of these ran against a stale fixture's state).

Cause 1 — GetPolicy on an existing policy-less group (fixture codeguruprofiler-get-policy-no-policy-not-found, step task#3; cluster size 3):
  expected: HTTP 200, body {}
  actual:   HTTP 404, body {'__type': 'ResourceNotFoundException', 'message': 'no resource-based policy is attached to profiling group sim2real-cgp-nopolicy-71ced095-87e2-4133-9ab4-ef5481a32663'}
  also affected: codeguruprofiler-resource-policy-lifecycle#task#3 (before any PutPermission), codeguruprofiler-resource-policy-lifecycle#task#9 (after the last RemovePermission)

Cause 2 — RemovePermission of the last statement (fixture codeguruprofiler-resource-policy-lifecycle, step task#8):
  expected: HTTP 200, body {}
  actual:   HTTP 200, body {'policy': '{"Version":"2012-10-17","Statement":[]}', 'revisionId': '595055b5-67cd-00d4-7cab-e3297229066e'}  (the revisionId is an example; the defect is that policy/revisionId are present at all)

Cause 3 — PutPermission policy document (fixture codeguruprofiler-resource-policy-lifecycle, step task#4; cluster also covers task#6). Status 200 matches; $.policy diverges:
  expected: '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "agentPermissions-statement",\n    "Effect" : "Allow",\n    "Principal" : {\n      "AWS" : "arn:aws:iam::000000000000:role/sim2real/<role>"\n    },\n    "Action" : [ "codeguru-profiler:ConfigureAgent", "codeguru-profiler:PostAgentProfile" ],\n    "Resource" : "arn:aws:codeguru-profiler:us-west-2:000000000000:profilingGroup/<group>"\n  } ]\n}'
  actual:   '{"Version":"2012-10-17","Statement":[{"Sid":"agentPermissions","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::000000000000:role/sim2real/<role>"]},"Action":["codeguru-profiler:ConfigureAgent","codeguru-profiler:PostAgentProfile"],"Resource":"arn:aws:codeguru-profiler:us-west-2:000000000000:profilingGroup/<group>"}]}'
  differences: Sid 'agentPermissions-statement' vs 'agentPermissions'; Principal.AWS string scalar vs one-element array; pretty-printed vs compact. Role/group names and the region/account are environment-specific examples.

Cause 4 — GetPolicy read path (fixture codeguruprofiler-resource-policy-lifecycle, step task#5): status 200 matches; $.policy diverges identically to Cause 3.

Cause 5 — RemovePermission with no policy attached (fixture codeguruprofiler-error-paths, step task#6). Status 404 matches:
  expected message form: 'No policy attached to profiling group name: <profilingGroupName> in accountId: <accountId>; '  (recorded: '... name: sim2real-probe-codeguruprofiler-pg-errors-bd415873 in accountId: 000000000000; ')
  actual:                'profiling group sim2real-probe-codeguruprofiler-pg-errors-bd415873 has no resource-based policy granting agentPermissions'

Cause 6 — RemovePermission with a stale revisionId (fixture codeguruprofiler-resource-policy-lifecycle, step task#7). Status 409 matches:
  expected message form: 'Policy has been updated since the revision: <requestRevisionId>, new RevisionId: <currentRevisionId>'  (recorded: '... revision: 0e5b1ecf-1922-8afe-2c40-ef410afc120a, new RevisionId: dace073c-6dff-e88c-8ada-bae079cd211c'; the uuids are examples)
  actual:                'the revisionId does not match the current revision of the resource-based policy of profiling group <group>; read it with GetPolicy and write against that revision'  (no revision id of either kind)

## Steps to Reproduce
Boot the emulator and point a codeguruprofiler client at its data listener (the replay used region us-west-2, account 000000000000).

1. Cause 1: CreateProfilingGroup(profilingGroupName=N); GetPolicy(profilingGroupName=N) -> 404 ResourceNotFoundException instead of 200 with an empty body. Replay fixture codeguruprofiler-get-policy-no-policy-not-found, step task#3.
2. Cause 3/4: PutPermission(profilingGroupName=N, actionGroup='agentPermissions', principals=['arn:aws:iam::000000000000:role/sim2real/<role>']) -> inspect the returned policy: Sid is 'agentPermissions' not 'agentPermissions-statement', Principal.AWS is an array not a string, document is compact. Then GetPolicy(profilingGroupName=N) -> same document. Replay fixture codeguruprofiler-resource-policy-lifecycle, steps task#4 and task#5.
3. Cause 6: RemovePermission(profilingGroupName=N, actionGroup='agentPermissions', revisionId=<a revision other than the current one>) -> 409 ConflictException whose message names neither the stale revision nor the current one. Replay fixture codeguruprofiler-resource-policy-lifecycle, step task#7.
4. Cause 2: RemovePermission(profilingGroupName=N, actionGroup='agentPermissions', revisionId=<current revisionId from GetPolicy>) -> 200 carrying policy '{"Version":"2012-10-17","Statement":[]}' and a revisionId, instead of a 200 with an empty body. Replay fixture codeguruprofiler-resource-policy-lifecycle, step task#8. A following GetPolicy then 404s (Cause 1 again, step task#9).
5. Cause 5: on a group that has never had a policy, RemovePermission(profilingGroupName=N, actionGroup='agentPermissions', revisionId=<any well-formed uuid>) -> 404 whose message names the action group instead of the profiling group plus accountId. Replay fixture codeguruprofiler-error-paths, step task#6.
6. Or re-replay the wire corpus and check the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids.
