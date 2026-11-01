Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases.get-case-field-not-on-case-not-found connectcases.search-all-related-items-across-cases connectcases.layout-create-update-content-round-trip connectcases.required-field-missing-on-case-create-rejected connectcases.related-item-type-content-mismatch-rejected
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases.get-case-field-not-on-case-not-found|run#8|AssertionError|assert got_unset["fields"] == [], (
Issue-Key: connectcases.search-all-related-items-across-cases|_title_field_id#5|AssertionError|raise AssertionError(f"domain {domain_id!r} has no system field named 'Title'")
Issue-Key: connectcases.layout-create-update-content-round-trip|run#25|AssertionError|assert update_error.get("Message") == "Requested resource not found.", (
Issue-Key: connectcases.required-field-missing-on-case-create-rejected|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: connectcases.related-item-type-content-mismatch-rejected|run#7|AssertionError|assert error.get("Code") == "ValidationException", f"expected ValidationException, got {error.get('Code')!r}"
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# connectcases: five response/refusal fidelity defects (GetCase unset fields, System field names, UpdateLayout + CreateCase error messages, CreateRelatedItem type/content validation)

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCase, ListFields, UpdateLayout, CreateCase, CreateRelatedItem

## Description
Five distinct net-new root causes found by replaying the behavioral suite against the connectcases emulator (5/22 checks passed). All five are in one service and are bundled here; each is numbered and carries its own failing check ids. A fixer must close ALL FIVE before submitting.

=== Cause 1 of 5: GetCase synthesizes `emptyValue` for a field that is in the domain schema but was never set on the case (failing check: connectcases.get-case-field-not-on-case-not-found) ===
AWS: recorded behavior only. Smithy: GetCase is @readonly, `GetCase.fields` (FieldIdentifierList, @length 1..220) is required and `GetCaseResponse.fields` (FieldValueList) is required; `FieldValueUnion.emptyValue` is documented as "An empty value" — a value a caller WRITES, not a placeholder the service invents. Real AWS answers 200 and simply OMITS a requested field id that the case holds no value for, so a request naming exactly one in-schema-but-unset field comes back with `fields` empty. (This is the distinct half of a two-case rule: an id the domain's schema does not define at all is refused with ValidationException before the case is read.)
Emulator: returns one entry per requested id, filling an unset one in with `{'emptyValue': {}}` — observed `fields=[{'id': '<unset field id>', 'value': {'emptyValue': {}}}]` where AWS returns `fields=[]`.
Why: a client cannot distinguish "this case never had a value for that field" from "that field was explicitly cleared"; any client that treats presence in `fields` as "this case carries data for that field" (rendering, diffing, export, migration) sees phantom values on every unset field it asks for.
Fix: the GetCase handler's response projection — only requested ids the case actually holds a value for belong in `fields` (services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/get_case.rs, the mapping at lines 221-235).
Same cause, same handler, downstream of the failing assertion so the replay did not reach it: the unknown-field-id refusal message is also wrong. AWS rejects an out-of-schema field id with ValidationException (HTTP 400) and message `Unknown field Ids: <ids>` — fixed text "Unknown field Ids: " followed by the unknown ids taken verbatim from the request. The emulator emits `fields names field ids that do not exist in domain <domainId>: <ids>` (get_case.rs lines 209-219). Fix both or the check fails again on its next assertion.

=== Cause 2 of 5: reserved System fields are named by their id, so no domain has a System field named `Title` (failing check: connectcases.search-all-related-items-across-cases) ===
AWS: recorded behavior only. Smithy: `FieldSummary.name` (FieldName, @length 1..100, pattern `^.*[\S]$`) is a REQUIRED member distinct from `FieldSummary.fieldId`, documented as "Name of the field". Every Cases domain is provisioned with System-namespace fields at creation; the case-title field is addressed by fieldId `title` and ListFields reports its `name` as the display name `Title` (namespace `System`).
Emulator: `reserved_system_fields()` sets each reserved field's `name` to its reserved id, so ListFields returns `{fieldId: 'title', name: 'title', namespace: 'System'}` and a client scanning for `namespace == 'System' and name == 'Title'` finds nothing (replay: "domain '<domain id>' has no system field named 'Title'"). Evidence that it is the NAME and not a missing field: the sibling check connectcases.required-field-missing-on-case-create-rejected resolves the same field with a case-insensitive name compare and got past that step, so the field exists and only its `name` diverges.
Why: the title field is required to create any case, and the documented way to resolve it is by its display name from ListFields. A client that does so gets no field id, cannot build CreateCase `fields`, and fails before it issues a single case write.
Fix: the state seeding of the reserved System fields, which ListFields and the BatchGetField summaries report (services/service-com-amazonaws-connectcases-amazonconnectcases/src/state.rs — `RESERVED_SYSTEM_FIELDS` at lines 106-117 and `reserved_system_fields()` at lines 1123-1161, whose doc comment states the deliberate choice "A reserved field's `name` IS its reserved id"); the field id stays the reserved word, the `name` becomes the service's display name for it.

=== Cause 3 of 5: UpdateLayout's ResourceNotFoundException message is interpolated instead of AWS's fixed sentence (failing check: connectcases.layout-create-update-content-round-trip) ===
AWS: recorded behavior only for the text; Smithy: `ResourceNotFoundException` (HTTP 404, members `message`/`resourceId`/`resourceType` all required) is modeled on UpdateLayout. For this error class AWS's `message` is the FIXED sentence `Requested resource not found.` with NO variable part — nothing from the request is interpolated into it.
Emulator: the error CODE is right (ResourceNotFoundException) but the message is `Layout <layoutId> was not found` — i.e. templated text carrying the requested layout id.
Why: clients and test suites match on the message for this error class; an interpolated message never equals the constant AWS sends, so error-class matching and log-based alerting diverge. (The emulator is otherwise correct here: UpdateLayout after a soft DeleteLayout must be refused.)
Fix: the UpdateLayout handler's not-found path, and the shared not-found constructor it calls, so this error class emits the fixed sentence (services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/update_layout.rs and the ResourceNotFoundException builders in src/state.rs).

=== Cause 4 of 5: CreateCase's missing-required-field ValidationException message is the emulator's own wording (failing check: connectcases.required-field-missing-on-case-create-rejected) ===
AWS: recorded behavior only for the text. Smithy: `CreateCase.fields` (FieldValueList) and `CreateCase.templateId` are required, ValidationException (HTTP 400) is modeled, and a template's `requiredFields` is documented as the "List of fields that must have a value provided to create a case". AWS refuses with ValidationException and message `Field value is not provided for a required field: <fieldId>` — fixed text "Field value is not provided for a required field: " followed by the one required field id the request provided no entry for (a value from the template/request, not from any single recording).
Emulator: same code (ValidationException) but message `fields provides no value for field ids template <templateId> requires: <fieldId>` — different fixed text, and it interpolates the template id, which AWS's message does not mention.
Why: the refusal itself is right, but a client that surfaces or matches the message (field-level form validation mapping the message back to the offending field) reads text AWS never sends.
Fix: the CreateCase handler's required-field validation message (services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/create_case.rs, using the missing-id list from `crate::state::ids_not_provided`, src/state.rs lines 705-717).

=== Cause 5 of 5: CreateRelatedItem does not validate declared `type` against the `content` union variant — it answers NotImplemented (failing check: connectcases.related-item-type-content-mismatch-rejected) ===
AWS: Smithy: `CreateRelatedItem.type` (RelatedItemType) and `CreateRelatedItem.content` (RelatedItemInputContent union: contact | comment | file | sla | connectCase | custom) are both required, and ValidationException (HTTP 400) is modeled. Declaring `type=Contact` while supplying the `comment` variant of `content` is refused with ValidationException and message `CONTACT type was specified with a mismatched related-item content.` — fixed sentence whose leading token is the declared type upper-cased — and no related item is left behind (SearchRelatedItems on the case returns an empty list).
Emulator: returns error code `NotImplemented` ("operation CreateRelatedItem is scaffolded but not implemented in this emulator"), an unmodeled code for this operation, so the cross-member consistency rule is not enforced at all.
Why: a client relying on AWS to reject an inconsistent type/content pair gets an unmodeled error it cannot classify; once the operation is stood up without this rule it would instead silently accept a related item whose declared type contradicts its content, corrupting the case's related-item stream.
Fix: the CreateRelatedItem handler's validation, before any state write (services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/create_related_item.rs). Related but not the same work item: an open issue filed from the checkout covers standing up the UNIMPLEMENTED CreateRelatedItem stub (it names the operation and that file); it does not name this type/content consistency rule, its error code or its message, which is what this cause records.

## Evidence
Suite report: connectcases 5/22 (23%), 17 failed; 12 of those are blocked by already-filed unimplemented stubs (GetCaseAuditEvents, TagResource, DeleteField, SearchCases, DeleteDomain, CreateRelatedItem, DeleteTemplate) and are skipped here. The 5 clustered below are the remaining mismatches.

Cause 1 — connectcases.get-case-field-not-on-case-not-found:
  expected: GetCase on an in-schema-but-unset field -> 200, fields == []
  actual:   AssertionError: expected an in-schema-but-unset field to be silently omitted, got fields=[{'id': '244cea8b-b28e-4907-82bc-4cf80fa4b0be', 'value': {'emptyValue': {}}}]
  (the id above is a per-run value; the requirement is that NO entry appears for an unset field)
  not yet reached by the replay, from source: unknown-field refusal message expected 'Unknown field Ids: <ids>', emulator emits 'fields names field ids that do not exist in domain <domainId>: <ids>'

Cause 2 — connectcases.search-all-related-items-across-cases:
  expected: ListFields on a freshly created domain reports a field with namespace='System' and name='Title'
  actual:   AssertionError: domain '6e22348f-678a-4b0b-a3a9-3140ae54ccee' has no system field named 'Title'; src/state.rs seeds name := fieldId, so the field comes back as name='title'

Cause 3 — connectcases.layout-create-update-content-round-trip:
  expected: UpdateLayout on a soft-deleted layout -> ResourceNotFoundException, Message == 'Requested resource not found.' (fixed text, no variable part)
  actual:   AssertionError: expected UpdateLayout message 'Requested resource not found.', got 'Layout 780ae2fe-fb34-42fc-a004-b256d705e9b0 was not found'

Cause 4 — connectcases.required-field-missing-on-case-create-rejected:
  expected: ValidationException, Message == 'Field value is not provided for a required field: <required fieldId>' (fixed text + the required field id from the template)
  actual:   AssertionError: expected message 'Field value is not provided for a required field: c2a77ba8-1b8a-468c-906f-8d24a9c83d33', got 'fields provides no value for field ids template 51031b84-b732-4fef-9463-52536fdb52fd requires: c2a77ba8-1b8a-468c-906f-8d24a9c83d33'
  (both ids are per-run values; the fixed text and the absence of any template id are the requirement)

Cause 5 — connectcases.related-item-type-content-mismatch-rejected:
  expected: CreateRelatedItem(type='Contact', content={'comment': {...}}) -> ValidationException (HTTP 400), Message == 'CONTACT type was specified with a mismatched related-item content.', and SearchRelatedItems on the case returns []
  actual:   AssertionError: expected ValidationException, got 'NotImplemented' (message: 'operation CreateRelatedItem is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Replay the five checks named in trace_ids against the connectcases emulator (`run_behavioral_suite`). Per cause:

Cause 1: CreateDomain -> poll GetDomain to Active; CreateField(type='Text') twice (one 'set', one 'unset'); CreateTemplate; CreateCase setting title + the 'set' field only; then GetCase(domainId, caseId, fields=[{'id': <unset field id>}]). Expect 200 with fields == []; emulator returns one entry with value {'emptyValue': {}}. Then GetCase with a field id that is not in the domain schema and compare the ValidationException message to 'Unknown field Ids: <that id>'.

Cause 2: CreateDomain -> poll to Active; ListFields(domainId) and look for an entry with namespace == 'System' and name == 'Title'. Emulator returns that field with name 'title' (fieldId 'title'), so an exact-name lookup finds nothing.

Cause 3: CreateDomain -> Active; CreateLayout; UpdateLayout (succeeds); DeleteLayout; then UpdateLayout again on the deleted layout. Code is ResourceNotFoundException as AWS; compare Message to the fixed sentence 'Requested resource not found.'

Cause 4: CreateDomain -> Active; CreateField(type='Text'); CreateTemplate with requiredFields=[{fieldId: <that field>}]; CreateCase supplying only the title field. Compare the ValidationException Message to 'Field value is not provided for a required field: <that field id>'.

Cause 5: CreateDomain -> Active; CreateField; CreateTemplate; CreateCase; then CreateRelatedItem(type='Contact', content={'comment': {'body': '...', 'contentType': 'Text/Plain'}}). Expect ValidationException with 'CONTACT type was specified with a mismatched related-item content.' and no item created; emulator answers NotImplemented.
