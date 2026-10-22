Issue-Source: trace-sweep-divergence
Issue-Trace: m2-application-not-found-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: m2-application-not-found-errors m2-environment-not-found-errors m2-engine-discovery-and-blu-insights
Issue-Shape-Id: com.amazonaws.m2#AwsSupernovaControlPlaneService
Issue-Key: m2|ListApplications|4xx->4xx|$.message
Issue-Key: m2|ListEnvironments|4xx->4xx|$.message
Issue-Key: m2|ListEngineVersions|4xx->4xx|$.message
Issue-Key: m2|GetApplication|4xx->4xx|$.message
Issue-Key: m2|GetApplicationVersion|4xx->4xx|$.message
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# m2 error messages are emulator-authored instead of the service's own texts (validation value rendering, enum set order, not-found prose)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplications

## Description
Five causes in which the m2 emulator produces the right status and error code but builds the `message` member by its own rules rather than the service's. Each message below has a derivable rule (built from the request, or from the model's own value set, or the service's fixed per-operation literal shown by the recording), so the rule — not the recorded sample text — is the requirement. All five must be closed before this issue is submitted.

Cause 1 of 5: ListApplications over-long `names` — the offending value is not rendered.
AWS: ListApplications (Smithy: GET /applications, names @httpQuery targets EntityNameList with @length min 1 max 10; ValidationException @httpError 400). The recorded 400 message is built from the request and renders the rejected value itself: "1 validation error detected: Value '[probe-name-0, probe-name-1, ..., probe-name-10]' at 'names' failed to satisfy constraint: Member must have length less than or equal to 10" — i.e. the list is rendered as a bracketed, comma-separated list of its members.
Emulator: same status/code, but the value is summarised as a count: "Value '11 items' at 'names' ...".
Why: the message is the only place the rejected input is reported back (ValidationException's fieldList is absent), so a client or test asserting which names were refused cannot recover them; the emulator's rendering is not a format AWS ever emits for a list member.
Fix: the m2 validation-message formatter (the shared constraint-violation serializer used by the query-parameter validators) — render a list member's value as its bracketed member list, not as an '<n> items' summary.
Traces: m2-application-not-found-errors#task#5.

Cause 2 of 5: ListEnvironments over-long `names` — same defect on the second operation that binds EntityNameList.
AWS: ListEnvironments (Smithy: GET /environments, names @httpQuery -> EntityNameList @length min 1 max 10, ValidationException 400); the recorded message again renders the full bracketed list of names.
Emulator: '11 items' in place of the value, identical to Cause 1 — confirming the defect is in the shared formatter and not one handler.
Why: as Cause 1; it also means fixing only ListApplications leaves this path wrong.
Fix: same formatter; verify both operations after the change.
Traces: m2-environment-not-found-errors#task#3.

Cause 3 of 5: ListEngineVersions enum violation — the enum value set is listed in the wrong order.
AWS: ListEngineVersions (Smithy: GET /engine-versions, engineType @httpQuery -> EngineType). The recorded 400 renders the permitted set sorted: "... failed to satisfy constraint: Member must satisfy enum value set: [bluage, microfocus]".
Emulator: emits the set in the model's declaration order, "[microfocus, bluage]" (Smithy EngineType values are declared microfocus, bluage — so the emulator is echoing declaration order while the service sorts the set).
Why: the enumeration of permitted values is the actionable part of the message; clients and docs tests that compare the refusal text against the service see a spurious difference, and the emulator's order is not reproducible against AWS.
Fix: the m2 validation-message formatter's enum-set rendering — sort the value set before rendering it.
Traces: m2-engine-discovery-and-blu-insights#task#5.

Cause 4 of 5: GetApplication not-found message is not built from the requested id in the service's form.
AWS: GetApplication (Smithy: GET /applications/{applicationId}, ResourceNotFoundException @error client @httpError 404, required `message`). The recorded 404 message is built from the request: 'app <applicationId> not found' (recording: 'app 00000000000000000000000000 not found').
Emulator: 404 with its own prose 'no application with the id <applicationId>'.
Why: m2's not-found errors carry no machine-readable discriminator beyond the message in this response (resourceId/resourceType are absent), so the message is what a caller logs, matches or asserts on; emulator-authored prose makes those assertions fail against the emulator and pass against AWS (or vice versa).
Fix: the m2 GetApplication handler's ResourceNotFoundException construction — emit the service's 'app {applicationId} not found' form.
Traces: m2-application-not-found-errors#task#1.

Cause 5 of 5: GetApplicationVersion reuses the same generic text where the service answers a different, fixed literal.
AWS: GetApplicationVersion (Smithy: GET /applications/{applicationId}/versions/{applicationVersion}, ResourceNotFoundException 404). Recorded behavior only: the message is the fixed literal 'Application not found.' — notably NOT the id-bearing form GetApplication uses, so the two operations have distinct not-found texts.
Emulator: returns the one generic 'no application with the id <applicationId>' for both operations.
Why: the emulator collapses two distinct service behaviours into one string, so no caller can distinguish which lookup failed, and neither text matches the service.
Fix: the m2 GetApplicationVersion handler's ResourceNotFoundException construction — give it its own message rather than sharing the application lookup's text.
Traces: m2-application-not-found-errors#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, bootstrap_signal false, state_reset_failures 0; 8 divergence clusters, all service=m2). In every cause below the HTTP status (and x-amzn-errortype where recorded) matched; only $.message diverged.

Cause 1 — m2-application-not-found-errors#task#5, ListApplications, 400 vs 400:
  expected "1 validation error detected: Value '[probe-name-0, probe-name-1, probe-name-2, probe-name-3, probe-name-4, probe-name-5, probe-name-6, probe-name-7, probe-name-8, probe-name-9, probe-name-10]' at 'names' failed to satisfy constraint: Member must have length less than or equal to 10"
  got      "1 validation error detected: Value '11 items' at 'names' failed to satisfy constraint: Member must have length less than or equal to 10"

Cause 2 — m2-environment-not-found-errors#task#3, ListEnvironments, 400 vs 400: byte-identical expected/actual pair to Cause 1 ('[probe-name-0, ... probe-name-10]' vs '11 items').

Cause 3 — m2-engine-discovery-and-blu-insights#task#5, ListEngineVersions, 400 vs 400:
  expected "... Value 'not-a-real-engine' at 'engineType' failed to satisfy constraint: Member must satisfy enum value set: [bluage, microfocus]"
  got      "... Member must satisfy enum value set: [microfocus, bluage]"
  Model: EngineType values declared [microfocus, bluage] — the emulator renders declaration order, the service renders sorted order.

Cause 4 — m2-application-not-found-errors#task#1, GetApplication, 404 vs 404:
  expected 'app 00000000000000000000000000 not found'
  got      'no application with the id 00000000000000000000000000'

Cause 5 — m2-application-not-found-errors#task#2, GetApplicationVersion, 404 vs 404:
  expected 'Application not found.'
  got      'no application with the id 00000000000000000000000000'  (the same generic text the emulator used for Cause 4)

Model evidence (services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json): EntityNameList @length{min:1,max:10}; ListApplications.names / ListEnvironments.names @httpQuery -> EntityNameList; EngineType enum values [microfocus, bluage]; ValidationException @httpError 400 {message required, reason, fieldList}; ResourceNotFoundException @httpError 404 {message required, resourceId, resourceType}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Causes 1 & 2: GET /applications?names=probe-name-0&...&names=probe-name-10 (11 values) and GET /environments with the same 11 names (fixtures m2-application-not-found-errors#task#5, m2-environment-not-found-errors#task#3). Expect the 400 message to render the bracketed list of the 11 names; observe "Value '11 items'".
3. Cause 3: GET /engine-versions?engineType=not-a-real-engine (fixture m2-engine-discovery-and-blu-insights#task#5). Expect 'enum value set: [bluage, microfocus]'; observe '[microfocus, bluage]'.
4. Cause 4: GET /applications/00000000000000000000000000 (fixture m2-application-not-found-errors#task#1). Expect 404 'app 00000000000000000000000000 not found'; observe 'no application with the id 00000000000000000000000000'.
5. Cause 5: GET /applications/00000000000000000000000000/versions/1 (fixture m2-application-not-found-errors#task#2). Expect 404 'Application not found.'; observe the same generic text as step 4.
