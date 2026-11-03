Issue-Source: trace-sweep-divergence
Issue-Trace: glue-classifier-custom-entity-and-data-quality-ruleset
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-classifier-custom-entity-and-data-quality-ruleset glue-empty-resource-listings-are-empty-not-errors
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|UpdateDataQualityRuleset|2xx->2xx|$.CreatedOn
Issue-Key: glue|UpdateDataQualityRuleset|2xx->2xx|$.Identifier
Issue-Key: glue|UpdateDataQualityRuleset|2xx->2xx|$.LastModifiedOn
Issue-Key: glue|UpdateDataQualityRuleset|2xx->2xx|$.TargetTable
Issue-Key: glue|ListWorkflows|2xx->2xx|$
Issue-Commit: e63ec425c23513ba26a0bb6d325f047894e211cc

# Glue list and update responses omit output members entirely

> _Note: this issue was identified against the code base at commit `e63ec425c23513ba26a0bb6d325f047894e211cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataQualityRuleset

## Description
Two net-new root causes in which a Glue operation answers HTTP 200 but serializes a response body that drops output members the real service always sends, so a client sees an absent member where AWS sends a value. Both are serialization/projection gaps on the success path, not status or error-code gaps. 2 causes; both must be fixed before this issue is closed.

Cause 1 of 2 — UpdateDataQualityRuleset returns only the echoed request fields, dropping the ruleset's identity and timestamps.
AWS: Smithy UpdateDataQualityRuleset (glue, awsJson1.1) models output UpdateDataQualityRulesetResponse{Name, Description, Ruleset}; the recorded AWS response additionally carries Identifier, TargetTable, CreatedOn and LastModifiedOn, i.e. the full stored ruleset projection (the same members GetDataQualityRuleset models: TargetTable: DataQualityTargetTable, CreatedOn/LastModifiedOn: Timestamp). Those four members are recorded behavior only — the model does not bind Identifier at all — so the requirement is taken from the recording: an update of an existing ruleset answers 200 and reports the ruleset's identifier, its target table and both timestamps alongside the updated Name/Description/Ruleset. The values are per-resource: Identifier is the ruleset's own identifier, TargetTable is the DatabaseName/TableName the ruleset was created against, CreatedOn is the creation time and LastModifiedOn the time of this update.
Emulator: answers 200 with Identifier, TargetTable, CreatedOn and LastModifiedOn all absent (null) — only the echoed Name/Description/Ruleset are present.
Why: a client that updates a ruleset and then uses the response to address or cache the resource (its identifier, its target table, its modification time) gets nulls, so it cannot correlate the update with the ruleset it just changed and must re-read it; code that reads CreatedOn/LastModifiedOn to order revisions sees no timestamps at all.
Fix: the UpdateDataQualityRuleset handler and the response projection it builds from the stored data-quality-ruleset record in the glue service crate — the stored record already holds the identifier, target table and timestamps (GetDataQualityRuleset and CreateDataQualityRuleset share the same projection gap, tracked separately by the open issues 20261029T201859Z-glue-delete-and-update-mutations-are-never-applied.md and 20261029T201859Z-glue-reads-report-stale-or-wrong-resource-state.md; neither carries this Update step's replay key).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#14.

Cause 2 of 2 — ListWorkflows answers with an empty JSON object instead of a response carrying the Workflows member.
AWS: Smithy ListWorkflows (glue) models output ListWorkflowsResponse{Workflows: WorkflowNames, NextToken} and is @paginated with items=Workflows, inputToken/outputToken=NextToken, pageSize=MaxResults. The recorded AWS response for an account with no workflows is 200 with the Workflows member PRESENT (an empty list of names) — an empty listing is an empty collection, not an omitted member and not an error. NextToken is absent when there is no further page.
Emulator: answers 200 with the body `{}` — the Workflows member is absent entirely.
Why: paginators and SDK result wrappers that iterate `response.Workflows` get null instead of an empty list, so a caller enumerating workflows either throws on a null collection or cannot distinguish "no workflows" from "member not returned"; the same shape makes the first page of a populated listing indistinguishable in structure from a broken one.
Fix: the ListWorkflows handler/serializer in the glue service crate — the empty-result path must still emit the Workflows list member (and omit NextToken only when there is no next page) rather than skipping empty collections during serialization. Note this is the list-pagination surface, distinct from the open issue 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md, which does not carry this step's replay key.
Traces: glue-empty-resource-listings-are-empty-not-errors#task#11.

## Evidence
Cause 1 of 2 — fixture glue-classifier-custom-entity-and-data-quality-ruleset, step task#14, operation UpdateDataQualityRuleset.
Expected (recorded AWS): HTTP 200 with $.Identifier, $.TargetTable ({DatabaseName, TableName} of the ruleset's target table), $.CreatedOn and $.LastModifiedOn all present.
Actual (emulator): HTTP 200; $.CreatedOn missing (got None); $.Identifier missing (got None); $.LastModifiedOn missing (got None); $.TargetTable missing (got None). Status matched (200 -> 200); only the body diverged.
Replay divergence key: service=glue|UpdateDataQualityRuleset|status=200->200|keys=$.CreatedOn,$.Identifier,$.LastModifiedOn,$.TargetTable. Cluster had no owning issue and no blocking stub.

Cause 2 of 2 — fixture glue-empty-resource-listings-are-empty-not-errors, step task#11, operation ListWorkflows.
Expected (recorded AWS): HTTP 200 with the Workflows member present (empty list for an account with no workflows).
Actual (emulator): HTTP 200 with body {} — empty_body, Workflows absent.
Replay divergence key: service=glue|ListWorkflows|status=200->200|keys=$. Cluster had no owning issue and no blocking stub.

Note on values: the fixture's concrete ruleset name, identifier, database/table names and epoch timestamps are per-run values that fill the response template; the requirement is that each member is PRESENT and derived from the request and the emulator's stored state, not that it equals any recorded literal.

## Steps to Reproduce
Cause 1 of 2:
1. CreateDataQualityRuleset with a Name, a Description, a Ruleset expression and a TargetTable {DatabaseName, TableName} that exists in the catalog.
2. Call UpdateDataQualityRuleset with that Name and a changed Description/Ruleset.
3. Inspect the 200 response body: Identifier, TargetTable, CreatedOn and LastModifiedOn are absent; AWS returns all four. Replay fixture glue-classifier-custom-entity-and-data-quality-ruleset step task#14 to confirm.

Cause 2 of 2:
1. Against an account/region with no Glue workflows, call ListWorkflows with no NextToken.
2. Inspect the 200 response body: it is {} with no Workflows member; AWS returns Workflows as an empty list. Replay fixture glue-empty-resource-listings-are-empty-not-errors step task#11 to confirm.
