Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases.case-create-and-get-field-values-round-trip connectcases.update-case-field-value-persists connectcases.delete-case-removes-case connectcases.get-case-field-not-on-case-not-found connectcases.related-item-connect-case-link connectcases.case-audit-events-track-create-and-update connectcases.batch-get-field-partial-errors connectcases.concurrent-create-field-same-name-single-winner
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|CreateCase|ValidationException
Issue-Key: connectcases.batch-get-field-partial-errors|run#10|AssertionError|assert resp["errors"][0]["errorCode"] == "ResourceNotFound", (
Issue-Key: connectcases.concurrent-create-field-same-name-single-winner|run#12|AssertionError|assert all(message == expected_conflict for _, message in losers), (
Issue-Commit: ef0850c794465478f23c924504b3a3641538d51f

# connectcases: domains lack reserved System fields, BatchGetField item errorCode wrong, CreateField conflict message template differs (3 causes)

> _Note: this issue was identified against the code base at commit `ef0850c794465478f23c924504b3a3641538d51f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCase

## Description
Three distinct root causes found replaying the connectcases behavioral suite against the emulator. All three are in the connectcases emulator (crate services/service-com-amazonaws-connectcases-amazonconnectcases). 3 causes total; each must be closed.

=== Cause 1 of 3: a newly created Cases domain carries no reserved System-namespace fields, so CreateCase rejects the required `title`/`customer_id` field ids ===
AWS: On real AWS, CreateDomain provisions a fixed set of reserved System-namespace fields in the new domain (Smithy: FieldNamespace enum = ["System", "Custom"]; FieldIdentifier/FieldValue.id target FieldId, a free-form string with @length 1..500 — the model does not enumerate the system ids). Recorded behavior only: every Cases domain carries the reserved field ids verbatim (not generated UUIDs), and CreateCase (Smithy: CreateCase POST /domains/{domainId}/cases, fields: FieldValueList required) accepts `{"id": "title", ...}` and `{"id": "customer_id", ...}` — indeed CreateCase *requires* `title` and `customer_id` independent of the template's own requiredFields — and returns 200 with caseId/caseArn.
Emulator: CreateCase against a freshly created, Active domain rejects those ids with HTTP 400 ValidationException, e.g. message `fields names field ids that do not exist in domain <domainId>: title` and `...: title, customer_id`. Only fields created explicitly via CreateField exist in the emulator's domain schema; the System namespace is never seeded.
Why: no client can create a case at all against an emulated domain, because the two fields AWS always requires are the two the emulator says do not exist. Every case data-plane flow (create/get/update/delete, related items, audit events, GetCase's distinction between an in-schema-but-unset field and an out-of-schema field) is unreachable, so an integration test suite passes on AWS and fails wholesale here.
Fix: domain state initialisation plus CreateCase field-id validation — seed the reserved System-namespace fields (at minimum `title` and `customer_id`) into the domain's field schema when the domain is created, and have CreateCase/GetCase resolve them like any other in-schema field.
Traces: connectcases.case-create-and-get-field-values-round-trip, connectcases.update-case-field-value-persists, connectcases.delete-case-removes-case, connectcases.get-case-field-not-on-case-not-found, connectcases.related-item-connect-case-link, connectcases.case-audit-events-track-create-and-update.

=== Cause 2 of 3: BatchGetField reports a per-item failure with errorCode "ResourceNotFoundException" instead of "ResourceNotFound" ===
AWS: BatchGetField (Smithy: POST /domains/{domainId}/fields-batch, output members `fields`: BatchGetFieldList and `errors`: BatchGetFieldErrorList, both required) partially succeeds: a resolvable field id comes back in `fields` and an unresolvable one in `errors` as a FieldError {id, errorCode (required, target String), message}. The model types errorCode as a free String and does not enumerate it — recorded behavior only: for a field id that does not exist, AWS puts the per-item code `ResourceNotFound` in FieldError.errorCode. The call itself is HTTP 200; the top-level ResourceNotFoundException is not used for a single bad item.
Emulator: returns the correct 200 and the correct partition (one entry in `fields`, one in `errors` with the right `id`), but sets errorCode to `ResourceNotFoundException` — the modeled exception shape name, not the per-item error code.
Why: a client branching on the per-item code (retry vs. drop vs. surface) never matches `ResourceNotFound` and treats a known, documented partial failure as an unknown error code.
Fix: the BatchGetField handler's per-item error mapping/serializer — emit the per-item FieldError.errorCode vocabulary (`ResourceNotFound`) rather than reusing the exception shape's name.
Traces: connectcases.batch-get-field-partial-errors.

=== Cause 3 of 3: CreateField duplicate-name ConflictException message does not follow AWS's template ===
AWS: Racing CreateField calls with the same name in one domain leave exactly one winner; the losers get HTTP 409 ConflictException (Smithy: CreateField errors include ConflictException, httpError 409). Recorded behavior only: the message is a fixed template filled from the request's name — `Field creation failed: field resource with the name <name> already exists!`.
Emulator: the status and error code are correct (ConflictException for every loser, single winner), but the message is a differently-shaped string built from the domain and winning field id, e.g. `a field named <name> already exists in domain <domainId> (fieldId <fieldId>)`.
Why: message-only divergence — lowest severity of this bundle. It breaks clients and test suites that match on AWS's message template (the message is request-derived, so the template is the requirement, not the particular recorded name/ids).
Fix: the CreateField handler's conflict-error construction — render AWS's template with the requested field name.
Traces: connectcases.concurrent-create-field-same-name-single-winner.

## Evidence
Suite: 0/22 connectcases checks passed. 14 failures are already filed as unimplemented-operation scaffolds (PutCaseEventConfiguration, TagResource, ListDomains, BatchPutFieldOptions, CreateLayout, ListFields, ListTemplates) and are excluded here. The 8 remaining failures cluster into the 3 causes above.

Cause 1 — expected: CreateCase(domainId, templateId, fields=[{id: "title", value:{stringValue: ...}}, {id: <custom field>, ...}]) -> HTTP 200 {caseId, caseArn}. Actual: HTTP 400 ValidationException, e.g. `An error occurred (ValidationException) when calling the CreateCase operation: fields names field ids that do not exist in domain ee6d2d5d-e4f3-41e0-b940-6e85109667fc: title` and, when customer_id is also sent, `... do not exist in domain 9532993c-...: title, customer_id`. Domain was created via CreateDomain and polled to domainStatus == "Active" first, so this is not a readiness race.

Cause 2 — expected: resp["errors"][0]["errorCode"] == "ResourceNotFound" for a bogus field id. Actual: 'ResourceNotFoundException'. `fields` and `errors` were otherwise partitioned correctly (1 resolved field with the right fieldId, 1 error entry with the right id), and HTTP status was 200.

Cause 3 — expected every loser's message to be `Field creation failed: field resource with the name <name> already exists!` with code ConflictException. Actual: code ConflictException (correct) with message `a field named <name> already exists in domain <domainId> (fieldId <fieldId>)`, identical across all 7 losers; exactly one CreateField succeeded.

## Steps to Reproduce
1. Clone the repo and boot the emulator; point botocore at the connectcases endpoint.
2. Cause 1: cc.create_domain(name=N) -> poll cc.get_domain until domainStatus == "Active"; cc.create_field(domainId, name, type="Text") -> fieldId; cc.create_template(domainId, name, status="Active") -> templateId; then cc.create_case(domainId=..., templateId=..., fields=[{"id":"title","value":{"stringValue":"t"}},{"id":fieldId,"value":{"stringValue":"v1"}}]). Observe HTTP 400 ValidationException naming `title` as a field id that does not exist in the domain, where AWS returns 200 with caseId/caseArn.
3. Cause 2: in an Active domain with one real field F, call cc.batch_get_field(domainId=..., fields=[{"id": F}, {"id": "sim2real-nonexistent-field-id-0000000000"}]). Observe errors[0]["errorCode"] == "ResourceNotFoundException" instead of "ResourceNotFound".
4. Cause 3: in an Active domain, fire 8 concurrent cc.create_field(domainId=..., name=SAME, type="Text") calls. Observe one success and seven ConflictException losers whose message is `a field named ... already exists in domain ... (fieldId ...)` instead of AWS's `Field creation failed: field resource with the name <name> already exists!`.
5. Re-replay: connectcases.case-create-and-get-field-values-round-trip, connectcases.update-case-field-value-persists, connectcases.delete-case-removes-case, connectcases.get-case-field-not-on-case-not-found, connectcases.related-item-connect-case-link, connectcases.case-audit-events-track-create-and-update, connectcases.batch-get-field-partial-errors, connectcases.concurrent-create-field-same-name-single-winner.
