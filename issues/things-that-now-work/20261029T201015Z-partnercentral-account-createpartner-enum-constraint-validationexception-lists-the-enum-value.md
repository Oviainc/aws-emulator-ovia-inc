Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-account-partner-onboarding-attempt
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-account-partner-onboarding-attempt
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|CreatePartner|4xx->4xx|$.Message
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# CreatePartner enum-constraint ValidationException lists the enum value set in Smithy declaration order, not AWS's

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartner

## Description
Cause 1 of 1: when CreatePartner is called with an out-of-set value for the enum-constrained member PrimarySolutionType (together with an over-long ClientToken), the emulator produces the right HTTP status, the right error code and the right overall multi-error message template, but renders the enum value set in the order the Smithy model declares the enum's members instead of the order AWS reports it in. Only the bracketed value-set clause of the message diverges.

AWS: CreatePartner's input member PrimarySolutionType is required and targets com.amazonaws.partnercentralaccount#PrimarySolutionType, an enum whose modeled members are SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES; ClientToken has @length min 1 max 64 (Smithy: CreatePartner input members PrimarySolutionType -> PrimarySolutionType (enum), ClientToken -> ClientToken @length(1,64); error ValidationException, errorKind client, httpError 400). The model fixes the code and status but says nothing about the message text, so the message is recorded behavior only. The recording shows AWS answering HTTP 400 with code ValidationException and a message that aggregates both constraint failures in request-member order:

    2 validation errors detected: Value '<clientToken>' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value '<primarySolutionType>' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]

Template breakdown. Fixed text: "<n> validation errors detected: ", "Value '", "' at '", "' failed to satisfy constraint: Member must have length less than or equal to ", "; ", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable from the request: the rejected values (the client token string, the solution-type string) and the member names as AWS reports them for this operation, lower-camel (clientToken, primarySolutionType). Variable from the model: the length limit (64) and the comma-separated enum value set. For THIS message class AWS does echo the rejected value ("Value '<v>' at '<member>'"), and the emulator already matches that part; the divergence is purely the ORDER of the enum value set. The recorded order for PrimarySolutionType is TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES. Hypothesis: AWS's validation framework enumerates the enum's permitted values out of a hash-ordered set, so the order is stable per enum but unrelated to declaration order; only the recorded order is a fact.

Emulator: the same call returns the correct HTTP 400 and the correct code ValidationException, the correct error count ("2 validation errors detected:"), the correct clientToken clause and the correct enum-clause prefix, but emits the value set as [SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES] — byte-for-byte the Smithy member declaration order of PrimarySolutionType, i.e. the formatter is iterating the modeled enum in model order.

Why: a client that relies on AWS's validation text gets the error wrong. Error-classification middleware, SDK wrappers that branch on Error.Message, UI surfacing of the message, and contract tests pinned to recorded AWS strings all compare the message or its value-set clause verbatim; the emulator's string never occurs on AWS, so an exact-match consumer fails to classify a refusal it handles correctly against the real service. Callers that parse the bracketed set to offer the user a list of permitted values also receive them in an order no AWS response produces, which makes recorded-vs-replayed diffs noisy for every enum-constrained member of this service.

Fix: the shared request-validation layer that formats constraint failures into ValidationException.message for this crate — the framework-driven deserialize/validate step that runs before the operation handler, not the CreatePartner handler itself (that handler is never reached for this input). The enum-constraint message builder must render the permitted value set in AWS's recorded order for the enum being violated rather than iterating the Smithy enum's members in declaration order. Fixing it in that one formatter keeps every enum-constrained member of the service (PrimarySolutionType and the other enums) worded consistently.

Related but NOT covering this: open issue 20261029T174704Z-partnercentral-account-listconnectioninvitations-enum-constraint-validationexception-message-echoes-the-rejected.md reports the same formatter for ListConnectionInvitations$Status from the behavioral corpus (where AWS additionally does NOT echo the rejected value). It carries no replay key for this wire fixture step and does not mention PrimarySolutionType, so this divergence is filed separately; a fixer should treat both as the same formatter and note that whether the rejected value is echoed differs per member/message class and must not be unified blindly.

Traces: fixture partnercentral-account-partner-onboarding-attempt, step task#3 (CreatePartner).

## Evidence
Fixture: partnercentral-account-partner-onboarding-attempt, step task#3, operation CreatePartner, service partnercentral-account. Verdict: mismatch. Expected HTTP 400, actual HTTP 400 — status and error code agree; the diverging key is $.Message (value_mismatch).

Request: CreatePartner with a ClientToken longer than the modeled 64-character maximum and PrimarySolutionType = "NOT_A_SOLUTION_TYPE".

Expected (AWS, recorded):
  "2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value 'NOT_A_SOLUTION_TYPE' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]"

Actual (emulator):
  "2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value 'NOT_A_SOLUTION_TYPE' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES]"

The two strings are identical up to and including "Member must satisfy enum value set: ["; only the ordering of the eight enum values inside the brackets differs. The specific client-token string above is this recording's value filling the template's value slot and is not itself a requirement; the fixed text, the member names (clientToken, primarySolutionType), the limit 64 and the enum value set with AWS's ordering are.

Smithy (services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/smithy-model/com-amazonaws-partnercentralaccount-partnercentralaccount.json):
  CreatePartner input: PrimarySolutionType required -> enum PrimarySolutionType; ClientToken @length(min 1, max 64) @pattern ^[A-Za-z0-9-_]+$; errors include ValidationException (client, httpError 400).
  PrimarySolutionType enum values, in declaration order: SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES — byte-for-byte the emulator's emitted order, which is the evidence that the formatter iterates model order.
  ValidationException members: Message (required), Reason (required enum), ErrorDetails.

## Steps to Reproduce
1. Boot the emulator and point a client at its data listener for service partnercentral-account in us-east-1.
2. Replay fixture partnercentral-account-partner-onboarding-attempt and let it reach step task#3, or call CreatePartner directly with Catalog="AWS", a ClientToken longer than 64 characters, PrimarySolutionType="NOT_A_SOLUTION_TYPE", plus the required LegalName, AllianceLeadContact and EmailVerificationCode.
3. Read the error response: HTTP status, Error.Code and Error.Message.
4. Observe status 400 and code ValidationException (both correct) but Message ending in "Member must satisfy enum value set: [SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES]" where AWS answers "Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]".
5. After fixing the enum-constraint message formatter, re-replay partnercentral-account-partner-onboarding-attempt step task#3 and confirm $.Message matches byte-for-byte; also confirm the clientToken length clause and the "2 validation errors detected" prefix are unchanged.
