Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped iam.iam-simulate-principal-policy iam.op-contract-listInstanceProfiles-missing-permission-denied iam.op-contract-listOpenIDConnectProviders-explicit-deny-rejected iam.op-contract-listUsers-missing-permission-denied iam.assumed-role-session-is-authorized-by-the-role-policy iam.session-policy-intersects-the-role-policy iam.permissions-boundary-condition-key-gates-a-delegated-create-user iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext iam.op-contract-getContextKeysForCustomPolicy iam.get-context-keys-for-custom-policy iam.op-contract-generateOrganizationsAccessReport
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped|run#27|AssertionError|assert sources == {f"user_{user}_{_DENY}"}, (
Issue-Key: iam.iam-simulate-principal-policy|run#21|AssertionError|assert sources == [f"user_{user}_{deny_name}"], (
Issue-Key: iam.op-contract-listInstanceProfiles-missing-permission-denied|run#22|AssertionError|assert sources == {f"user_{user}_{_DENY}"}, (
Issue-Key: iam.op-contract-listOpenIDConnectProviders-explicit-deny-rejected|run#40|AssertionError|assert sources == {f"user_{user}_{_DENY}"}, (
Issue-Key: iam.op-contract-listUsers-missing-permission-denied|run#37|AssertionError|assert sources == {f"user_{deny_user}_{_DENY}"}, (
Issue-Key: iam.assumed-role-session-is-authorized-by-the-role-policy|_denial_message#29|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: iam.session-policy-intersects-the-role-policy|run#18|AssertionError|assert narrow_list == "AccessDenied", (
Issue-Key: iam.permissions-boundary-condition-key-gates-a-delegated-create-user|_poll#23|AssertionError|raise AssertionError(
Issue-Key: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied|_denied_code#26|AssertionError|raise AssertionError(f"expected AccessDenied, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext|_settle#22|Timeout|time.sleep(interval_s)
Issue-Key: iam.op-contract-getContextKeysForCustomPolicy|run#7|AssertionError|assert expected <= keys, f"missing context keys: {expected - keys}, got {keys}"
Issue-Key: iam.get-context-keys-for-custom-policy|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-generateOrganizationsAccessReport|run#9|AssertionError|assert message == _GATE_MESSAGE, f"expected message {_GATE_MESSAGE!r}, got {message!r}"
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam: 9 net-new authorization and policy-evaluation root causes — simulate SourcePolicyId unqualified, IAM's own API never authorized under a session, session Policy ignored, iam:PermissionsBoundary and iam:PolicyARN conditions not evaluated

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
Nine distinct net-new root causes in IAM's authorization and policy-evaluation surface. Each must be closed before this issue is submitted.

Cause 1 of 9: SimulatePrincipalPolicy's MatchedStatements do not carry AWS's qualified SourcePolicyId for an inline policy.
AWS: for a statement that came from an inline policy attached to a user, the matched statement's SourcePolicyId is the qualified form 'user_<UserName>_<PolicyName>' (fixed 'user_' prefix, the entity name, '_', the inline policy name), and exactly one statement is matched for the deny under test.
Emulator: SourcePolicyId is the bare inline policy name '<PolicyName>' with no 'user_<UserName>_' qualification.
Why: a client that attributes a decision to the policy that produced it (the whole point of the simulate API) cannot tell which entity's inline policy matched, and cannot join the id back to the policy it read.
Fix: the MatchedStatements/SourcePolicyId construction in the policy simulator's result assembly.
Traces: iam.explicit-deny-blocks-a-real-signed-call-and-is-scoped, iam.iam-simulate-principal-policy, iam.op-contract-listInstanceProfiles-missing-permission-denied, iam.op-contract-listOpenIDConnectProviders-explicit-deny-rejected, iam.op-contract-listUsers-missing-permission-denied

Cause 2 of 9: IAM's own control plane performs no authorization for calls made under an assumed-role session.
AWS: a signed IAM read made with an assumed-role session whose role policy does not grant it is refused with AccessDenied.
Emulator: ListAttachedRolePolicies answers HTTP 200 with {'AttachedPolicies': [], 'IsTruncated': False} for a session AWS refuses.
Why: every IAM least-privilege assertion is unenforced — a session can read IAM state it is not granted, and a negative control cannot fail.
Fix: IAM's request authorization layer (evaluate the caller's identity policies for iam:* actions, for role sessions as well as users).
Traces: iam.assumed-role-session-is-authorized-by-the-role-policy

Cause 3 of 9: AssumeRole's session Policy parameter is ignored, so the session is not the intersection of the role policy and the session policy.
AWS: an action allowed by the role's permissions policy but absent from the inline session policy passed to AssumeRole is refused with exactly AccessDenied.
Emulator: the call succeeds (the check records 'UNEXPECTED_SUCCESS'), so the session carries the role's full permissions.
Why: scoped-down sessions — the standard way callers hand out least-privilege credentials — over-permit, silently.
Fix: STS AssumeRole must persist the session Policy with the session, and the authorization layer must intersect it with the role's policies.
Traces: iam.session-policy-intersects-the-role-policy

Cause 4 of 9: the iam:PermissionsBoundary condition key is never satisfied, so a delegated CreateUser grant conditioned on it never takes effect.
AWS: a delegating identity policy that allows iam:CreateUser only when iam:PermissionsBoundary equals a given boundary ARN admits a CreateUser request that sets exactly that boundary.
Emulator: that request stays AccessDenied for the whole ~55 s poll window — the condition never matches.
Why: the canonical permissions-boundary delegation pattern is unusable; a correctly-granted administrator is locked out.
Fix: the condition-key resolution in IAM's policy evaluator must populate iam:PermissionsBoundary from the request's PermissionsBoundary parameter.
Traces: iam.permissions-boundary-condition-key-gates-a-delegated-create-user

Cause 5 of 9: the iam:PolicyARN condition key is not evaluated, so an attach scoped to one policy ARN admits any.
AWS: an identity policy allowing iam:AttachUserPolicy only when iam:PolicyARN equals one ARN refuses an attach of a different policy with AccessDenied.
Emulator: AttachUserPolicy answers HTTP 200 for the non-matching policy ARN.
Why: the delegation is wide open — the delegate can attach any policy, including an administrative one.
Fix: the condition-key resolution in IAM's policy evaluator must populate iam:PolicyARN from the request's PolicyArn parameter.
Traces: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied

Cause 6 of 9: the aws:SecureTransport policy-composition check reaches no verdict — it is killed at the 60 s cap.
AWS: a policy requiring aws:SecureTransport false refuses a call made over TLS, and the decision is reached immediately.
Emulator: the check produces no result at all within 60 s, so no decision is ever observable (recorded: 'Check timed out after 60.0s').
Why: a client cannot determine whether the condition is evaluated; the whole secure-transport condition is unverifiable, and an operation that never returns is itself a divergence.
Fix: the evaluation/propagation path for aws:SecureTransport in IAM's policy evaluator (and whatever poll it is blocking on).
Traces: iam.sep-policy-composition-securetransport-condition-denied-when-policy-requires-plaintext

Cause 7 of 9: GetContextKeysForCustomPolicy omits aws:username from the keys it reports.
AWS: the operation returns every condition key the submitted policy documents reference; a document referencing aws:username lists it in ContextKeyNames. Smithy: GetContextKeysForCustomPolicy returns ContextKeyNames (ContextKeyNamesResultListType) and models only InvalidInputException.
Emulator: ContextKeyNames comes back as {'aws:CurrentTime', 'aws:MultiFactorAuthPresent'} with aws:username missing.
Why: a caller enumerating the keys it must supply to a simulation omits one, so the subsequent simulation evaluates under an incomplete context.
Fix: the condition-key extraction in the get_context_keys_for_custom_policy handler (it must walk all Condition blocks and policy variables).
Traces: iam.op-contract-getContextKeysForCustomPolicy

Cause 8 of 9: GetContextKeysForCustomPolicy's malformed-document refusal is re-worded.
AWS: a syntactically invalid entry in PolicyInputList is refused with the message 'Policy input list item <n> has invalid content' — fixed text 'Policy input list item ' + the 1-based index of the offending entry + ' has invalid content'. Smithy: the operation models InvalidInputException (code InvalidInput, HTTP 400).
Emulator: answers 'Syntax errors in policy.' — generic text that names no entry.
Why: a caller submitting several documents cannot tell which one was rejected.
Fix: the InvalidInput message construction in the get_context_keys_for_custom_policy handler.
Traces: iam.get-context-keys-for-custom-policy

Cause 9 of 9: GenerateOrganizationsAccessReport's refusal for an unauthorized caller uses the wrong message.
AWS: the refusal message is 'You are not authorized to perform this operation.'
Emulator: answers 'To call this operation, you must be signed in using the Organizations management account credentials of organization o-simparityorgid.' — a different sentence that additionally discloses an organization id.
Why: a client matching AWS's authorization message diverges, and the emulator leaks an organization identifier AWS does not put in this refusal.
Fix: the refusal-message construction in the generate_organizations_access_report handler.
Traces: iam.op-contract-generateOrganizationsAccessReport

## Evidence
Cause 1 — expected matched sources `{'user_<user>_<inline-policy>'}` (e.g. 'user_sim2real-behav-user-a033cf27a006_sim2real-behav-deny-1d39b1563d2e'), got `['<inline-policy>']` (e.g. ['sim2real-behav-deny-1d39b1563d2e']); same shape in the three op-contract list checks.
Cause 2 — expected AccessDenied. actual: `real AWS refuses this call, but it SUCCEEDED and returned {'AttachedPolicies': [], 'IsTruncated': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 3 — `iam:ListAttachedRolePolicies is allowed by the role policy but absent from the session policy, so it must be refused with exactly AccessDenied; got 'UNEXPECTED_SUCCESS' -- an emulator that ignores the Policy parameter over-permits here`.
Cause 4 — `the conditioned iam:CreateUser grant takes effect for the required boundary did not happen within 54.87s; last observation 'AccessDenied'`.
Cause 5 — expected AccessDenied. actual: `expected AccessDenied, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 6 — `Check timed out after 60.0s`.
Cause 7 — `missing context keys: {'aws:username'}, got {'aws:CurrentTime', 'aws:MultiFactorAuthPresent'}`.
Cause 8 — expected message `'Policy input list item 1 has invalid content'`, got `'Syntax errors in policy.'`.
Cause 9 — expected message `'You are not authorized to perform this operation.'`, got `'To call this operation, you must be signed in using the Organizations management account credentials of organization o-simparityorgid.'`.

## Steps to Reproduce
(1) Put an inline deny policy on an IAM user, call SimulatePrincipalPolicy for the denied action and read MatchedStatements[].SourcePolicyId. (2) Assume a role whose policy does not grant iam:ListAttachedRolePolicies and call it with the session credentials. (3) Call sts:AssumeRole with an inline Policy omitting an action the role policy allows, then call that action. (4) Grant iam:CreateUser conditioned on iam:PermissionsBoundary == <arn> and call CreateUser with PermissionsBoundary=<arn>. (5) Grant iam:AttachUserPolicy conditioned on iam:PolicyARN == <arn> and attach a different policy. (6) Run the aws:SecureTransport policy-composition check. (7) Call GetContextKeysForCustomPolicy with a document referencing aws:username. (8) Call it with a syntactically invalid document. (9) Call GenerateOrganizationsAccessReport as a non-management-account principal.
