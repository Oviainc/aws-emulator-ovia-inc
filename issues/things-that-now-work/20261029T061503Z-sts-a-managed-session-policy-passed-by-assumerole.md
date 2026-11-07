Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-session-policy-arns-intersect-role-permissions
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-session-policy-arns-intersect-role-permissions|_poll#44|Timeout|time.sleep(interval_s)
Issue-Commit: 4373217f8f4cfab6c9129dcbd57530e9c7b55ce8

# STS: a managed session policy passed by AssumeRole PolicyArns alone never narrows the session — iam:ListRoles stays allowed and the check exhausts the 60 s cap

> _Note: this issue was identified against the code base at commit `4373217f8f4cfab6c9129dcbd57530e9c7b55ce8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
One net-new root cause. Cause 1 of 1: `AssumeRole` accepts `PolicyArns` (a MANAGED session policy, passed with no inline `Policy` alongside it) and mints credentials, but the resulting session is not scoped by that policy at all: an action the role's identity policy allows and the managed session policy omits keeps succeeding forever.

AWS: Smithy — `AssumeRole.PolicyArns` targets `policyDescriptorListType` and its documentation states the contract verbatim: "The resulting session's permissions are the intersection of the role's identity-based policy and the session policies… You cannot use session policies to grant more permissions than those allowed by the identity-based policy of the role that is being assumed." A managed session policy passed by ARN therefore narrows exactly as an inline `Policy` does. With a role whose inline policy allows `s3:ListAllMyBuckets` and `iam:ListRoles`, and a managed session policy allowing only `s3:ListAllMyBuckets`, the session may call `s3:ListAllMyBuckets` and the called service refuses `iam:ListRoles` with error code `AccessDenied` at HTTP 403. The message AWS words for that class is fixed text `User: <arn> is not authorized to perform: <action> on resource: <resource>` — its fixed parts are `User: `, ` is not authorized to perform: ` and ` on resource: `; the variable parts are the assumed-role session ARN of the signing credential, the action namespaced as `iam:ListRoles`, and the resource ARN derived from the request, all taken from the request and the emulator's state, never from any one recording. Smithy also models `AssumeRoleResponse.PackedPolicySize` (`nonNegativeIntegerType`, range min 0) and the same member documentation states it "indicates by percentage how close the policies and tags for your request are to the upper size limit", with the managed policy ARNs explicitly among the things compressed into that packed form — so a mint carrying an extra managed session policy must report a strictly larger percentage than the same mint carrying none.

Emulator: the `AssumeRole` call with `PolicyArns=[{arn: <managed policy>}]` returns 200 and usable credentials, the positive control (`s3:ListAllMyBuckets` under that session) succeeds, and `iam:ListRoles` under the SAME session also keeps succeeding — the bounded poll that waits for the `AccessDenied` refusal never sees it, so the check runs to the harness ceiling and reports `Check timed out after 60.0s` with verdict `mismatch`. Nothing in the response diverges; the divergence is that no refusal is ever produced, i.e. the managed session policy was accepted and discarded.

Why: a client relying on AWS's behavior believes a managed session policy is a security boundary. Code that hands a broad role to a subsystem and scopes it down with `PolicyArns` gets, against this emulator, a session with the role's FULL permissions. Every negative test of such a scope-down passes vacuously, so a privilege-escalation defect is green in CI and only appears in production. This is the overly-permissive direction, which is why it is CRITICAL rather than a cosmetic mismatch.

Fix: the `AssumeRole` handler's session-minting path in the STS crate must RECORD the managed session policy ARNs (and resolve them to their policy documents in the IAM store) on the minted session alongside the inline `Policy`, and the services that ACCEPT the credential must intersect that recorded scope with the role's identity policy before deciding — here IAM's own authorization gate, which is what refuses `iam:ListRoles`. The `PackedPolicySize` computation in the same handler must include the managed policy ARNs in the packed form so the reported percentage grows when they are supplied. The shared policy-evaluation component already exists in the tree; what is missing is that a minted session carries its `PolicyArns` scope and that the deciding service reads it.

Related but NOT covering this check: open issue `20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md` describes the same missing session-scope directory for INLINE session policies, permissions boundaries and principal-tag conditions, and carries `sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two` (PolicyArns combined WITH an inline Policy). It does not carry this check's replay key, and this check isolates the PolicyArns-ALONE path plus the `PackedPolicySize` growth assertion, neither of which that issue's trace list exercises. Fixing it may well close this too; this report exists so the PolicyArns-alone path is re-replayed rather than assumed.

Traces: sts.assume-role-session-policy-arns-intersect-role-permissions.

## Evidence
Failing check (behavioral suite `simparity-behavioral-ga-prod-v1`): [mismatch] `sts.assume-role-session-policy-arns-intersect-role-permissions` — `Error: Check timed out after 60.0s`.

Expected (AWS): after `AssumeRole(RoleArn=<role>, RoleSessionName=<s>, PolicyArns=[{arn: <managed-policy>}])`:
  * `PackedPolicySize` is an integer 1..100 and strictly greater than the `PackedPolicySize` of the same assume with no session policy.
  * `s3:ListAllMyBuckets` under the scoped credentials -> HTTP 200.
  * `iam:ListRoles` under the scoped credentials -> HTTP 403, error code `AccessDenied`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: iam:ListRoles on resource: <resource arn>` (fixed text as quoted; account, role name, session name, action and resource come from the request and the emulator's state).
  * The unscoped control session's `iam:ListRoles` still succeeds.

Actual (emulator): the assume succeeds and the credentials are live (`GetCallerIdentity` answers), the permitted `s3:ListAllMyBuckets` answers 200, and `iam:ListRoles` under the SAME scoped session ALSO answers 200 indefinitely. The check's bounded `_poll_denied(..., "AccessDenied", timeout_s=60)` therefore never satisfies and the check is cut off at the 60 s harness cap. The check's own assertion text for this leg is: "PolicyArns must narrow the session exactly as an inline Policy does; iam:ListRoles returned <code> instead of AccessDenied".

## Steps to Reproduce
1. Boot the emulator and replay check id `sts.assume-role-session-policy-arns-intersect-role-permissions`.
2. Manually: `iam.create_role(...)` with a trust policy allowing `sts:AssumeRole` from the account root, and `iam.put_role_policy` granting `["s3:ListAllMyBuckets", "iam:ListRoles"]` on `*`.
3. `iam.create_policy(PolicyDocument=Allow ["s3:ListAllMyBuckets"] on "*")` — create it only; do NOT attach it to the role.
4. `base = sts.assume_role(RoleArn=<role arn>, RoleSessionName="ctl")` and record `base["PackedPolicySize"]`.
5. `scoped = sts.assume_role(RoleArn=<role arn>, RoleSessionName="scoped", PolicyArns=[{"arn": <policy arn>}])`.
6. Observe `scoped["PackedPolicySize"]` is not strictly greater than `base["PackedPolicySize"]` — the managed session policy was never packed in.
7. Build an IAM client from `scoped["Credentials"]` and call `list_roles()`: the emulator answers HTTP 200 where AWS answers HTTP 403 `AccessDenied`. An S3 client from the same credentials calling `list_buckets()` correctly answers 200, so the session is live and only the narrowing is missing.
