Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy|_error#22|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# PutClusterPolicy accepts a policy whose principal is not a resolvable IAM entity (200) instead of rejecting it with ValidationException

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
Cause 1 of 1: PutClusterPolicy performs no resolvability check on the principals named in the policy document, so a policy referencing a non-existent IAM entity is attached and the policy version is advanced. Failing check: dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy.

AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) models PutClusterPolicy POST /cluster/{identifier}/policy with input members identifier (ClusterId, @httpLabel, pattern ^[a-z0-9]{26}$), policy (PolicyDocument, @length 1..20480), bypassPolicyLockoutSafetyCheck (default false) and expectedPolicyVersion, and declares ValidationException (HTTP 400) among the service-common errors for the operation; the model does not spell out which policy documents are invalid. Recorded behavior (the check states it was verified empirically against real AWS) shows DSQL validates that a policy's principals resolve to real IAM entities: a document whose Principal is arn:aws:iam::<account>:role/<role-that-does-not-exist> is rejected with HTTP 400 / ValidationException and NO policy is attached — the currently attached policy and its policyVersion are unchanged. Recorded message, as an EXAMPLE only (its rule is not stated by the model): "Invalid input for 'policy'. Aurora DSQL can't apply the policy because the policy document isn't valid. Make sure your policy includes all required elements (Version, Statement, Principal, Action, and Resource) in valid JSON format and try again." The requirement is the 400 status, the ValidationException code, and the absence of any state change.

Emulator: the call SUCCEEDS. Replay output: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'policyVersion': '3'}" — the unresolvable-principal document was attached and the version advanced. The handler parses the document as JSON and then applies only the lockout-safety test before storing it; there is no principal-resolvability validation anywhere in the path.

Why: a client that relies on DSQL refusing an unusable resource policy gets silent acceptance of a policy that grants/denies to a principal that cannot exist. The cluster ends up with a policy real AWS would never have stored, the policyVersion token advances (so subsequent optimistic-concurrency calls built against the expected pre-rejection version now conflict), and error-handling paths that depend on ValidationException for malformed policy documents never run — a misconfiguration that AWS surfaces at write time is deferred to a silent authorization surprise.

Fix: the validation stage of the PutClusterPolicy handler — it must validate the policy document's principals before the document is stored and the version is minted, answering the modeled ValidationException (400) and leaving cluster.policy / the version counter untouched.

Traces: failing check dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy (the stale-expectedPolicyVersion ConflictException boundary earlier in the same check PASSES; the failure is at the unresolvable-principal boundary). File citations: services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs lines 227-256 (parse_policy then lockout check, then straight to storing) and lines 299-312 (document stored, version advanced); helper locks_out_every_principal at lines 345-356 is the only policy-content test the handler has.

## Evidence
Expected (recorded AWS): PutClusterPolicy(identifier=<cluster>, policy={"Version":"2012-10-17","Statement":[{"Sid":"DenyUnresolvablePrincipal","Effect":"Deny","Principal":{"AWS":"arn:aws:iam::<account>:role/sim2real-does-not-exist"},"Action":"dsql:DbConnect","Resource":"*"}], expectedPolicyVersion=<current>) -> HTTP 400, Error.Code = ValidationException; a following GetClusterPolicy still returns the PREVIOUS policy document at the PREVIOUS policyVersion. Example recorded message: "Invalid input for 'policy'. Aurora DSQL can't apply the policy because the policy document isn't valid. Make sure your policy includes all required elements (Version, Statement, Principal, Action, and Resource) in valid JSON format and try again."

Actual (emulator): HTTP 200 with body {"policyVersion": "3"}. The check's _error() helper reports: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '21', ...}, 'RetryAttempts': 0}, 'policyVersion': '3'}. The unusable policy is now the attached policy and the version counter has advanced past the one the caller passed as expectedPolicyVersion.

## Steps to Reproduce
1. CreateCluster(deletionProtectionEnabled=false); poll GetCluster until status == ACTIVE.
2. PutClusterPolicy(identifier=<id>, policy=<well-formed Allow policy naming arn:aws:iam::<account>:root>) -> note policyVersion v1.
3. PutClusterPolicy(identifier=<id>, policy=<second well-formed policy>, expectedPolicyVersion=v1) -> note policyVersion v2.
4. PutClusterPolicy(identifier=<id>, policy={"Version":"2012-10-17","Statement":[{"Sid":"DenyUnresolvablePrincipal","Effect":"Deny","Principal":{"AWS":"arn:aws:iam::<account>:role/sim2real-does-not-exist"},"Action":"dsql:DbConnect","Resource":"*"}]}, expectedPolicyVersion=v2).
5. Observe HTTP 200 and a new policyVersion instead of HTTP 400 / ValidationException.
6. GetClusterPolicy(identifier=<id>) — the unresolvable-principal document is now attached (AWS would still show the step-3 document at v2).
