Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: socialmessaging.associate-business-account-access-token-exceeds-max-rejected
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: socialmessaging.associate-business-account-access-token-exceeds-max-rejected|_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# socialmessaging: AssociateWhatsAppBusinessAccount accepts a console-only signup callback with HTTP 200 instead of refusing it 403 AccessDeniedException 'CSRF header does not exist'

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWhatsAppBusinessAccount

## Description
One net-new behavioral root cause. ONE cause must be closed. The same underlying defect is already described as Cause 1 of the open wire issue 20261029T171221Z-social-messaging-whatsapp-wire-responses-diverge-from-recording.md (Issue-Key social-messaging|AssociateWhatsAppBusinessAccount|4xx->2xx|status); this issue records the BEHAVIORAL check that also fails on it, so the fixer re-replays that check too. Nothing here asks for a second fix: closing the admission gate closes both.

Cause 1 of 1 — the console/CSRF admission gate is missing, so a signup presented outside the console is accepted and mutates state.
AWS: Smithy: AssociateWhatsAppBusinessAccount, POST /v1/whatsapp/signup; input `signupCallback` (WhatsAppSignupCallback, whose `accessToken` is @length max 1000) and `setupFinalization`; operation errors DependencyException 502, InvalidParametersException 400, LimitExceededException 400, ThrottledRequestException 429, plus service-common AccessDeniedException 403 and ValidationException 400. The operation is documented as 'only used through the Amazon Web Services console during sign-up'. Recorded behavior: an accessToken one character over the limit (1001 chars) is refused 400 ValidationException '1 validation error detected: Value at \'signupCallback.accessToken\' failed to satisfy constraint: Member must have length less than or equal to 1000' (fixed text; the member path is the request-derived slot, and the token value itself is not echoed) — that half already passes. A token AT or UNDER the limit passes the length check and is then refused 403 AccessDeniedException with the fixed message 'CSRF header does not exist' (wholly fixed text: nothing from the request appears in it), because the callback did not arrive through the console's CSRF-protected flow.
Emulator: answers HTTP 200 with a success body — signupCallbackResult.associateInProgressToken (a freshly minted 'signup-<hex>' token), linkedAccountsWithIncompleteSetup and statusCode 200 — for the valid-length token. No refusal of any kind, and the accepted call RECORDS a pending signup, so a follow-up setupFinalization succeeds against the emulator and 403s against AWS.
Why: the console-only door is wide open. A client, or a test of least-privilege/negative behavior, gets a success where AWS denies access; SDK error handling keyed on AccessDeniedException/403 never fires; and the emulator's state diverges permanently because a signup AWS never created now exists.
Fix: AssociateWhatsAppBusinessAccount's request-admission gate (the console/CSRF precondition), evaluated ahead of every store mutation so a refused call leaves the pending-signup store untouched, returning 403 AccessDeniedException with AWS's fixed text. The gate belongs in the AssociateWhatsAppBusinessAccount handler; its existing checks cover only member presence, token validity and store capacity.
Traces: socialmessaging.associate-business-account-access-token-exceeds-max-rejected

## Evidence
Behavioral suite simparity-behavioral-aws-v1: socialmessaging.associate-business-account-access-token-exceeds-max-rejected [mismatch], not marked ALREADY FILED and not OWNED by any open issue.

Assertion from the replay (the 1001-character ValidationException half passed; the second call is the divergence):
  File "check_socialmessaging_associate_business_account_access_token_exceeds_max_rejected.py", line 36, in run -> _error(...)
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'signupCallbackResult': {'associateInProgressToken': 'signup-<hex>', 'linkedAccountsWithIncompleteSetup': {}}, 'statusCode': 200}

Request that diverges: AssociateWhatsAppBusinessAccount(signupCallback={'accessToken': <a <=1000-char token that Meta would reject>, 'callbackUrl': 'https://example.com/callback'}).
Expected: HTTP 403, error code AccessDeniedException, message 'CSRF header does not exist' (fixed text).
Actual: HTTP 200 with a minted associateInProgressToken; a pending signup is recorded in the service state.

Source evidence: services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/associate_whats_app_business_account.rs (handle: presence/token/capacity gates only, no console-origin precondition; pending signup minted and stored on the path AWS refuses); .../src/state.rs (pending-signup store, PENDING_SIGNUP_TTL, MAX_PENDING_SIGNUPS). Model/contract: OPERATIONS.md lines 48-53 and .../smithy-model/com-amazonaws-socialmessaging-socialmessaging.json (service-common AccessDeniedException, HTTP 403).

## Steps to Reproduce
Replay socialmessaging.associate-business-account-access-token-exceeds-max-rejected from simparity-behavioral-aws-v1 against the emulator built from this checkout. By hand:

1. POST /v1/whatsapp/signup with signupCallback.accessToken of 1001 characters -> must stay 400 ValidationException '1 validation error detected: Value at \'signupCallback.accessToken\' failed to satisfy constraint: Member must have length less than or equal to 1000' (already correct).
2. POST /v1/whatsapp/signup with a <=1000-character accessToken and a callbackUrl, i.e. a callback not presented through the console flow -> expect HTTP 403, code AccessDeniedException, message 'CSRF header does not exist'; the emulator answers HTTP 200 with signupCallbackResult.associateInProgressToken.
3. Confirm the refused call records NO pending signup (a subsequent setupFinalization using any token from step 2 must not succeed).

The check must pass before this issue is closed.
