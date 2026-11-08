Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: freetier.get-account-plan-state-access-denied-for-restricted-principal
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier.get-account-plan-state-access-denied-for-restricted-principal|run#12|AssertionError|assert denied == "AccessDeniedException", (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# freetier GetAccountPlanState performs no IAM authorization check: a principal with no policy granting freetier:GetAccountPlanState is served instead of denied

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountPlanState

## Description
ONE cause.

AWS: Smithy: GetAccountPlanState models AccessDeniedException ("You don't have sufficient access to perform this action", errorKind client, @httpError 403) as one of its two operation errors, alongside ResourceNotFoundException. Recorded behavior: a signed call made with the credentials of a brand-new IAM user that has ZERO policies attached is refused by IAM's default implicit deny — AccessDeniedException with the message template "User: <caller principal ARN> is not authorized to perform: freetier:GetAccountPlanState on resource: arn:aws:freetier:us-east-1:<account id>:/GetAccountPlanState because no identity-based policy allows the freetier:GetAccountPlanState action". The fixed text is everything outside the angle brackets, including the action name freetier:GetAccountPlanState (twice), the resource path form arn:aws:freetier:us-east-1:<account>:/GetAccountPlanState (FreeTier is a global API served from us-east-1, so the region in that ARN is fixed text, not a per-run value) and the trailing clause "because no identity-based policy allows the ... action". The variable parts are the calling principal's ARN and the account id, both taken from the request's signing identity. Attaching an inline Allow for freetier:GetAccountPlanState to that same user lifts the denial on the identical call over the identical connection, after which the call either succeeds or reaches the documented no-plan-record ResourceNotFoundException "Missing data for account: <account id>".
Emulator: answers the zero-policy user's signed call with ResourceNotFoundException — the same answer it gives the ambient, fully privileged caller. The request authorization step is simply not performed: the handler resolves straight to the account's (absent) plan record regardless of what the signing principal is allowed to do, so AccessDeniedException is never produced on this operation even though the model declares it.
Why: the emulator reports a resource-state error where AWS reports an authorization failure, which inverts the meaning of the response. A caller cannot distinguish "I may not call this" from "this account has no Free Tier plan record", so retry/alerting logic keyed on AccessDeniedException never fires, and least-privilege work done against the emulator is unverifiable: a policy that grants nothing looks indistinguishable from one that grants the action, so an under-privileged deployment passes here and fails on AWS. Equally, nothing can test the recovery path (grant the Allow, call again) because the denial it recovers from never occurs.
Fix: request authorization for this operation — the per-request IAM evaluation that must run in services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_plan_state.rs (and the equivalent gate on the other operations that model AccessDeniedException, i.e. UpgradeAccountPlan) before the handler reads plan state, raising the modeled AccessDeniedException with the message template above when no identity-based policy allows the action.
Traces: freetier.get-account-plan-state-access-denied-for-restricted-principal.

This is the emulator's own answer, not a harness problem: the check's positive control — the ambient privileged caller's GetAccountPlanState — was served by the emulator with the correct ResourceNotFoundException and AWS's exact "Missing data for account: <account id>" wording before anything about the restricted principal was exercised, the new IAM user and access key were created successfully through the emulator's own IAM, and the probe polled past the credential-propagation codes (UnrecognizedClientException / InvalidClientTokenId / SignatureDoesNotMatch) to a settled answer. The settled answer was ResourceNotFoundException. The denial is asserted through a real signed call with the minted key, not through IAM's advisory simulate_principal_policy, so what diverges is enforcement on the live call.

## Evidence
Zero-policy IAM user, its own minted access key, real signed call to GetAccountPlanState:
  expected: Code=AccessDeniedException (modeled, HTTP 403), Message="User: <new user's ARN> is not authorized to perform: freetier:GetAccountPlanState on resource: arn:aws:freetier:us-east-1:<account id>:/GetAccountPlanState because no identity-based policy allows the freetier:GetAccountPlanState action"
  actual:   Code='ResourceNotFoundException'. The check's assertion reads: "a brand-new IAM user with no policy attached must get the default implicit deny (AccessDeniedException) on a real signed call to freetier:GetAccountPlanState; got 'ResourceNotFoundException'".

Supporting observations from the same replay:
  - positive control, ambient privileged caller, GetAccountPlanState: served by the emulator, and its ResourceNotFoundException message matched AWS's "Missing data for account: <account id>" exactly (that assertion passed) — so the operation is implemented and reachable and its no-plan-record wording is already right.
  - iam.create_user and iam.create_access_key for the restricted principal both succeeded against the emulator.
  - the probe polled for up to 90 s past the credential-propagation error codes; the settled answer never became AccessDeniedException.
  - consequently the restore leg (attach an inline Allow for freetier:GetAccountPlanState to the same user, re-call over the same connection, expect the denial to lift) is unreachable: there is no denial to lift.
The principal ARN and account id in the expected message are per-environment values that fill AWS's template; the template itself — action name, resource path arn:aws:freetier:us-east-1:<account>:/GetAccountPlanState, and the "because no identity-based policy allows the ... action" clause — is the requirement.

## Steps to Reproduce
1. Boot the emulator with the freetier, iam and sts services mounted.
2. As the ambient (privileged) caller, call GetAccountPlanState and confirm it either succeeds or answers ResourceNotFoundException "Missing data for account: <account id>" — the service is healthy for this API.
3. iam.create_user with a fresh name and attach NO policy of any kind; iam.create_access_key for that user.
4. Build a freetier client signing with that user's access key id / secret and call GetAccountPlanState, polling past UnrecognizedClientException / InvalidClientTokenId / SignatureDoesNotMatch to the settled answer.
5. Observe Code='ResourceNotFoundException'. AWS answers AccessDeniedException with the implicit-deny message naming the user's ARN, the action freetier:GetAccountPlanState and the resource arn:aws:freetier:us-east-1:<account id>:/GetAccountPlanState.
6. Attach an inline policy {"Effect":"Allow","Action":"freetier:GetAccountPlanState","Resource":"*"} to the same user and re-call over the same connection: once the gate exists this must stop being denied and either succeed or answer ResourceNotFoundException "Missing data for account: <account id>".
7. Re-replay freetier.get-account-plan-state-access-denied-for-restricted-principal to confirm the fix.
