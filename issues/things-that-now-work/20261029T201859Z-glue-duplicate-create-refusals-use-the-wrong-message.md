Issue-Source: trace-sweep-divergence
Issue-Trace: glue-blueprint-duplicate-create-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-blueprint-duplicate-create-error glue-blueprint-error-paths glue-classifier-custom-entity-and-data-quality-ruleset glue-classifier-errors glue-custom-entity-type-errors glue-custom-entity-type-workflow glue-data-quality-ruleset-errors glue-glossary-idempotent-create-and-invalid-delete glue-jdbc-connection-management glue-job-duplicate-create-error glue-job-trigger-error-paths glue-schema-registry-versioning glue-workflow-definition-and-batch-get-not-found glue-workflow-lifecycle-and-duplicate-error
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateBlueprint|4xx->4xx|$.Message
Issue-Key: glue|CreateClassifier|4xx->4xx|$.Message
Issue-Key: glue|CreateCustomEntityType|4xx->4xx|$.Message
Issue-Key: glue|CreateDataQualityRuleset|4xx->4xx|$.Message
Issue-Key: glue|CreateGlossary|4xx->4xx|$.Message
Issue-Key: glue|CreateConnection|4xx->4xx|$.Message
Issue-Key: glue|CreateJob|4xx->4xx|$.Message
Issue-Key: glue|CreateJob|4xx->4xx|$.__type
Issue-Key: glue|CreateSchema|4xx->4xx|$.Message
Issue-Key: glue|CreateWorkflow|4xx->4xx|$.Message
Issue-Key: glue|DeleteClassifier|4xx->4xx|$.Message
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue duplicate-create refusals use the wrong message text and one wrong error code

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBlueprint

## Description
This issue bundles 10 net-new causes. Nine are duplicate-create refusals where the emulator emits one generic '<Resource> already exists.' sentence for every resource type instead of the per-operation text real Glue returns (several of which include the resource name, and one of which is a JSON-wrapped body); the tenth is the same message-composition defect producing a doubled 'not found. not found.' Cause 7 additionally picks the wrong modeled error code, which is a behavioural break, so this bundle is HIGH.

Cause 1 of 10: CreateBlueprint with a name that already exists.
AWS: Smithy: CreateBlueprint error AlreadyExistsException (client, 400). HTTP 400, Message is the fixed text 'Blueprint with given name already exists' (no trailing period, no name).
Emulator: Message 'Blueprint already exists.'
Why: clients and snapshot assertions keyed on AWS's sentence do not match.
Fix: CreateBlueprint handler's duplicate-name refusal path.
Traces: glue-blueprint-duplicate-create-error#task#2 (also glue-blueprint-error-paths#task#3).

Cause 2 of 10: CreateClassifier with an existing classifier name.
AWS: Smithy: AlreadyExistsException (client, 400). Message template '<classifierName> already exists. already exists' — fixed text that embeds the requested classifier name and (in real Glue) repeats the 'already exists' suffix.
Emulator: Message 'Classifier already exists.'
Why: the caller loses the name that collided.
Fix: CreateClassifier handler's duplicate refusal path.
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#4 (also glue-classifier-errors#task#3).

Cause 3 of 10: CreateCustomEntityType duplicate.
AWS: Smithy: AlreadyExistsException (client, 400). Message is the fixed text 'A Custom Entity Type with the same name exist in this account'.
Emulator: Message 'Custom entity type already exists.'
Why: as above.
Fix: CreateCustomEntityType handler's duplicate refusal path.
Traces: glue-custom-entity-type-errors#task#3 (also glue-custom-entity-type-workflow#task#3).

Cause 4 of 10: CreateDataQualityRuleset duplicate.
AWS: Smithy: CreateDataQualityRuleset error AlreadyExistsException (client, 400). Message template 'Another ruleset with the same name already exists: <name>' with the requested Name appended.
Emulator: Message 'Data quality ruleset already exists.'
Why: as above.
Fix: CreateDataQualityRuleset handler's duplicate refusal path.
Traces: glue-data-quality-ruleset-errors#task#3.

Cause 5 of 10: CreateGlossary duplicate.
AWS: Smithy: CreateGlossary error AlreadyExistsException (client, 400). The metadata-catalog operations wrap their message as a JSON document: Message is the literal string '{"Message":"A glossary with name '<name>' already exists."}' — fixed JSON envelope plus the requested glossary name in single quotes.
Emulator: Message 'Glossary already exists.' — plain sentence, no JSON envelope.
Why: a client that parses the nested JSON envelope (as AWS's metadata-catalog APIs require) fails to extract any message.
Fix: CreateGlossary handler's duplicate refusal path plus the metadata-catalog error serializer that builds the JSON-wrapped Message.
Traces: glue-glossary-idempotent-create-and-invalid-delete#task#2.

Cause 6 of 10: CreateConnection duplicate.
AWS: Smithy: AlreadyExistsException (client, 400). Message is the fixed text 'Connection already exists.' with NO name.
Emulator: Message 'Connection already exists: <name>' — adds the name AWS omits (the inverse of the other causes).
Why: as above.
Fix: CreateConnection handler's duplicate refusal path.
Traces: glue-jdbc-connection-management#task#4.

Cause 7 of 10: CreateJob re-submitted for an existing job name with a different configuration — WRONG ERROR CODE.
AWS: Smithy: CreateJob errors include both AlreadyExistsException and IdempotentParameterMismatchException (both client, httpError 400). Re-creating an existing job with a different configuration is the idempotency-mismatch case: __type IdempotentParameterMismatchException, Message the fixed text 'Job already submitted with different configuration.'
Emulator: __type AlreadyExistsException, Message 'Job already exists: <name>'.
Why: a client that retries on AlreadyExistsException but surfaces IdempotentParameterMismatchException as a configuration bug (or vice versa) branches the wrong way; the error code is the contract, not just the text.
Fix: CreateJob handler's conflict classification (compare submitted configuration against stored job before choosing the error) plus its message.
Traces: glue-job-duplicate-create-error#task#3 (also glue-job-trigger-error-paths#task#3). Related open issue '20261029T185330Z-glue-9-net-new-root-causes-in-control.md' mentions CreateJob idempotency but does not carry this fixture step.

Cause 8 of 10: CreateSchema duplicate.
AWS: Smithy: AlreadyExistsException (client, 400). Message template 'Schema already exists. SchemaName: <schemaName>' with the requested SchemaName appended.
Emulator: Message 'Schema already exists.' — the SchemaName clause is dropped.
Why: as above.
Fix: CreateSchema handler's duplicate refusal path.
Traces: glue-schema-registry-versioning#task#5.

Cause 9 of 10: CreateWorkflow duplicate.
AWS: Smithy: AlreadyExistsException (client, 400). Message is the fixed text 'Workflow with given name already Exists' (note AWS's capital 'E', no trailing period).
Emulator: Message 'Workflow already exists.'
Why: as above.
Fix: CreateWorkflow handler's duplicate refusal path.
Traces: glue-workflow-definition-and-batch-get-not-found#task#2 (also glue-workflow-lifecycle-and-duplicate-error#task#3).

Cause 10 of 10: DeleteClassifier for a classifier that no longer exists — malformed, doubled message.
AWS: Smithy: DeleteClassifier error EntityNotFoundException (client, 400). Message template '<accountId>:<classifierName> not found.' — the account id and classifier name joined by a colon, then the fixed suffix ' not found.' once.
Emulator: Message 'Classifier <classifierName> not found. not found.' — the emulator appends its fixed suffix to a string that already ends in it, so the sentence is emitted twice, and the account-id qualifier is missing.
Why: the body is not text AWS ever returns and reads as a formatting bug to any consumer.
Fix: DeleteClassifier handler's not-found refusal path (stop concatenating a pre-formatted sentence with the suffix).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#21.

## Evidence
Replay of simparity-wire-aws-v1. Expected vs actual per cause (status 400 on both sides unless noted):
1. CreateBlueprint $.Message: expected 'Blueprint with given name already exists' got 'Blueprint already exists.'
2. CreateClassifier $.Message: expected 'Classifier sim2real-probe-glue-sim2real-probe-csv-949b6a30 already exists. already exists' got 'Classifier already exists.'
3. CreateCustomEntityType $.Message: expected 'A Custom Entity Type with the same name exist in this account' got 'Custom entity type already exists.'
4. CreateDataQualityRuleset $.Message: expected 'Another ruleset with the same name already exists: sim2real-probe-glue-dup-dqruleset-cbfaa253' got 'Data quality ruleset already exists.'
5. CreateGlossary $.Message: expected '{"Message":"A glossary with name 'sim2real-probe-glue-glossary-idem-68a30bc9' already exists."}' got 'Glossary already exists.'
6. CreateConnection $.Message: expected 'Connection already exists.' got 'Connection already exists: sim2real-probe-glue-sim2real-probe-conn-08fe166e'
7. CreateJob $.__type: expected 'IdempotentParameterMismatchException' got 'AlreadyExistsException'; $.Message: expected 'Job already submitted with different configuration.' got 'Job already exists: sim2real-probe-glue-job-dup-4b316450'
8. CreateSchema $.Message: expected 'Schema already exists. SchemaName: sim2real-probe-glue-sim2real-probe-schema-9a546660' got 'Schema already exists.'
9. CreateWorkflow $.Message: expected 'Workflow with given name already Exists' got 'Workflow already exists.'
10. DeleteClassifier $.Message: expected 'Classifier 000000000000:sim2real-probe-glue-sim2real-probe-csv-949b6a30 not found.' got 'Classifier sim2real-probe-glue-sim2real-probe-csv-949b6a30 not found. not found.'
(Names/ids shown are that run's values; the requirement is the fixed text plus the named variable parts.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-blueprint-duplicate-create-error#task#2, glue-classifier-custom-entity-and-data-quality-ruleset#task#4, glue-custom-entity-type-errors#task#3, glue-data-quality-ruleset-errors#task#3, glue-glossary-idempotent-create-and-invalid-delete#task#2, glue-jdbc-connection-management#task#4, glue-job-duplicate-create-error#task#3, glue-schema-registry-versioning#task#5, glue-workflow-definition-and-batch-get-not-found#task#2, glue-classifier-custom-entity-and-data-quality-ruleset#task#21.
3. Each step creates a resource, creates it again (or deletes an absent one) and asserts the refusal body; compare $.__type and $.Message with the AWS values quoted per cause.
