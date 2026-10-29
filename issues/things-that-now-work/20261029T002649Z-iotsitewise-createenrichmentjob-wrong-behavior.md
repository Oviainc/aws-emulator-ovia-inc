Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A different CreateEnrichmentJob reusing a spent clientToken replays the first job instead of the modelled conflict, so the second job is never created

_Filed: 2026-10-29T00:26:49Z_
_Operation: CreateEnrichmentJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEnrichmentJob resolves a reused `clientToken` with `enrichment_job_for_client_token(&workspace, token)`, which matches on workspace and token only. `jobConfiguration` — the whole description of the work the job does — is never compared, though the `EnrichmentJobRecord` stores it. A request that reuses a token with a DIFFERENT `jobConfiguration` (another dataset, another time series, another job type) is answered 200 with the first job's `jobId`, `status` and `createdAt`, creates nothing, and the work it asked for is silently never started. The member doc the handler itself quotes conditions the replay on the request being the same, and this crate's other creates make a different request under a spent token the modelled `ConflictingOperationException` (which this operation models, 409, and already constructs for duplicate jobs).

## Evidence

Handler — services/.../src/ops/create_enrichment_job.rs:225-235:
```
        // "If you submit the same request with the same token within the idempotency
        // window, you receive the original job details without creating a duplicate":
        ...
        if let Some(token) = input.client_token.as_deref() {
            if let Some(existing) =
                store.enrichment_job_for_client_token(&input.workspace_name, token)
            {
                return Ok(created(existing, now));
            }
        }
```
The quoted doc says "the same request with the same token", but the lookup compares only workspace + token — state.rs:4942-4946:
```
        self.enrichment_jobs.values().find(|record| {
            record.workspace_name == workspace_name
                && record.client_token.as_deref() == Some(client_token)
        })
```
The configuration is stored and therefore comparable — create_enrichment_job.rs:298-309: `configuration: input.job_configuration.clone(), client_token: input.client_token.clone()`.

The crate rule skipped — create_task.rs:193-198 ("a DIFFERENT request reusing it belongs to another call and is the modelled conflict rather than a second task"), implemented generically at state.rs:9338-9342; sibling create_application.rs:229-232 compares every member and refuses with `ConflictingOperationException`. CreateEnrichmentJob already builds that error for a duplicate job (create_enrichment_job.rs:258-266) and models it (smithy_query: `{"shape": "ConflictingOperationException", "httpError": 409}`).

## Steps to Reproduce

1. CreateWorkspace W and two datasets D1 and D2 in it.
2. CreateEnrichmentJob in W whose jobConfiguration targets D1, with clientToken T; note jobId J1.
3. CreateEnrichmentJob in W whose jobConfiguration targets D2, with the same clientToken T.
4. Observe 200 carrying jobId J1 (the D1 job) and no job for D2: ListEnrichmentJobs in W shows a single job, instead of the modelled ConflictingOperationException for a token already spent on another request.
