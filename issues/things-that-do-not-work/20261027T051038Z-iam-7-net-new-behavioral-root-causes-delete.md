Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.delete-instance-profile-with-role iam.delete-role-with-inline-policy iam.detach-not-attached-policy iam.instance-profile-single-role-constraint iam.policy-missing-statement iam.update-service-linked-role-unmodifiable-rejected iam.group-policy-explicit-deny-overrides-the-users-own-allow iam.op-contract-list-users-live-enforcement-denied
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.delete-instance-profile-with-role|run#18|AssertionError|assert message == "Cannot delete entity, must remove roles from instance profile first.", (
Issue-Key: iam.delete-role-with-inline-policy|run#16|AssertionError|assert conflict_err.get("Message") == "Cannot delete entity, must delete policies first.", (
Issue-Key: iam.detach-not-attached-policy|run#15|AssertionError|assert message == f"Policy {unattached_arn} was not found.", (
Issue-Key: iam.instance-profile-single-role-constraint|run#14|AssertionError|assert message == "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1", (
Issue-Key: iam.policy-missing-statement|run#11|AssertionError|assert message == "Syntax errors in policy.", (
Issue-Key: iam.update-service-linked-role-unmodifiable-rejected|run#7|AssertionError|assert code1 == "UnmodifiableEntity", (
Issue-Key: iam.group-policy-explicit-deny-overrides-the-users-own-allow|_poll#23|AssertionError|raise AssertionError(
Issue-Key: iam.op-contract-list-users-live-enforcement-denied|_poll_for_code#16|Timeout|time.sleep(interval_s)
Issue-Commit: 4846872b237bc4d9f92d9945507291eab648a721

# iam: 7 net-new behavioral root causes — delete-conflict/detach/quota/policy-syntax refusal messages are emulator prose, UpdateAssumeRolePolicy cannot see a service-linked role, and group policies and an explicit Deny on iam:ListUsers are never evaluated

> _Note: this issue was identified against the code base at commit `4846872b237bc4d9f92d9945507291eab648a721`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInstanceProfile

## Description
Seven distinct net-new root causes in the bootstrapped `iam` service. None of the eight failing checks below is marked ALREADY FILED or OWNED in the replay report; every operation named answers (no scaffold stub is on any of these paths). A fixer must close all SEVEN causes before submitting.

Nearest open neighbours, named so they are not re-fixed or mistaken for coverage: 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md (its Cause 5 covers the DeleteConflict text for DeleteRole-with-attached-managed-policy and DeleteInstanceProfile via other check ids, and its Cause 8 covers unparseable-JSON / no-Statement text for CreatePolicy+CreateRole), 20261027T000203Z-iam-4-net-new-root-causes-createpolicyversion-s.md (its Cause 2 is the same policy-document syntax message, reached through iam.malformed-policy-document-rejected), 20261026T024842Z-iam-getpolicy-s-nosuchentity-message-says-does-not.md (GetPolicy's NoSuchEntity text, not DetachRolePolicy's), 20261025T170540Z-iam-4-net-new-root-causes-the-authorization.md (the authorization gate is wired into only a handful of operations) and 20261026T051836Z-iam-deleterolepermissionsboundary-is-an-unimplemented-501-scaffold-stub.md (service-linked-role protection on a different operation). Causes 1 and 4 below reach the same message constants from check ids none of those issues carries; causes 2, 3, 5, 6 and 7 are new behavior.

Cause 1 of 7: the DeleteConflict refusal messages for a role-holding instance profile and for a role with an inline policy are emulator-authored prose that interpolates entity names AWS never includes.
AWS: Smithy iam#DeleteInstanceProfile and iam#DeleteRole both model DeleteConflictException (awsQueryError code DeleteConflict, httpResponseCode 409). Recorded behavior for the message text: deleting an instance profile that still holds a role is refused with exactly "Cannot delete entity, must remove roles from instance profile first." and deleting a role that still has an inline policy with exactly "Cannot delete entity, must delete policies first." — both messages are entirely FIXED text with no variable part: no role name, no instance-profile name, no counts. The HTTP status and the error code the emulator returns are already correct; only the message bodies diverge.
Emulator: the instance-profile delete is refused with "Cannot delete entity: the role <roleName> must be removed from instance profile <profileName> first." and the role delete with "Cannot delete entity: 1 inline policy must be deleted from the role <roleName> first." — different templates that splice in the blocking entity's name and a count taken from emulator state.
Why: clients, runbooks and IaC cleanup loops branch on IAM's exact DeleteConflict sentence to decide which remediation to run (remove-roles-from-profile vs delete-policies vs detach-policies); a reworded, resource-bearing message defeats that match, and the emulator's text additionally discloses which entity blocked the delete, which AWS deliberately does not.
Fix: the refusal-message constants in the DeleteInstanceProfile and DeleteRole handlers' conflict checks (the inline-policy branch of DeleteRole is a separate branch from the attached-managed-policy branch that the neighbour issue above cites, and both must emit AWS's fixed sentences).
Traces: iam.delete-instance-profile-with-role, iam.delete-role-with-inline-policy.

Cause 2 of 7: DetachRolePolicy's NoSuchEntity message is rewritten to name the role, where AWS's message names only the policy.
AWS: Smithy iam#DetachRolePolicy takes RoleName (required, roleNameType) and PolicyArn (required, arnType @length 20-2048) and models NoSuchEntityException (awsQueryError code NoSuchEntity, httpResponseCode 404). Recorded behavior: detaching a policy that is not attached to the named role is refused with the message "Policy <policyArn> was not found." — fixed text "Policy " + " was not found." with one variable part, the PolicyArn taken verbatim from the request. The role name does not appear.
Emulator: the same call is refused with the right code but the message "Policy <policyArn> was not found attached to role <roleName>." — AWS's sentence with its terminator replaced and a second, role-naming clause appended.
Why: a client cannot distinguish IAM's not-found wording from an emulator variant, and the appended clause makes the message vary per role where AWS's does not, so any exact-match or regex on IAM's text fails.
Fix: the NoSuchEntity message rendering in the DetachRolePolicy handler's attachment lookup.
Traces: iam.detach-not-attached-policy.

Cause 3 of 7: AddRoleToInstanceProfile's LimitExceeded refusal names the wrong quota.
AWS: Smithy iam#AddRoleToInstanceProfile models LimitExceededException (awsQueryError code LimitExceeded, httpResponseCode 409). An instance profile may hold exactly one role, and the recorded refusal for adding a second is exactly "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1" — fixed text "Cannot exceed quota for InstanceSessionsPerInstanceProfile: " plus the quota value (1). The quota NAME in that message is part of the contract, since it is what identifies which of the operation's several LimitExceeded causes fired.
Emulator: the refusal carries the right code and the right limit value but a different quota name: "Cannot exceed quota for RolesPerInstanceProfile: 1".
Why: the quota name is the only machine-readable discriminator in a LimitExceeded body; tooling that maps IAM quota names to Service Quotas entries or to remediation (and documentation that lists InstanceSessionsPerInstanceProfile) cannot resolve an invented name, and support/automation keyed on it misroutes.
Fix: the quota-name constant used by the single-role check in the AddRoleToInstanceProfile handler.
Traces: iam.instance-profile-single-role-constraint.

Cause 4 of 7: CreatePolicy's MalformedPolicyDocument message for a document with no Statement appends a generated diagnosis to AWS's fixed sentence.
AWS: Smithy iam#CreatePolicy models MalformedPolicyDocumentException (awsQueryError code MalformedPolicyDocument, httpResponseCode 400). Recorded behavior for a PolicyDocument that is valid JSON but carries no Statement element: the message is exactly "Syntax errors in policy." — the whole message is fixed text, with NO variable part and no diagnostic detail at all.
Emulator: the same document is refused with the right code but "Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element" — AWS's sentence with its terminating period replaced by a colon and an emulator-authored explanation appended. The check's positive control (a well-formed document accepted and read back through GetPolicy) passed, and the rejected policy correctly does not persist, so only the message text diverges.
Why: AWS gives no detail here on purpose; a client comparing or logging IAM's message sees a longer, differently punctuated string, and the appended clause will drift with the emulator's parser while AWS's text never changes.
Fix: the shared policy-document validator that produces MalformedPolicyDocumentException for this service must emit AWS's fixed message verbatim and keep its diagnosis internal. This is the same validator path as Cause 2 of 20261027T000203Z-iam-4-net-new-root-causes-createpolicyversion-s.md; this cause adds the CreatePolicy/no-Statement replay key that issue does not carry, and both should be closed together.
Traces: iam.policy-missing-statement.

Cause 5 of 7: UpdateAssumeRolePolicy on an existing service-linked role answers NoSuchEntity instead of UnmodifiableEntity, so a service-linked role's trust policy carries no protection.
AWS: Smithy iam#UpdateAssumeRolePolicy takes RoleName (required) and PolicyDocument (required) and models UnmodifiableEntityException (awsQueryError code UnmodifiableEntity, httpResponseCode 400) alongside NoSuchEntityException (NoSuchEntity, 404). A service-linked role (path /aws-service-role/<service>/, e.g. AWSServiceRoleForElasticBeanstalk) is modifiable by AWS only — the one documented caller-permitted mutation is its description — so a trust-policy update is refused UnmodifiableEntity 400 with the message "Cannot perform the operation on the protected role '<roleName>' - this role is only modifiable by AWS" (fixed text around one variable part, the RoleName from the request), and the stored trust policy is left unchanged.
Emulator: the role plainly exists — the check's immediately preceding GetRole(RoleName=AWSServiceRoleForElasticBeanstalk) returned the role and its AssumeRolePolicyDocument — yet UpdateAssumeRolePolicy for that same name is refused with code NoSuchEntity. The handler's role resolution does not see roles under the service-linked-role path, so the UnmodifiableEntity guard is never reached and the operation reports the role as absent.
Why: two separate contracts break. A client is told a role it just read does not exist, which is self-contradictory and makes error handling (recreate? retry?) impossible; and the protection that stops anyone from rewriting a service-linked role's trust policy is simply not there — any caller that retries past the 404, or any code path that resolves the role by a different route, has no UnmodifiableEntity barrier.
Fix: the role lookup in the UpdateAssumeRolePolicy handler must resolve roles stored under the /aws-service-role/ path and, for a service-linked role, return the modeled UnmodifiableEntityException with AWS's protected-role message instead of falling through to not-found. (Related but distinct: 20261026T051836Z-iam-deleterolepermissionsboundary-is-an-unimplemented-501-scaffold-stub.md is the same protection on another operation.)
Traces: iam.update-service-linked-role-unmodifiable-rejected.

Cause 6 of 7: identity-based policies attached to a user's GROUPS are not part of that user's effective policy set, so an explicit Deny in a group policy never refuses the member's signed call.
AWS: IAM policy evaluation collects every identity-based policy that applies to the requesting principal — the user's own inline and attached policies AND the inline and attached policies of every group the user belongs to — and an explicit Deny in any of them overrides every Allow. Smithy iam#GetUser models only NoSuchEntityException and ServiceFailureException, so the authorization refusal is IAM's unmodeled AccessDenied (code AccessDenied, HTTP 403) whose message follows AWS's fixed template "User: <caller ARN> is not authorized to perform: <action> on resource: <resource ARN> with an explicit deny in an identity-based policy" — fixed text with the caller ARN, the action and the resource ARN taken from the request and the caller's identity.
Emulator: with the user holding its own inline Allow on iam:GetUser+iam:ListUserTags for its own ARN, added to a group by AddUserToGroup (GetGroup confirms the single member) and the group given an inline policy whose only statement is Deny on iam:GetUser for that same ARN (GetGroupPolicy confirms Effect=Deny was stored), the member's real signed GetUser keeps returning success for the whole ~55 s convergence window — observation 'OK' at the deadline. The group edge and the group policy are both readable; they just contribute nothing to the authorization decision.
Why: groups are the standard way to administer permissions for users, and a group-level Deny is the standard guardrail. Here it is inert: the console/API reports the Deny, every audit tool reads it back, and the action it forbids still succeeds. Any security control expressed through a group — and any test of one — silently passes while granting access.
Fix: the policy-collection step of the iam crate's authorization component must union the caller's own inline and attached policies with the inline and attached policies of every group the caller is a member of before evaluating effect precedence (explicit Deny wins).
Traces: iam.group-policy-explicit-deny-overrides-the-users-own-allow.

Cause 7 of 7: an explicit Deny naming iam:ListUsers in the calling role's own inline policy is never enforced on a real signed call.
AWS: Smithy iam#ListUsers takes an optional PathPrefix and models only ServiceFailureException, so an authorization refusal is IAM's unmodeled AccessDenied (code AccessDenied, HTTP 403). With the caller's identity policy carrying Allow on * plus a statement Deny on iam:ListUsers, the explicit Deny wins and the call is refused AccessDenied 403 with the message "User: <assumed-role session ARN> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user<pathPrefix> with an explicit deny in an identity-based policy" — fixed text with the session ARN, the account id and the requested path prefix as its variable parts; removing the Deny restores success.
Emulator: the positive control passes (with only the broad Allow, the assumed role's signed ListUsers succeeds), then PutRolePolicy stores an inline Deny whose only statement is Deny iam:ListUsers on *, and the signed ListUsers keeps succeeding — the check polls for AccessDenied and is killed at the 60 s harness cap having never seen a refusal. The Deny is accepted, stored and readable; it has no effect on the operation.
Why: the live-enforcement contract — the thing that makes an IAM policy a control rather than a document — does not hold for ListUsers. A tenant that revokes a principal's ability to enumerate users by attaching a Deny still has that principal enumerating users, and nothing in the API surface reveals it. Note that the identical failure for iam:ListGroups was filed and marked fixed (20261025T015953Z-iam-an-explicit-deny-naming-iam-listgroups-is.md); ListUsers was not covered by that fix and is not owned by any open issue in this report, which indicates the gate is still per-operation rather than applied by the request authorizer.
Fix: route ListUsers through the same request-authorizer gate the fixed ListGroups path now uses, in the iam crate's authorization component, so that every served operation evaluates the caller's identity policies (and honours explicit Deny) rather than only an enumerated subset of handlers.
Traces: iam.op-contract-list-users-live-enforcement-denied.

## Evidence
Suite simparity-behavioral-ga-prod-v1, replayed against this checkout. Service token `iam` is bootstrapped; all eight checks below reached real handlers (no NotImplemented on any of these calls).

Cause 1 — iam.delete-instance-profile-with-role [mismatch], check line 86:
  AssertionError: expected the exact DeleteConflict message AWS returns, got 'Cannot delete entity: the role sim2real-behav-role-d06971841406 must be removed from instance profile sim2real-behav-ip-74d882f47f33 first.'
  Expected message (entirely fixed text): "Cannot delete entity, must remove roles from instance profile first." The role and profile names in the emulator's message are this run's generated values and have no place in AWS's template. Code DeleteConflict / HTTP 409 were already correct.
  iam.delete-role-with-inline-policy [mismatch], check line 103:
  AssertionError: expected the exact DeleteConflict message AWS returns, got 'Cannot delete entity: 1 inline policy must be deleted from the role sim2real-behav-role-512a6e3a9212 first.'
  Expected message (entirely fixed text): "Cannot delete entity, must delete policies first."

Cause 2 — iam.detach-not-attached-policy [mismatch], check line 70:
  AssertionError: expected the exact NoSuchEntity message AWS returns, got 'Policy arn:aws:iam::000000000000:policy/sim2real-behav-pol-unattached-fcf808611617 was not found attached to role sim2real-behav-role-e2d22a714afc.'
  Expected: NoSuchEntity / HTTP 404 with message "Policy <PolicyArn> was not found." — fixed text "Policy " + " was not found.", single variable part = the request's PolicyArn. The role name must not appear.

Cause 3 — iam.instance-profile-single-role-constraint [mismatch], check line 59:
  AssertionError: expected the exact LimitExceeded message AWS returns, got 'Cannot exceed quota for RolesPerInstanceProfile: 1'
  Expected: "Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1" — fixed text "Cannot exceed quota for InstanceSessionsPerInstanceProfile: " plus the quota value 1. Code LimitExceeded / HTTP 409 already correct; the quota NAME diverges.

Cause 4 — iam.policy-missing-statement [mismatch], check line 51:
  AssertionError: expected the exact MalformedPolicyDocument message AWS returns, got 'Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element'
  Request: CreatePolicy(PolicyName=<generated>, PolicyDocument='{"Version": "2012-10-17"}') — valid JSON, no Statement.
  Expected: MalformedPolicyDocument / HTTP 400, message exactly "Syntax errors in policy." (all fixed text, no variable part). Controls that passed: a well-formed document was accepted and read back by GetPolicy, and the rejected policy did not persist (GetPolicy on its deterministic ARN answered NoSuchEntity with the correct 'Policy <arn> was not found.' text).

Cause 5 — iam.update-service-linked-role-unmodifiable-rejected [mismatch], check line 48:
  AssertionError: expected UnmodifiableEntity from UpdateAssumeRolePolicy, got 'NoSuchEntity'
  Sequence: the service-linked role AWSServiceRoleForElasticBeanstalk is ensured to exist, then GetRole(RoleName='AWSServiceRoleForElasticBeanstalk') SUCCEEDS and returns its AssumeRolePolicyDocument (check line 44) — so the role is present in state. The very next call, UpdateAssumeRolePolicy(RoleName='AWSServiceRoleForElasticBeanstalk', PolicyDocument=<an ec2.amazonaws.com trust policy>), is refused NoSuchEntity.
  Expected: UnmodifiableEntity (Smithy UnmodifiableEntityException, awsQueryError UnmodifiableEntity, HTTP 400) with message "Cannot perform the operation on the protected role '<roleName>' - this role is only modifiable by AWS", and the stored trust policy unchanged.

Cause 6 — iam.group-policy-explicit-deny-overrides-the-users-own-allow [mismatch], check line 101 via _poll at line 182:
  AssertionError: the group's explicit Deny refuses iam:GetUser for the member did not happen within 54.86s; last observation 'OK'
  Setup verified in-check before the conclusion: PutUserPolicy gave the user Allow on iam:GetUser + iam:ListUserTags for its own ARN; both actions were confirmed effective for a freshly minted access key (positive control); AddUserToGroup then GetGroup reported exactly that one member; PutGroupPolicy stored a single statement Deny on iam:GetUser for the same ARN and GetGroupPolicy read back Effect == 'Deny'.
  Expected: the member's signed GetUser is refused AccessDenied / HTTP 403 with "User: <user ARN> is not authorized to perform: iam:GetUser on resource: <user ARN> with an explicit deny in an identity-based policy", while the sibling action iam:ListUserTags keeps working and leaving the group restores access.
  Actual: success ('OK') for the entire poll window.

Cause 7 — iam.op-contract-list-users-live-enforcement-denied [mismatch]: Check timed out after 60.0s.
  Sequence: CreateUser under a unique path; iam.list_users(PathPrefix=<path>) by the account principal reports it; a role trusted by the account root is created with an inline Allow on '*' and assumed; the assumed role's signed list_users(PathPrefix=<path>) SUCCEEDS (positive control, check line 117); PutRolePolicy then adds one inline policy whose only statement is {Effect: Deny, Action: iam:ListUsers, Resource: '*'}; the check polls list_users for error code AccessDenied (90 s budget, 3 s interval) and the harness kills it at 60 s with no refusal ever observed.
  Expected after the Deny: AccessDenied / HTTP 403 with message "User: <assumed-role session ARN> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user<path> with an explicit deny in an identity-based policy"; deleting the inline Deny restores success.

## Steps to Reproduce
Boot the emulator from this checkout and point an `iam` client at it, signing as a principal permitted to call IAM.

Cause 1:
1. CreateRole, CreateInstanceProfile, AddRoleToInstanceProfile, then DeleteInstanceProfile — observe 409 DeleteConflict with 'Cannot delete entity: the role <role> must be removed from instance profile <profile> first.' instead of 'Cannot delete entity, must remove roles from instance profile first.'
2. CreateRole, PutRolePolicy (one inline policy), then DeleteRole — observe 409 DeleteConflict with 'Cannot delete entity: 1 inline policy must be deleted from the role <role> first.' instead of 'Cannot delete entity, must delete policies first.'
   Replay: iam.delete-instance-profile-with-role, iam.delete-role-with-inline-policy

Cause 2:
1. CreateRole and CreatePolicy, but do NOT attach the policy.
2. DetachRolePolicy(RoleName=<role>, PolicyArn=<the unattached policy>) — observe 404 NoSuchEntity with 'Policy <arn> was not found attached to role <role>.' instead of 'Policy <arn> was not found.'
   Replay: iam.detach-not-attached-policy

Cause 3:
1. CreateInstanceProfile, CreateRole twice, AddRoleToInstanceProfile with the first role (succeeds), then AddRoleToInstanceProfile with the second.
2. Observe 409 LimitExceeded with 'Cannot exceed quota for RolesPerInstanceProfile: 1' instead of 'Cannot exceed quota for InstanceSessionsPerInstanceProfile: 1'.
   Replay: iam.instance-profile-single-role-constraint

Cause 4:
1. CreatePolicy(PolicyName=<any>, PolicyDocument='{"Version":"2012-10-17"}') — valid JSON, no Statement.
2. Observe 400 MalformedPolicyDocument with 'Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element' instead of the exact 'Syntax errors in policy.'
   Replay: iam.policy-missing-statement

Cause 5:
1. CreateServiceLinkedRole(AWSServiceName='elasticbeanstalk.amazonaws.com') so AWSServiceRoleForElasticBeanstalk exists; confirm with GetRole(RoleName='AWSServiceRoleForElasticBeanstalk').
2. UpdateAssumeRolePolicy(RoleName='AWSServiceRoleForElasticBeanstalk', PolicyDocument=<an ec2.amazonaws.com trust policy>).
3. Observe 404 NoSuchEntity instead of 400 UnmodifiableEntity with "Cannot perform the operation on the protected role 'AWSServiceRoleForElasticBeanstalk' - this role is only modifiable by AWS", and confirm GetRole still returns the role with its original trust policy.
   Replay: iam.update-service-linked-role-unmodifiable-rejected

Cause 6:
1. CreateUser; PutUserPolicy giving it Allow on iam:GetUser and iam:ListUserTags for its own ARN; CreateAccessKey and sign a client with that key; confirm both actions succeed.
2. CreateGroup; AddUserToGroup; confirm GetGroup lists the user.
3. PutGroupPolicy on the group with a single statement {Effect: Deny, Action: iam:GetUser, Resource: <the user ARN>}; confirm GetGroupPolicy reads it back as Deny.
4. Call GetUser with the user's own key and poll — observe continued HTTP 200 instead of AccessDenied 403 with 'with an explicit deny in an identity-based policy', while iam:ListUserTags stays allowed.
   Replay: iam.group-policy-explicit-deny-overrides-the-users-own-allow

Cause 7:
1. CreateUser under a unique path. CreateRole trusted by arn:aws:iam::<account>:root, PutRolePolicy with Allow '*' on '*', AssumeRole it, and confirm the assumed role's signed ListUsers(PathPrefix=<path>) succeeds.
2. PutRolePolicy on that role with a second inline policy whose only statement is {Effect: Deny, Action: iam:ListUsers, Resource: '*'}.
3. Call ListUsers(PathPrefix=<path>) with the assumed-role credentials repeatedly — observe continued success instead of AccessDenied 403; then DeleteRolePolicy and confirm it still succeeds (the Deny never had an effect).
   Replay: iam.op-contract-list-users-live-enforcement-denied
