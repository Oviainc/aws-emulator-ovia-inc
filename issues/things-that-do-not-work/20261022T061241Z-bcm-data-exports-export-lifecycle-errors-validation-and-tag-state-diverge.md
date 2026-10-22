Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-data-exports-delete-export-removes-from-list
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-data-exports-delete-export-removes-from-list bcm-data-exports-export-not-found-errors bcm-data-exports-export-validation-errors bcm-data-exports-tag-resource-and-list-tags-round-trip
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports|GetExport|4xx->4xx|status
Issue-Key: bcm-data-exports|DeleteExport|4xx->4xx|status
Issue-Key: bcm-data-exports|UpdateExport|4xx->4xx|status
Issue-Key: bcm-data-exports|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: bcm-data-exports|ListTagsForResource|4xx->4xx|status
Issue-Key: bcm-data-exports|GetExport|2xx->2xx|$.Export.DataQuery.TableConfigurations
Issue-Key: bcm-data-exports|GetExport|2xx->2xx|$.ExportStatus.CreatedAt
Issue-Key: bcm-data-exports|GetExport|2xx->2xx|$.ExportStatus.LastUpdatedAt
Issue-Key: bcm-data-exports|CreateExport|4xx->2xx|$.Message
Issue-Key: bcm-data-exports|CreateExport|4xx->2xx|$.__type
Issue-Key: bcm-data-exports|CreateExport|4xx->2xx|status
Issue-Key: bcm-data-exports|CreateExport|4xx->4xx|$.message
Issue-Key: bcm-data-exports|ListTagsForResource|2xx->2xx|$.ResourceTags[*]
Issue-Commit: f95a296744f71657ae3614221270df79d0b4a393

# Export lifecycle errors, validation and tag state diverge

> _Note: this issue was identified against the code base at commit `f95a296744f71657ae3614221270df79d0b4a393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
The export-definition side of the emulator diverges from AWS in three related ways: the not-found error path answers with the wrong HTTP status on every operation that has one, the stored export definition and its tags are not round-tripped, and CreateExport skips a validation AWS performs. 9 causes.

Cause 1 of 9: GetExport for a deleted/unknown export ARN answers HTTP 404 instead of 400.
AWS: recorded behavior — AWS answers HTTP 400 for the not-found condition on this service (the recording asserts status 400 for the step that reads a just-deleted export). Smithy models ResourceNotFoundException with @httpError(404) (members Message/ResourceId/ResourceType all required), so model and recorded wire disagree; the recording is the wire contract being replayed. Hypothesis: like several other awsJson1_1 billing services, this one collapses client errors onto 400 regardless of the modeled httpError trait.
Emulator: HTTP 404.
Why: SDK/CLI retry and error-classification logic branches on the HTTP status before the __type; a 404 where AWS sends 400 makes clients treat a deterministic client error differently (and HTTP-level proxies/gateways treat 404 as a routing failure rather than an API error).
Fix: the error-to-status mapping used when the export-definition store misses, in this service's error serializer.
Traces: bcm-data-exports-delete-export-removes-from-list#task#7 (cluster also covers bcm-data-exports-export-not-found-errors#task#1).

Cause 2 of 9: DeleteExport for an unknown export ARN answers 404 instead of 400.
AWS: recorded behavior — status 400; Smithy DeleteExport errors = ResourceNotFoundException (modeled @httpError 404) + common ValidationException (400).
Emulator: HTTP 404.
Why: as cause 1 — a delete of an already-absent export is misclassified by status.
Fix: same error-to-status mapping, DeleteExport path.
Traces: bcm-data-exports-export-not-found-errors#task#2.

Cause 3 of 9: UpdateExport for an unknown export ARN answers 404 instead of 400.
AWS: recorded behavior — status 400; Smithy UpdateExport errors = ResourceNotFoundException + common ValidationException.
Emulator: HTTP 404.
Why: as cause 1.
Fix: same error-to-status mapping, UpdateExport path.
Traces: bcm-data-exports-export-not-found-errors#task#3.

Cause 4 of 9: ListTagsForResource for an unknown export ARN answers 404 instead of 400.
AWS: recorded behavior — status 400; Smithy ListTagsForResource errors = AccessDeniedException(403), ResourceNotFoundException(404 modeled), ThrottlingException(429), ValidationException(400).
Emulator: HTTP 404. The body text also differs (recorded Message 'Resource not found' vs emulator 'No matching definition found'); the status is the requirement, the message text is quoted only as an example.
Fix: same error-to-status mapping, ListTagsForResource path.
Traces: bcm-data-exports-export-not-found-errors#task#4.

Cause 5 of 9: GetExport drops DataQuery.TableConfigurations from the stored export definition.
AWS: Smithy DataQuery = { QueryStatement (required), TableConfigurations (map) }, and GetExport returns the Export it stored. The recording shows the created export's TableConfigurations coming back intact (a COST_AND_USAGE_REPORT entry carrying BILLING_VIEW_ARN, INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY, INCLUDE_RESOURCES, INCLUDE_SPLIT_COST_ALLOCATION_DATA and TIME_GRANULARITY).
Emulator: $.Export.DataQuery.TableConfigurations is absent from the GetExport response. In the same step ExportStatus.CreatedAt/LastUpdatedAt also diverge in shape from what the recording asserts (the emulator emits a fractional epoch value where the recording asserts the timestamp's recorded shape); the timestamp *values* are environment-specific and are not the requirement, the presence/shape is.
Why: TableConfigurations is the per-table property set (granularity, resource inclusion, billing view) that determines what the export actually contains. A client that reads an export back to edit it and then calls UpdateExport with the round-tripped definition silently drops those properties, changing the export's content.
Fix: the export-definition state store / GetExport serializer — TableConfigurations must be persisted on create and re-serialized on read.
Traces: bcm-data-exports-delete-export-removes-from-list#task#4.

Cause 6 of 9: CreateExport accepts a non-existent table name and returns 200 instead of a ValidationException.
AWS: Smithy CreateExport errors include the service-common ValidationException (@httpError 400, Message required). The recording shows AWS rejecting an export whose query names an unknown table with status 400, __type 'ValidationException' and a Message naming the offending table (recorded example: 'Table totally_bogus_table_that_does_not_exist is invalid').
Emulator: HTTP 200 with no __type and no Message — the export is created.
Why: the invalid export is persisted, so the failure surfaces later (or never) instead of at the create call; a client's error handling for an unsupported table never runs and it believes it has a working export.
Fix: CreateExport input validation — validate the query's table name against the table catalog and raise ValidationException(400) naming the table.
Traces: bcm-data-exports-export-validation-errors#task#2.

Cause 7 of 9: the CreateExport validation message renders the export-name pattern with regex anchors AWS strips.
AWS: the standard AWS validation message is built from the request and names the member and the constraint it failed; Smithy Export.Name is @pattern("^[0-9A-Za-z\\-_]+$"), and the recording shows AWS rendering that constraint without the anchors: "1 validation error detected: Value 'bad name!' at 'export.name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z\\-_]+". The rule (message built from the member path plus its rendered pattern) is the requirement, not the recorded sentence.
Emulator: same 400 status and same sentence but with the raw anchored pattern '^[0-9A-Za-z\\-_]+$'.
Why: clients and test suites that assert on, parse or display the constraint from the validation message see a pattern string AWS never emits.
Fix: the validation-message renderer for pattern constraints (strip the leading ^ / trailing $ when rendering a member pattern).
Traces: bcm-data-exports-export-validation-errors#task#1.

Cause 8 of 9: tags set by TagResource are not returned by ListTagsForResource.
AWS: Smithy TagResource attaches ResourceTag{Key,Value} pairs to an export and ListTagsForResource returns them in ResourceTags (ResourceTagList, max 200). The recording shows both tags coming back after a successful TagResource.
Emulator: ListTagsForResource answers 200 with ResourceTags empty — $.ResourceTags[0] and [1] are absent.
Why: tagging appears to succeed but is not readable, so tag-based discovery, cost allocation and any read-modify-write of tags operate on an empty set.
Fix: the tag state attached to the export definition (TagResource must persist into the same store ListTagsForResource reads).
Traces: bcm-data-exports-tag-resource-and-list-tags-round-trip#task#5.

Cause 9 of 9: after UntagResource removes one key, ListTagsForResource returns no tags instead of the remaining one.
AWS: UntagResource removes only the named keys; the recording shows the surviving tag still listed afterwards.
Emulator: ResourceTags is empty — $.ResourceTags[0] absent.
Why: a partial untag is indistinguishable from removing every tag, so a client that removes one key loses the rest of its tag set as far as the API can tell. (Follows from cause 8 but is a distinct recorded step and must be re-replayed separately.)
Fix: same tag state store, UntagResource path — remove only the requested keys and keep the rest readable.
Traces: bcm-data-exports-tag-resource-and-list-tags-round-trip#task#7.

## Evidence
Replayed against the emulator booted from mainline (emulator_up=true, state_reset_failures=0, 10 fixtures replayed, 49 steps, 10 passed, 32 mismatched).

Cause 1 — bcm-data-exports-delete-export-removes-from-list#task#7, GetExport: 'status: expected 400, got 404'. divergence_key service=bcm-data-exports|GetExport|status=400->404|keys=status; affected_count=2 (also bcm-data-exports-export-not-found-errors#task#1).
Cause 2 — bcm-data-exports-export-not-found-errors#task#2, DeleteExport: 'status: expected 400, got 404'.
Cause 3 — bcm-data-exports-export-not-found-errors#task#3, UpdateExport: 'status: expected 400, got 404'.
Cause 4 — bcm-data-exports-export-not-found-errors#task#4, ListTagsForResource: 'status: expected 400, got 404' plus $.Message value_mismatch (expected 'Resource not found', got 'No matching definition found').
Cause 5 — bcm-data-exports-delete-export-removes-from-list#task#4, GetExport 200 vs 200: $.Export.DataQuery.TableConfigurations missing (expected {'COST_AND_USAGE_REPORT': {'BILLING_VIEW_ARN': 'arn:aws:billing::000000000000:billingview/primary', 'INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY': 'FALSE', 'INCLUDE_RESOURCES': 'FALSE', 'INCLUDE_SPLIT_COST_ALLOCATION_DATA': 'FALSE', 'TIME_GRANULARITY': 'HOURLY'}}, got None); $.ExportStatus.CreatedAt and $.ExportStatus.LastUpdatedAt value_mismatch against the recording's shape assertion (got a fractional epoch value) — timestamp values themselves are environment-specific and not asserted as a requirement here.
Cause 6 — bcm-data-exports-export-validation-errors#task#2, CreateExport: 'status: expected 400, got 200'; $.__type missing (expected 'ValidationException'); $.Message missing (expected 'Table totally_bogus_table_that_does_not_exist is invalid').
Cause 7 — bcm-data-exports-export-validation-errors#task#1, CreateExport 400 vs 400: $.message value_mismatch — expected "1 validation error detected: Value 'bad name!' at 'export.name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z\\-_]+", got the same sentence ending '^[0-9A-Za-z\\-_]+$'.
Cause 8 — bcm-data-exports-tag-resource-and-list-tags-round-trip#task#5, ListTagsForResource 200 vs 200: $.ResourceTags[0] missing (expected {'Key': 'env', 'Value': 'test'}), $.ResourceTags[1] missing (expected {'Key': 'team', 'Value': 'billing'}).
Cause 9 — bcm-data-exports-tag-resource-and-list-tags-round-trip#task#7, ListTagsForResource 200 vs 200: $.ResourceTags[0] missing (expected {'Key': 'team', 'Value': 'billing'}).

Model consulted: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model — ResourceNotFoundException(errorKind client, httpError 404, Message/ResourceId/ResourceType required), ValidationException(httpError 400, Message required), Export.Name @pattern ^[0-9A-Za-z\-_]+$, DataQuery{QueryStatement required, TableConfigurations map}, ListTagsForResourceResponse.ResourceTags(ResourceTagList, length 0..200), ResourceTag{Key,Value required}, CreateExport/GetExport/DeleteExport/UpdateExport/ListTagsForResource error lists.

## Steps to Reproduce
1. git clone the mainline checkout and boot the emulator from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture bcm-data-exports-delete-export-removes-from-list to step task#7 — GetExport on the ARN just deleted; observe HTTP 404 where the recording expects 400.
4. Causes 2-4: replay fixture bcm-data-exports-export-not-found-errors, steps task#2 (DeleteExport), task#3 (UpdateExport), task#4 (ListTagsForResource) against an unknown export ARN; each answers 404 instead of 400.
5. Cause 5: replay fixture bcm-data-exports-delete-export-removes-from-list, step task#4 — CreateExport with DataQuery.TableConfigurations then GetExport; observe TableConfigurations absent from the returned Export.
6. Cause 6: replay fixture bcm-data-exports-export-validation-errors, step task#2 — CreateExport whose query names a table that does not exist; observe 200 instead of 400 ValidationException.
7. Cause 7: same fixture, step task#1 — CreateExport with Export.Name 'bad name!'; observe the validation message quoting the anchored pattern.
8. Causes 8-9: replay fixture bcm-data-exports-tag-resource-and-list-tags-round-trip, step task#5 (ListTagsForResource after TagResource) and step task#7 (after UntagResource of one key); observe ResourceTags empty in both.
