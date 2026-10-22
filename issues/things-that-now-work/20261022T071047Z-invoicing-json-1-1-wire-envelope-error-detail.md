Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-create-invoice-unit-description-round-trip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-create-invoice-unit-description-round-trip invoicing-invoice-unit-lifecycle invoicing-invoice-unit-errors
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|GetInvoiceUnit|2xx->2xx|$headers.content-type
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.fieldList
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.message
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.reason
Issue-Key: invoicing|GetInvoiceUnit|4xx->4xx|$.message
Issue-Key: invoicing|GetInvoiceUnit|4xx->4xx|$.resourceId
Issue-Key: invoicing|GetInvoiceUnit|4xx->4xx|$headers.content-type
Issue-Key: invoicing|UpdateInvoiceUnit|4xx->4xx|$.message
Issue-Key: invoicing|UpdateInvoiceUnit|4xx->4xx|$.resourceId
Issue-Key: invoicing|UpdateInvoiceUnit|4xx->4xx|$headers.content-type
Issue-Key: invoicing|DeleteInvoiceUnit|4xx->4xx|$.message
Issue-Key: invoicing|DeleteInvoiceUnit|4xx->4xx|$.resourceId
Issue-Key: invoicing|DeleteInvoiceUnit|4xx->4xx|$headers.content-type
Issue-Key: invoicing|CreateInvoiceUnit|2xx->2xx|$headers.content-type
Issue-Key: invoicing|ListInvoiceUnits|2xx->2xx|$headers.content-type
Issue-Key: invoicing|UpdateInvoiceUnit|2xx->2xx|$headers.content-type
Issue-Key: invoicing|ListTagsForResource|2xx->2xx|$.ResourceTags[*]
Issue-Key: invoicing|DeleteInvoiceUnit|2xx->2xx|$headers.content-type
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Invoicing JSON 1.1 wire envelope, error detail members and tag state diverge

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvoiceUnit

## Description
The Invoicing emulator diverges from the recorded AWS wire on four distinct causes: the JSON protocol envelope it advertises on every response, two missing members of its modeled error shapes, and invoice-unit tag state that is never returned. This issue bundles FOUR causes; all four must be closed before it is resolved.

Cause 1 of 4 - every Invoicing response advertises content-type application/x-amz-json-1.0 instead of application/x-amz-json-1.1 (CreateInvoiceUnit, GetInvoiceUnit, UpdateInvoiceUnit, DeleteInvoiceUnit, ListInvoiceUnits, and the error responses too).
AWS: recorded behavior only - every recorded Invoicing response, success and error alike, carries 'content-type: application/x-amz-json-1.1'. Note the on-disk model disagrees with the wire and must NOT be used to settle this: Smithy service com.amazonaws.invoicing#Invoicing (2024-12-01) declares protocol awsJson1_0, yet the live service answers as JSON 1.1 - consistent with the recorded error bodies, whose '__type' values are stamped in the com.amazonaws.awsinsightsindexservice.v0 namespace (the Cost Explorer backend that fronts Invoicing). The recording is the requirement here.
Emulator: serializes and announces application/x-amz-json-1.0 on all of the above.
Why: a client that selects its deserializer or protocol behavior from the response content-type (and any strict proxy/assertion on the negotiated protocol) sees a different protocol version than real Invoicing, so correct JSON 1.1 handling cannot be exercised against the emulator.
Fix: the service-wide response serializer / protocol binding for Invoicing - the content-type it emits, applied to success and error paths alike, not per-operation.
Traces: invoicing-create-invoice-unit-description-round-trip#task#2 (GetInvoiceUnit, 200, also invoicing-invoice-unit-lifecycle#task#2 and #task#5); invoicing-invoice-unit-lifecycle#setup#1 (CreateInvoiceUnit, 200); invoicing-invoice-unit-lifecycle#task#3 (ListInvoiceUnits, 200); invoicing-invoice-unit-lifecycle#task#4 (UpdateInvoiceUnit, 200); invoicing-invoice-unit-lifecycle#teardown#10 (DeleteInvoiceUnit, 200); and the 404 clusters of Cause 3, which diverge on content-type as well.

Cause 2 of 4 - ValidationException omits the 'reason' and 'fieldList' members on a failed input constraint.
AWS: Smithy com.amazonaws.invoicing#ValidationException (@error client, httpError 400) models members message, resourceName, reason (enum ValidationExceptionReason, whose values include 'fieldValidationFailed') and fieldList (ValidationExceptionFieldList of ValidationExceptionField{name, message - both @required}). CreateInvoiceUnit.InvoiceReceiver targets AccountIdString with @pattern ^\d{12}$, so a non-conforming account id is a modeled ValidationException; the recording shows status 400 with reason='fieldValidationFailed' and one fieldList entry naming the offending member ('invoiceReceiver') and the constraint it failed.
Emulator: status 400 is correct, but the body carries only 'message' - '$.reason' and '$.fieldList' are absent. The message text also differs in form (it embeds the rejected value and the anchored pattern); the message is an example, the requirement is the structured reason/fieldList pair plus a per-field entry naming the member and the constraint it violated.
Why: SDK and client code that branches on ValidationException.reason, or that maps fieldList entries back to form fields to report which input was rejected, gets a null reason and an empty field list and cannot distinguish a field-level validation failure from any other validation error.
Fix: the Invoicing validation/error-serialization path that builds ValidationException - populate reason and fieldList when a member constraint rejects an input.
Traces: invoicing-invoice-unit-errors#task#1 (CreateInvoiceUnit, 400->400).

Cause 3 of 4 - ResourceNotFoundException omits the member carrying the missing resource's ARN, and interpolates the ARN into the message instead.
AWS: Smithy com.amazonaws.invoicing#ResourceNotFoundException (@error client, httpError 404) is bound to GetInvoiceUnit, UpdateInvoiceUnit, DeleteInvoiceUnit and ListTagsForResource and models a structured member for the resource identity alongside 'message'; the shape's own documentation gives the message as the fixed sentence 'The resource could not be found.'. The recording shows status 404 with that fixed message and the requested invoice-unit ARN carried in its own body member ('resourceId' on the wire). The rule is therefore known: the message is a constant and the ARN travels as a separate field.
Emulator: status 404 is correct, but the ARN member is missing from the body and the ARN is spliced into the message instead (e.g. 'The resource arn:...:invoice-unit/<id> was not found'), so the response has no machine-readable resource identity. Note the model spells this member 'resourceName' while the recorded wire sends 'resourceId'; follow the recorded wire name.
Why: a client that reads the resource identifier off the exception - to retry against a different ARN, to log which unit was missing, or to correlate the failure with a batch item - gets nothing, and anything matching the documented constant message fails because the emulator's message is request-dependent.
Fix: the Invoicing error serializer / not-found construction shared by the invoice-unit read, update and delete handlers - emit the constant message and the ARN member.
Traces: invoicing-invoice-unit-errors#task#2 (GetInvoiceUnit, 404->404, also invoicing-invoice-unit-lifecycle#teardown#11); invoicing-invoice-unit-errors#task#3 (UpdateInvoiceUnit, 404->404); invoicing-invoice-unit-errors#task#4 (DeleteInvoiceUnit, 404->404).

Cause 4 of 4 - ListTagsForResource returns an empty ResourceTags list for an invoice unit that carries tags.
AWS: Smithy ListTagsForResource -> ListTagsForResourceResponse.ResourceTags (ResourceTagList of ResourceTag{Key, Value - both @required}, length 0..200); CreateInvoiceUnit accepts the same ResourceTagList as input, so tags supplied when the unit is created (or added afterwards) are readable back through ListTagsForResource. The recording shows the created unit's tag present in the response list.
Emulator: status 200, but ResourceTags is empty - the first expected entry (a Key/Value pair set on the unit) is missing, i.e. tag state is accepted on write and then dropped rather than stored against the invoice unit.
Why: any tag-driven workflow against the emulator silently sees untagged resources - cost allocation by tag, tag-based access or grouping, and read-after-write assertions on tagging all break, and the client cannot tell a dropped tag from a resource that was genuinely never tagged.
Fix: Invoicing invoice-unit state - persist the ResourceTags supplied on create/tag and have the ListTagsForResource handler read them back from that state.
Traces: invoicing-invoice-unit-lifecycle#task#7 (ListTagsForResource, 200->200).

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 46 steps, 6 passed, score 13%, state_reset_failures 0, bootstrap_signal false, emulator up on :34499) reported 10 invoicing divergence clusters, all verdict=mismatch, none with a stub_path and none owned_by an existing issue.

Cause 1 (content-type), 8 clusters:
- invoicing-create-invoice-unit-description-round-trip#task#2 GetInvoiceUnit expected 200 / actual 200: $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0'); affected also invoicing-invoice-unit-lifecycle#task#2, invoicing-invoice-unit-lifecycle#task#5.
- invoicing-invoice-unit-lifecycle#setup#1 CreateInvoiceUnit 200/200, same content-type mismatch.
- invoicing-invoice-unit-lifecycle#task#3 ListInvoiceUnits 200/200, same.
- invoicing-invoice-unit-lifecycle#task#4 UpdateInvoiceUnit 200/200, same.
- invoicing-invoice-unit-lifecycle#teardown#10 DeleteInvoiceUnit 200/200, same.
- the three 404 clusters below additionally report the same content-type mismatch.

Cause 2 (ValidationException), invoicing-invoice-unit-errors#task#1 CreateInvoiceUnit expected 400 / actual 400:
- $.reason: missing (expected 'fieldValidationFailed', got None)
- $.fieldList: missing (expected [{'__type': 'com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField', 'message': "Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}", 'name': 'invoiceReceiver'}], got None)
- $.message: value_mismatch (expected "1 validation error detected: Value at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}", got "1 validation error detected: Value 'not-an-account-id' at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$") - message quoted as an example; the requirement is the structured reason + fieldList.

Cause 3 (ResourceNotFoundException), expected 404 / actual 404 on three clusters, each reporting:
- $.resourceId: missing (expected 'arn:aws:invoicing:us-east-1:000000000000:invoice-unit/does-not-exist-000', got None)
- $.message: value_mismatch (expected 'The resource could not be found.', got 'The resource arn:aws:invoicing:us-east-1:000000000000:invoice-unit/does-not-exist-000 was not found')
- $headers.content-type: expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0'
Clusters: invoicing-invoice-unit-errors#task#2 GetInvoiceUnit (affected also invoicing-invoice-unit-lifecycle#teardown#11); invoicing-invoice-unit-errors#task#3 UpdateInvoiceUnit; invoicing-invoice-unit-errors#task#4 DeleteInvoiceUnit.

Cause 4 (tags), invoicing-invoice-unit-lifecycle#task#7 ListTagsForResource expected 200 / actual 200:
- $.ResourceTags[0]: missing (expected {'Key': 'Environment', 'Value': 'Test'}, got None)

Model citations used: service com.amazonaws.invoicing#Invoicing v2024-12-01 (declares awsJson1_0, contradicted by the recorded 1.1 wire); ValidationException members message/resourceName/reason/fieldList with ValidationExceptionReason value 'fieldValidationFailed' and ValidationExceptionField{name,message @required}; ResourceNotFoundException (httpError 404) with message + resource-name member, doc 'The resource could not be found.'; CreateInvoiceUnit.InvoiceReceiver @pattern ^\\d{12}$; ListTagsForResourceResponse.ResourceTags -> ResourceTagList of ResourceTag{Key,Value @required}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus, no arguments).
3. Cause 1: replay fixture invoicing-create-invoice-unit-description-round-trip step task#2 (GetInvoiceUnit) and inspect the response content-type header - it is application/x-amz-json-1.0, the recording is application/x-amz-json-1.1. Same for invoicing-invoice-unit-lifecycle steps setup#1, task#3, task#4, teardown#10.
4. Cause 2: replay invoicing-invoice-unit-errors step task#1 - CreateInvoiceUnit with InvoiceReceiver 'not-an-account-id' (violating ^\\d{12}$); the 400 body has no 'reason' and no 'fieldList'.
5. Cause 3: replay invoicing-invoice-unit-errors steps task#2/task#3/task#4 - GetInvoiceUnit/UpdateInvoiceUnit/DeleteInvoiceUnit against a non-existent invoice-unit ARN; the 404 body has no resource-ARN member and the message embeds the ARN instead of being the constant 'The resource could not be found.'.
6. Cause 4: replay invoicing-invoice-unit-lifecycle through step task#7 - create an invoice unit carrying ResourceTags, then call ListTagsForResource on its ARN; ResourceTags comes back empty.
