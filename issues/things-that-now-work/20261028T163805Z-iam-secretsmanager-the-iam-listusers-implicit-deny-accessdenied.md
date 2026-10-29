Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol
Issue-Key: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol|run#16|AssertionError|assert iam_msg == want, f"IAM denial message: expected {want!r}, got {iam_msg!r}"
Issue-Commit: 804fb9e39527c7d8ad6b71682b2c9d408f11768a

# iam__secretsmanager: the iam:ListUsers implicit-deny AccessDenied message again appends a console-URL / GetRequestAuthorizationDetails trailer AWS does not emit (1 cause)

> _Note: this issue was identified against the code base at commit `804fb9e39527c7d8ad6b71682b2c9d408f11768a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUsers

## Description
One net-new root cause. Cause count: 1.

Not covered by an open issue. Related but NOT covering: open issue 20261028T012639Z-iam-the-implicit-deny-accessdenied-message-no-longer.md reports the OPPOSITE direction for a different refusal (iam:ListAccessKeys, where the recording DOES carry the trailer and the emulator omitted it) and carries that check's replay key, not this one's. Open issue 20261025T093540Z-iam-secretsmanager-secrets-manager-performs-no-identity-based.md covers Secrets Manager's missing identity-based authorization, which is the *second half* of this same check and is never reached here. Fixed issue 20261027T032337Z-iam-secretsmanager-iam-accessdenied-message-appends-a-console-url-getrequestauthorizationdetails.md asked for exactly this trailer to be removed from the iam:ListUsers implicit-deny message; it is marked fixed but the trailer is back, so this is a regression of that fix and a resolved issue does not cover a live failure. The subject call is correctly refused (error code AccessDenied is what the check wants), so this is a message-text divergence on a correct refusal, not a call left refused.

=== Cause 1 of 1: the implicit-deny AccessDenied message for iam:ListUsers carries a second 'Go to <console URL> ... authorization id: <id>' sentence AWS does not emit ===
AWS: IAM speaks awsQuery (service AWSIdentityManagementV20100508, version 2010-05-08). Smithy: ListUsers models only ServiceFailureException (awsQueryError ServiceFailure, HTTP 500) — the authorization refusal AccessDenied / HTTP 403 is a service-common error whose message text is not modeled, so this is recorded behavior only. The recorded refusal of iam:ListUsers for a principal that no identity-based policy permits is exactly ONE sentence, with no trailing period and no pointer sentence:
  `User: <caller-arn> is not authorized to perform: <service>:<Action> on resource: <resource-description> because no identity-based policy allows the <service>:<Action> action`
Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action` (and nothing after it). Variable parts, taken from the signed request and the emulator's state: `<caller-arn>` (the ARN of the principal whose access key signed the request), `<service>:<Action>` (the IAM action of the refused operation, here `iam:ListUsers`), `<resource-description>` (how IAM names the evaluated resource for this action — for ListUsers the recorded form is the account's user path root, `arn:aws:iam::<account>:user/`). The concrete account id, user name and path in any one run fill that template and are not part of the requirement.
Emulator: it returns the correct error code (AccessDenied) and the correct first sentence, then appends a whole second sentence: `. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>`. The divergence is that entire extra sentence, emitted on every such refusal — not a per-run value inside the message; the per-request authorization id merely fills a slot in text that must not be there at all.
Why: a client that compares AWS's implicit-deny denial text (policy-troubleshooting tooling, SDK/CLI assertions, log scrapers and parity harnesses that match the documented sentence) sees an unexpected trailer and fails to recognise or parse the refusal, even though the refusal decision itself is right. Combined with the opposite-direction report in 20261028T012639Z, the emulator's denial text is now wrong in both directions: the trailer is emitted where AWS records none (iam:ListUsers) and was missing where AWS records one (iam:ListAccessKeys). Hypothesis: the trailer is decided globally in one shared denial-message builder rather than per refusal, so each fix flips every refusal at once — removing it broke ListAccessKeys (20261028T012639Z) and re-adding it has now broken ListUsers again.
Fix: the shared authorization-refusal denial-message builder in the IAM crate — the component that renders the AccessDenied message for iam operations, and which the fixes for 20261027T032337Z and 20261028T012639Z both changed. It must select the trailer per refusal (per the action's recorded message) instead of appending or suppressing it unconditionally, so iam:ListUsers' implicit-deny message ends at '... action' while the refusals AWS records with the authorization-details pointer keep theirs (with the console host and region query parameter pinned to us-east-1).
Traces: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol (assertion at check line 96, `assert iam_msg == want`).

## Evidence
Check `iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol`, verdict mismatch (per-service group iam__secretsmanager 0/1).

The check creates an IAM user with no identity policy at all, mints an access key for it, and calls iam ListUsers(MaxItems=1) signed with that key alone. The emulator correctly refuses with error code `AccessDenied` (the preceding assertion on the code passes); the message comparison then fails:

AssertionError: IAM denial message: expected 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user/ because no identity-based policy allows the iam:ListUsers action', got 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user/ because no identity-based policy allows the iam:ListUsers action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>'

Expected and received differ by exactly the whole second sentence; the expected string is a strict prefix of what the emulator returned. The check builds its expected text from the request and its own account id, so no per-run value is being compared loosely — the extra sentence is the entire difference.

Because the run aborts at this assertion, the check's remaining claims (HTTP 403 on the awsQuery AccessDenied, and Secrets Manager's awsJson1_1 ListSecrets answering AccessDeniedException at HTTP 400 with its own message) were never exercised and nothing is asserted about them here; the Secrets Manager half is already carried by open issue 20261025T093540Z-iam-secretsmanager-secrets-manager-performs-no-identity-based.md.

## Steps to Reproduce
1. Confirm the positive controls: with the account's own credentials call iam ListUsers(MaxItems=1) and secretsmanager ListSecrets(MaxResults=1) — both succeed.
2. iam CreateUser(UserName=<u>) with no identity-based policy attached and no inline policy.
3. iam CreateAccessKey(UserName=<u>); build an iam client signing with that access key id / secret alone (no session token).
4. Call iam ListUsers(MaxItems=1) with that client, polling past any InvalidClientTokenId / SignatureDoesNotMatch / UnrecognizedClientException while the key propagates.
5. Read Error.Code and Error.Message. Error.Code is `AccessDenied` (correct). Error.Message is 'User: arn:aws:iam::<account>:user/<u> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user/ because no identity-based policy allows the iam:ListUsers action' followed by the extra sentence '. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>'. AWS's recorded message ends at '... iam:ListUsers action'.
