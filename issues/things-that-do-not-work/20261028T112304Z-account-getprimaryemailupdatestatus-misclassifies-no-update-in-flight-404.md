Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: account.get-primary-email-update-status-never-started-not-found account.start-primary-email-update-self-target-denied
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account.get-primary-email-update-status-never-started-not-found|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: account.start-primary-email-update-self-target-denied|run#7|AssertionError|assert status_code == "ValidationException", f"expected ValidationException, got {status_code!r}"
Issue-Commit: ad2eaa5d43ab5cd0296a6d5c6ea30e6c9d7fc9f9

# account GetPrimaryEmailUpdateStatus misclassifies "no update in flight": 404 ResourceNotFoundException instead of AWS's 400 ValidationException

> _Note: this issue was identified against the code base at commit `ad2eaa5d43ab5cd0296a6d5c6ea30e6c9d7fc9f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPrimaryEmailUpdateStatus

## Description
One root cause, grounded in two failing behavioral checks.

Cause 1 of 1: GetPrimaryEmailUpdateStatus answers the "this account has no most-recent primary email update to report" condition with the modelled 404 `ResourceNotFoundException`, where real AWS answers the modelled 400 `ValidationException`. Failing checks: account.get-primary-email-update-status-never-started-not-found, account.start-primary-email-update-self-target-denied.

AWS: Smithy (services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json) models GetPrimaryEmailUpdateStatus with input member `AccountId` (target AccountId, pattern ^\d{12}$, NOT required) and errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), TooManyRequestsException (429), ValidationException (400, "the operation failed because one of the input parameters was invalid"). The model does not state WHICH of those two client errors covers an omitted AccountId on an account that is not an organization management/delegated-admin account, so for that discrimination this is recorded behavior only: the recording shows real AWS classifying it as `ValidationException` (HTTP 400) with message `No email update found for account`. Message rule: the entire sentence `No email update found for account` is FIXED text — it interpolates nothing, no account id, no address (PrimaryEmailAddress is @sensitive), so it is the same sentence whichever account the call resolved to and whether or not AccountId was supplied. Hypothesis (not stated by the model or recording): AWS treats this as input-shape validation because a caller with no organization to manage cannot supply the member AccountId the operation needs, so the request is refused on its parameters rather than on a missing resource.

Emulator: the handler resolves the account, finds `account.primary_email_update` is None, and returns `ResourceNotFoundException` built by `crate::state::no_primary_email_update()`. The message text is already correct (`No email update found for account`), so the divergence is purely the error class and status: emulator answers code `ResourceNotFoundException` / HTTP 404, AWS answers code `ValidationException` / HTTP 400. The crate's own doc comment argues for the 404 explicitly, so this is a deliberate-but-wrong classification, not an oversight. The same divergence shows up a second time as the effect-absence probe of the self-target check: StartPrimaryEmailUpdate correctly refuses a self-targeting request with AccessDeniedException, but the follow-up status read that proves no update was recorded then returns ResourceNotFoundException instead of ValidationException.

Why: a client branching on the error code gets the wrong branch. `ResourceNotFoundException` reads as "the account/update resource is gone" and is commonly treated as a benign, retry-or-create signal, while `ValidationException` reads as "your request was not well-formed for this caller" and is terminal. SDK retry/error-handling layers and waiters that key on 404 vs 400 therefore diverge, and any caller asserting on the modelled `reason` field of ValidationException never sees that error at all. It also makes the "no email update is in flight" probe — the normal way to confirm no update was started — unusable as a portable assertion.

Fix: the GetPrimaryEmailUpdateStatus handler's no-update branch (services/service-com-amazonaws-account-account/src/ops/get_primary_email_update_status.rs) must return the modelled ValidationException rather than ResourceNotFoundException, and the shared refusal constructor in the service state (services/service-com-amazonaws-account-account/src/state.rs, `no_primary_email_update()` / `NO_PRIMARY_EMAIL_UPDATE_MESSAGE`) must build that 400 payload, keeping the fixed message text unchanged. Note that `AcceptPrimaryEmailUpdate` shares the same constructor, so the change must be made where the two callers' AWS classifications actually agree — if accept keeps 404, the constructor has to be split rather than retargeted wholesale.

Traces: account.get-primary-email-update-status-never-started-not-found (direct: asserts code == ValidationException and message == "No email update found for account"); account.start-primary-email-update-self-target-denied (same refusal reached as the effect-absence probe after the AccessDeniedException self-target refusal, which the emulator gets right).

## Evidence
Check account.get-primary-email-update-status-never-started-not-found:
  AssertionError: expected ValidationException, got 'ResourceNotFoundException'
  (check_account_get_primary_email_update_status_never_started_not_found.py line 29)
  Expected: error code ValidationException, HTTP 400, Message "No email update found for account"
  Actual:   error code ResourceNotFoundException, HTTP 404, Message "No email update found for account"

Check account.start-primary-email-update-self-target-denied:
  AssertionError: expected ValidationException, got 'ResourceNotFoundException'
  (check_account_start_primary_email_update_self_target_denied.py line 41)
  The StartPrimaryEmailUpdate self-target refusal itself is correct (AccessDeniedException); the
  subsequent GetPrimaryEmailUpdateStatus effect-absence probe diverges identically.

Emulator source:
  services/service-com-amazonaws-account-account/src/ops/get_primary_email_update_status.rs:206-219
    `let Some(update) = account.primary_email_update.as_ref() else { ... ResourceNotFoundException(crate::state::no_primary_email_update()) ... }`
  services/service-com-amazonaws-account-account/src/state.rs:1355-1364
    `pub fn no_primary_email_update() -> ResourceNotFoundException { message: NO_PRIMARY_EMAIL_UPDATE_MESSAGE ... }`
    `pub const NO_PRIMARY_EMAIL_UPDATE_MESSAGE: &str = "No email update found for account";`

Smithy model (services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json):
  GetPrimaryEmailUpdateStatus input { AccountId: AccountId, pattern ^\d{12}$, optional }
  errors: AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404),
          TooManyRequestsException(429), ValidationException(400)
  http: POST /getPrimaryEmailUpdateStatus, success 200

## Steps to Reproduce
1. Boot the emulator and point a botocore `account` client at it with credentials for an account that belongs to no organization (organizations:DescribeOrganization answers AWSOrganizationsNotInUseException).
2. Confirm the service answers at all: `account.get_account_information()` -> 200 with an AccountId (positive control; this passes today).
3. Call `account.get_primary_email_update_status()` with NO AccountId, on an account for which no StartPrimaryEmailUpdate has ever been recorded.
4. Observe the raised ClientError: emulator reports Error.Code == 'ResourceNotFoundException' with HTTP 404. AWS reports Error.Code == 'ValidationException' with HTTP 400 and Message 'No email update found for account'.
5. Second path to the same refusal: call `account.start_primary_email_update(AccountId=<own account id>, PrimaryEmail='<malformed address>')`, observe the correct AccessDeniedException, then call `account.get_primary_email_update_status()` again to prove no update was recorded — the same ResourceNotFoundException/ValidationException divergence appears.
