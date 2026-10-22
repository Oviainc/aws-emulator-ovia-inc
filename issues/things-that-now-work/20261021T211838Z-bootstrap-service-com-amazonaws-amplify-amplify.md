Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWS Amplify emulator

_Filed: 2026-10-21T21:18:38Z_
_Operation: service-com-amazonaws-amplify-amplify | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 operations in `services/service-com-amazonaws-amplify-amplify/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, so the service answers HTTP 501 for everything and holds zero state. There is no working emulator to find behavioral gaps in.

Implement a FIRST, SMALL stateful slice: the Amplify app + branch lifecycle, which is the service's primary resource loop (restJson1, sigv4 name `amplify`).

### Happy path to implement (in order)

1. **CreateApp** — `POST /apps`. Input: `name` (required, 1..255), plus optional `description`, `repository`, `platform` (WEB | WEB_COMPUTE | WEB_DYNAMIC), `environmentVariables`, `enableBranchAutoBuild`, `enableBasicAuth`, `basicAuthCredentials`, `buildSpec`, `customRules`, `tags`. Output: `app` (App, required). The emulator must MINT and persist a new `appId` matching the modeled pattern `^d[a-z0-9]+$` (len 1..20), derive `appArn` as `arn:aws:amplify:<region>:<account>:apps/<appId>`, set `defaultDomain` (e.g. `<appId>.amplifyapp.com`), and set `createTime`/`updateTime`. All App members marked `@required` in the model (`appId`, `appArn`, `name`, `description`, `repository`, `platform`, `createTime`, `updateTime`, `environmentVariables`, `defaultDomain`, `enableBranchAutoBuild`, `enableBasicAuth`) must be present in the response, defaulted where the request omitted them (empty string / empty map / `false` / `WEB`).
2. **GetApp** — `GET /apps/{appId}`. Output: the SAME `app` record stored by CreateApp, read back from state field-for-field (same `appId`, `name`, `description`, `platform`, `environmentVariables`, `createTime`). Unknown `appId` → `NotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListApps** — `GET /apps`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=apps)`. Must return every app currently in state, honoring `maxResults` (range 0..100) as a page bound and round-tripping `nextToken` so a second call continues where the first stopped. A deleted app must not appear.
4. **CreateBranch** — `POST /apps/{appId}/branches`. Input: `appId` (@httpLabel), `branchName` (required), plus optional `description`, `stage`, `framework`, `enableAutoBuild`, `environmentVariables`, `buildSpec`, `ttl`, `displayName`. Output: `branch` (Branch, required) persisted under the parent app, with `branchArn` = `<appArn>/branches/<branchName>`, `createTime`/`updateTime`, and the `@required` Branch members populated (`branchName`, `description`, `stage` default `NONE`-equivalent per model enum / `DEVELOPMENT`, `displayName`, `enableNotification`, `environmentVariables`, `enableAutoBuild`, `customDomains` (empty list), `framework`, `activeJobId` (empty), `totalNumberOfJobs` ("0"), `enableBasicAuth`, `ttl`, `enablePullRequestPreview`). A `branchName` already present on that app must not silently duplicate; a nonexistent `appId` → `NotFoundException` (404).
5. **GetBranch / ListBranches** — `GET /apps/{appId}/branches/{branchName}` returns the stored branch; `GET /apps/{appId}/branches` (`@paginated`, items=`branches`, `maxResults` 0..50) returns exactly the branches created on that app and nothing from other apps.
6. **DeleteApp** — `DELETE /apps/{appId}`. Output: `app` (the record as it existed at delete time), and the app plus its branches must be REMOVED from state: a following GetApp → `NotFoundException` (404), and ListApps no longer lists it.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an `appId` that is never stored. A CreateApp followed by GetApp must return what was created; a DeleteApp followed by GetApp must 404.

## State Model

In `src/state.rs` (`ComAmazonawsAmplifyAmplifyState`, reached via `crate::state::service_state(&*self.engine)` — never a fresh engine):

- `apps: HashMap<String /* appId */, AmplifyApp>` — keyed by the minted `appId`.
- `AmplifyApp { app_id: String, app_arn: String, name: String, description: String, repository: String, platform: Platform, compute_role_arn: Option<String>, iam_service_role_arn: Option<String>, environment_variables: HashMap<String,String>, default_domain: String, enable_branch_auto_build: bool, enable_branch_auto_deletion: Option<bool>, enable_basic_auth: bool, basic_auth_credentials: Option<String>, custom_rules: Vec<CustomRule>, build_spec: Option<String>, custom_headers: Option<String>, enable_auto_branch_creation: Option<bool>, auto_branch_creation_patterns: Vec<String>, tags: HashMap<String,String>, create_time: SystemTime, update_time: SystemTime, branches: HashMap<String /* branchName */, AmplifyBranch> }`
- `AmplifyBranch { branch_arn: String, branch_name: String, description: String, stage: Stage, display_name: String, framework: String, enable_notification: bool, enable_auto_build: bool, enable_basic_auth: bool, basic_auth_credentials: Option<String>, environment_variables: HashMap<String,String>, build_spec: Option<String>, ttl: String, custom_domains: Vec<String>, active_job_id: String, total_number_of_jobs: String, enable_pull_request_preview: bool, tags: HashMap<String,String>, create_time: SystemTime, update_time: SystemTime }`
- `next_app_id_seq: u64` — a monotonic counter used to mint `appId` values that satisfy `^d[a-z0-9]+$` (e.g. `d` + base36 of the counter mixed with random suffix), so two CreateApp calls never collide.

Branches live INSIDE their owning app so DeleteApp removes them transitively and ListBranches can never leak another app's branches.

### Out of scope for this first slice

UpdateApp, UpdateBranch, DeleteBranch, webhooks, domain associations, backend environments, jobs/deployments (StartJob, StartDeployment, GetJob, ListJobs, StopJob, DeleteJob, CreateDeployment, artifacts), GenerateAccessLogs and the tag operations stay stubbed at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-amplify-amplify/OPERATIONS.md` documents all 37 operations with their routes, inputs, outputs and errors — CreateApp (`POST /apps`, output `app` required, lines 26-31), GetApp (`GET /apps/{appId}`, NotFoundException HTTP 404, lines 117-122), ListApps (`GET /apps`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=apps)`, `maxResults` range 0..100, lines 166-172), CreateBranch (`POST /apps/{appId}/branches`, output `branch` required, lines 40-45), GetBranch (lines 138-143), ListBranches (`@paginated`, items=branches, lines 188-194) and DeleteApp (`DELETE /apps/{appId}`, output `app` required, lines 68-73). The `App`, `Branch` and `JobSummary` shape definitions from the Smithy model give the `@required` response members and the `appId` pattern `^d[a-z0-9]+$` (len 1..20) that the emulator must mint. `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-amplify-amplify/src/ops/` matches all 37 handler files, confirming nothing is implemented and no test exercises any of these paths.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Amplify client at it (restJson1, signing name `amplify`).
2. `aws amplify create-app --name demo-app --platform WEB --environment-variables KEY=VAL` — expect HTTP 200 with an `app` whose `appId` matches `^d[a-z0-9]+$`, plus `appArn`, `defaultDomain`, `createTime`. Actual: HTTP 501 NotImplemented.
3. `aws amplify get-app --app-id <appId>` — expect the same app back with `name=demo-app`, `platform=WEB`, `environmentVariables={KEY:VAL}`. Actual: 501.
4. `aws amplify list-apps --max-results 10` — expect the created app in `apps`, with `nextToken` round-tripping across pages when more apps exist. Actual: 501.
5. `aws amplify create-branch --app-id <appId> --branch-name main --stage PRODUCTION` — expect 200 with a `branch` carrying `branchArn` under the app's ARN. Actual: 501.
6. `aws amplify get-branch --app-id <appId> --branch-name main` and `aws amplify list-branches --app-id <appId>` — expect the stored branch, scoped to that app only. Actual: 501.
7. `aws amplify get-app --app-id dNOSUCHAPP1` — expect `NotFoundException` with HTTP 404. Actual: 501.
8. `aws amplify delete-app --app-id <appId>` then `aws amplify get-app --app-id <appId>` — expect 200 with the deleted app, then `NotFoundException` 404, and the app absent from list-apps. Actual: 501 for all three.
