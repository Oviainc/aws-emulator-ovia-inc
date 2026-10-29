Issue-Source: trace-sweep-divergence
Issue-Trace: security-ir-create-case-validation-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: security-ir-create-case-validation-error
Issue-Shape-Id: com.amazonaws.securityir#SecurityIncidentResponse
Issue-Key: security-ir|CreateCase|4xx->4xx|$.message
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# security-ir ValidationException message is not the service's JSON-schema validation report

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCase

## Description
Cause 1 of 1: CreateCase refuses an out-of-enum `engagementType` with the right HTTP status and error shape but with message text the service never produces.

AWS: `CreateCase` (POST /v1/create-case, Smithy: securityir#CreateCase) models `engagementType` as required and targeting the enum `com.amazonaws.securityir#EngagementType`, whose only values are `Security Incident` and `Investigation`; an out-of-enum value is refused with the modeled `ValidationException` (errorKind client, httpError 400). The Smithy model does not state the message text, so the message is recorded behavior only: Security IR reports request-validation failures as a JSON-schema style report rather than the usual AWS constraint sentence. The message is a bracketed, comma-separated list of validation findings: its variable finding, for an out-of-enum member, is `instance value ("<submitted value>") not found in enum (possible values: [<the member's enum values, JSON-quoted and comma-separated>])`, and it is followed by this service's fixed trailing findings `format attribute "password" not supported, format attribute "double" not supported`. Fixed text in that template: the literal words `instance value (`, `) not found in enum (possible values: [`, `])`, the two `format attribute ... not supported` findings, and the surrounding `[` `]`. Variable parts: the rejected value, taken from the request, and the enum value list, taken from the model for the member being validated. The recorded refusal carries HTTP 400.

Emulator: answers HTTP 400 as well, but with the generic AWS constraint-violation sentence instead: `1 validation error detected: Value 'BadType' at 'engagementType' failed to satisfy constraint: Member must satisfy enum value set: [Security Incident, Investigation]`. The divergence is confined to `$.message` (status and error shape agree).

Why: a client that keys off the refusal text — a console surface, an SDK wrapper or a test that asserts on the validation report — reads a message in a format Security IR never emits. In particular the recorded form is a machine-readable list of findings (callers split on it to attribute a failure to a member), while the emulator's single sentence has no list structure, so any parsing of the report fails and the per-member attribution is lost.

Fix: the security-ir crate's request-validation / error-serialization path for CreateCase — the component that builds the `ValidationException` message. It is currently emitting the shared AWS-wide "N validation error(s) detected" formatter; security-ir needs its own JSON-schema-report formatter that renders one finding per failed member plus this service's fixed trailing `format attribute` findings.

Traces: fixture `security-ir-create-case-validation-error`, step `task#1` (CreateCase with engagementType=`BadType`).

## Evidence
Fixture security-ir-create-case-validation-error, step task#1, operation CreateCase.
Expected status 400, actual status 400 — divergence key `service=security-ir|CreateCase|status=400->400|keys=$.message`.
Expected $.message (recorded): `[instance value ("BadType") not found in enum (possible values: ["Security Incident","Investigation"]), format attribute "password" not supported, format attribute "double" not supported]`
Actual $.message (emulator): `1 validation error detected: Value 'BadType' at 'engagementType' failed to satisfy constraint: Member must satisfy enum value set: [Security Incident, Investigation]`
Smithy: CreateCase.engagementType required, target EngagementType enum values ["Security Incident", "Investigation"]; error ValidationException (client, httpError 400, required members message + reason).

## Steps to Reproduce
1. Boot the emulator and replay fixture `security-ir-create-case-validation-error`, step `task#1`.
2. Equivalently: POST /v1/create-case against the security-ir endpoint with a well-formed CreateCase body (resolverType, title, description, reportedIncidentStartDate, impactedAccounts, watchers all present) but `engagementType` set to a value outside the EngagementType enum, e.g. `BadType`.
3. Observe HTTP 400 ValidationException. The `message` member is the generic `1 validation error detected: Value 'BadType' at 'engagementType' failed to satisfy constraint: Member must satisfy enum value set: [...]` sentence instead of the bracketed JSON-schema finding list `[instance value ("BadType") not found in enum (possible values: ["Security Incident","Investigation"]), format attribute "password" not supported, format attribute "double" not supported]`.
