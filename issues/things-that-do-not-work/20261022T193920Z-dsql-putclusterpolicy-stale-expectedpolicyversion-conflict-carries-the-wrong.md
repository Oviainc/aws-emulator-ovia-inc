Issue-Severity: HIGH
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# dsql PutClusterPolicy: stale expectedPolicyVersion conflict carries the wrong message, and the policy-lockout safety check lets a deny-all policy through

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
Two distinct root causes in the dsql PutClusterPolicy handler. Both are grounded in failing behavioral checks; neither is covered by the two open dsql issues (`dsql-getclusterpolicy-stub`, `dsql-untagresource-stub`), which are about unimplemented scaffold stubs for other operations.

Cause 1 of 2: ConflictException for a stale/unknown `expectedPolicyVersion` uses the emulator's own wording instead of Aurora DSQL's.
AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) models PutClusterPolicy with `expectedPolicyVersion` (PolicyVersion) and errors ConflictException (HTTP 409) / ResourceNotFoundException (HTTP 404) / ValidationException (HTTP 400). The model does not state the message text; recorded behavior shows AWS answers a mismatched `expectedPolicyVersion` with code `ConflictException` and a message BUILT FROM THE REQUEST, naming the version the caller claimed and the resource it claimed it for: "Aurora DSQL can't find policy version <expectedPolicyVersion> for resource <identifier>. Verify that the policy exists and that the version is correct." The rule (name the requested version and the identifier in that sentence) is the requirement, not one recording's literal values. The emulator's own state module already encodes exactly that rule.
Emulator: the HTTP status and error code are correct (409 / ConflictException) but the message is a different sentence, e.g. 'The policy version of cluster 01m5jhgkr2wwtteke9g69b0c5g is not 1.' — it neither names the sentence AWS words this case with nor matches the wording the service's own helper produces for the adjacent 404 case. All four grounded checks fail only on this message.
Why: a client that keys off the refusal text (optimistic-concurrency retry loops, user-facing surfacing of a policy-version conflict, log scrapers) sees text it will never see from AWS, and the wording does not tell the caller which version was not found. In the concurrency race check, all 5 losing racers produce the same wrong sentence, so a client cannot distinguish a version conflict reason from a generic conflict.
Fix: the PutClusterPolicy handler's `expectedPolicyVersion` branch — it should word the conflict with the same request-derived rule the service's existing "policy version not found" helper already applies, rather than formatting a bespoke sentence inline.
Traces: services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs lines 273-296 formats the conflict message inline; services/service-com-amazonaws-dsql-dsql/src/state.rs `cluster_policy_version_not_found` (around line 890) already carries the documented wording for the sibling 404 path. Failing checks: dsql.create-cluster-inline-policy-attached, dsql.put-cluster-policy-version-conflict-rejected, dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy, dsql.put-cluster-policy-concurrent-version-race.

Cause 2 of 2: the policy-lockout safety check does not fire for a Deny-all policy scoped to the cluster's OWN ARN, so an un-replaceable policy is ACCEPTED with HTTP 200.
AWS: Smithy models `bypassPolicyLockoutSafetyCheck` (BypassPolicyLockoutSafetyCheck, default false) on PutClusterPolicy, and ValidationException (HTTP 400) among its errors. Recorded behavior: with the default false, a policy whose single statement is Effect=Deny / Principal="*" / Action="dsql:*" / Resource=<the cluster's own ARN> is refused with code `ValidationException`, nothing is attached (a following GetClusterPolicy answers `ResourceNotFoundException`), and the message tells the caller to set the bypass flag — recorded example: "The new policy would result in the calling principal being unable to further update and delete this cluster's policy. Set the 'bypassPolicyLockoutSafetyCheck' parameter to 'true' to confirm this intent." The requirement graded here is the 400 / ValidationException classification and the absent effect; the exact sentence is an example.
Emulator: the call SUCCEEDS — HTTP 200 with a body of {"policyVersion": "1"} — and the deny-all policy is attached. The emulator does implement a lockout check, but it only treats a statement as covering the resource when the `Resource` value contains a `*` wildcard; a statement naming the cluster's exact ARN (which is precisely the resource being protected) is therefore judged not to cover it and the check is skipped. The same gap means `Action: "dsql:*"` against the literal ARN passes.
Why: this is over-permissive — the emulator accepts input real AWS rejects. A caller testing its guardrails against the emulator concludes a cluster-locking policy is a legal configuration; on real AWS the identical call is a 400, and in the emulator the cluster is left with a policy that, by the very rule the check exists to enforce, no principal can ever replace or delete.
Fix: the PutClusterPolicy handler's lockout-safety predicate (its resource-coverage test) must treat the target cluster's own ARN — not only a wildcard — as covering the resource the policy is being attached to, and must reject with ValidationException attaching nothing.
Traces: services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs lines 232-248 (the gate) and lines 406-416 (`covers_every_resource`, which requires `resource.contains('*')`). Failing check: dsql.put-cluster-policy-lockout-safety-rejected.

## Evidence
Cause 1 — expected (AWS, request-derived rule): code `ConflictException`, message "Aurora DSQL can't find policy version <expectedPolicyVersion> for resource <identifier>. Verify that the policy exists and that the version is correct."
Actual (emulator): code `ConflictException`, message 'The policy version of cluster 01m5jheakxg6n6xg74wc10sk0a is not 2.'
From dsql.put-cluster-policy-concurrent-version-race, all 5 losing racers: [('lost', 'ConflictException', 'The policy version of cluster 01m5jh443s7364cv2v4x77b72m is not 1.', 1), ... , ('lost', 'ConflictException', 'The policy version of cluster 01m5jh443s7364cv2v4x77b72m is not 1.', 5)] — correct code, wrong sentence in every one.
From dsql.put-cluster-policy-version-conflict-rejected: AssertionError: The policy version of cluster 01m5jhgkr2wwtteke9g69b0c5g is not 1.

Cause 2 — expected (AWS): HTTP 400, code `ValidationException`, no policy attached (subsequent GetClusterPolicy -> `ResourceNotFoundException`).
Actual (emulator): HTTP 200 and the policy attached. Replay output: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'policyVersion': '1'}
The policy submitted was {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"dsql:*","Resource":"<the cluster's own ARN>"}]} with `bypassPolicyLockoutSafetyCheck` left at its modeled default of false.

## Steps to Reproduce
Cause 1:
1. CreateCluster(deletionProtectionEnabled=false) and poll GetCluster until status=ACTIVE.
2. PutClusterPolicy(identifier=<id>, policy=<any valid JSON policy>) -> note the returned policyVersion V.
3. PutClusterPolicy(identifier=<id>, policy=<any valid JSON policy>, expectedPolicyVersion=<any value other than the current one, e.g. V+1 or a superseded V>).
4. Observe HTTP 409 / ConflictException whose Message is 'The policy version of cluster <id> is not <requested>.' instead of "Aurora DSQL can't find policy version <requested> for resource <id>. Verify that the policy exists and that the version is correct."

Cause 2:
1. CreateCluster(deletionProtectionEnabled=false); capture `arn`; poll GetCluster until status=ACTIVE.
2. PutClusterPolicy(identifier=<id>, policy=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"dsql:*","Resource":<arn>}]})) with bypassPolicyLockoutSafetyCheck omitted.
3. Observe HTTP 200 + policyVersion in the body; AWS answers HTTP 400 ValidationException and attaches nothing.
4. GetClusterPolicy(identifier=<id>) confirms the emulator attached the deny-all policy.
