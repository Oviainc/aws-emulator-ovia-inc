Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f0ec4f9c2d80e54faf7f45278fba8cf656265d52

# A readable account with no primary email update is refused 400 ValidationException instead of the modelled 404 ResourceNotFoundException

_Filed: 2026-10-27T16:38:24Z_
_Operation: GetPrimaryEmailUpdateStatus | Class: error-response_

> _Note: this issue was identified against the code base at commit `f0ec4f9c2d80e54faf7f45278fba8cf656265d52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the target account is resolvable and readable but carries no `primary_email_update` record at all, `GetPrimaryEmailUpdateStatus` (and, through the same shared helper, `AcceptPrimaryEmailUpdate`) answers HTTP 400 `ValidationException` with the message "No email update found for account" and NO `reason` member.

That is the wrong modelled error for this condition, and the crate says so itself:

1. The model documents `ValidationException` as "The operation failed because one of the input parameters was invalid." Here no input parameter was invalid — the request's only member (`AccountId`) satisfied `^\d{12}$` and named an account the caller may read. The helper's own doc comment concedes exactly this and omits `reason` for that reason, so the handler ships a 400 whose modelled `reason` member cannot be populated because nothing was invalid, while its `message` text ("No email update found") states a resource-absence fact.
2. Both operations DECLARE `ResourceNotFoundException` (HTTP 404), documented as "The operation failed because it specified a resource that can't be found", and neither handler can ever produce it — the error is modelled but unreachable.
3. Two sibling handlers in this same crate decide the identical shape of situation — account resolved and readable, the named sub-resource absent — the other way, with `ResourceNotFoundException`: `GetAlternateContact` for a contact type that is not stored, and `GetGovCloudAccountInformation` for an account with no linked GovCloud account. The "account the caller may not read" case is already separated out and answered 403 `AccessDeniedException` in all of these handlers, so the only remaining case is the missing sub-resource, and this one handler classifies it differently from its siblings.

Effect: a client cannot distinguish "your request was malformed" from "this account has no update in flight" by status code, and the 404 both operations model is dead.

## Evidence

Model, `smithy_query(query="shape", shape="ValidationException")`: doc = "The operation failed because one of the input parameters was invalid."; `smithy_query(query="shape", shape="ResourceNotFoundException")`: doc = "The operation failed because it specified a resource that can't be found.", httpError 404.
Model, `smithy_query(query="operation", operation="GetPrimaryEmailUpdateStatus")` errors include `{"shape": "ResourceNotFoundException", "errorKind": "client", "httpError": 404}` — likewise for `AcceptPrimaryEmailUpdate`.

The handler, services/service-com-amazonaws-account-account/src/ops/get_primary_email_update_status.rs:206-215:
```
        let Some(update) = account.primary_email_update.as_ref() else {
            // No update has ever been started for this account, and `Status` is a
            // REQUIRED output member: there is no most-recent update to report.
            return Err(
                GetPrimaryEmailUpdateStatusErrorModeled::ValidationException(
                    crate::state::no_primary_email_update(),
                )
```
The same helper on the second call site, services/service-com-amazonaws-account-account/src/ops/accept_primary_email_update.rs:240-247: `return Err(AcceptPrimaryEmailUpdateErrorModeled::ValidationException(crate::state::no_primary_email_update())...)`.

The helper's own admission, services/service-com-amazonaws-account-account/src/state.rs:1336-1345:
```
/// ... The modelled refusal is the 400 `ValidationException`; the
/// `reason` member is left absent because neither
/// `ValidationExceptionReason` value (`fieldValidationFailed |
/// invalidRegionOptTarget`) describes it — no input member was invalid.
pub fn no_primary_email_update() -> ValidationException {
    ValidationException {
        message: "No email update found for account".to_string(),
        reason: None,
```

The sibling rule in this crate, services/service-com-amazonaws-account-account/src/ops/get_alternate_contact.rs:213-218:
```
            // The account exists, but nothing of that type is stored on it: the
            // resource this call names can't be found.
            None => Err(GetAlternateContactErrorModeled::ResourceNotFoundException(
                crate::state::alternate_contact_not_found(),
```
and services/service-com-amazonaws-account-account/src/ops/get_gov_cloud_account_information.rs:220-229:
```
            // The account is readable and simply has no GovCloud account linked
            // to it: the operation reports a linked GovCloud account "if it
            // exists", so there is no resource to report.
            return Err(
                GetGovCloudAccountInformationErrorModeled::ResourceNotFoundException(
```

Test coverage that currently pins the 400 (both state only "has nothing to report/accept", neither justifies the code over the modelled 404): tests/all/primary_email_update.rs:158-171 (`a_status_read_with_no_update_started_is_refused`, row "no AccountId at all, so the caller's own account" expects 400 "ValidationException") and tests/all/primary_email_update_acceptance.rs:12-18 ("an account with no update at all has nothing to accept (HTTP 400 `ValidationException` ...)"). This finding does not touch the unknown-account/own-account rows, which are 403 `AccessDeniedException` and stay that way.

## Steps to Reproduce

1. Start the emulator with only this service mounted (as tests/all/primary_email_update.rs::emulator does).
2. POST /getPrimaryEmailUpdateStatus with body `{}` — the caller's own account, which the store always holds, so the account is readable and no input member is invalid.
3. Observe HTTP 400 with `__type` `ValidationException`, message "No email update found for account" and no `reason` member — although the operation models `ResourceNotFoundException` (404) and the request carried no invalid input parameter.
4. Compare with POST /getAlternateContact `{"AlternateContactType":"BILLING"}` on the same (readable) account with no contact stored: HTTP 404 `ResourceNotFoundException` — the sibling handler's answer for the identical 'account readable, sub-resource absent' condition.
5. Same divergence via the second call site: POST /startPrimaryEmailUpdate for a member account, then POST /acceptPrimaryEmailUpdate for a DIFFERENT member account id (readable only after a write creates it) that has no update — 400 ValidationException instead of the modelled 404.
