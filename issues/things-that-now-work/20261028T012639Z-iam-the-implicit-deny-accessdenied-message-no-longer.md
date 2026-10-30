Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.permissions-boundary-alone-never-grants-a-real-signed-call
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.permissions-boundary-alone-never-grants-a-real-signed-call|run#22|AssertionError|assert ungranted_message == expected, (
Issue-Commit: 86b938589494b175b407afe6e041c3f131390f74

# iam: the implicit-deny AccessDenied message no longer carries the authorization-details trailer AWS emits for IAM refusals (1 cause)

> _Note: this issue was identified against the code base at commit `86b938589494b175b407afe6e041c3f131390f74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccessKeys

## Description
One net-new root cause. Cause count: 1.

Not covered by an existing issue. Related but NOT covering: open issue 20261027T032550Z-iam-5-net-new-behavioral-root-causes-authorization.md reports the same trailer, but its Cause 1 reports it being appended where AWS does not emit it (ListGroups/ListRoles/UntagPolicy/ListPoliciesGrantingServiceAccess) and its Cause 2 reports the trailer's console URL naming the caller's region instead of us-east-1 for iam:ListOpenIDConnectProviders; neither carries this check's replay key, and the failure reported here is the opposite direction — the trailer is now ABSENT on a refusal that must carry it. Fixed issue 20261027T032337Z-iam-secretsmanager-iam-accessdenied-message-appends-a-console-url-getrequestauthorizationdetails.md asked for the trailer to be removed from iam:ListUsers' implicit-deny message; the fix appears to have removed it unconditionally from the shared denial-message builder, which is why this refusal's message is now a strict prefix of the required text. The subject call here is correctly refused (AccessDenied is what the check wants), so this is a message-text divergence on a correct refusal, not a call left refused.

=== Cause 1 of 1: the implicit-deny AccessDenied message for iam:ListAccessKeys stops at '...action' and omits the authorization-details sentence ===
AWS: IAM speaks awsQuery (service AWSIdentityManagementV20100508, version 2010-05-08). Smithy: ListAccessKeys models only NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404) and ServiceFailureException (500) — the authorization refusal AccessDenied / HTTP 403 is a service-common error whose message text is not modeled, so this is recorded behavior only. The recorded refusal for an action no identity-based policy allows is TWO sentences: the documented implicit-deny sentence followed by a pointer to the request's authorization details:
  `User: <caller-arn> is not authorized to perform: <service>:<Action> on resource: <resource-description> because no identity-based policy allows the <service>:<Action> action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>`
Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/`, ` for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: `. The us-east-1 host and region literal are fixed text, because IAM is a global service signed in us-east-1. Variable parts, taken from the signed request and the emulator's state: `<caller-arn>` (the ARN of the principal whose access key signed the request), `<service>:<Action>` (the IAM action of the refused operation, here `iam:ListAccessKeys`), `<resource-description>` (how IAM names the evaluated resource for this action — for ListAccessKeys the recorded form is `user <UserName>`), and `<authorization-id>` (the per-request authorization id; the check reads this value back out of the emulator's own message, so its concrete value is never asserted and is not part of the requirement).
Emulator: it returns the correct error code (AccessDenied) and the correct first sentence, then ends the message there. Observed message: `User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListAccessKeys on resource: user <user> because no identity-based policy allows the iam:ListAccessKeys action` — the entire second sentence (console URL + 'or call the GetRequestAuthorizationDetails API with the following authorization id: <id>') is missing, and with it any authorization id to point at. The divergence is the whole missing sentence, present on every such refusal, not a per-run id.
Why: a client that reads AWS's denial text to route a developer to the authorization-details page, or that asserts on the full documented message, receives a truncated string: the pointer to the per-request authorization id disappears, so policy-troubleshooting tooling has nothing to follow and nothing to pass to GetRequestAuthorizationDetails. Together with the already-filed opposite-direction reports, the emulator's denial text is now wrong for both families of refusal — some messages lacked the trailer, some carried it with the wrong region, and it is now absent everywhere.
Fix: the shared authorization-refusal denial-message builder in the IAM crate (the component that renders AccessDenied for iam operations, and which the fix for 20261027T032337Z changed). It must decide the trailer per refusal rather than globally: emit the documented implicit-deny/explicit-deny sentence, and for the refusals AWS records with the authorization-details pointer also emit that sentence, with the console host and region query parameter pinned to us-east-1 and the authorization id taken from the request's authorization context.
Traces: iam.permissions-boundary-alone-never-grants-a-real-signed-call (assertion at check line 123, `assert ungranted_message == expected`).

## Evidence
Check `iam.permissions-boundary-alone-never-grants-a-real-signed-call`, verdict mismatch.

The check creates a user whose only identity policy Allows iam:ListUserTags, attaches a permissions boundary that Allows both iam:ListUserTags and iam:ListAccessKeys, mints an access key for the user and signs iam:ListAccessKeys with it. The emulator correctly refuses with error code `AccessDenied` (the check's preceding assertion on the code passes), then the message comparison fails:

AssertionError: expected message 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListAccessKeys on resource: user <user> because no identity-based policy allows the iam:ListAccessKeys action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<absent> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <absent>', got 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListAccessKeys on resource: user <user> because no identity-based policy allows the iam:ListAccessKeys action'

The `<absent>` placeholders are the check's own fallback: its `_with_authorization_details()` helper reads the authorization id back out of the received message after the marker 'authorization id: ' and substitutes '<absent>' only when the marker is not present at all. So the expected and received strings differ by exactly the whole second sentence; the per-run authorization id is never compared.

## Steps to Reproduce
1. iam CreateUser(UserName=<u>).
2. iam PutUserPolicy(UserName=<u>, PolicyName=<baseline>) with {Effect: Allow, Action: iam:ListUserTags, Resource: <u's ARN>} — and no statement allowing iam:ListAccessKeys.
3. iam CreatePolicy with a document Allowing both iam:ListUserTags and iam:ListAccessKeys, and iam PutUserPermissionsBoundary(UserName=<u>, PermissionsBoundary=<that policy ARN>).
4. iam CreateAccessKey(UserName=<u>) and build an iam client signing with that key alone (no session token).
5. Call iam ListAccessKeys(UserName=<u>) with it.
6. Observe HTTP 403 / Error.Code 'AccessDenied' (correct) and Error.Message ending at '... because no identity-based policy allows the iam:ListAccessKeys action'. AWS's recorded message continues with '. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>'.
