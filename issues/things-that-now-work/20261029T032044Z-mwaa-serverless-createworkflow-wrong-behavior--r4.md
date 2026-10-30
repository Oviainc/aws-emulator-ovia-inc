Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# A CreateWorkflow ClientToken replay recomputes the answer from the workflow's CURRENT version instead of replaying the first answer

_Filed: 2026-10-29T03:20:44Z_
_Operation: CreateWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateWorkflow's `@idempotencyToken` ledger records only the workflow ARN, and the replay branch then builds its response from the LIVE workflow record rather than from the answer the first call gave. The version, revision and status a workflow reports are re-derived every time its active version changes (`Workflow::promote_newest_version`), so once an UpdateWorkflow has appended a second version — or a DeleteWorkflow has started a teardown — a repeated ClientToken with the identical create request answers a `WorkflowVersion`, `RevisionId`, `IsLatestVersion` and `WorkflowStatus` that the first create never returned, i.e. it reports a version that create did not create.

This contradicts the rule the handler states immediately above the replay block ("replays its first answer", "All this handler keeps is how it answers from the record") and the rule the shared ledger states ("answer from the record that first use left"). The sibling tokened operation does it the other way round and is unaffected: `IdempotentResult::StartWorkflowRun` keeps the `run_id`, so its replay answers the very run the first use produced. `IdempotentResult::CreateWorkflow` keeps no version, so the first answer is not recoverable at all — the handler has no choice but to report whatever the workflow currently holds.

## Evidence

The stated rule, in the handler, directly above the code that breaks it — services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs:292-299:
```
// A repeated @idempotencyToken replays its first answer rather than minting a
// second workflow; the same token with a different request is a conflict. Both
// are the shared rule (`crate::state::TokenLedger`) ...
// All this handler keeps is how it answers from the record.
```
The replay, which answers from the live workflow and not from the record — create_workflow.rs:308-313:
```
if let Some(recorded) = replayed {
    let workflow = store
        .workflow_in(&region, recorded.workflow_arn())
        .ok_or_else(unavailable)?;
    return Ok(response_for(workflow));
}
```
What `response_for` reads — create_workflow.rs:529-540:
```
fn response_for(workflow: &crate::state::Workflow) -> CreateWorkflowOutput {
    CreateWorkflowOutput {
        ...
        revision_id: Some(workflow.revision_id.clone()),
        workflow_status: Some(workflow.status.clone()),
        workflow_version: Some(workflow.current_version.clone()),
        is_latest_version: Some(
            workflow
                .version(Some(&workflow.current_version))
                .is_some_and(|rec| rec.is_latest),
        ),
```
Those three fields are re-derived whenever a version is appended — src/state.rs:788-790:
```
        self.current_version = self.versions[newest].version.clone();
        self.code_snapshotted_at = self.versions[newest].created_at;
        self.revision_id = mint_revision_id(&self.arn, &self.current_version);
```
called by UpdateWorkflow — src/ops/update_workflow.rs:331-346:
```
        workflow.versions.push(crate::state::WorkflowVersionRecord { ... });
        ...
        workflow.promote_newest_version(now);
```
The ledger's own stated contract — src/state.rs:985-987 (`TokenLedger::replay`): "What the token this request carries already produced: `Some(record)` to replay that first answer from", and src/state.rs:944-947: "look the token up, refuse a first use whose request digests differently, otherwise answer from the record that first use left".
The sibling that DOES keep what it produced — src/state.rs:854-862:
```
    CreateWorkflow {
        request_fingerprint: String,
        workflow_arn: String,
    },
    StartWorkflowRun {
        request_fingerprint: String,
        workflow_arn: String,
        run_id: String,
    },
```
Test coverage: tests/all/idempotency_token_ledger.rs:142 is named `every_tokened_operation_replays_its_first_answer_and_refuses_a_changed_request`, but its replay assertion (line 187-199) repeats the call immediately and checks only the identity member (`replay[row.identity_member], identity`), so no test pins what the replay reports for `WorkflowVersion`/`RevisionId`/`WorkflowStatus` after the workflow has moved on.

## Steps to Reproduce

1. CreateWorkflow with Name="wf", a DefinitionS3Location, a RoleArn and ClientToken="T"; note the answered WorkflowVersion v1 and RevisionId r1.
2. UpdateWorkflow on that WorkflowArn with any configuration, which appends version v2 and promotes it (update_workflow.rs:331-346).
3. Repeat the EXACT same CreateWorkflow request again with ClientToken="T": the fingerprint matches, so the ledger replays.
4. Observe the replay answers WorkflowVersion=v2 and RevisionId=r2 (the version the update created) instead of the v1/r1 the first create answered, with IsLatestVersion describing v2.
5. Variant: after step 1, call DeleteWorkflow and repeat the create with ClientToken="T" inside the DELETING span — the replay answers WorkflowStatus=DELETING where the first create answered READY.
