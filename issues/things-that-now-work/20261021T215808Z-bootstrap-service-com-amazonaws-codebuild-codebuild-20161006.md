Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CodeBuild emulator

_Filed: 2026-10-21T21:58:08Z_
_Operation: service-com-amazonaws-codebuild-codebuild-20161006 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 59 CodeBuild operations in `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST, SMALL stateful slice: the project -> build happy path of the primary resources (Project and Build), over the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: CodeBuild_20161006.<Operation>`).

## Happy path to implement (in order)

1. **CreateProject** — input `name` (required, 2..150, `^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`), `source` (ProjectSource, required), `artifacts` (ProjectArtifacts, required), `environment` (ProjectEnvironment, required), `serviceRole` (required), plus optional `description`, `timeoutInMinutes` (5..2160), `queuedTimeoutInMinutes` (5..480), `tags`, `cache`, `encryptionKey`, `badgeEnabled`. Output `project` (Project) must be the STORED record: `name`, generated `arn` (`arn:aws:codebuild:<region>:<account>:project/<name>`), echoed-and-persisted `source`/`artifacts`/`environment`/`serviceRole`/`description`/`tags`, `created` and `lastModified` timestamps, and defaults applied (`timeoutInMinutes` 60 when absent). A second CreateProject with the same `name` must fail with `ResourceAlreadyExistsException` (HTTP 400); a name violating the length/pattern constraint must fail with `InvalidInputException` (HTTP 400).
2. **BatchGetProjects** — input `names` (list, 1..100, required). Output `projects` must be the persisted Project structures for names that exist and `projectsNotFound` must list the names that do not. It must reflect exactly what CreateProject stored (no echo of request, no canned Project).
3. **StartBuild** — input `projectName` (required); honor at least `sourceVersion`, `environmentVariablesOverride`, `buildspecOverride`, `timeoutInMinutesOverride`, `idempotencyToken`. Unknown project -> `ResourceNotFoundException` (HTTP 400). On success create and persist a Build and return it as `build`: `id` = `<projectName>:<uuid>`, `arn` = `arn:aws:codebuild:<region>:<account>:build/<id>`, `buildNumber` monotonically incremented per project starting at 1, `projectName`, `startTime`, `buildStatus` = `IN_PROGRESS`, `currentPhase` = `QUEUED`/`SUBMITTED`, `buildComplete` = false, `initiator`, and `source`/`artifacts`/`environment`/`serviceRole`/`timeoutInMinutes` resolved from the project with the overrides applied.
4. **BatchGetBuilds** — input `ids` (list, 1..100, required). Output `builds` = the persisted Build records for ids that exist (same `id`, `buildNumber`, `projectName`, `buildStatus`, `startTime` as StartBuild returned), `buildsNotFound` = the rest. A build may advance from `IN_PROGRESS` to a terminal status (e.g. `SUCCEEDED`, with `endTime`, `buildComplete` = true, and `phases` populated) deterministically from stored state; it must never be re-fabricated per call.
5. **ListBuildsForProject** — input `projectName` (required), `sortOrder` (ASCENDING|DESCENDING), `nextToken`. Output `ids` must be the ids of the builds persisted for that project, ordered by build number per `sortOrder` (DESCENDING default, as on real CodeBuild), with `nextToken` paging the stored list. Unknown project -> `ResourceNotFoundException`. (`ListProjects` should list the stored project names the same way.)
6. **StopBuild** — input `id` (required). Unknown id -> `ResourceNotFoundException`. Must MUTATE the stored build: `buildStatus` = `STOPPED`, `buildComplete` = true, `endTime` set, and return the updated `build`; a following BatchGetBuilds must show `STOPPED`.
7. **DeleteProject** — input `name` (required). Must remove the project from the store (empty output). A following BatchGetProjects for that name must report it in `projectsNotFound`. Deleting an absent project is a no-op success, matching the real API.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back without storing it, and no generating an id/arn that is never recorded — those are failures, not implementations. All other CodeBuild operations (fleets, report groups, reports, webhooks, sandboxes, batch builds, source credentials, resource policies) stay at 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsCodebuildCodebuild20161006State`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `projects: BTreeMap<String /*project name*/, Project>` where `Project { name, arn, description, source: ProjectSource, secondary_sources, source_version, artifacts: ProjectArtifacts, secondary_artifacts, cache, environment: ProjectEnvironment, service_role, timeout_in_minutes, queued_timeout_in_minutes, encryption_key, tags, vpc_config, logs_config, badge, concurrent_build_limit, project_visibility, created: SystemTime, last_modified: SystemTime }`
- `builds: BTreeMap<String /*build id*/, Build>` where `Build { id, arn, build_number: i64, project_name, start_time, end_time: Option<_>, current_phase, build_status: StatusType, build_complete: bool, source_version, resolved_source_version, source, artifacts, cache, environment (after overrides), service_role, timeout_in_minutes, queued_timeout_in_minutes, initiator, phases: Vec<BuildPhase>, logs }`
- `builds_by_project: BTreeMap<String /*project name*/, Vec<String /*build id, insertion order*/>>` to back ListBuildsForProject ordering and paging
- `next_build_number: BTreeMap<String /*project name*/, i64>` so `buildNumber` is 1 for a project's first build and increments per build, never reused after deletion
- `account_id` / `region` (or derived from the request context) for ARN construction

Done = a client script running CreateProject -> BatchGetProjects -> StartBuild -> BatchGetBuilds -> ListBuildsForProject -> StopBuild -> DeleteProject -> BatchGetProjects succeeds end-to-end with correct, stateful response bodies (not merely 2xx).

## Evidence

`services/service-com-amazonaws-codebuild-codebuild-20161006/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-codebuild-codebuild-20161006.json`): 59 operations, awsJson1_1, target prefix `CodeBuild_20161006`. It gives CreateProject (lines 87-91: required `name`/`source`/`artifacts`/`environment`/`serviceRole`; errors AccountLimitExceeded/InvalidInput/ResourceAlreadyExists; output `project`), BatchGetProjects (57-61: `names` -> `projects` + `projectsNotFound`), BatchGetBuilds (39-43: `ids` -> `builds` + `buildsNotFound`), StartBuild (319-323: required `projectName`, many `*Override` members, output `build`, ResourceNotFoundException), ListBuildsForProject (212-217: `projectName` required, `sortOrder`, @paginated nextToken -> `ids`), StopBuild (349-353: `id` required -> `build`), DeleteProject (117+). smithy_query on shapes `Project` and `Build` confirms the stored field sets quoted in the State Model (Project: arn/created/lastModified/badge/projectVisibility; Build: id/arn/buildNumber ("buildNumber of its first build is 1 ... each subsequent build is incremented by 1")/buildStatus (IN_PROGRESS|SUCCEEDED|FAILED|STOPPED|...)/buildComplete/phases/startTime/endTime). All 59 files under `src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, so the service holds no state at all today; no tests assert any of this behavior (only `tests/state_roundtrip.rs`, a state-API example).

## Steps to Reproduce

1. POST / with X-Amz-Target: CodeBuild_20161006.CreateProject and body {"name":"demo-proj","source":{"type":"NO_SOURCE","buildspec":"version: 0.2\nphases:\n  build:\n    commands:\n      - echo hi"},"artifacts":{"type":"NO_ARTIFACTS"},"environment":{"type":"LINUX_CONTAINER","image":"aws/codebuild/standard:7.0","computeType":"BUILD_GENERAL1_SMALL"},"serviceRole":"arn:aws:iam::123456789012:role/service-role/codebuild"} — expect 200 with project.name=demo-proj, a project ARN, created/lastModified timestamps (today: 501)
2. Repeat the same CreateProject — expect 400 ResourceAlreadyExistsException
3. CodeBuild_20161006.BatchGetProjects {"names":["demo-proj","nope"]} — expect projects=[the stored project, matching step 1 exactly] and projectsNotFound=["nope"]
4. CodeBuild_20161006.StartBuild {"projectName":"demo-proj","environmentVariablesOverride":[{"name":"FOO","value":"bar","type":"PLAINTEXT"}]} — expect build.id="demo-proj:<uuid>", buildNumber=1, buildStatus=IN_PROGRESS, buildComplete=false, startTime set; record the id
5. CodeBuild_20161006.StartBuild {"projectName":"demo-proj"} again — expect buildNumber=2
6. CodeBuild_20161006.StartBuild {"projectName":"missing-proj"} — expect 400 ResourceNotFoundException
7. CodeBuild_20161006.BatchGetBuilds {"ids":["<id from step 4>","demo-proj:00000000-0000-0000-0000-000000000000"]} — expect builds=[the persisted build with the same id/buildNumber/projectName/startTime] and buildsNotFound=[the bogus id]
8. CodeBuild_20161006.ListBuildsForProject {"projectName":"demo-proj"} — expect ids containing both build ids, newest first
9. CodeBuild_20161006.StopBuild {"id":"<id from step 4>"} — expect build.buildStatus=STOPPED, buildComplete=true, endTime set; then BatchGetBuilds on that id must still report STOPPED
10. CodeBuild_20161006.DeleteProject {"name":"demo-proj"} — expect 200; then BatchGetProjects {"names":["demo-proj"]} must return projectsNotFound=["demo-proj"] and empty projects
