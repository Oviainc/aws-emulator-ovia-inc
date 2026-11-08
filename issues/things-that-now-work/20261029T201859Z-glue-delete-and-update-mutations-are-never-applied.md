Issue-Source: trace-sweep-divergence
Issue-Trace: glue-blueprint-lifecycle
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-blueprint-lifecycle glue-data-quality-ruleset-lifecycle glue-data-quality-ruleset-listing glue-formtype-delete-then-not-found glue-integration-resource-property-lifecycle glue-job-trigger-lifecycle glue-trigger-event-batching glue-trigger-ondemand-lifecycle glue-trigger-scheduled-with-workflow glue-workflow-trigger-lifecycle glue-workflow-definition-and-batch-get-not-found glue-workflow-definition-and glue-workflow-lifecycle-and-duplicate-error glue-classifier-custom-entity-and-data-quality-ruleset glue-database-table-partition-lifecycle glue-partition-update-reflected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetBlueprint|4xx->2xx|$.Message
Issue-Key: glue|GetBlueprint|4xx->2xx|$.__type
Issue-Key: glue|GetBlueprint|4xx->2xx|status
Issue-Key: glue|GetDataQualityRuleset|4xx->2xx|$
Issue-Key: glue|GetDataQualityRuleset|4xx->2xx|$.Message
Issue-Key: glue|GetDataQualityRuleset|4xx->2xx|$.__type
Issue-Key: glue|GetDataQualityRuleset|4xx->2xx|status
Issue-Key: glue|GetFormType|4xx->2xx|$.Message
Issue-Key: glue|GetFormType|4xx->2xx|$.__type
Issue-Key: glue|GetFormType|4xx->2xx|status
Issue-Key: glue|GetIntegrationResourceProperty|4xx->2xx|$
Issue-Key: glue|GetIntegrationResourceProperty|4xx->2xx|$.Message
Issue-Key: glue|GetIntegrationResourceProperty|4xx->2xx|$.__type
Issue-Key: glue|GetIntegrationResourceProperty|4xx->2xx|status
Issue-Key: glue|GetTrigger|4xx->2xx|$
Issue-Key: glue|GetTrigger|4xx->2xx|$.Message
Issue-Key: glue|GetTrigger|4xx->2xx|$.__type
Issue-Key: glue|GetTrigger|4xx->2xx|status
Issue-Key: glue|GetWorkflow|4xx->2xx|$
Issue-Key: glue|GetWorkflow|4xx->2xx|$.Message
Issue-Key: glue|GetWorkflow|4xx->2xx|$.__type
Issue-Key: glue|GetWorkflow|4xx->2xx|status
Issue-Key: glue|GetDataQualityRuleset|2xx->2xx|$.Description
Issue-Key: glue|GetDataQualityRuleset|2xx->2xx|$.Identifier
Issue-Key: glue|GetDataQualityRuleset|2xx->2xx|$.Ruleset
Issue-Key: glue|GetPartition|2xx->2xx|$.Partition.Parameters
Issue-Key: glue|GetPartition|2xx->2xx|$.Partition.StorageDescriptor.Columns
Issue-Key: glue|GetPartition|2xx->2xx|$.Partition.StorageDescriptor.Location
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue delete and update mutations are never applied to stored resource state

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprint

## Description
This issue bundles 10 net-new causes with one shared fault: a mutating Glue call is accepted (200) but the emulator's state store is never changed, so the following read still returns the pre-mutation resource. Seven causes are deletes whose resource remains readable (read answers 200 instead of refusing with EntityNotFoundException / 400), three are updates whose new values are never visible. Every one of these makes a lifecycle scenario silently wrong rather than merely cosmetic, so the bundle is HIGH.

Cause 1 of 10: DeleteBlueprint then GetBlueprint.
AWS: Smithy: GetBlueprint error EntityNotFoundException (client, httpError 400). After a successful DeleteBlueprint the blueprint is gone: GetBlueprint answers HTTP 400 with __type EntityNotFoundException and Message 'Blueprint with given name not found.'
Emulator: GetBlueprint answers 200 with the full Blueprint body.
Why: a teardown/recreate flow believes the name is still taken; a client polling for deletion never terminates.
Fix: DeleteBlueprint handler must remove the blueprint from state (state store), so the GetBlueprint lookup misses.
Traces: glue-blueprint-lifecycle#teardown#5.

Cause 2 of 10: DeleteDataQualityRuleset then GetDataQualityRuleset.
AWS: Smithy: GetDataQualityRuleset error EntityNotFoundException (client, 400). Message template 'Cannot find Data Quality Ruleset in account <accountId> with name <name>'.
Emulator: GetDataQualityRuleset keeps answering 200 with the ruleset for the whole poll window (29 attempts), never producing an error code.
Why: deletion is unobservable; a delete-then-recreate scenario collides.
Fix: DeleteDataQualityRuleset handler must delete from state.
Traces: glue-data-quality-ruleset-lifecycle#teardown#5 (also glue-data-quality-ruleset-listing#teardown#5).

Cause 3 of 10: DeleteFormType then GetFormType.
AWS: Smithy: GetFormType error EntityNotFoundException (client, 400); the metadata-catalog message is JSON-wrapped: '{"Message":"Metadata form type <formTypeName> not found."}'.
Emulator: GetFormType answers 200 with the form type still present.
Why: as above.
Fix: DeleteFormType handler must remove the form type from state.
Traces: glue-formtype-delete-then-not-found#task#4.

Cause 4 of 10: DeleteIntegrationResourceProperty then GetIntegrationResourceProperty.
AWS: Smithy: GetIntegrationResourceProperty error EntityNotFoundException (client, 400). Message template 'The target resource <resourceArn> is not found.' with the ResourceArn from the request.
Emulator: answers 200 with the stored property for the whole poll window.
Why: as above.
Fix: DeleteIntegrationResourceProperty handler must delete the stored property.
Traces: glue-integration-resource-property-lifecycle#task#7.

Cause 5 of 10: DeleteTrigger then GetTrigger.
AWS: Smithy: GetTrigger error EntityNotFoundException (client, 400), Message 'Trigger not found.'
Emulator: GetTrigger answers 200 with the trigger (Name/Type/State/Actions) for the whole 60s poll.
Why: as above.
Fix: DeleteTrigger handler must remove the trigger from state. Note the open stub issue '20261029T131744Z-glue-deletetrigger-stub.md' asks for DeleteTrigger to exist at all; this cause records the wire step that proves the delete is not observable and must be re-replayed after the fix.
Traces: glue-job-trigger-lifecycle#teardown#9 (cluster also covers glue-trigger-event-batching#task#7, glue-trigger-ondemand-lifecycle#task#7, glue-trigger-scheduled-with-workflow#teardown#10, glue-workflow-trigger-lifecycle#task#10).

Cause 6 of 10: DeleteWorkflow then GetWorkflow (polled).
AWS: Smithy: GetWorkflow error EntityNotFoundException (client, 400), Message 'Entity not found'.
Emulator: GetWorkflow answers 200 with the workflow for the whole 60s poll.
Why: as above.
Fix: DeleteWorkflow handler must remove the workflow from state.
Traces: glue-trigger-scheduled-with-workflow#teardown#12 (also glue-workflow-definition-and-batch-get-not-found#task#14).

Cause 7 of 10: DeleteWorkflow then GetWorkflow (immediate read).
AWS: as cause 6 — 400 / EntityNotFoundException / 'Entity not found'.
Emulator: 200 with the workflow body and no error members.
Why: as above.
Fix: same component as cause 6; kept as its own cause because it is a distinct fixture cluster that must be re-replayed.
Traces: glue-workflow-definition-and#task#10 (also glue-workflow-lifecycle-and-duplicate-error#task#5).

Cause 8 of 10: UpdateDataQualityRuleset then GetDataQualityRuleset.
AWS: Smithy: GetDataQualityRuleset output members Description and Ruleset. After an update the read returns the NEW Description and the NEW Ruleset text supplied to the update.
Emulator: returns the original Ruleset and no Description at all — the update is discarded.
Why: a client that edits a ruleset and re-reads it sees its edit vanish.
Fix: UpdateDataQualityRuleset handler must write Description/Ruleset into state (see open stub issue '20261029T031345Z-glue-updatedataqualityruleset-stub.md'; this cause carries the wire step).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#15.

Cause 9 of 10: UpdatePartition (values + storage location + parameters) then GetPartition.
AWS: GetPartition returns the partition as last written: Partition.StorageDescriptor.Location and Partition.Parameters reflect the UpdatePartition input.
Emulator: returns the pre-update Location and omits Parameters entirely.
Why: a client that relocates a partition keeps reading the old S3 prefix and writes to the wrong place.
Fix: UpdatePartition handler must persist the supplied PartitionInput over stored state (see open stub issue '20261029T185330Z-glue-updatepartition-stub.md').
Traces: glue-database-table-partition-lifecycle#task#15.

Cause 10 of 10: UpdatePartition storage-location change then GetPartition (second fixture).
AWS: as cause 9 — the read reflects the updated StorageDescriptor.Location.
Emulator: returns the original Location.
Why: as cause 9.
Fix: same component as cause 9; separate cluster, separate replay.
Traces: glue-partition-update-reflected#task#5.

## Evidence
Replay of simparity-wire-aws-v1:
1. GetBlueprint: status expected 400, got 200; $.__type expected 'EntityNotFoundException' got None; $.Message expected 'Blueprint with given name not found.' got None.
2. GetDataQualityRuleset: expected 400, got 200 after 29 poll attempts; body still {'Name': ..., 'Ruleset': ..., 'CreatedOn': ..., 'LastModifiedOn': ...}.
3. GetFormType: expected 400, got 200; expected Message '{"Message":"Metadata form type FormTypeRetention0d613640 not found."}'.
4. GetIntegrationResourceProperty: expected 400, got 200 after 29 attempts; expected Message 'The target resource arn:aws:glue:...:database/... is not found.'
5. GetTrigger: expected 400, got 200 after 61 attempts (poll capped at 60s); body {'Trigger': {... 'State': 'CREATED' ...}}.
6. GetWorkflow: expected 400, got 200 after 61 attempts; body {'Workflow': {...}}.
7. GetWorkflow: expected 400, got 200; $.__type and $.Message absent.
8. GetDataQualityRuleset 200/200: $.Ruleset expected 'Rules = [ ColumnExists "id", IsComplete "id" ]' got 'Rules = [ ColumnExists "id" ]'; $.Description expected 'sim2real updated' got None.
9. GetPartition 200/200: $.Partition.StorageDescriptor.Location expected '.../dt=2024-01-01-updated/' got '.../dt=2024-01-01/'; $.Partition.Parameters expected present, got None.
10. GetPartition 200/200: $.Partition.StorageDescriptor.Location expected '.../relocated/year=2022/' got '.../year=2022/'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-blueprint-lifecycle#teardown#5, glue-data-quality-ruleset-lifecycle#teardown#5, glue-formtype-delete-then-not-found#task#4, glue-integration-resource-property-lifecycle#task#7, glue-job-trigger-lifecycle#teardown#9, glue-trigger-scheduled-with-workflow#teardown#12, glue-workflow-definition-and#task#10, glue-classifier-custom-entity-and-data-quality-ruleset#task#15, glue-database-table-partition-lifecycle#task#15, glue-partition-update-reflected#task#5.
3. Each fixture creates a resource, deletes or updates it, then reads it back; the read must refuse with EntityNotFoundException (deletes) or return the updated values (updates).
