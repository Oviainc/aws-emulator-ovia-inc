Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Launch Wizard emulator

_Filed: 2026-10-21T21:08:25Z_
_Operation: service-com-amazonaws-launchwizard-launchwizard | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-launchwizard-launchwizard/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Implement a FIRST stateful slice covering the deployment lifecycle of the primary resource (a Deployment, created against a workload/deployment-pattern catalog).

## Happy path to implement (restJson1, sigv4 name `launchwizard`)

1. **ListWorkloads** (`POST /listWorkloads`) — return the seeded workload catalog as `workloads: [WorkloadDataSummary]` (`workloadName`, `displayName`, `status`). Seed at least one realistic workload (e.g. `SAP`) at service construction so CreateDeployment has something valid to reference; honour `maxResults`/`nextToken` by slicing the seeded list, echoing no canned token when the page is the last one.
2. **CreateDeployment** (`POST /createDeployment`) — required inputs `workloadName`, `deploymentPatternName`, `name`, `specifications` (map 1..100), `dryRun` (default false); optional `tags`. Must (a) reject an unknown `workloadName`/`deploymentPatternName` with `ResourceNotFoundException` (404) resolved against the seeded catalog, (b) reject a duplicate deployment `name` with `ValidationException` (400), (c) when `dryRun` is true, validate and persist NOTHING, (d) otherwise mint a new `deploymentId` matching `^[a-zA-Z0-9-]+$` (len 2..128), store the deployment with `status = CREATING`, `createdAt`/`modifiedAt` = now, the submitted `specifications` and `tags`, a synthesized `deploymentArn` (`arn:aws:launchwizard:<region>:<account>:deployment/<id>`), and return only `deploymentId`. The id must be generated AND stored — a random id with no backing record is a failure.
3. **GetDeployment** (`POST /getDeployment`) — input `deploymentId`; return `deployment` (DeploymentData) read back from the store: `id`, `name`, `workloadName`, `patternName`, `status`, `createdAt`, `modifiedAt`, `specifications`, `tags`, `deploymentArn`, plus `deletedAt` once deleted. Unknown id ⇒ `ResourceNotFoundException` (404). Every field must come from what CreateDeployment persisted, not echoed from this request.
4. **ListDeployments** (`POST /listDeployments`) — return `deployments: [DeploymentDataSummary]` (`id`, `name`, `workloadName`, `patternName`, `status`, `createdAt`, `modifiedAt`) for all stored deployments. Apply `filters` (DeploymentFilter `name`/`values`, values OR-joined) and page with `maxResults` (1..100) + opaque `nextToken`; emit `nextToken` only when more items remain, and resume exactly where the previous page ended.
5. **DeleteDeployment** (`POST /deleteDeployment`) — input `deploymentId`; transition the stored record (e.g. to `DELETE_IN_PROGRESS` then `DELETED`, setting `deletedAt`) and return the real `status` (+ `statusReason` when applicable) read from state. Unknown id ⇒ `ResourceNotFoundException` (404). The operation is `@idempotent`: a repeat delete on an already-deleted deployment must not error differently or resurrect state. A subsequent GetDeployment/ListDeployments must observably reflect the deletion.

Optional sixth step if cheap: **ListDeploymentEvents** (`POST /listDeploymentEvents`) returning `deploymentEvents: [DeploymentEventDataSummary]` (`name`, `description`, `status`, `statusReason`, `timestamp`) from events appended by CreateDeployment/DeleteDeployment — so the event list grows as a real consequence of lifecycle calls rather than being canned.

Rules for the slice: NO canned responses, NO echoing the request back, NO id generated without a stored record. Every response field must be computed from persisted state under the `crate::state::service_state` lock, and must survive across requests in one process.

## State Model

```text
LaunchWizardState {
    // seeded, read-only catalog
    workloads: Map<workloadName -> Workload {
        workload_name: String,
        display_name: String,
        description: String,
        status: WorkloadStatus,
        patterns: Map<deploymentPatternName -> DeploymentPattern {
            pattern_name: String,
            display_name: String,
            workload_name: String,
            status: WorkloadDeploymentPatternStatus,
            // version names used by Get/ListDeploymentPatternVersions later
            versions: Vec<String>,
            specifications: Vec<DeploymentSpecificationsField>,
        }>,
    }>,

    // the authoritative mutable store
    deployments: Map<deployment_id -> Deployment {
        id: String,                 // ^[a-zA-Z0-9-]+$, 2..128
        name: String,               // unique across live deployments
        workload_name: String,
        pattern_name: String,
        status: DeploymentStatus,   // CREATING | IN_PROGRESS | COMPLETED | DELETE_IN_PROGRESS | DELETED | ...
        status_reason: Option<String>,
        specifications: Map<String,String>,   // sensitive
        resource_group: Option<String>,
        deployment_arn: String,
        tags: Map<String,String>,
        created_at: Timestamp,
        modified_at: Timestamp,
        deleted_at: Option<Timestamp>,
        events: Vec<DeploymentEvent { name, description, status, status_reason, timestamp, metadata }>,
    }>,

    names_in_use: Set<deployment_name>,   // for the duplicate-name ValidationException
    next_id_seq: u64,                     // deterministic id minting
}
```

Tag state should live on `Deployment.tags` keyed by `deployment_arn` so ListTagsForResource/TagResource/UntagResource can be layered on later without a second store.

Everything else (UpdateDeployment, GetWorkload, GetWorkloadDeploymentPattern, ListWorkloadDeploymentPatterns, Get/ListDeploymentPatternVersions, Tag/Untag/ListTagsForResource) stays at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-launchwizard-launchwizard/OPERATIONS.md` lists all 15 bound operations with their routes, input/output members and errors (CreateDeployment `POST /createDeployment` -> `deploymentId`; GetDeployment -> `deployment` (DeploymentData); ListDeployments `@paginated(inputToken=nextToken, outputToken=nextToken, items=deployments)`; DeleteDeployment -> `status` (DeploymentStatus enum incl. DELETED/DELETE_IN_PROGRESS); ListWorkloads `@paginated ... items=workloads`). `smithy_query(shape="DeploymentData,DeploymentDataSummary,DeploymentFilter,DeploymentEventDataSummary")` gives the exact persisted fields named in the State Model (id, name, workloadName, patternName, status, createdAt, modifiedAt, specifications, resourceGroup, deletedAt, tags, deploymentArn). `src/ops/create_deployment.rs` lines 4 and 181 show the scaffold header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDeployment"))`; every other file in `src/ops/` is the same generated stub, so the whole service answers 501 and stores nothing.

## Steps to Reproduce

1. Point an AWS SDK/CLI client at the emulator endpoint with service name `launchwizard`.
2. `aws launch-wizard list-workloads` — expect a non-empty seeded catalog containing a workload name usable below (today: HTTP 501 NotImplemented).
3. `aws launch-wizard create-deployment --workload-name <seeded> --deployment-pattern-name <seeded pattern> --name demo-deployment --specifications KeyName=my-key --no-dry-run` — expect 200 with a `deploymentId`.
4. `aws launch-wizard get-deployment --deployment-id <id>` — expect the deployment with name `demo-deployment`, the same workload/pattern, the specifications submitted, a non-null createdAt and a deploymentArn.
5. `aws launch-wizard list-deployments --max-results 1` — expect the deployment in `deployments`; with more than one deployment created, expect a `nextToken` and a second page that returns the remaining item(s) and no token.
6. `aws launch-wizard create-deployment ... --name demo-deployment` again — expect 400 ValidationException (duplicate name), and with a bogus `--workload-name` expect 404 ResourceNotFoundException.
7. `aws launch-wizard create-deployment ... --dry-run` — expect no new deployment to appear in list-deployments.
8. `aws launch-wizard delete-deployment --deployment-id <id>` — expect a real DeploymentStatus (DELETE_IN_PROGRESS/DELETED); repeat the call and expect idempotent behaviour.
9. `aws launch-wizard get-deployment --deployment-id <id>` after deletion — expect the deleted state (status DELETED with deletedAt) or 404, consistently with delete, and `get-deployment --deployment-id doesnotexist` to be 404 ResourceNotFoundException.
