Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.get-admin-account-unconfigured-not-found fms.batch-disassociate-resource-without-admin-rejected fms.get-apps-list-nonexistent-not-found fms.list-admin-accounts-for-organization-unconfigured-not-found fms.put-apps-list-without-admin-rejected fms.put-protocols-list-without-admin-rejected fms.put-resource-set-invalid-resource-type-rejected fms.get-admin-scope-malformed-account-rejected
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.get-admin-account-unconfigured-not-found|run#4|AssertionError|assert message == "", f"unexpected ResourceNotFoundException message: {message!r}"
Issue-Key: fms.batch-disassociate-resource-without-admin-rejected|run#6|AssertionError|assert admin_message == "", f"unexpected ResourceNotFoundException message: {admin_message!r}"
Issue-Key: fms.get-apps-list-nonexistent-not-found|run#4|AssertionError|assert admin_message == "", f"unexpected ResourceNotFoundException message: {admin_message!r}"
Issue-Key: fms.list-admin-accounts-for-organization-unconfigured-not-found|run#4|AssertionError|assert admin_account_message == "", f"unexpected ResourceNotFoundException message: {admin_account_message!r}"
Issue-Key: fms.put-apps-list-without-admin-rejected|run#4|AssertionError|assert admin_message == "", f"unexpected ResourceNotFoundException message: {admin_message!r}"
Issue-Key: fms.put-protocols-list-without-admin-rejected|run#4|AssertionError|assert health_message == "", f"unexpected ResourceNotFoundException message: {health_message!r}"
Issue-Key: fms.put-resource-set-invalid-resource-type-rejected|run#4|AssertionError|assert health_message == "", (
Issue-Key: fms|DescribeOrganization|AWSOrganizationsNotInUseException
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# FMS: GetAdminAccount's unconfigured ResourceNotFoundException carries the service-wide not-found text instead of an empty message, and the emulated account is in no AWS Organization so DescribeOrganization refuses

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdminAccount

## Description
Two distinct root causes, both grounded in failing fms behavioral checks. A fixer must close BOTH before submitting.

=== Cause 1 of 2: GetAdminAccount's "no administrator associated" ResourceNotFoundException must carry an EMPTY message ===

AWS: Smithy (services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json) models GetAdminAccount with errors InternalErrorException | InvalidOperationException | ResourceNotFoundException (all HTTP 400, awsJson1_1); com.amazonaws.fms#ResourceNotFoundException declares exactly one member, Message (target ErrorMessage), with no literal text. For the specific case where the caller's account has never associated a Firewall Manager administrator, recorded behavior is: HTTP 400, error code "ResourceNotFoundException", and NO message content — botocore's response["Error"].get("Message", "") is the empty string. Seven independent checks in this suite assert that empty message as the discriminator for the "FMS onboarding never completed" state (each uses GetAdminAccount as its positive control, precisely because this error is distinguishable from other refusals). Fixed text for this error class: none at all; there are no variable parts, since the request carries no lookup key to restate.

Emulator: GetAdminAccount returns ResourceNotFoundException with Message = "The referenced item does not exist." (the exact string the assertions report). The handler takes the service-wide not-found constructor for a lookup that has no lookup key, so the keyed-lookup text leaks onto the keyless administrator-slot miss.

Why: the error code alone no longer tells a client apart from AWS, because the payload differs — an error body that AWS sends with no Message arrives with a sentence in it. Clients (and parity harnesses) that key the "no FM administrator configured" state off the exact error payload of GetAdminAccount mis-classify the account's FMS onboarding state, and every downstream decision built on that control (is FMS reachable? is the refusal admin-association or a blanket denial?) is taken on the wrong evidence.

Fix: the GetAdminAccount handler's not-found site must not borrow the service-wide keyed-lookup not-found message; the administrator-slot miss answers ResourceNotFoundException with no Message. Belongs in that operation's handler (and, if the text stays centralised, in a distinct keyless not-found constructor in the service state module next to the existing shared one). Keep the existing constant for the keyed lookups whose recordings justify it.

Traces: services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_admin_account.rs:167-172 (the not-found site) and src/state.rs:2436-2449 (RESOURCE_NOT_FOUND_MESSAGE / resource_not_found()). Failing checks: fms.get-admin-account-unconfigured-not-found, fms.batch-disassociate-resource-without-admin-rejected, fms.get-apps-list-nonexistent-not-found, fms.list-admin-accounts-for-organization-unconfigured-not-found, fms.put-apps-list-without-admin-rejected, fms.put-protocols-list-without-admin-rejected, fms.put-resource-set-invalid-resource-type-rejected. NOTE: six of those seven abort on this assertion BEFORE exercising their real subject (the AccessDeniedException / InvalidOperationException admin gate on BatchDisassociateResource, GetAppsList, ListAdminAccountsForOrganization, PutAppsList/ListAppsLists, PutProtocolsList/ListProtocolsLists, PutResourceSet/ListResourceSets), so those legs are still unverified and may surface further divergences once this cause is fixed.

=== Cause 2 of 2: the emulated account belongs to no AWS Organization, so DescribeOrganization refuses and every organization-gated FMS answer is unreachable ===

AWS: recorded behavior only for the Organizations membership — in the environment these checks were recorded against, organizations DescribeOrganization SUCCEEDS and returns Organization.MasterAccountId, which the checks then use to assert the FMS organization gate: GetAdminScope with a well-formed but unregistered 12-digit account, and ListAdminAccountsForOrganization from a non-management account, both answer InvalidOperationException (HTTP 400; com.amazonaws.fms#InvalidOperationException, modeled on both operations) with the message whose FIXED text is "<caller account> is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is <management account id>" — the two variable parts are the calling account id (from the request's credentials) and the organization's management account id (from Organizations state); neither is part of the template.

Emulator: the Organizations store starts with no organization and nothing in the emulator ever creates one, so DescribeOrganization answers AWSOrganizationsNotInUseException, HTTP 400, "Your account is not a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization." The check errors out there; the FMS GetAdminScope organization-gate leg is never reached (its ValidationException leg for a non-numeric AdminAccount did pass).

Why: a client cannot discover the management account of the emulated account at all, so no organization-scoped behavior is emulable: every FMS operation whose refusal names the management account, and more generally any caller that resolves its organization before calling an org-gated API, gets a hard "not a member of an organization" instead of the organization AWS reports.

Fix: the Organizations emulator must present the emulated account as a member of an organization by default — the organization slot in its service state, populated with a management account id — so DescribeOrganization reports it; the FMS side then needs that id to word its InvalidOperationException. Open issue 20261024T045911Z-billingconductor-createbillinggroup-event-propagation.md observes the same "Organizations CreateOrganization not observed" gap from the billing-payer side and does NOT cover DescribeOrganization for these FMS checks.

Traces: services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/describe_organization.rs:135-143 (the refusal when state.organization is None). Failing check: fms.get-admin-scope-malformed-account-rejected (check id's leading service token is fms; the diverging response is served by the Organizations emulator the check calls).

## Evidence
Cause 1 — expected vs actual, all seven checks:
  expected: ResourceNotFoundException with Message "" (no message content)
  actual:   ResourceNotFoundException with Message "The referenced item does not exist."
  e.g. fms.get-admin-account-unconfigured-not-found:
    AssertionError: unexpected ResourceNotFoundException message: 'The referenced item does not exist.'
  and fms.put-resource-set-invalid-resource-type-rejected:
    AssertionError: expected the exact ResourceNotFoundException message AWS returns, got 'The referenced item does not exist.'
  Error code and HTTP status match (ResourceNotFoundException / 400); only the message body diverges.
  Source of the emulator's text: src/state.rs RESOURCE_NOT_FOUND_MESSAGE = "The referenced item does not exist.", returned by resource_not_found(), used by src/ops/get_admin_account.rs for the empty administrator slot.

Cause 2 — expected vs actual:
  expected: organizations DescribeOrganization -> 200 with Organization.MasterAccountId, after which fms GetAdminScope(AdminAccount="999999999999") -> InvalidOperationException "<caller> is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is <MasterAccountId>"
  actual:   organizations DescribeOrganization -> AWSOrganizationsNotInUseException: "An error occurred (AWSOrganizationsNotInUseException) when calling the DescribeOrganization operation: Your account is not a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization." — the check raises there, so the FMS InvalidOperationException leg is never observed.

## Steps to Reproduce
Cause 1:
1. Boot the emulator with a fresh store (no AssociateAdminAccount / PutAdminAccount call).
2. Call fms GetAdminAccount with no parameters.
3. Observe HTTP 400 ResourceNotFoundException whose Message is "The referenced item does not exist."; AWS sends this error with no message content.
4. Equivalently, replay fms.get-admin-account-unconfigured-not-found (and the six sibling checks listed in trace_ids), which use GetAdminAccount as their positive control.

Cause 2:
1. Boot the emulator with a fresh store.
2. Call organizations DescribeOrganization.
3. Observe HTTP 400 AWSOrganizationsNotInUseException instead of an Organization carrying MasterAccountId.
4. Replay fms.get-admin-scope-malformed-account-rejected: its first leg (GetAdminScope with AdminAccount="not-an-account-id" -> ValidationException) passes, then the DescribeOrganization lookup aborts the check before the organization-gate assertion on GetAdminScope(AdminAccount="999999999999").
