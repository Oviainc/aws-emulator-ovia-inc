Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the AWS Resilience Hub emulator

_Filed: 2026-10-21T21:28:34Z_
_Operation: service-com-amazonaws-resiliencehub-awsresiliencehub | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 63 operations in `services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Resilience Hub client can do anything at all. Implement a FIRST, SMALL, stateful slice: the lifecycle of the primary resource (the Resilience Hub *application*) plus the resiliency policy it references.

Wire protocol (OPERATIONS.md): `restJson1`, sdkId `resiliencehub`, signing name `resiliencehub`.

### Happy path to implement (in order)

1. **CreateResiliencyPolicy** — `POST /create-resiliency-policy`. Input: `policyName` (required, `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`), `tier` (required enum: CoreServices|Critical|Important|MissionCritical|NonCritical|NotApplicable), `policy` (required map<DisruptionType,FailurePolicy>), `policyDescription`, `dataLocationConstraint`, `tags`, `clientToken`. Output: `policy` (ResiliencyPolicy, required) — must carry a newly minted `policyArn` of the form `arn:aws:resiliencehub:<region>:<account>:resiliency-policy/<uuid>`, plus `policyName`, `policyDescription`, `tier`, `dataLocationConstraint`, `policy` exactly as stored, and a real `creationTime`. Store the policy; do not merely echo the input.

2. **CreateApp** — `POST /create-app`. Input: `name` (required), `description`, `policyArn`, `tags`, `assessmentSchedule` (Daily|Disabled), `permissionModel`, `eventSubscriptions`, `awsApplicationArn`, `clientToken` (@idempotencyToken). Output: `app` (App, required) with a minted `appArn` = `arn:aws:resiliencehub:<region>:<account>:app/<uuid>`, `name`, `description`, `policyArn`, `creationTime` (required), `status` = `Active`, `complianceStatus` = `NotAssessed`, `resiliencyScore` = 0, `tags`, `assessmentSchedule`, `driftStatus`. Persist the app keyed by its ARN. Reusing the same `clientToken` with the same input must return the same app rather than creating a second one.

3. **DescribeApp** — `POST /describe-app`. Input: `appArn` (required). Output: `app` (App, required) — must be read back from the store and be field-for-field what CreateApp/UpdateApp last persisted (same ARN, name, description, policyArn, creationTime, tags). An unknown or already-deleted ARN must return `ResourceNotFoundException` (HTTP 404), never a 200 with an empty/synthesised app.

4. **ListApps** — `GET /list-apps` (@paginated, @readonly). Input (all `@httpQuery`): `nextToken`, `maxResults` (1..100), `name`, `appArn`, `fromLastAssessmentTime`, `toLastAssessmentTime`, `reverseOrder`, `awsApplicationArn`. Output: `appSummaries` (list<AppSummary>, required) + `nextToken`. Each summary is projected from the stored app (`appArn`, `name`, `description`, `creationTime`, `complianceStatus`, `resiliencyScore`, `assessmentSchedule`, `status`, `driftStatus`). Must honour `name`/`appArn` filtering, `maxResults` as a real page bound with a resumable `nextToken`, and `reverseOrder`.

5. **UpdateApp** — `POST /update-app`. Input: `appArn` (required), `description`, `policyArn`, `clearResiliencyPolicyArn`, `assessmentSchedule`, `permissionModel`, `eventSubscriptions`. Output: `app` (App, required). Must mutate the STORED app (partial update: only supplied members change; `clearResiliencyPolicyArn=true` removes `policyArn`) so a subsequent DescribeApp observes the change. Unknown ARN -> `ResourceNotFoundException` (404).

6. **DeleteApp** — `POST /delete-app`. Input: `appArn` (required), `forceDelete`, `clientToken`. Output: `appArn` (required). Must remove the app from the store; a following DescribeApp/ListApps must no longer see it, and a second DeleteApp of the same ARN must return `ResourceNotFoundException` (404).

Also: **DeleteResiliencyPolicy** semantics are out of this slice, but CreateApp referencing a `policyArn` that was never created should fail with `ResourceNotFoundException` (404) rather than silently storing a dangling reference.

### Non-negotiables

- NEVER STUBS: each of the six operations must compute its response from persisted state reached through `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned responses, no echoing the input back without storing it, no minting an ARN that is never recorded.
- Validation beyond the modeled constraints is refused with `ValidationException` (400) via `crate::scaffolded::Scaffolded::validation(..)`; missing resources with the modeled `ResourceNotFoundException` (404); duplicate-name/clientToken conflicts with `ConflictException` (409).
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the six files as they are implemented so the stub census stays accurate.
- Done = an integration test runs the full workflow below and asserts the response BODIES, not just 2xx.

## State Model

In `ComAmazonawsResiliencehubAwsresiliencehubState`:

- `apps: HashMap<String /* appArn */, App>` where `App { app_arn, name, description, policy_arn: Option<String>, creation_time, status: AppStatusType, compliance_status: AppComplianceStatusType, resiliency_score: f64, last_app_compliance_evaluation_time: Option<Timestamp>, last_resiliency_score_evaluation_time: Option<Timestamp>, assessment_schedule: Option<AppAssessmentScheduleType>, permission_model: Option<PermissionModel>, event_subscriptions: Vec<EventSubscription>, drift_status: Option<AppDriftStatusType>, rto_in_secs: Option<i32>, rpo_in_secs: Option<i32>, aws_application_arn: Option<String>, tags: HashMap<String,String> }`
- `app_name_index: HashMap<String /* name */, String /* appArn */>` for name filtering and duplicate-name conflict detection.
- `policies: HashMap<String /* policyArn */, ResiliencyPolicy>` where `ResiliencyPolicy { policy_arn, policy_name, policy_description, data_location_constraint, tier, estimated_cost_tier, policy: HashMap<DisruptionType, FailurePolicy>, creation_time, tags }`
- `idempotency_tokens: HashMap<String /* clientToken */, String /* created resource ARN */>` so a replayed `clientToken` returns the first result.
- `account_id` / `region` (from request context) used to mint `arn:aws:resiliencehub:<region>:<account>:app/<uuid>` and `:resiliency-policy/<uuid>`.
- Optional for later slices (leave the fields but keep handlers stubbed): `app_versions: HashMap<String /* appArn */, Vec<AppVersionSummary>>`, `assessments: HashMap<String /* assessmentArn */, AppAssessment>`.

All other operations (app versions, components, resources, assessments, recommendations, metrics, tagging) stay at 501 and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-resiliencehub-awsresiliencehub/OPERATIONS.md` lists 63 operations from the Smithy model with the routes and members quoted above (CreateApp line 47-52, CreateResiliencyPolicy 75-80, DeleteApp 82-87, DescribeApp 131-136, ListApps 271-277, UpdateApp 453-458), and the wire protocol block (lines 8-14) gives restJson1 / sdkId `resiliencehub`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/` matches all 63 operation files, e.g. `src/ops/create_app.rs:4` with `handle` returning `Err(crate::scaffolded::Scaffolded::not_implemented("CreateApp"))` at line 210. Field shapes for `App`, `AppSummary` and `ResiliencyPolicy` are taken from the model via smithy_query (App.appArn/name/creationTime are @required; AppSummary.appArn/name/creationTime are @required; ListApps is @paginated(inputToken=nextToken, outputToken=nextToken) and @readonly). No tests assert any of this behaviour today.

## Steps to Reproduce

1. POST /create-resiliency-policy with {"policyName":"demo-policy","tier":"Important","policy":{"AZ":{"rtoInSecs":3600,"rpoInSecs":3600}}} -> expect 200 with policy.policyArn minted; today 501 NotImplemented
2. POST /create-app with {"name":"demo-app","description":"bootstrap demo","policyArn":"<policyArn from step 1>","tags":{"env":"test"}} -> expect 200 with app.appArn minted, app.status=Active, app.creationTime set
3. POST /describe-app with {"appArn":"<appArn from step 2>"} -> expect 200 and app identical to what was created (name, description, policyArn, tags, creationTime)
4. GET /list-apps?name=demo-app -> expect 200 with exactly one appSummary whose appArn matches step 2
5. POST /update-app with {"appArn":"<appArn>","description":"updated","assessmentSchedule":"Daily"} then POST /describe-app again -> expect description="updated" and assessmentSchedule="Daily" persisted
6. POST /delete-app with {"appArn":"<appArn>"} -> expect 200 echoing appArn; then POST /describe-app with the same ARN -> expect 404 ResourceNotFoundException, and GET /list-apps -> app absent
7. POST /describe-app with a well-formed but never-created ARN -> expect 404 ResourceNotFoundException (not 501, not a hollow 200)
