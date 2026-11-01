Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: macie2.create-member-self-account-rejected macie2.custom-data-identifier-invalid-regex-rejected macie2.get-custom-data-identifier-not-found
Issue-Shape-Id: com.amazonaws.macie2#Macie2
Issue-Key: macie2.create-member-self-account-rejected|run#5|AssertionError|assert error["Code"] == "ValidationException", f"unexpected code: {error['Code']}"
Issue-Key: macie2.custom-data-identifier-invalid-regex-rejected|_reject#5|AssertionError|raise AssertionError(
Issue-Key: macie2.get-custom-data-identifier-not-found|run#6|AssertionError|assert error["Message"] == f"Custom Data Identifier not found for id: {bogus_id}", (
Issue-Commit: 85498e1f8b4eef953ced847d4ecb0ca61f30a5a5

# macie2: 3 net-new behavioral root causes — CreateMember answers 501 NotImplemented, CreateCustomDataIdentifier accepts a syntactically invalid regex, and GetCustomDataIdentifier's not-found message is emulator-authored

> _Note: this issue was identified against the code base at commit `85498e1f8b4eef953ced847d4ecb0ca61f30a5a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMember

## Description
Three independent, net-new root causes in the macie2 emulator, all grounded in failing behavioral checks. None of them is covered by an auto-filed scaffold-stub issue in this run (the stub issues filed for this suite name GetAutomatedDiscoveryConfiguration, UpdateAutomatedDiscoveryConfiguration, GetClassificationExportConfiguration, CreateClassificationJob, CreateAllowList, CreateSampleFindings, CreateFindingsFilter, GetFindingsPublicationConfiguration, ListMembers and TestCustomDataIdentifier — CreateMember is NOT among them). A fixer must close all three before submitting.

=== Cause 1 of 3: CreateMember is an unimplemented scaffold stub, so the whole member plane is unreachable ===
AWS: Smithy (services/service-com-amazonaws-macie2-macie2/smithy-model/com-amazonaws-macie2-macie2.json) models CreateMember as POST /members, success 200, output {arn}, with modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). Real AWS refuses an attempt to add the CALLER'S OWN account as a member with HTTP 400 ValidationException and the message "The request is rejected because an account cannot become a member of itself." — that whole sentence is FIXED text with no variable part. For a well-formed account id that is not an associated member, GetMember answers HTTP 400 ValidationException "The request is rejected because the given account ID is not an associated member of the current account." (also entirely fixed text). A CreateMember naming a genuinely different account id succeeds with 200 and the member is then readable through GetMember with accountId echoed.
Emulator: every CreateMember call answers HTTP 501 with error code NotImplemented. The handler is the untouched generated scaffold: services/service-com-amazonaws-macie2-macie2/src/ops/create_member.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateMember"))` (line 174), so neither the self-account refusal nor the positive create path exists.
Why: a client cannot build or test a Macie administrator/member topology at all. Code 501/NotImplemented is not a modeled error for this operation, so botocore surfaces it as an unmodeled failure: retry/branching logic that keys on ValidationException vs ConflictException never fires, and a guard rail that depends on AWS refusing self-membership silently never triggers — the client concludes the call is a transport fault rather than a rejected request.
Fix: implement the operation in its own handler, services/service-com-amazonaws-macie2-macie2/src/ops/create_member.rs — validate account.accountId against the caller's own account id (reject with the modeled ValidationException and the fixed sentence above) before mutating state, then record the member in crate::state so GetMember/DeleteMember observe it, and delete the SCAFFOLDED marker line.
Traces: macie2.create-member-self-account-rejected.

=== Cause 2 of 3: CreateCustomDataIdentifier accepts a syntactically invalid regex and creates an identifier ===
AWS: Smithy models CreateCustomDataIdentifier as POST /custom-data-identifiers with required members name and regex and ValidationException(400) among its errors. Recorded behavior: a syntactically invalid pattern (an unclosed character class, e.g. "[A-Z(unclosed") is refused up front with HTTP 400 ValidationException and the message "Unsafe regex, keywords, or ignoreWords." — that message is entirely FIXED text; it does not interpolate the pattern, the name or any request value. The refusal leaves no identifier behind, so the ListCustomDataIdentifiers item count is unchanged across the failed call. (Name uniqueness is NOT enforced by AWS: two creates with distinct clientTokens and the same name must both succeed and mint distinct ids — that part of the check passes only once the regex refusal exists.)
Emulator: the create SUCCEEDS and mints an identifier (the replay observed a 200 returning a fresh customDataIdentifierId). services/service-com-amazonaws-macie2-macie2/src/ops/create_custom_data_identifier.rs validates only documented LENGTH/COUNT bounds in `documented_violation` (lines 309-397) and its comment at lines 206-209 states the regex "is stored as sent and never compiled", so pattern syntax is never checked and no ValidationException is ever raised for it.
Why: an over-permissive accept. A client that relies on AWS rejecting a malformed pattern at create time instead gets a 200 and a persisted identifier that can never match anything; the error surfaces later (or never), and validation-error handling paths in the client are never exercised. Worse, the emulator leaks a resource for a call AWS treats as having no effect.
Fix: the operation's validation in services/service-com-amazonaws-macie2-macie2/src/ops/create_custom_data_identifier.rs — compile/syntax-check the `regex` member as part of the pre-store validation pass and return the modeled ValidationException with the fixed message above, before any state mutation, so the refused call leaves the identifier registry unchanged.
Traces: macie2.custom-data-identifier-invalid-regex-rejected.

=== Cause 3 of 3: GetCustomDataIdentifier's not-found message is emulator-authored, not AWS's wording ===
AWS: Smithy models GetCustomDataIdentifier as GET /custom-data-identifiers/{id} with ResourceNotFoundException(404) among its errors. Recorded behavior: a well-formed-but-never-created id (a UUID) is refused with code ResourceNotFoundException and the message "Custom Data Identifier not found for id: <id>" — the fixed text is "Custom Data Identifier not found for id: " and the only variable part is the id taken verbatim from the request path.
Emulator: the code and status are right, but the message is reworded: "The request failed because the custom data identifier <id> doesn't exist." The string is authored at services/service-com-amazonaws-macie2-macie2/src/ops/get_custom_data_identifier.rs lines 202-212.
Why: the message is part of the contract. Clients, SDK-adjacent tooling and log scrapers that match on AWS's sentence (or surface it to a user) see text no AWS region ever produces, so message-based assertions and support diagnostics diverge even though the error class matches.
Fix: the operation's error construction in services/service-com-amazonaws-macie2-macie2/src/ops/get_custom_data_identifier.rs — build the ResourceNotFoundException message from AWS's template, interpolating only the requested id.
Traces: macie2.get-custom-data-identifier-not-found.

## Evidence
Suite: simparity-behavioral-aws-v1 — macie2 2/22 passed (9%), 20 failed.

Cause 1 — macie2.create-member-self-account-rejected [mismatch]:
  check_macie2_create_member_self_account_rejected.py line 22:
    assert error["Code"] == "ValidationException"
    AssertionError: unexpected code: NotImplemented
  Expected: HTTP 400, Code=ValidationException, Message="The request is rejected because an account cannot become a member of itself."
  Actual:   HTTP 501, Code=NotImplemented (scaffold stub; src/ops/create_member.rs:174 returns Scaffolded::not_implemented("CreateMember")).

Cause 2 — macie2.custom-data-identifier-invalid-regex-rejected [mismatch]:
  check_macie2_custom_data_identifier_invalid_regex_rejected.py line 103:
    AssertionError: expected CreateCustomDataIdentifier to reject the invalid regex, but it SUCCEEDED and created 'e29kfstryf317yahkb9xhd'
  Request: create_custom_data_identifier(name=<unique>, regex="[A-Z(unclosed", clientToken=<unique>)
  Expected: HTTP 400, Code=ValidationException, Message="Unsafe regex, keywords, or ignoreWords.", and no new identifier in ListCustomDataIdentifiers.
  Actual:   HTTP 200 with a minted customDataIdentifierId (the id value itself is per-run and not part of the requirement).

Cause 3 — macie2.get-custom-data-identifier-not-found [mismatch]:
  check_macie2_get_custom_data_identifier_not_found.py line 27:
    AssertionError: unexpected message: "The request failed because the custom data identifier 417f5a0c-7101-4ff9-a58f-7c11f6cba020 doesn't exist."
  Expected message: "Custom Data Identifier not found for id: <the requested uuid>" (fixed prefix + the request's id)
  Actual message:   "The request failed because the custom data identifier <the requested uuid> doesn't exist."
  Code/status (404 ResourceNotFoundException) match; only the wording diverges. The uuid shown is per-run.

## Steps to Reproduce
Boot the emulator and point a botocore macie2 client at it (the account must have a Macie session: call get_macie_session, and enable_macie if it answers ResourceNotFoundException/AccessDeniedException).

Cause 1:
  own = sts.get_caller_identity()["Account"]
  macie.create_member(account={"accountId": own, "email": "self@example.com"})
  -> observe HTTP 501 NotImplemented; expected HTTP 400 ValidationException "The request is rejected because an account cannot become a member of itself."
  Then macie.create_member(account={"accountId": "<a different, well-formed account id>", "email": "x@example.com"}) -> also 501; expected 200 with arn, and GetMember then echoing that accountId.

Cause 2:
  before = len(list_custom_data_identifiers items)
  macie.create_custom_data_identifier(name="any", regex="[A-Z(unclosed", clientToken="tok-1")
  -> observe HTTP 200 with a customDataIdentifierId and the item count incremented; expected HTTP 400 ValidationException "Unsafe regex, keywords, or ignoreWords." with the count unchanged.

Cause 3:
  macie.get_custom_data_identifier(id=str(uuid.uuid4()))
  -> observe 404 ResourceNotFoundException with message "The request failed because the custom data identifier <id> doesn't exist."; expected "Custom Data Identifier not found for id: <id>".
