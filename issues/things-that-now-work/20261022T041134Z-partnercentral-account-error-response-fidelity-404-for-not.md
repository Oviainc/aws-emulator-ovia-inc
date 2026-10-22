Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-account-connection-invitation-create-and-cancel
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-account-connection-invitation-create-and-cancel partnercentral-account-connection-invitation-reject-then-accept-error partnercentral-account-connection-invitation-lifecycle-errors partnercentral-account-connection-management-errors partnercentral-account-partner-onboarding-attempt partnercentral-account-send-email-verification-code-malformed-email-rejected
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|CreateConnectionInvitation|4xx->2xx|$.ErrorDetails
Issue-Key: partnercentral-account|CreateConnectionInvitation|4xx->2xx|$.Message
Issue-Key: partnercentral-account|CreateConnectionInvitation|4xx->2xx|$.Reason
Issue-Key: partnercentral-account|CreateConnectionInvitation|4xx->2xx|$.__type
Issue-Key: partnercentral-account|CreateConnectionInvitation|4xx->2xx|status
Issue-Key: partnercentral-account|ListConnectionInvitations|2xx->2xx|$.ConnectionInvitationSummaries[*]
Issue-Key: partnercentral-account|GetConnectionInvitation|4xx->4xx|$.Message
Issue-Key: partnercentral-account|GetConnectionInvitation|4xx->4xx|status
Issue-Key: partnercentral-account|AcceptConnectionInvitation|4xx->4xx|$.Message
Issue-Key: partnercentral-account|AcceptConnectionInvitation|4xx->4xx|status
Issue-Key: partnercentral-account|GetConnection|4xx->4xx|$.Message
Issue-Key: partnercentral-account|GetConnection|4xx->4xx|status
Issue-Key: partnercentral-account|CancelConnection|4xx->4xx|$.Message
Issue-Key: partnercentral-account|CancelConnection|4xx->4xx|status
Issue-Key: partnercentral-account|CreatePartner|4xx->4xx|$.Message
Issue-Key: partnercentral-account|SendEmailVerificationCode|4xx->4xx|$.Message
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# partnercentral-account error-response fidelity: 404 for not-found, ValidationException with no Message, unvalidated ReceiverIdentifier

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnectionInvitation (also ListConnectionInvitations, GetConnectionInvitation, AcceptConnectionInvitation, GetConnection, CancelConnection, CreatePartner, SendEmailVerificationCode)

## Description
Replay of the simparity-wire-aws-v1 corpus against the PartnerCentralAccount emulator diverged on 9 of 38 steps, clustering into THREE root causes in this one service. All three concern how the service reports (or fails to report) a client error. A fixer must close ALL THREE causes below before submitting; each cause names its own fixture and step.

=== Cause 1 of 4: a lookup of a connection / connection invitation that does not exist is answered HTTP 404, where AWS answers HTTP 400 ===
AWS: recorded behavior only for the status. Smithy models ResourceNotFoundException with @httpError 404 (required members Message + Reason: ResourceNotFoundExceptionReason, values include CONNECTION_INVITATION_NOT_FOUND and CONNECTION_NOT_FOUND), but the recordings show the real PartnerCentralAccount frontend serializing this awsJson1_0 error at HTTP 400 — the recorded __type and Reason agree with the emulator's, only the status (and the message text) differ, across four separate operations and three fixtures. The recorded message is built from the request, so its rule is known: "Connection invitation not found with catalog: <Catalog> and identifier: <Identifier>." and "Connection not found with catalog: <Catalog> and identifier: <Identifier>." (example values from the recordings: catalog Sandbox, identifiers pacinv-0000000000000 / pac-0000000000000).
Emulator: answers 404 and phrases the message the other way round, e.g. "Connection invitation pacinv-0000000000000 was not found in catalog Sandbox." / "Connection pac-0000000000000 was not found in catalog Sandbox." Confirmed in the handler source (see Traces).
Why: an SDK/CLI client keys retry and error-classification behavior off the HTTP status. A client (or proxy, or test harness) that treats 404 as a transport/routing failure, or that distinguishes a modeled 400 client error from a 404, takes the wrong branch against the emulator; message-matching clients also see the wording inverted.
Fix: the status mapping for ResourceNotFoundException in this service's error serializer (the service must serialize this error at 400, not at the model's @httpError 404), plus the message format in each of the four handlers that raise it.
Traces: partnercentral-account-connection-invitation-create-and-cancel#task#3 (GetConnectionInvitation, expected 400, got 404); partnercentral-account-connection-invitation-lifecycle-errors#task#1 (AcceptConnectionInvitation, expected 400, got 404); partnercentral-account-connection-management-errors#task#2 (GetConnection, expected 400, got 404); partnercentral-account-connection-management-errors#task#3 (CancelConnection, expected 400, got 404). Source: services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/get_connection_invitation.rs lines 196-209 construct the ResourceNotFoundException with the diverging message.

=== Cause 2 of 4: ValidationException raised for input-constraint violations carries NO Message member ===
AWS: Smithy: ValidationException @error client, @httpError 400, members Message (required), Reason (required, ValidationExceptionReason = REQUEST_VALIDATION_FAILED | BUSINESS_VALIDATION_FAILED), ErrorDetails. Message is REQUIRED, and the recordings show the standard AWS framework text, built from the request plus the modeled constraints: "<N> validation error(s) detected: Value '<value>' at '<memberName>' failed to satisfy constraint: <constraint>", multiple errors joined with "; ", the member named in lowerCamelCase. Recorded examples: CreatePartner -> "2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value 'NOT_A_SOLUTION_TYPE' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]" (model: CreatePartnerRequest.ClientToken length 1..64, PrimarySolutionType enum); SendEmailVerificationCode -> "1 validation error detected: Value 'not-an-email-address' at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@..." (model: SendEmailVerificationCodeRequest.Email pattern).
Emulator: returns the right status (400) and __type (ValidationException) but the body has NO Message key at all (replay reports "$.Message: missing ... got None"), i.e. the generic deserialize/validate layer emits a Message-less ValidationException even though the model marks Message required.
Why: a client that logs or branches on the error message gets nothing, and an SDK that enforces required members on the error shape can fail to deserialize the error entirely — the caller then sees a parse failure instead of a modeled ValidationException. The per-field diagnostics that tell a user WHICH member was rejected are lost, so a validation-error test cannot distinguish one bad member from another.
Fix: the shared request-validation path that converts constraint violations into ValidationException (populate the required Message in the AWS format above, aggregating all violations of the request, and keep Reason = REQUEST_VALIDATION_FAILED). Note an issue of the same class was filed and resolved for partnercentral-benefits (20261022T012817Z-partnercentral-benefits-validationexception-message-format-missing-partner-entitlement.md) — that is a DIFFERENT service and does not cover this one.
Traces: partnercentral-account-partner-onboarding-attempt#task#3 (CreatePartner); partnercentral-account-send-email-verification-code-malformed-email-rejected#task#1 (SendEmailVerificationCode).

=== Cause 3 of 4: CreateConnectionInvitation accepts a ReceiverIdentifier that AWS rejects for the requested ConnectionType, returning 200 instead of 400 ===
AWS: recorded behavior only. For a CreateConnectionInvitation whose ReceiverIdentifier is not a valid receiver for the requested ConnectionType, AWS answers HTTP 400 with __type com.amazonaws.partnercentralaccount#ValidationException, Message "1 validation error detected.", Reason REQUEST_VALIDATION_FAILED and ErrorDetails [{"FieldValidationError": {"Name": "ReceiverIdentifier", "Code": "INVALID_VALUE", "Message": "Invalid receiver identifier for this connection type."}}]. Model support: ValidationException.ErrorDetails is a list of FieldValidationError (Name/Message/Code required; FieldValidationCode includes INVALID_VALUE), ConnectionType = OPPORTUNITY_COLLABORATION | SUBSIDIARY, and ReceiverIdentifier is a ParticipantIdentifier (len 1..100, ^[a-zA-Z0-9-]+$) documented only as "The identifier of the organization or partner to invite for connection" — the model states no cross-member rule, so the requirement here is the recorded one: the pair (ConnectionType, ReceiverIdentifier) is validated and a receiver that is not addressable for that connection type is a field-level INVALID_VALUE on ReceiverIdentifier. Hypothesis: the receiver identifier form permitted differs per connection type (the rejected request used a partner-prefixed identifier with ConnectionType OPPORTUNITY_COLLABORATION).
Emulator: creates the invitation and answers 200 with a full invitation record. The handler validates only the catalog's ARN-safety, the client token fingerprint and the invitation-count quota; there is no ReceiverIdentifier/ConnectionType check anywhere before the record is minted.
Why: a client exercising the rejection path gets a successful create instead of a ValidationException, so negative-path code (error handling, user-facing field diagnostics on ReceiverIdentifier) is never exercised, and the caller is handed an invitation id that could never exist on AWS. Two independent fixtures depend on this rejection.
Fix: validation in the CreateConnectionInvitation handler, before the record is minted — reject the request with ValidationException (Reason REQUEST_VALIDATION_FAILED, ErrorDetails carrying a FieldValidationError for ReceiverIdentifier with Code INVALID_VALUE) when the receiver is not valid for the requested ConnectionType.
Traces: partnercentral-account-connection-invitation-create-and-cancel#task#1 and partnercentral-account-connection-invitation-reject-then-accept-error#task#1 (same cluster: expected 400 ValidationException, got 200). Source: services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/create_connection_invitation.rs lines 227-345 (handle), where the only validation is catalog ARN-safety at lines 278-296.

=== Cause 4 of 4: the invitation that should have been rejected leaks into ListConnectionInvitations ===
AWS: recorded behavior only. After the CreateConnectionInvitation of cause 3 is rejected, the subsequent ListConnectionInvitations for that catalog returns no ConnectionInvitationSummaries entry — there is nothing to list, because nothing was created.
Emulator: ListConnectionInvitations returns one summary (the record cause 3 wrongly minted), reported as "$.ConnectionInvitationSummaries[0]: extra".
Why: a client enumerating invitations sees a phantom invitation that AWS would never have stored, so list/count assertions and any pagination over that list diverge. This is the state-visible consequence of cause 3 rather than an independent list defect, but it is its own replayed step and must be re-replayed to confirm the fix: once the create is rejected the list must come back empty, so if it still returns a summary the list path filters/stores invitations wrongly too.
Fix: no separate change expected beyond cause 3 (the create must not persist the record); verify ListConnectionInvitations against the store afterwards.
Traces: partnercentral-account-connection-invitation-create-and-cancel#task#2.

Replay context: emulator_up=true, bootstrap_signal=false, state_reset_failures=0 (so no fixture ran against a previous fixture's state), 38 steps replayed, 0 blocked, 0 errored. The service is bootstrapped (service-com-amazonaws-partnercentralaccount-partnercentralaccount is in the resolved set and its bootstrap issue is closed), so these are fidelity defects, not a bootstrap gap.

## Evidence
Summary line: {"trace_set_id": "simparity-wire-aws-v1", "emulator_up": true, "bootstrap_signal": false, "steps_total": 38, "steps_passed": 0, "steps_mismatched": 38, "steps_diverged": 9, "steps_blocked": 0, "steps_errored": 0, "state_reset_failures": 0, "divergence_count": 8}

Cause 1 (expected 400, got 404; message rule inverted) — 4 clusters:
- GetConnectionInvitation, fixture partnercentral-account-connection-invitation-create-and-cancel step task#3: "status: expected 400, got 404" / "$.Message: value_mismatch (expected 'Connection invitation not found with catalog: Sandbox and identifier: pacinv-0000000000000.', got 'Connection invitation pacinv-0000000000000 was not found in catalog Sandbox.')"
- AcceptConnectionInvitation, fixture partnercentral-account-connection-invitation-lifecycle-errors step task#1: same expected/actual pair (expected 400, got 404).
- GetConnection, fixture partnercentral-account-connection-management-errors step task#2: "status: expected 400, got 404" / "$.Message: value_mismatch (expected 'Connection not found with catalog: Sandbox and identifier: pac-0000000000000.', got 'Connection pac-0000000000000 was not found in catalog Sandbox.')"
- CancelConnection, fixture partnercentral-account-connection-management-errors step task#3: same expected/actual pair (expected 400, got 404).
In every one of these clusters only `status` and `$.Message` diverged — `__type` and `Reason` matched, so the error identity is right and the STATUS mapping is wrong.

Cause 2 (ValidationException without Message) — 2 clusters:
- CreatePartner, fixture partnercentral-account-partner-onboarding-attempt step task#3: status 400 == 400, but "$.Message: missing (expected \"2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value 'NOT_A_SOLUTION_TYPE' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]\", got None)"
- SendEmailVerificationCode, fixture partnercentral-account-send-email-verification-code-malformed-email-rejected step task#1: status 400 == 400, but "$.Message: missing (expected \"1 validation error detected: Value 'not-an-email-address' at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\", got None)"
Model (Smithy): ValidationException.Message is REQUIRED, @httpError 400.

Cause 3 (missing ReceiverIdentifier/ConnectionType validation) — 1 cluster, 2 affected steps:
- CreateConnectionInvitation, fixture partnercentral-account-connection-invitation-create-and-cancel step task#1 (also affects partnercentral-account-connection-invitation-reject-then-accept-error#task#1): "status: expected 400, got 200"; "$.__type: missing (expected 'com.amazonaws.partnercentralaccount#ValidationException', got None)"; "$.ErrorDetails: missing (expected [{'FieldValidationError': {'Code': 'INVALID_VALUE', 'Message': 'Invalid receiver identifier for this connection type.', 'Name': 'ReceiverIdentifier'}}], got None)"; "$.Message: missing (expected '1 validation error detected.', got None)"; "$.Reason: missing (expected 'REQUEST_VALIDATION_FAILED', got None)".

Cause 4 (phantom invitation in the list) — 1 cluster:
- ListConnectionInvitations, fixture partnercentral-account-connection-invitation-create-and-cancel step task#2: "$.ConnectionInvitationSummaries[0]: extra (expected None, got {'Catalog': 'Sandbox', 'Id': 'pacinv-0000000000001', 'ConnectionType': 'OPPORTUNITY_COLLABORATION', 'OtherParticipantIdentifier': 'partner-0000000000001', 'ParticipantType': 'SENDER', 'Status': 'PENDING', ...})" — the generated id/arn/timestamps in that record are environment-specific and are NOT requirements; the requirement is that the list is empty because the create was rejected.

No cluster in this service carried a `stub_path` or an `owned_by`, so none of these is already filed.

## Steps to Reproduce
Prerequisite: the PartnerCentralAccount emulator built from this checkout, serving the awsJson1_0 endpoint (replay reports /__admin/health 200 and the data listener accepting).

1. Replay the whole corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1. 9 of 38 steps diverge, in 8 clusters, ALL on service partnercentral-account.

2. Cause 1 — not-found status/message. Replay fixture partnercentral-account-connection-invitation-create-and-cancel step task#3: call GetConnectionInvitation with Catalog="Sandbox", Identifier="pacinv-0000000000000" (an id no fixture created). Expect HTTP 400 with Message "Connection invitation not found with catalog: Sandbox and identifier: pacinv-0000000000000."; observe HTTP 404 with "Connection invitation pacinv-0000000000000 was not found in catalog Sandbox.". Repeat for AcceptConnectionInvitation (partnercentral-account-connection-invitation-lifecycle-errors#task#1) and, with Identifier="pac-0000000000000", for GetConnection and CancelConnection (partnercentral-account-connection-management-errors#task#2 and #task#3).

3. Cause 2 — Message-less ValidationException. Replay partnercentral-account-send-email-verification-code-malformed-email-rejected#task#1: call SendEmailVerificationCode with Catalog="Sandbox" and Email="not-an-email-address" (violates the modeled Email pattern). The response is 400 ValidationException but the JSON body has no "Message" key. Likewise replay partnercentral-account-partner-onboarding-attempt#task#3: CreatePartner with a ClientToken longer than 64 chars AND PrimarySolutionType="NOT_A_SOLUTION_TYPE" — expect a 400 whose Message aggregates BOTH violations; observe a 400 with no Message.

4. Causes 3 and 4 — unvalidated receiver, then the leak. Replay partnercentral-account-connection-invitation-create-and-cancel from step task#1: CreateConnectionInvitation with Catalog="Sandbox", ConnectionType="OPPORTUNITY_COLLABORATION" and the fixture's ReceiverIdentifier (a partner-prefixed identifier, e.g. partner-0000000000001). Expect HTTP 400 ValidationException with Reason REQUEST_VALIDATION_FAILED and ErrorDetails naming ReceiverIdentifier with Code INVALID_VALUE; observe HTTP 200 and a created invitation. Then run step task#2, ListConnectionInvitations for Catalog="Sandbox": expect no ConnectionInvitationSummaries entries, observe the invitation minted by step task#1. The same create cluster also covers partnercentral-account-connection-invitation-reject-then-accept-error#task#1.

After fixing, re-replay all four of those fixtures: partnercentral-account-connection-invitation-create-and-cancel (task#1, task#2, task#3), partnercentral-account-connection-invitation-lifecycle-errors (task#1), partnercentral-account-connection-invitation-reject-then-accept-error (task#1), partnercentral-account-connection-management-errors (task#2, task#3), partnercentral-account-partner-onboarding-attempt (task#3) and partnercentral-account-send-email-verification-code-malformed-email-rejected (task#1).
