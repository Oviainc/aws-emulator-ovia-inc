Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DeviceFarm emulator

_Filed: 2026-10-21T21:58:11Z_
_Operation: service-com-amazonaws-devicefarm-devicefarm-20150623 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 77 operations in `services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Device Farm client can do anything against the emulator. Implement a FIRST small, stateful slice: the lifecycle of the service's primary resource, the Project (`awsJson1_1`, `POST /` with `X-Amz-Target: DeviceFarm_20150623.<Operation>`).

Happy path to implement, in order:

1. **CreateProject** — input `name` (required, len 0..256), optional `defaultJobTimeoutMinutes`, `vpcConfig`, `environmentVariables` (len 1..32), `executionRoleArn` (pattern `^arn:aws:iam::[0-9]{12}:role/.+$`). Mint a stable ARN of the form `arn:aws:devicefarm:<region>:<account>:project:<uuid>` (len 32..1011, pattern `^arn:aws:devicefarm:.+$`), set `created` to now, store the record, and return `project` (Project{arn, name, defaultJobTimeoutMinutes, created, vpcConfig, environmentVariables, executionRoleArn}) built from the stored record. Default `defaultJobTimeoutMinutes` to 150 when absent (documented default in the Project shape).
2. **GetProject** — input `arn` (required). Look the ARN up in the store and return the SAME `project` that CreateProject persisted (same arn, name, created timestamp). Unknown/deleted ARN → `NotFoundException` (HTTP 400); malformed ARN → `ArgumentException` (HTTP 400).
3. **ListProjects** — optional `arn` filter, optional `nextToken` (len 4..1024). Return `projects` from the store (all projects, or just the one matching `arn`). Must include every project created and exclude deleted ones. The operation is `@paginated(inputToken=nextToken, outputToken=nextToken, items=projects)`, so emit `nextToken` only when a page is truncated and honour a token handed back.
4. **UpdateProject** — input `arn` (required) plus optional `name`, `defaultJobTimeoutMinutes`, `vpcConfig`, `environmentVariables`, `executionRoleArn`. Mutate the stored record in place (fields absent from the request stay unchanged), return the updated `project`; a later GetProject/ListProjects must show the new values. Unknown ARN → `NotFoundException`.
5. **DeleteProject** — input `arn` (required), empty output. Remove the record; afterwards GetProject on that ARN returns `NotFoundException` and ListProjects no longer lists it. Deleting an unknown ARN → `NotFoundException`.

No canned, echoed, or hardcoded responses: every response field must be read back out of the persisted record (the ARN and `created` timestamp minted at create time must be byte-identical on every later read). A random ARN returned without storing the project is a failure. Errors must be the modeled DeviceFarm shapes (`NotFoundException`, `ArgumentException`, `LimitExceededException`, `ServiceAccountException`) over awsJson1_1, not 501 and not a generic framework error.

## State Model

In `src/state.rs` (`ComAmazonawsDevicefarmDevicefarm20150623State`, reached via `crate::state::service_state(&*self.engine)`):

- `projects: BTreeMap<String /* project ARN */, ProjectRecord>` — ARN-keyed so Get/Update/Delete are O(log n) and ListProjects has a stable, deterministic order.
- `ProjectRecord { arn: String, name: String, default_job_timeout_minutes: i32 /* 150 when unspecified */, created: DateTime, vpc_config: Option<VpcConfig>, environment_variables: Vec<EnvironmentVariable>, execution_role_arn: Option<String> }` — the single source of truth converted into the modeled `Project` on the way out.
- `next_project_id: u64` or a UUID source used only to mint the ARN suffix at create time; the minted ARN is stored on the record, never recomputed per response.
- (Room for the next slice, not required here: `device_pools`, `runs`, `uploads` maps keyed by ARN and carrying their owning `project_arn`.)

Everything outside these five operations (device pools, uploads, ScheduleRun/GetRun/StopRun, remote access sessions, test grid, offerings, tagging, account settings) stays at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate: header says "77 operations", protocol `awsJson1_1` with `X-Amz-Target: DeviceFarm_20150623.<OperationName>`; CreateProject (line 45) input `name` required + `defaultJobTimeoutMinutes`/`vpcConfig`/`environmentVariables`/`executionRoleArn`, output `project` (Project); GetProject (line 190) input `arn` required, output `project`; ListProjects (line 316) `@paginated(inputToken=nextToken, outputToken=nextToken, items=projects)`; UpdateProject (line 483); DeleteProject (line 99) input `arn` required, no output members. All five declare `ArgumentException`, `LimitExceededException`, `NotFoundException`, `ServiceAccountException` (HTTP 400). `smithy_query shape=Project` gives the member set {arn, name, defaultJobTimeoutMinutes (doc: "default value is 150 minutes"), created, vpcConfig, environmentVariables, executionRoleArn} and the ARN pattern `^arn:aws:devicefarm:.+$`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every handler file in the crate, including create_project.rs, get_project.rs, list_projects.rs, update_project.rs and delete_project.rs — nothing is implemented and no test asserts any Project behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI devicefarm client at it (region us-west-2).
2. CreateProject with name="bootstrap-demo" and defaultJobTimeoutMinutes=30 — expect 200 with project.arn matching ^arn:aws:devicefarm:.+:project:.+$, project.name="bootstrap-demo", project.created set. Actual: HTTP 501 NotImplemented.
3. GetProject with the arn returned above — expect the identical project (same arn, name, created, defaultJobTimeoutMinutes=30).
4. ListProjects with no arguments — expect projects[] to contain exactly that project.
5. UpdateProject with the same arn and name="renamed" — expect project.name="renamed" and the original arn/created unchanged; GetProject again must return "renamed".
6. DeleteProject with the same arn — expect 200 with an empty body.
7. GetProject with the deleted arn — expect NotFoundException (HTTP 400) in the awsJson1_1 error shape; ListProjects must return an empty projects list.
