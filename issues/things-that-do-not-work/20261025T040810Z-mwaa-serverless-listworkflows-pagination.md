Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Scope the ListWorkflows page cursor to the regional endpoint that issued it

_Filed: 2026-10-25T04:08:10Z_
_Operation: ListWorkflows | Class: pagination_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListWorkflows mints its cursor from the ARN of the last workflow on the page and resumes with `workflows_in_after(region, Some(cursor_arn))`, which walks the GLOBAL ARN-ordered map from that ARN and only then filters by the caller's region. The cursor carries no record of the endpoint it was issued at, so a cursor minted at one regional endpoint and presented at another silently corrupts the walk instead of being refused: because a `WorkflowArn` embeds its region, an `eu-central-1` endpoint given a `us-east-1` cursor starts past every `eu-central-1` ARN (`...:airflow-serverless:eu-...` sorts before `...:airflow-serverless:us-...`) and answers 200 with an empty list, while a `us-east-1` endpoint given an `eu-central-1` cursor restarts the walk from the beginning and re-serves rows the client already consumed.

Every other region-sensitive key in this crate carries the region explicitly (the idempotency ledger key), and the sibling ListWorkflowVersions refuses a cursor that was not issued for the walk it is replayed on. This listing is the one that neither scopes nor refuses.

## Evidence

The crate's own partitioning rule — `src/state.rs:11-15`:
```
//! The store is PARTITIONED BY REGION: a workflow belongs to the endpoint it was
//! created through ([`request_region`], recorded as [`Workflow::region`]), and every
//! lookup, listing, name-uniqueness check and idempotency token is scoped to the
//! region of the calling request, because a regional AWS endpoint serves only the
//! resources created through it.
```
How a region-scoped key is built elsewhere in the crate — `src/ops/create_workflow.rs:426-428`:
```
fn token_key(region: &str, client_token: &str) -> String {
    format!("CreateWorkflow:{region}:{client_token}")
}
```
The cursor this handler mints and honours carries no region/endpoint scope — `src/ops/list_workflows.rs:190-201` and `227-233`:
```
        // The cursor is the ARN of the last item on the previous page, so a workflow
        // deleted between pages cannot restart or skip the walk.
        let after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
```
```
        let next_token = match workflows.last() {
            Some(last) if page.next().is_some() => Some(crate::state::encode_page_token(
                PAGE_KIND,
                &last.workflow_arn,
            )),
```
and the resume walk filters region only AFTER seeking past the cursor — `src/state.rs:634-642`:
```
        let lower = match after {
            Some(last) => std::ops::Bound::Excluded(last),
            None => std::ops::Bound::Unbounded,
        };
        self.workflows
            .range::<str, _>((lower, std::ops::Bound::Unbounded))
            .map(|(_, workflow)| workflow)
            .filter(move |workflow| workflow.region == region)
```
The sibling that does refuse an out-of-scope cursor — `src/ops/list_workflow_versions.rs:240-245` ("A cursor minted while walking ANOTHER workflow's versions was not issued for this walk ... honouring it would silently restart or truncate the listing"). Test coverage: `tests/all/workflow_lifecycle.rs:644` exercises foreign NextToken shapes and `tests/all/region_isolation.rs` exercises per-region lookups, but no test presents a ListWorkflows cursor at a second regional endpoint.

## Steps to Reproduce

1. Create 2 workflows through the eu-central-1 endpoint and 2 through the us-east-1 endpoint of the same emulator.
2. Call ListWorkflows signed for us-east-1 with MaxResults=1 and keep the NextToken (its cursor is a us-east-1 ARN).
3. Call ListWorkflows signed for eu-central-1 with MaxResults=10 and that NextToken.
4. Expected: the cursor is refused (400 ValidationException), the rule ListWorkflowVersions applies to a cursor not issued for the walk.
5. Actual: 200 with an empty `Workflows` list — both eu-central-1 workflows are skipped because their ARNs sort before the us-east-1 cursor. Repeating the experiment with the roles reversed (an eu-central-1 cursor at the us-east-1 endpoint) instead restarts the walk and re-serves already-paged workflows.
