Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-field-and-options-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-field-and-options-lifecycle connectcases-field-lifecycle-and-batch-get
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.errors[*].message
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.fields[*]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Assigned Queue"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Assigned User"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Assigned queue"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Assigned user"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Case Id"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Case Reason"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Case reason"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Created By"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Customer Id"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Customer"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/Time Last Closed"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/Time Last Reopened"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/Time Opened"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/Time Updated"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/time created"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Date/time last updated"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Last Updated User"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Next SLA Breach Date/Time"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Reference Id"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Reference Number"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Reference number"]
Issue-Key: connectcases|ListFields|2xx->2xx|$.fields[name="Status"].type
Issue-Related: 20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# connectcases field state: System field seed and UpdateField mutations missing

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFields

## Description
Three net-new wire divergences about the STATE behind the field APIs rather than the shape of a refusal: a new domain is not seeded with the System-namespace fields AWS creates for it, an UpdateField does not change what a later read returns, and the per-entry error inside BatchGetField's `errors` list is worded by the emulator rather than by AWS. All three surface on 200 responses. THIS ISSUE HAS THREE CAUSES - close all three before submitting.

=== Cause 1 of 3 (representative): a field renamed by UpdateField still reads back with its old name, and the BatchGetField per-entry error message is the emulator's own (fixture connectcases-field-and-options-lifecycle, step task#8) ===
AWS: Smithy: UpdateField = PUT /domains/{domainId}/fields/{fieldId}, doc 'Updates the properties of an existing field.'; BatchGetField = POST /domains/{domainId}/fields-batch returns `fields` (detail entries) and `errors` (BatchGetFieldErrorList of FieldError{id required, errorCode required, message}). Recorded behavior: after a successful UpdateField the field detail returned by BatchGetField reflects the NEW name (and the new description) - the mutation is durable and visible to the next read. For an id the domain does not hold, AWS does not fail the call: it returns 200 and an `errors` entry whose message is the fixed sentence `This resource cannot be found` - entirely fixed text, with NO variable parts (no field name, no domain id); the requested id is carried in the entry's `id` member, which is where a client reads it from.
Emulator: BatchGetField returns the field with its ORIGINAL (pre-update) name, and the per-entry message is `Field <name> was not found in domain <domainId>` - a generated sentence interpolating the requested identifier and the domain id. (The field's update being lost is consistent with UpdateField being an unimplemented scaffold - see the open issue 20261102T010613Z-connectcases-updatefield-stub.md, which owns the UpdateField operation itself but not this read-back divergence or the errors-entry wording; both are filed here because the replay reports this cluster with no stub_path and no owner.)
Why: a client that renames a field and re-reads it sees the rename silently discarded, so configuration flows appear to succeed and then operate on stale metadata; and code matching the per-entry message to classify a partial batch failure never matches AWS's sentence.
Fix: UpdateField's handler/state mutation (persist name and description onto the stored field so BatchGetField/ListFields observe it) and BatchGetField's errors projection (the fixed per-entry message, with the id carried in the entry's `id` member).
Traces: connectcases-field-and-options-lifecycle#task#8.

=== Cause 2 of 3: a new domain is not seeded with AWS's System fields (fixture connectcases-field-lifecycle-and-batch-get, step task#5) ===
AWS: Smithy: ListFields = POST /domains/{domainId}/fields-list returning FieldSummary{fieldId, fieldArn, name, type, namespace, attributes} (first five required). Recorded behavior: every newly created Cases domain already contains AWS's built-in System-namespace fields, listed alongside any Custom field the caller created. The recording shows them with namespace 'System', stable snake_case fieldIds, a fieldArn of the form arn:aws:cases:<region>:<account>:domain/<domainId>/field/<fieldId>, attributes null, and AWS's own display names - e.g. case_id 'Case Id' (Text), assigned_queue 'Assigned Queue' (Text), assigned_user 'Assigned User' (Text), case_reason 'Case Reason' (SingleSelect), created_by 'Created By' (User), customer_id 'Customer Id' (Text), last_closed_datetime 'Date/Time Last Closed', and the other Date/Time, Reference and Status fields in the same set.
Emulator: the recorded System fields are absent from ListFields. Where the emulator does seed something, it uses DIFFERENT display names for the same concept (the diff reports both 'Assigned Queue' and 'Assigned queue', 'Case Reason' and 'Case reason', 'Customer Id' and 'Customer', 'Reference Number' and 'Reference number', 'Date/Time Opened' and 'Date/time created' as separate keys), and the Status field's `type` diverges as well.
Why: the System field set is the case object model. A client that resolves 'title'/'status'/'assigned_user' by listing fields, or renders a case using the System field names, finds them missing or under a name it does not recognise - so case creation, layout construction and case rendering all fail against the emulator on a domain that would work on AWS. This is a data-content gap, not a formatting one.
Fix: the domain-creation path in the service's state module - seed the full System field set (ids, display names, types, namespace, derived ARNs) at CreateDomain, and align the Status field's type with the recorded value.
Traces: connectcases-field-lifecycle-and-batch-get#task#5; cluster also holds #task#10.

=== Cause 3 of 3: a field's updated description is not visible to BatchGetField (fixture connectcases-field-lifecycle-and-batch-get, step task#7) ===
AWS: as Cause 1: after UpdateField changes a field's `description` (FieldDescription, @length max 255), the next BatchGetField returns the NEW description; and the per-entry error for an unknown id is the fixed sentence `This resource cannot be found`. Recorded: the renamed-field description is the post-update text.
Emulator: the entry carries the PRE-update description, the errors entry carries the generated `Field <name> was not found in domain <domainId>` sentence, and `fields[0].tags` is omitted (the tags/timestamp serialization half of this step is recorded as a cause of the companion issue 'connectcases responses serialize timestamps as epoch numbers and drop empty members'; the requirement here is the stale value and the error wording).
Why: same as Cause 1 - a description edit is silently lost, so a client cannot tell a successful UpdateField from a no-op.
Fix: same as Cause 1 (UpdateField state mutation + BatchGetField errors projection); listed separately because it is a distinct mutated member on a distinct fixture and must be re-replayed on its own.
Traces: connectcases-field-lifecycle-and-batch-get#task#7.

## Evidence
Replay summary: simparity-wire-aws-v1, emulator_up=true, 182 steps, 117 passed, 56 mismatched, score 64.3%, 22 clusters (all connectcases). All three clusters below are verdict=mismatch on expected 200 / actual 200 with empty stub_path and empty owned_by.

Cause 1 - connectcases-field-and-options-lifecycle#task#8, BatchGetField:
  $.errors[0].message: expected 'This resource cannot be found', got 'Field <requested-id> was not found in domain <domainId>' (recorded: 'Field sim2real-probe-connectcases-missing-field-00000000 was not found in domain f99c5651-...'; the id and domain id are per-run values, the point is that AWS interpolates nothing)
  $.fields[0]: expected the entry with name '<renamed>' (recorded ...-text-field-upd-a49a0aae) plus attributes/description/tags keys, got the entry still named '<original>' (recorded ...-text-field-3b333bc5) - reported as both missing and extra because the whole entry differs

Cause 2 - connectcases-field-lifecycle-and-batch-get#task#5, ListFields:
  missing System-namespace entries, each expected as {fieldId, fieldArn: arn:aws:cases:<region>:<account>:domain/<domainId>/field/<fieldId>, name, type, namespace 'System', attributes null}: 'Assigned Queue' (assigned_queue, Text), 'Assigned User' (assigned_user, Text), 'Case Id' (case_id, Text), 'Case Reason' (case_reason, SingleSelect), 'Created By' (created_by, User), 'Customer Id' (customer_id, Text), 'Date/Time Last Closed' (last_closed_datetime), and the remaining Date/Time Last Reopened / Date/Time Opened / Date/Time Updated / Last Updated User / Next SLA Breach Date/Time / Reference Id / Reference Number entries in the same cluster key list
  also keyed in the same cluster: 'Assigned queue', 'Assigned user', 'Case reason', 'Customer', 'Date/time created', 'Date/time last updated', 'Reference number' (the emulator's divergent display names for the same concepts) and $.fields[name="Status"].type
  affected: connectcases-field-lifecycle-and-batch-get#task#10

Cause 3 - connectcases-field-lifecycle-and-batch-get#task#7, BatchGetField:
  $.errors[0].message: expected 'This resource cannot be found', got 'Field <requested-id> was not found in domain <domainId>'
  $.fields[0].description: expected the post-UpdateField text (recorded 'sim2real text field, renamed'), got the pre-update text (recorded 'sim2real text field')
  $.fields[0].tags: missing (expected {}) - serialization half, filed in the companion issue

Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): UpdateField = PUT /domains/{domainId}/fields/{fieldId} 'Updates the properties of an existing field.'; BatchGetFieldResponse = {fields required, errors required}; FieldError = {id required, errorCode required, message}; FieldSummary = {fieldId, fieldArn, name, type, namespace required, attributes}; GetFieldResponse.description -> FieldDescription @length max 255.

Related open issue, which does NOT cover these clusters: 20261102T010613Z-connectcases-updatefield-stub.md (UpdateField unimplemented scaffold stub) - it owns the UpdateField step itself; the read-back staleness, the System field seed and the errors-entry wording are reported by the replay with no owner.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the connectcases emulator and read the three clusters named under Traces. Manually:
1. CreateDomain(name=<unique>); poll GetDomain until domainStatus == 'Active'.
2. ListFields(domainId) on that brand-new domain -> expect AWS's System-namespace field set (case_id 'Case Id' Text, assigned_queue 'Assigned Queue' Text, assigned_user 'Assigned User' Text, case_reason 'Case Reason' SingleSelect, created_by 'Created By' User, customer_id 'Customer Id' Text, the Date/Time* fields, Reference Id/Number, Status, ...) each with namespace 'System' and fieldArn arn:aws:cases:<region>:<account>:domain/<domainId>/field/<fieldId>. The emulator returns neither that set nor those names (Cause 2).
3. CreateField(domainId, name=N1, type='Text', description='d1') -> fieldId.
4. UpdateField(domainId, fieldId, name=N2, description='d2').
5. BatchGetField(domainId, fields=[{id: fieldId}, {id: '<an id never created>'}]) -> expect 200 with fields[0].name == N2 and fields[0].description == 'd2', and errors[0] == {id: '<the id never created>', errorCode: <AWS's code>, message: 'This resource cannot be found'}. The emulator returns the pre-update name/description and the message 'Field <id> was not found in domain <domainId>' (Causes 1 and 3).
