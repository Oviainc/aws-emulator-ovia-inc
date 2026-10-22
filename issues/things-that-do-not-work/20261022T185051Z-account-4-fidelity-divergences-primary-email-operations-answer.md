Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: account.get-primary-email-update-status-never-started-not-found account.start-primary-email-update-self-target-denied account.account-name-too-long-rejected account.alternate-contact-delete-removes-record account.alternate-contact-not-found-when-unset
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account.get-primary-email-update-status-never-started-not-found|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: account.start-primary-email-update-self-target-denied|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: account.account-name-too-long-rejected|run#8|AssertionError|assert message == (
Issue-Key: account.alternate-contact-delete-removes-record|run#11|AssertionError|assert message == "No contact of the inputted alternate contact type found.", message
Issue-Key: account.alternate-contact-not-found-when-unset|run#8|AssertionError|assert get_message == "No contact of the inputted alternate contact type found.", get_message
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# account: 4 fidelity divergences — primary-email operations answer 501 NotImplemented instead of their modeled refusals, and ValidationException/ResourceNotFoundException message rules diverge

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPrimaryEmailUpdateStatus, StartPrimaryEmailUpdate, PutAccountName, GetAlternateContact, DeleteAlternateContact

## Description
The account (AWS Account Management) emulator is bootstrapped and answers, but 4 distinct root causes diverge from real AWS. Each is numbered below with the failing behavioral check ids it is grounded in; a fixer must close ALL FOUR before submitting. No existing open issue covers any of them (the open list contains only ecr/fis/xray/billingconductor/devopsguru/codedeploy items; the separately-filed account stub issues cover GetContactInformation, GetRegionOptStatus and ListRegions only — NOT the two operations named in Causes 1 and 2).

=== Cause 1 of 4: GetPrimaryEmailUpdateStatus is an unimplemented scaffold stub, so it answers 501 NotImplemented instead of the modeled 400 ValidationException ===
AWS: Smithy (services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json) models GetPrimaryEmailUpdateStatus with input AccountId (optional, pattern ^\d{12}$), output Status (PrimaryEmailUpdateStatus: ACCEPTED|COMPLETED|FAILED|PENDING, required) + UpdatedAt, and errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400). Recorded behavior: called with no AccountId from a standalone (non-organization-management) account where no email update has ever been started, real AWS refuses the call with HTTP 400, error code ValidationException (recorded message, an example only: "No email update found for account"). The requirement is the status 400 and the code ValidationException; 501/NotImplemented is not a modeled outcome of this operation at all.
Emulator: the call returns HTTP 501 with error code NotImplemented and message "operation GetPrimaryEmailUpdateStatus is scaffolded but not implemented in this emulator" (handler is the generated stub: the SCAFFOLDED marker is still at the top of the file and handle() returns Scaffolded::not_implemented).
Why: a client's error handling branches on the modeled code. A caller that catches ValidationException/ResourceNotFoundException to decide "no update in flight" instead sees an unmodeled NotImplemented, which botocore surfaces as an unretryable unknown-error and no SDK error-matching branch catches; any readiness or status-polling path built on this operation cannot run at all.
Fix: implement the operation's handler and its refusal path, then delete the SCAFFOLDED marker line.
Traces: check account.get-primary-email-update-status-never-started-not-found (asserted code ValidationException, got 'NotImplemented').

=== Cause 2 of 4: StartPrimaryEmailUpdate is an unimplemented scaffold stub, so a self-targeting request is accepted-as-501 instead of refused with 403 AccessDeniedException ===
AWS: Smithy models StartPrimaryEmailUpdate with required AccountId (pattern ^\d{12}$) and required PrimaryEmail (@length 5..64, sensitive), output Status, and errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400). The operation updates only MEMBER accounts from the management or delegated-admin account (per the model's own AccountId documentation), never the caller's own account. Recorded behavior: a request whose AccountId equals the caller's own account id is refused with HTTP 403, error code AccessDeniedException — the refusal happens before the PrimaryEmail value is acted on (the recorded request's address is deliberately malformed and no update is started).
Emulator: the call returns HTTP 501 with error code NotImplemented ("operation StartPrimaryEmailUpdate is scaffolded but not implemented in this emulator"); the self-target authorization rule is absent because no handler exists. Handler is the generated stub with the SCAFFOLDED marker still present.
Why: the whole primary-email update surface is unusable, and the specific guardrail AWS enforces (you cannot retarget your own account's primary email through this API) is not expressed, so code that relies on being refused gets an unmodeled error code instead of 403/AccessDeniedException and cannot distinguish "not permitted" from "service failure".
Fix: implement the operation's handler, including the self-target authorization refusal, and the state the status read in Cause 1 reports; then delete the SCAFFOLDED marker line.
Traces: check account.start-primary-email-update-self-target-denied (asserted code AccessDeniedException, got 'NotImplemented').

=== Cause 3 of 4: PutAccountName length-constraint refusal uses a generic framework validation message instead of the account service's own message rule ===
AWS: Smithy: PutAccountName.AccountName is required, @length(min:1,max:50), pattern ^[ -;=?-~]+$, sensitive; error ValidationException (HTTP 400). For an AccountName longer than 50 characters AWS refuses with 400/ValidationException and a message built deterministically from the request — the offending string, its actual length and the maximum: '[string "<value>" is too long (length: <len>, maximum allowed: 50)]'. That is a rule, not a recorded constant: every element of it is derived from the request and the modeled constraint.
Emulator: the status (400) and code (ValidationException) are correct, but the boundary validator emits the generic shape-validation text instead: "1 validation error detected: Value '<value>' at 'accountName' failed to satisfy constraint: Member must have length less than or equal to 50".
Why: clients and test harnesses that parse the Error.Message of an account-management validation failure to report which value was rejected and against what maximum read a different grammar than AWS emits, so message-driven assertions and user-facing error surfacing built against real AWS break against the emulator even though the code is right.
Fix: the per-service validation/refusal message formatting for the account service (the length-violation path that produces the ValidationException for PutAccountName), not the operation's state handling — the write itself is correctly not applied.
Traces: check account.account-name-too-long-rejected (code ValidationException matched; message assertion failed).

=== Cause 4 of 4: the alternate-contact ResourceNotFoundException message is request-dependent instead of the fixed message AWS returns ===
AWS: Smithy: GetAlternateContact and DeleteAlternateContact both take required AlternateContactType (BILLING|OPERATIONS|SECURITY) and optional AccountId, and both model ResourceNotFoundException (HTTP 404) — "The operation failed because it specified a resource that can't be found." Recorded behavior: for a contact type with no stored contact (never set, or just deleted), both operations return 404/ResourceNotFoundException with the FIXED message "No contact of the inputted alternate contact type found." The message is invariant of the request: two independent checks observe the identical literal for two different contact types (BILLING and OPERATIONS), across both the Get and the Delete path, so the rule is 'one constant message, naming neither the contact type nor the account'.
Emulator: status and code are correct (404 ResourceNotFoundException on Get-after-Delete, on a second Delete, and on a never-set type), but the message is synthesized per request and leaks the resolved identifiers, e.g. "No BILLING alternate contact is set for account 000000000000" and "No OPERATIONS alternate contact is set for account 000000000000" (built by crate::state::alternate_contact_not_found and returned from the Get/Delete handlers).
Why: the emulator's message varies with the contact type and embeds an account id, so clients or fixtures that match on the account service's not-found message see per-request text where AWS guarantees one constant string; it also surfaces an account identifier in an error body where AWS surfaces none.
Fix: the shared not-found error construction for alternate contacts in the account service's state/error module, used by both the Get and the Delete handler. The stored-state behavior itself is correct (delete removes the record, the type is reusable by a later Put).
Traces: checks account.alternate-contact-delete-removes-record and account.alternate-contact-not-found-when-unset (both asserted the code successfully and failed on the message).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): account 8/22 passed, 14 failed. 9 of the failures are errors already covered by the auto-filed stub issues for GetContactInformation, GetRegionOptStatus and ListRegions and are excluded here. The 5 remaining failures are the ones clustered above:

1) account.get-primary-email-update-status-never-started-not-found [mismatch]
   expected: HTTP 400, Error.Code == "ValidationException" (recorded message example: "No email update found for account")
   actual:   HTTP 501, Error.Code == "NotImplemented", message "operation GetPrimaryEmailUpdateStatus is scaffolded but not implemented in this emulator"
   assertion: AssertionError: expected ValidationException, got 'NotImplemented'

2) account.start-primary-email-update-self-target-denied [mismatch]
   expected: HTTP 403, Error.Code == "AccessDeniedException" for StartPrimaryEmailUpdate(AccountId=<caller's own id>)
   actual:   HTTP 501, Error.Code == "NotImplemented"
   assertion: AssertionError: expected AccessDeniedException, got 'NotImplemented'

3) account.account-name-too-long-rejected [mismatch]
   PutAccountName(AccountName=<55 chars>)
   expected: 400 ValidationException, message '[string "<value>" is too long (length: 55, maximum allowed: 50)]'
   actual:   400 ValidationException, message "1 validation error detected: Value '<value>' at 'accountName' failed to satisfy constraint: Member must have length less than or equal to 50"

4) account.alternate-contact-delete-removes-record [mismatch]
   GetAlternateContact(AlternateContactType=BILLING) after DeleteAlternateContact
   expected: 404 ResourceNotFoundException, message "No contact of the inputted alternate contact type found."
   actual:   404 ResourceNotFoundException, message "No BILLING alternate contact is set for account 000000000000"

5) account.alternate-contact-not-found-when-unset [mismatch]
   Get/DeleteAlternateContact(AlternateContactType=OPERATIONS) with nothing stored
   expected: 404 ResourceNotFoundException, message "No contact of the inputted alternate contact type found." (identical literal to case 4 — hence invariant of contact type and account)
   actual:   404 ResourceNotFoundException, message "No OPERATIONS alternate contact is set for account 000000000000"

Code grounding: services/service-com-amazonaws-account-account/src/ops/get_primary_email_update_status.rs:4 and :175 (SCAFFOLDED marker; handle returns Scaffolded::not_implemented), services/service-com-amazonaws-account-account/src/ops/start_primary_email_update.rs:4 (same), services/service-com-amazonaws-account-account/src/ops/get_alternate_contact.rs:192-195 (crate::state::alternate_contact_not_found(&account_id, contact_type)), services/service-com-amazonaws-account-account/src/ops/put_account_name.rs:183-185 (length/pattern refused by the shared boundary validator before the handler).

## Steps to Reproduce
Boot the emulator and point an account-service client at it (credentials/region irrelevant; the resolved account is the emulator's own account id).

Cause 1: account.get_primary_email_update_status() with no AccountId and no update ever started. AWS: ClientError with HTTP 400 / Code=ValidationException. Emulator: HTTP 501 / Code=NotImplemented.

Cause 2: own = account.get_account_information()["AccountId"]; then account.start_primary_email_update(AccountId=own, PrimaryEmail="sim2real-behav-not-an-email-address"). AWS: HTTP 403 / Code=AccessDeniedException and no update recorded. Emulator: HTTP 501 / Code=NotImplemented.

Cause 3: account.put_account_name(AccountName="sim2real-behav-" + "x"*40) (55 chars, over the modeled 50 max). Both AWS and the emulator answer 400 ValidationException; compare Error.Message — AWS: '[string "<value>" is too long (length: 55, maximum allowed: 50)]'; emulator: "1 validation error detected: Value '<value>' at 'accountName' failed to satisfy constraint: ...".

Cause 4: account.put_alternate_contact(AlternateContactType="BILLING", ...) then account.delete_alternate_contact(AlternateContactType="BILLING") then account.get_alternate_contact(AlternateContactType="BILLING"); repeat with "OPERATIONS" never set (both Get and a second Delete). Each raises 404 ResourceNotFoundException; compare Error.Message — AWS returns the same constant "No contact of the inputted alternate contact type found." in every case; the emulator returns a different string per contact type that embeds the account id.

Replay to confirm: the 5 check ids listed in trace_ids.
