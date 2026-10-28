Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.list-policies-granting-service-access iam.op-contract-listGroups-missing-permission-denied iam.op-contract-listRoles-missing-permission-denied iam.untag-policy iam.op-contract-list-open-id-connect-providers iam.get-nonexistent-entity-returns-nosuchentity iam.delegation-request-workflow-lifecycle iam.group-policy-explicit-deny-overrides-the-users-own-allow
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.list-policies-granting-service-access|run#23|AssertionError|assert message2 == expected_denied, (
Issue-Key: iam.op-contract-listGroups-missing-permission-denied|run#21|AssertionError|assert denial.get("message") == expected_denial, (
Issue-Key: iam.op-contract-listRoles-missing-permission-denied|run#22|AssertionError|assert denial.get("message") == expected_denial, (
Issue-Key: iam.untag-policy|run#25|AssertionError|assert message2 == expected2, f"expected message {expected2!r}, got {message2!r}"
Issue-Key: iam.op-contract-list-open-id-connect-providers|run#19|AssertionError|assert denied_msg == expected_denial, (
Issue-Key: iam.get-nonexistent-entity-returns-nosuchentity|_expect#28|AssertionError|assert got_message == message, f"{label}: expected message {message!r}, got {got_message!r}"
Issue-Key: iam.delegation-request-workflow-lifecycle|run#12|AssertionError|assert code_get == "NoSuchEntity", f"expected NoSuchEntity for GetDelegationRequest, got {code_get}"
Issue-Key: iam.group-policy-explicit-deny-overrides-the-users-own-allow|_poll#23|Timeout|time.sleep(interval_s)
Issue-Commit: 23cbd5c82660cd4b0853a2ce8e5f5a3364e63161

# iam: 5 net-new behavioral root causes — authorization-refusal messages carry an extra console/authorization-id suffix AWS does not emit (and the wrong region when it does), GetLoginProfile's NoSuchEntity text, GetDelegationRequest answers NotImplemented, and a group policy's explicit Deny is never aggregated

> _Note: this issue was identified against the code base at commit `23cbd5c82660cd4b0853a2ce8e5f5a3364e63161`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGroups

## Description
Five distinct net-new root causes in the implemented IAM paths, all grounded in failing behavioral checks. None of these checks is marked ALREADY FILED or OWNED in the replay report. Related-but-different open issues: 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md and 20261027T000203Z-iam-4-net-new-root-causes-createpolicyversion-s.md both report refusal messages that are *emulator prose* (wholly invented text); 20261026T024842Z-iam-getpolicy-s-nosuchentity-message-says-does-not.md reports GetPolicy's NoSuchEntity wording. The causes below are different: here the message body is AWS's, but an extra trailer is appended (Causes 1-2), a *different* AWS message template is used for the wrong condition (Cause 3), a modeled error is replaced by an unmodeled one (Cause 4), and the authorizer never traverses the group edge (Cause 5).

=== Cause 1 of 5: every IAM authorization refusal message has an unrequested console/authorization-id trailer appended ===
AWS: IAM's authorization refusal for these operations is AccessDenied / HTTP 403 whose Message is exactly one of the two documented templates and nothing more. Implicit deny: "User: <caller-arn> is not authorized to perform: <service:Action> on resource: <resource-arn> because no identity-based policy allows the <service:Action> action". Explicit deny: "User: <caller-arn> is not authorized to perform: <service:Action> on resource: <resource-arn> with an explicit deny in an identity-based policy". Fixed text is everything outside the angle brackets; the caller ARN, the action name and the resource ARN are variable parts taken from the signed request and the emulator's state. The recorded behavior for ListPoliciesGrantingServiceAccess, ListGroups, ListRoles and UntagPolicy shows the message TERMINATING at "...action" / "...identity-based policy" with no trailer. Smithy: ListGroups errors = ServiceFailureException only (AccessDenied is the service-common authorization refusal); UntagPolicy errors = ConcurrentModification/InvalidInput/NoSuchEntity/ServiceFailure.
Emulator: it appends a fixed trailer to every such message: ". Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>". Observed verbatim on the four checks below, e.g. "...because no identity-based policy allows the iam:UntagPolicy action. Go to https://us-west-2.console.aws.amazon.com/iam/home?region=us-west-2#/authorization-details/7iycgpktansf3o9cipxgxbbk42otkmq6 for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: 7iycgpktansf3o9cipxgxbbk42otkmq6". The authorization id is also the same constant on every refusal across every operation and run, so it is not a per-run value that happens to differ — the whole trailer is unconditional.
Why: a client that classifies or asserts on IAM denial text — the standard way least-privilege test suites and CI policy gates confirm *which* policy mechanism refused a call — never matches, because the message it is told to expect is a strict prefix of what it receives. Code that strips or parses the documented tail ("...allows the X action" vs "...with an explicit deny...") mis-classifies implicit vs explicit deny.
Fix: the authorization layer's denial-message builder in the IAM crate's authorization/refusal path (the component that renders AccessDenied for iam operations), which must emit only the documented template and must not append a console URL or authorization id unless AWS does for that refusal (see Cause 2).
Traces: iam.list-policies-granting-service-access, iam.op-contract-listGroups-missing-permission-denied, iam.op-contract-listRoles-missing-permission-denied, iam.untag-policy.

=== Cause 2 of 5: when AWS DOES emit the authorization-details trailer, its console URL names IAM's global endpoint region, not the caller's region ===
AWS: recorded behavior only. For ListOpenIDConnectProviders the recorded refusal DOES carry the trailer, and its console host and region query parameter are us-east-1: "...because no identity-based policy allows the iam:ListOpenIDConnectProviders action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>". Fixed text is the sentence template plus the us-east-1 host/region literal (IAM is a global service whose console and endpoint live in us-east-1); the variable parts are the caller ARN, the action, the resource ARN and the authorization id.
Emulator: emits the trailer with the CALLING client's region substituted into both the host and the region parameter — "https://us-west-2.console.aws.amazon.com/iam/home?region=us-west-2#/..." — so the message diverges from AWS for every caller outside us-east-1.
Why: the trailer is the pointer a developer follows to the authorization-details page; pointing it at a regional IAM console that does not exist for a global service sends them to a dead link, and any client asserting on the full message text fails.
Fix: the same denial-message builder as Cause 1 — the console URL for IAM refusals must be pinned to IAM's global console region (us-east-1) rather than derived from the request's region.
Traces: iam.op-contract-list-open-id-connect-providers.

=== Cause 3 of 5: GetLoginProfile on a user that does not exist reports the "no login profile" message instead of the "no such user" message ===
AWS: Smithy: GetLoginProfile errors = NoSuchEntityException (awsQueryError code NoSuchEntity, HTTP 404) and ServiceFailureException. AWS distinguishes two NoSuchEntity conditions by message: when the NAMED USER does not exist the message is "The user with name <UserName> cannot be found."; when the user exists but has no login profile the message is "Login Profile for User <UserName> cannot be found.". Fixed text is each sentence template; the variable part is the UserName copied from the request.
Emulator: for a user that was never created it returns NoSuchEntity with the login-profile template — "Login Profile for User <UserName> cannot be found." — i.e. it collapses both conditions onto the second message.
Why: a caller cannot tell "this user does not exist" from "this user has no console password". Provisioning and remediation code that branches on that distinction (create the user vs create only the login profile) takes the wrong branch.
Fix: the GetLoginProfile handler in the IAM crate — look the user up first and raise the user-not-found NoSuchEntity message before the login-profile lookup.
Traces: iam.get-nonexistent-entity-returns-nosuchentity.

=== Cause 4 of 5: GetDelegationRequest for an unknown delegation-request id answers an unmodeled NotImplemented instead of NoSuchEntity 404 ===
AWS: Smithy: GetDelegationRequest input DelegationRequestId is required, @length 16-128, pattern ^[\w-]+$; modeled errors are NoSuchEntityException (awsQueryError code NoSuchEntity, HTTP 404) and ServiceFailureException (500). An id that names no delegation request is refused with NoSuchEntity / 404.
Emulator: the operation is a scaffold stub and answers error code NotImplemented, which is not in the operation's modeled error set, so the lifecycle check's unknown-id probe reads NotImplemented where NoSuchEntity is required. Open issue 20261025T000319Z-iam-getdelegationrequest-stub.md names the same unimplemented operation; this check adds the specific refusal contract that fix must satisfy (unknown id -> NoSuchEntity 404), and it is not listed as a replay key on that issue, so it would otherwise go unverified.
Why: a client cannot distinguish "no such delegation request" from "this API does not exist here"; botocore raises an unmodeled error class, so `except iam.exceptions.NoSuchEntityException` never catches and the caller's not-found path is dead code.
Fix: the GetDelegationRequest handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_delegation_request.rs) and the delegation-request state it must read.
Traces: iam.delegation-request-workflow-lifecycle.

=== Cause 5 of 5: an explicit Deny carried by a GROUP policy never refuses a member's signed call, so the check exhausts its budget ===
AWS: recorded behavior only for this check (it ships as a draft with no recorded oracle, so the mechanism below is stated as a hypothesis). AWS's documented evaluation logic aggregates the principal's own identity policies with the policies of every group it belongs to, and any explicit Deny in that aggregate overrides every Allow: a user whose own inline policy Allows iam:GetUser and iam:ListUserTags on itself, who is then added to a group carrying an inline Deny on iam:GetUser for the same resource, must have the real signed GetUser refused with exactly AccessDenied while ListUserTags continues to succeed.
Emulator: the check's positive control passes, then its bounded poll for AccessDenied on GetUser never observes one and the check is killed at the 60 s harness cap ("Check timed out after 60.0s"); the only outcome the poll can report is a non-denial. Hypothesis: the authorizer evaluates only the policies attached directly to the signing principal and never traverses the group-membership edge, so the group's Deny is invisible and GetUser keeps returning 200 until the poll's deadline. This is over-permissiveness, not merely slowness: a correct target satisfies the poll on its first iteration.
Why: a developer who enforces a guardrail by putting a Deny on a group — the normal way an organization applies a blanket restriction to a set of users — is told the guardrail works when the call is still permitted. Every member of the group retains access the policy revokes.
Fix: the IAM authorization evaluator's policy-collection step (it must union the principal's own inline and attached policies with the inline and attached policies of each group the user belongs to before applying deny-overrides-allow), plus SimulatePrincipalPolicy's attribution so a deny reached through a group is reported as explicitDeny rather than implicitDeny.
Traces: iam.group-policy-explicit-deny-overrides-the-users-own-allow.

## Evidence
Cause 1 — iam.untag-policy:
  expected message 'User: arn:aws:iam::<account>:user/<caller> is not authorized to perform: iam:UntagPolicy on resource: policy arn:aws:iam::<account>:policy/<policy> because no identity-based policy allows the iam:UntagPolicy action'
  got      '...allows the iam:UntagPolicy action. Go to https://us-west-2.console.aws.amazon.com/iam/home?region=us-west-2#/authorization-details/7iycgpktansf3o9cipxgxbbk42otkmq6 for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: 7iycgpktansf3o9cipxgxbbk42otkmq6'
Same trailer appended, same constant authorization id, on:
  iam.list-policies-granting-service-access (implicit deny on iam:ListPoliciesGrantingServiceAccess)
  iam.op-contract-listGroups-missing-permission-denied (explicit deny: '...iam:ListGroups on resource: arn:aws:iam::<account>:group/ with an explicit deny in an identity-based policy' + trailer)
  iam.op-contract-listRoles-missing-permission-denied (explicit deny: '...iam:ListRoles on resource: arn:aws:iam::<account>:role/ with an explicit deny in an identity-based policy' + trailer)

Cause 2 — iam.op-contract-list-open-id-connect-providers:
  expected '...allows the iam:ListOpenIDConnectProviders action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<id> ...'
  got      '...allows the iam:ListOpenIDConnectProviders action. Go to https://us-west-2.console.aws.amazon.com/iam/home?region=us-west-2#/authorization-details/<id> ...'
  (identical apart from the console host/region: us-east-1 required, caller's region emitted)

Cause 3 — iam.get-nonexistent-entity-returns-nosuchentity, GetLoginProfile on a never-created user:
  expected message 'The user with name <UserName> cannot be found.'
  got      'Login Profile for User <UserName> cannot be found.'
  (error code NoSuchEntity / HTTP 404 is correct; only the template is wrong)

Cause 4 — iam.delegation-request-workflow-lifecycle:
  AssertionError: expected NoSuchEntity for GetDelegationRequest, got NotImplemented

Cause 5 — iam.group-policy-explicit-deny-overrides-the-users-own-allow:
  Error: Check timed out after 60.0s. The check's positive control (user's own Allow makes both iam:GetUser and iam:ListUserTags effective) is satisfied; the subsequent bounded poll for outcome == 'AccessDenied' on iam:GetUser after a Deny is written to the group's inline policy never observes a denial and runs past the harness cap. GetGroupPolicy confirms the Deny is stored (Effect == 'Deny') and GetGroup confirms the user is the group's only member before the poll begins, so the policy and the membership are both in place.

## Steps to Reproduce
Causes 1 and 2 (denial-message trailer):
1. Create an IAM user with an access key and no identity-based policy (implicit-deny case), or with a policy that Allows an action and a second policy that explicitly Denies it (explicit-deny case).
2. Sign a real request as that user for iam:ListGroups, iam:ListRoles, iam:UntagPolicy, iam:ListPoliciesGrantingServiceAccess or iam:ListOpenIDConnectProviders.
3. Read Error.Message from the AccessDenied response. Observe the appended '. Go to https://<caller-region>.console.aws.amazon.com/iam/home?region=<caller-region>#/authorization-details/... or call the GetRequestAuthorizationDetails API with the following authorization id: ...' trailer, with the caller's region rather than us-east-1.

Cause 3 (GetLoginProfile):
1. Pick a UserName that was never created.
2. Call iam:GetLoginProfile(UserName=<that name>).
3. Observe NoSuchEntity with message 'Login Profile for User <name> cannot be found.' instead of 'The user with name <name> cannot be found.'

Cause 4 (GetDelegationRequest):
1. Call iam:GetDelegationRequest with a syntactically valid DelegationRequestId (16-128 chars, ^[\w-]+$) that names no delegation request.
2. Observe error code NotImplemented instead of NoSuchEntity / HTTP 404.

Cause 5 (group Deny):
1. Create a user, give it an inline policy Allowing iam:GetUser and iam:ListUserTags on its own ARN, and mint an access key for it.
2. Create a group, add the user to it, and confirm GetGroup lists exactly that user.
3. Put an inline policy on the GROUP with {Effect: Deny, Action: iam:GetUser, Resource: <user ARN>} and confirm GetGroupPolicy stores Effect == 'Deny'.
4. Sign iam:GetUser as the user. Observe it still succeeds instead of being refused AccessDenied, while iam:ListUserTags (not named by the Deny) also succeeds.
5. Call SimulatePrincipalPolicy on the user's ARN for iam:GetUser and observe it does not report 'explicitDeny'.
