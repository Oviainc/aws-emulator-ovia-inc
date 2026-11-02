Issue-Source: trace-sweep-divergence
Issue-Trace: iam-service-linked-role-lifecycle
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-service-linked-role-lifecycle iam-err-service-linked-role-unmodifiable iam-account-authorization-details-read
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRole|4xx->2xx|$
Issue-Key: iam|GetRole|4xx->2xx|status
Issue-Key: iam|GetRole|4xx->2xx|$.ErrorResponse
Issue-Key: iam|ListAttachedRolePolicies|2xx->2xx|$.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies
Issue-Key: iam|GetAccountAuthorizationDetails|2xx->2xx|$.GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Service-linked role deletion never removes the role and attached policy missing

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 3 - DeleteServiceLinkedRole never removes the role. AWS: DeleteServiceLinkedRole starts an asynchronous deletion task and the role does disappear; GetRole on the role name then answers 404 with Code NoSuchEntity and Message 'The role with name <roleName> cannot be found.' (fixed text with the role name substituted). Emulator: GetRole keeps answering 200 with the full Role indefinitely - the fixtures poll for the NoSuchEntity 404 and both exhaust their 60-second cap after 61 attempts. Why: a client that waits for a service-linked role to be gone before recreating it (the documented pattern, since the name is fixed per service principal) waits forever and then recreates into a name that is still occupied. Fix: the DeleteServiceLinkedRole handler must actually remove the role from role state when the deletion task completes, and the deletion-status handler must report that completion.
Traces: iam-service-linked-role-lifecycle#setup#2; iam-service-linked-role-lifecycle#task#6.

Cause 2 of 3 - a service-linked role is created without its service-role policy attached. AWS: CreateServiceLinkedRole attaches the service's own aws-service-role policy, so ListAttachedRolePolicies on the role answers 200 with that one policy - for the Auto Scaling SLR, AttachedPolicies/member{PolicyName 'AutoScalingServiceRolePolicy', PolicyArn 'arn:aws:iam::aws:policy/aws-service-role/AutoScalingServiceRolePolicy'}. Emulator: AttachedPolicies is serialised empty (''). Why: a client auditing that a service-linked role carries its managed policy sees an unpermissioned role, and anything that keys off the attachment (detach-before-delete, drift detection) misbehaves. Fix: attach the service's aws-service-role policy in the CreateServiceLinkedRole handler.
Traces: iam-err-service-linked-role-unmodifiable#task#11.

Cause 3 of 3 - GetAccountAuthorizationDetails serialises role details with the wrong shape. AWS: RoleDetailList carries one member per role with Path, RoleName, Arn and the role's AttachedManagedPolicies, serialised as a repeated member list. Emulator: it emits a single object rather than the member list, and adds InstanceProfileList and RolePolicyList as empty elements where the recording has neither - so a client parsing the authorization-details document gets a different document shape for a single-role account than for a multi-role one. Why: GAAD is what account-wide policy exporters and least-privilege tooling parse; a shape that changes with cardinality breaks them on exactly the accounts they are first tried on. Fix: the GetAccountAuthorizationDetails serializer - always emit RoleDetailList as a repeated member list and omit the sublists the recording does not carry.
Traces: iam-account-authorization-details-read#task#4.

## Evidence
Cause 1: expected (recorded AWS, after DeleteServiceLinkedRole) HTTP 404, ErrorResponse/Error{Type Sender, Code NoSuchEntity, Message 'The role with name AWSServiceRoleForElasticLoadBalancing cannot be found.'}; actual HTTP 200 with GetRoleResult/Role{Path '/aws-service-role/elasticloadbalancing.amazonaws.com/', RoleName AWSServiceRoleForElasticLoadBalancing, ...}, and the replay reports 'poll capped at 60s' and "poll: error code None not in ['NoSuchEntity'] after 61 attempt(s)".
Cause 2: expected ListAttachedRolePoliciesResult/AttachedPolicies/member{PolicyArn 'arn:aws:iam::aws:policy/aws-service-role/AutoScalingServiceRolePolicy', PolicyName 'AutoScalingServiceRolePolicy'}; actual AttachedPolicies = '' (type_mismatch).
Cause 3: expected GetAccountAuthorizationDetailsResult/RoleDetailList/member as a list of role entries {Path, RoleName, Arn, AttachedManagedPolicies}; actual a single object additionally carrying InstanceProfileList '' and RolePolicyList '' (type_mismatch, list vs object).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-service-linked-role-lifecycle: setup#1 DeleteServiceLinkedRole for elasticloadbalancing.amazonaws.com, setup#2 poll GetRole for NoSuchEntity -> GetRole keeps answering 200 until the 60 s cap; the same at task#6.
3. Replay iam-err-service-linked-role-unmodifiable#task#11 - ListAttachedRolePolicies on the Auto Scaling SLR returns an empty AttachedPolicies.
4. Replay iam-account-authorization-details-read#task#4 and compare the RoleDetailList serialisation against the recording.
