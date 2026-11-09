Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-channel.delete-program-management-account-nonexistent-error
Issue-Shape-Id: com.amazonaws.partnercentralchannel#PartnerCentralChannel
Issue-Key: partnercentral-channel|ListProgramManagementAccounts|AccessDenied
Issue-Commit: 8dd4ff80cd36ae979d1a78db550805b7fd844998

# partnercentral-channel: ListProgramManagementAccounts / DeleteProgramManagementAccount entitlement refusal is served as unmodeled AccessDenied "Sign the request with the emulator credentials" instead of AccessDeniedException / ValidationException (1 cause)

> _Note: this issue was identified against the code base at commit `8dd4ff80cd36ae979d1a78db550805b7fd844998`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListProgramManagementAccounts

## Description
One net-new root cause in the partnercentral-channel service, reached by a replay key the already-open issue does not carry.

Related but NOT covering: the open issue 20261029T174824Z-partnercentral-channel-is-unusable-every-operation-is-refused.md describes the same family of refusals, and even lists this check id, but its recorded replay keys for this check bind to a different assertion (`...delete-program-management-account-nonexistent-error|run#4|AssertionError|assert code0 == "ValidationException"...`). In this run the check fails EARLIER, at the ListProgramManagementAccounts entitlement probe (run.py line 33), so the suite did not mark this failure OWNED and it is filed here with its actual key. Fixing it must also satisfy the assertion that issue records.

Cause 1 of 1 — the program-management-account plane answers every call with the pre-dispatch signing refusal, so neither the entitlement denial nor input-shape validation nor an existence check ever runs.
AWS: ListProgramManagementAccounts is a read of the program-management-account plane (Smithy: PartnerCentralChannel, awsJson1_0, authSchemes sigv4/sigv4a; ListProgramManagementAccounts -> ListProgramManagementAccountsResponse{items, nextToken}; input requires `catalog` @length 1..64 @pattern ^[a-zA-Z]*$, `maxResults` @range 1..100 default 20; modeled errors AccessDeniedException 403, ValidationException 400, ResourceNotFoundException 404, ConflictException 409, ThrottlingException 429, InternalServerException 500). For a SigV4-signed caller whose account holds no active AWS Partner benefit, the call is refused with the modeled AccessDeniedException (awsJson1_0 `__type` AccessDeniedException, HTTP 403, member `message` @required) whose message for this class is fixed text with no variable part: `This action requires an active AWS Partner benefit. Register as an AWS Partner if you haven't already, or contact AWS Partner Central Support.` (recorded behavior only; the model documents AccessDeniedException merely as insufficient permissions). For a caller that DOES hold the benefit the call is HTTP 200 with `items` always present (empty list when nothing matches).
The same request-handling order governs DeleteProgramManagementAccount: constraint validation first, so `identifier` violating ProgramManagementAccountIdentifier (@length 17..1011, @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/program-management-account/)?pma-[a-z0-9]{13}$) — e.g. a value that is not a pma- id at all — is ValidationException 400 whose message is the fixed sentence `1 validation error detected.` with a fieldList entry naming the offending member (`name` = `identifier`, `code` = `INVALID_STRING_FORMAT`, `message` = `Value at 'identifier' failed to satisfy constraint: <the pattern rule>`); the member name and rule text come from the request and the model, the count and wrapper sentence are fixed. Only for a well-formed identifier does the entitlement/existence decision follow: AccessDeniedException with the fixed partner-benefit text when the account has no benefit, otherwise ResourceNotFoundException 404 — and the same code and message on an immediate repeat of the identical call.
Emulator: every one of these calls is answered HTTP 403 with awsJson1_0 error code `AccessDenied` and message `Sign the request with the emulator credentials`, observed verbatim as `AssertionError: {'Message': 'Sign the request with the emulator credentials', 'Code': 'AccessDenied'}` on the ListProgramManagementAccounts probe. No modeled error is ever produced: not AccessDeniedException, not ValidationException, not ResourceNotFoundException, and no `fieldList`.
Why: a client cannot classify or act on the failure. `AccessDenied` is not in this service's modeled error set at all, so a typed `except AccessDeniedException` / `except ValidationException` handler catches nothing and the error falls through as an unknown fault; the message tells a partner-onboarding flow to re-sign its request instead of to register as an AWS Partner. The 400-vs-403 boundary between "your request is malformed" and "you may not do this" disappears, so a retry policy that retries authorization faults will retry a permanently malformed identifier forever, and the whole program-management-account plane is unreadable and undeletable.
Fix: the pre-dispatch request-authentication/credential gate the router applies to this service (it must accept the emulator's standard SigV4 credentials for partnercentral-channel as every other bootstrapped service does), then the request-validation stage and the entitlement/existence decision of the ListProgramManagementAccounts and DeleteProgramManagementAccount handlers in services/service-com-amazonaws-partnercentralchannel-partnercentralchannel/src/ops/, plus the awsJson1_0 error serializer that must emit the modeled AccessDeniedException / ValidationException (with fieldList) / ResourceNotFoundException bodies rather than a signing-layer `AccessDenied`.
Traces: partnercentral-channel.delete-program-management-account-nonexistent-error.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): partnercentral-channel 0/20 passed (0%) — 13 mismatched, 7 errored, 0 skipped, 0 unloadable. 19 of the 20 failures were marked "OWNED by 20261029T174824Z-partnercentral-channel-is-unusable-every-operation-is-refused.md" and were skipped. This failure was NOT marked owned.

[mismatch] partnercentral-channel.delete-program-management-account-nonexistent-error
  File "check_partnercentral_channel_delete_program_management_account_nonexistent_error.py", line 33, in run
    assert (error["Code"], error.get("Message")) == ("AccessDeniedException", _NO_PARTNER_BENEFIT), error
  AssertionError: {'Message': 'Sign the request with the emulator credentials', 'Code': 'AccessDenied'}

Expected (ListProgramManagementAccounts, caller without an active partner benefit): HTTP 403, Error.Code = `AccessDeniedException`, Error.Message = `This action requires an active AWS Partner benefit. Register as an AWS Partner if you haven't already, or contact AWS Partner Central Support.` — fixed text, no variable part. Or, for an entitled caller, HTTP 200 with `items` present.
Observed: HTTP 403, Error.Code = `AccessDenied`, Error.Message = `Sign the request with the emulator credentials`.

The same check then grades DeleteProgramManagementAccount and expects, in order:
  identifier='not-a-valid-pma-identifier' -> ValidationException (400), message `1 validation error detected.` + fieldList[{name: identifier, code: INVALID_STRING_FORMAT, message: "Value at 'identifier' failed to satisfy constraint: <pattern rule>"}];
  identifier='pma-1111111111111' (well formed, nonexistent) -> AccessDeniedException with the fixed partner-benefit text when the account has no benefit, else ResourceNotFoundException;
  an immediate second identical call -> the same code and message.
None of these can be reached because the service refuses at the signing gate.

Smithy (services/service-com-amazonaws-partnercentralchannel-partnercentralchannel/smithy-model/com-amazonaws-partnercentralchannel-partnercentralchannel.json):
  ListProgramManagementAccounts — readonly, paginated(items/nextToken/maxResults); input catalog @required @length 1..64 @pattern ^[a-zA-Z]*$, maxResults @range 1..100 default 20; errors AccessDeniedException(403), ValidationException(400), ResourceNotFoundException(404), ThrottlingException(429), InternalServerException(500).
  DeleteProgramManagementAccount — idempotent; input catalog @required, identifier @required target ProgramManagementAccountIdentifier @length 17..1011 @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/program-management-account/)?pma-[a-z0-9]{13}$, clientToken @idempotencyToken; errors AccessDeniedException(403), ConflictException(409), ValidationException(400), ResourceNotFoundException(404), ThrottlingException(429), InternalServerException(500).
  `AccessDenied` is not a modeled error shape of this service.

Bootstrap gate: partnercentral-channel is stood up — crate service-com-amazonaws-partnercentralchannel-partnercentralchannel is in get_bootstrap_status().bootstrapped and get_bootstrap_status().pending is empty — and the service answers HTTP; it answers wrongly.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. run_behavioral_suite() and look at the partnercentral-channel group: 0/20 pass; partnercentral-channel.delete-program-management-account-nonexistent-error fails at its first call.
3. With the emulator's standard SigV4 credentials, call ListProgramManagementAccounts(catalog='Sandbox'). Expected either HTTP 200 with `items` present, or HTTP 403 AccessDeniedException with the fixed message "This action requires an active AWS Partner benefit. Register as an AWS Partner if you haven't already, or contact AWS Partner Central Support.". Observed HTTP 403 AccessDenied "Sign the request with the emulator credentials".
4. Call DeleteProgramManagementAccount(catalog='Sandbox', identifier='not-a-valid-pma-identifier'). Expected ValidationException 400 with message "1 validation error detected." and a fieldList entry for `identifier`. Observed the same AccessDenied signing refusal.
5. Call DeleteProgramManagementAccount(catalog='Sandbox', identifier='pma-1111111111111'), twice. Expected AccessDeniedException with the partner-benefit text (unentitled account) or ResourceNotFoundException (entitled account), identical on both calls. Observed the same AccessDenied signing refusal.
6. Re-replay partnercentral-channel.delete-program-management-account-nonexistent-error to confirm.
