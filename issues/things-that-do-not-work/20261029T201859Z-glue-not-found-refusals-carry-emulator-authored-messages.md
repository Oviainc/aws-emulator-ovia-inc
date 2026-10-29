Issue-Source: trace-sweep-divergence
Issue-Trace: glue-blueprint-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-blueprint-error-paths glue-catalog-error-paths glue-trigger-error-paths glue-workflow-definition-and-batch-get-not-found glue-jdbc-connection-management glue-session-error-paths glue-session-statement-error-paths glue-trigger-scheduled-with-workflow glue-workflow-trigger-lifecycle glue-table-versioning-and-column-statistics glue-data-quality-ruleset-errors glue-database-update-and-table-batch-delete
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetBlueprint|4xx->4xx|$.Message
Issue-Key: glue|GetCatalog|4xx->4xx|$.Message
Issue-Key: glue|GetTrigger|4xx->4xx|$.Message
Issue-Key: glue|GetWorkflow|4xx->4xx|$.Message
Issue-Key: glue|DeleteConnection|4xx->4xx|$.Message
Issue-Key: glue|GetStatement|4xx->4xx|$.Message
Issue-Key: glue|GetWorkflowRunProperties|4xx->4xx|$.Message
Issue-Key: glue|GetTableVersion|4xx->4xx|$.Message
Issue-Key: glue|GetDataQualityRuleset|4xx->4xx|$.Message
Issue-Key: glue|BatchDeleteTableVersion|2xx->2xx|$.Errors[*].ErrorDetail.ErrorMessage
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue not-found refusals carry emulator-authored messages instead of AWS's text

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprint

## Description
This issue bundles 10 net-new causes, all the same fault: the emulator returns the right HTTP status and the right error code for a missing Glue entity, but re-words the error MESSAGE. In every cause the emulator interpolates the requested resource name into text of its own, where real Glue returns a fixed sentence (or interpolates a different variable). The error message is part of the contract, so each operation needs its own AWS text.

Cause 1 of 10: GetBlueprint, unknown blueprint name.
AWS: Smithy: GetBlueprint error EntityNotFoundException (errorKind client, httpError 400). HTTP 400, __type EntityNotFoundException, Message is the fixed text 'Blueprint with given name not found.' with NO variable part — the requested name is not echoed.
Emulator: HTTP 400 / EntityNotFoundException, Message 'Blueprint <name> not found.' where <name> is the Name from the request.
Why: a client matching on the message (or a snapshot test over the refusal body) sees text AWS never emits, and infers the service leaks the requested name.
Fix: the GetBlueprint handler's not-found refusal path (message construction).
Traces: glue-blueprint-error-paths#task#1.

Cause 2 of 10: GetCatalog, unknown catalog id.
AWS: Smithy: GetCatalog error EntityNotFoundException (client, 400). HTTP 400, Message is the fixed text 'Catalog not found.' — no variable part.
Emulator: Message 'Catalog <catalogId> not found.', interpolating the CatalogId from the request.
Why: same as cause 1.
Fix: GetCatalog handler's not-found refusal path.
Traces: glue-catalog-error-paths#task#1.

Cause 3 of 10: GetTrigger, unknown trigger name.
AWS: Smithy: EntityNotFoundException (client, 400). Message is the fixed text 'Trigger not found.'
Emulator: Message 'Trigger <name> not found.'
Why: same as cause 1.
Fix: GetTrigger handler's not-found refusal path.
Traces: glue-trigger-error-paths#task#3 (also observed at task#6 of the same fixture).

Cause 4 of 10: GetWorkflow, unknown workflow name.
AWS: Smithy: EntityNotFoundException (client, 400). Message is the fixed text 'Entity not found' — no trailing period and no name.
Emulator: Message 'Workflow <name> not found.'
Why: same as cause 1.
Fix: GetWorkflow handler's not-found refusal path.
Traces: glue-workflow-definition-and-batch-get-not-found#task#4.

Cause 5 of 10: DeleteConnection, unknown connection name.
AWS: Smithy: DeleteConnection error EntityNotFoundException (client, 400). Message is the fixed text 'Connection not found' — no trailing period, no name.
Emulator: Message 'Connection not found: <name>'.
Why: same as cause 1.
Fix: DeleteConnection handler's not-found refusal path.
Traces: glue-jdbc-connection-management#teardown#13 (also teardown#14).

Cause 6 of 10: GetStatement against a session id that does not exist.
AWS: Smithy: EntityNotFoundException (client, 400). Message template 'Session ID <sessionId> not found' — fixed text 'Session ID ' + the SessionId from the request + ' not found', with no trailing period.
Emulator: Message 'Session <sessionId> not found.' — drops 'ID' and adds a period.
Why: same as cause 1.
Fix: GetStatement handler's session-lookup refusal path.
Traces: glue-session-error-paths#task#3 (also glue-session-statement-error-paths#task#3).

Cause 7 of 10: GetWorkflowRunProperties for a run id that does not exist.
AWS: Smithy: EntityNotFoundException (client, 400). Message is the fixed text 'Entity not found'.
Emulator: Message 'Workflow run <runId> not found for workflow <workflowName>.' — emulator-authored sentence built from the request.
Why: same as cause 1.
Fix: GetWorkflowRunProperties handler's not-found refusal path.
Traces: glue-trigger-scheduled-with-workflow#task#8 (also glue-workflow-trigger-lifecycle#task#8).

Cause 8 of 10: GetTableVersion for a version id that does not exist.
AWS: Smithy: EntityNotFoundException (client, 400). Message is the fixed text 'Version not found.'
Emulator: Message 'Table version not found.'
Why: same as cause 1.
Fix: GetTableVersion handler's not-found refusal path.
Traces: glue-table-versioning-and-column-statistics#task#8.

Cause 9 of 10: GetDataQualityRuleset for an unknown ruleset.
AWS: Smithy: GetDataQualityRuleset error EntityNotFoundException (client, 400). Message template 'Cannot find Data Quality Ruleset in account <accountId> with name <name>' — fixed text with the caller's account id and the requested Name substituted, no trailing period.
Emulator: Message 'Data quality ruleset with name <name> does not exist' — different fixed text and no account id.
Why: same as cause 1.
Fix: GetDataQualityRuleset handler's not-found refusal path.
Traces: glue-data-quality-ruleset-errors#task#2.

Cause 10 of 10: BatchDeleteTableVersion per-item error text.
AWS: the operation answers 200 and reports failures in Errors[].ErrorDetail. For a missing version the ErrorCode is EntityNotFoundException and ErrorMessage follows the template 'Table version (<versionId>) not found' — fixed text with the VersionId from the request in parentheses, no trailing period.
Emulator: ErrorMessage 'Version not found.'
Why: a batch caller cannot tell which version failed, because the id AWS puts in the message is gone.
Fix: BatchDeleteTableVersion handler's per-item error serializer.
Traces: glue-database-update-and-table-batch-delete#task#12.

Related open issue (does NOT cover these): '20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md' reports ten other emulator-authored refusal messages (GetTable/GetDatabase, GetDashboardUrl, ...). The ten causes above are different operations and are not named there.

## Evidence
Replay of simparity-wire-aws-v1 (121 fixtures, 906 steps, 544 passed). Per-cause expected vs actual, all with matching status/__type and a diverging $.Message:
1. GetBlueprint 400/400: expected 'Blueprint with given name not found.' got 'Blueprint sim2real-probe-glue-blueprint-missing-46737b3e not found.'
2. GetCatalog 400/400: expected 'Catalog not found.' got 'Catalog sim2real-probe-glue-subcatalog-missing-88c986cd not found.'
3. GetTrigger 400/400: expected 'Trigger not found.' got 'Trigger sim2real-probe-glue-glue-trg-missing-baf1254c not found.'
4. GetWorkflow 400/400: expected 'Entity not found' got 'Workflow sim2real-probe-glue-sim2real-probe-wf-neverexisted-b7cd7f32 not found.'
5. DeleteConnection 400/400: expected 'Connection not found' got 'Connection not found: sim2real-probe-glue-sim2real-probe-conn-08fe166e'
6. GetStatement 400/400: expected 'Session ID sim2real-probe-glue-session-missing-b114dae6 not found' got 'Session sim2real-probe-glue-session-missing-b114dae6 not found.'
7. GetWorkflowRunProperties 400/400: expected 'Entity not found' got 'Workflow run wr_nonexistentrunid00000000000000 not found for workflow sim2real-probe-glue-workflow-16483f15.'
8. GetTableVersion 400/400: expected 'Version not found.' got 'Table version not found.'
9. GetDataQualityRuleset 400/400: expected 'Cannot find Data Quality Ruleset in account 000000000000 with name sim2real-probe-glue-no-such-dqruleset-cbfaa253' got 'Data quality ruleset with name sim2real-probe-glue-no-such-dqruleset-cbfaa253 does not exist'
10. BatchDeleteTableVersion 200/200: $.Errors[0].ErrorDetail.ErrorMessage expected 'Table version (0) not found' got 'Version not found.'
(The names/ids above are that run's values; the requirement is the fixed text plus the named variable parts.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Re-replay the listed fixtures/steps: glue-blueprint-error-paths#task#1, glue-catalog-error-paths#task#1, glue-trigger-error-paths#task#3, glue-workflow-definition-and-batch-get-not-found#task#4, glue-jdbc-connection-management#teardown#13, glue-session-error-paths#task#3, glue-trigger-scheduled-with-workflow#task#8, glue-table-versioning-and-column-statistics#task#8, glue-data-quality-ruleset-errors#task#2, glue-database-update-and-table-batch-delete#task#12.
3. Each step issues the named operation against a resource that does not exist and asserts the refusal body; compare $.Message against the AWS text quoted per cause.
