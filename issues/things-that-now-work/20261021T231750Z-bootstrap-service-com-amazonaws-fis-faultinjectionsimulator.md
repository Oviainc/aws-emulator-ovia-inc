Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the fis (AWS Fault Injection Simulator) emulator

_Filed: 2026-10-21T23:17:50Z_
_Operation: service-com-amazonaws-fis-faultinjectionsimulator | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 26 operations in `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/` is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and answering HTTP 501 `NotImplemented`, so no FIS client can do anything. Implement a FIRST, SMALL, stateful slice: the experiment-template -> experiment lifecycle (restJson1, sigv4 name `fis`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateExperimentTemplate** — `POST /experimentTemplates`. Input: `clientToken` (required, @idempotencyToken), `description` (required), `stopConditions` (required list), `actions` (required map), `roleArn` (required), optional `targets`, `tags`, `logConfiguration`, `experimentOptions`, `experimentReportConfiguration`. Must mint a fresh template `id` (FIS-style short id, e.g. `EXT<12 chars>`) and `arn` (`arn:aws:fis:<region>:<account>:experiment-template/<id>`), set `creationTime`/`lastUpdateTime`, PERSIST the whole record, and return it as `experimentTemplate` (ExperimentTemplate: id, arn, description, targets, actions, stopConditions, creationTime, lastUpdateTime, roleArn, tags). Replaying the same `clientToken` must return the SAME template, not a second one.
2. **GetExperimentTemplate** — `GET /experimentTemplates/{id}`. Must read the persisted record and return exactly what was created (including tags and the generated arn); unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **ListExperimentTemplates** — `GET /experimentTemplates`. Must return one `ExperimentTemplateSummary` (id, arn, description, creationTime, lastUpdateTime, tags) per persisted template, honouring `maxResults` (1..100) and round-tripping `nextToken` so a paged client sees every template exactly once.
4. **StartExperiment** — `POST /experiments`. Input: `clientToken` (required), `experimentTemplateId` (required), optional `experimentOptions`, `tags`. Unknown template id -> `ResourceNotFoundException` 404. Must mint an experiment `id` + `arn` (`.../experiment/<id>`), COPY the template's `roleArn`, `targets`, `actions`, `stopConditions` onto the experiment record, set `creationTime`/`startTime`, set `state.status` to a valid ExperimentStatus (`initiating` or `running`), persist it, and return it as `experiment`. Same `clientToken` replay returns the same experiment.
5. **GetExperiment** — `GET /experiments/{id}`. Must return the persisted experiment with its current `state.status`, `experimentTemplateId`, copied targets/actions/stopConditions and timestamps; unknown id -> `ResourceNotFoundException` 404. ListExperiments filtered by `experimentTemplateId` may be added here cheaply but is optional for the slice.
6. **StopExperiment** — `DELETE /experiments/{id}`. Must transition the persisted experiment's `state.status` to `stopping`/`stopped` with a `state.reason`, set `endTime`, and return the updated `experiment`. A following GetExperiment must observe the stopped status. Stopping an unknown id -> 404.
7. **DeleteExperimentTemplate** — `DELETE /experimentTemplates/{id}`. Must REMOVE the template from state and return the deleted `experimentTemplate`; a following GetExperimentTemplate must answer `ResourceNotFoundException` 404 and ListExperimentTemplates must no longer list it.

NEVER STUBS: each of these must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without storing it, no random id without a record behind it. Validation only where the model already constrains it (missing required members, id/arn length+pattern, `maxResults` range) surfaced as `ValidationException` 400.

## State Model

In `src/state.rs`, on the service state struct:

- `experiment_templates: HashMap<String /* template id */, ExperimentTemplateRecord>` where `ExperimentTemplateRecord { id, arn, description, targets: HashMap<String, ExperimentTemplateTarget>, actions: HashMap<String, ExperimentTemplateAction>, stop_conditions: Vec<ExperimentTemplateStopCondition>, role_arn, tags: HashMap<String,String>, log_configuration: Option<..>, experiment_options: Option<..>, experiment_report_configuration: Option<..>, creation_time, last_update_time, target_account_configurations_count: i64 }`
- `experiments: HashMap<String /* experiment id */, ExperimentRecord>` where `ExperimentRecord { id, arn, experiment_template_id, role_arn, state: ExperimentStateRecord { status: ExperimentStatus, reason: Option<String>, error: Option<..> }, targets, actions, stop_conditions, tags, experiment_options, creation_time, start_time: Option<..>, end_time: Option<..> }`
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` so CreateExperimentTemplate and StartExperiment are idempotent per token.
- Monotonic id generation helpers (`next_template_id`, `next_experiment_id`) plus the account/region used to format ARNs.

Tags should live on the records themselves so TagResource/ListTagsForResource can later be wired to the same maps by ARN. All the other 19 operations (target-account configurations, actions/target-resource-type catalogs, safety levers, resolved targets, update paths, tagging) stay at 501 and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-fis-faultinjectionsimulator/OPERATIONS.md` lists all 26 operations with routes, inputs, outputs and errors (CreateExperimentTemplate `POST /experimentTemplates` -> `experimentTemplate`; StartExperiment `POST /experiments` -> `experiment`; StopExperiment `DELETE /experiments/{id}`; GetExperiment `GET /experiments/{id}`; DeleteExperimentTemplate `DELETE /experimentTemplates/{id}`; ListExperimentTemplates paginated by nextToken/maxResults). `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 26 handler files, confirming the service is all-stubs with zero persisted state. Shape members cited above come from the Smithy model shapes `ExperimentTemplate`, `Experiment`, `ExperimentState`, `ExperimentStatus` (enum: pending|initiating|running|completed|stopping|stopped|failed|cancelled) and `ExperimentTemplateSummary`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `fis` client at it.
2. `aws fis create-experiment-template --description demo --role-arn arn:aws:iam::123456789012:role/fis --stop-conditions '[{"source":"none"}]' --actions '{"stopInstances":{"actionId":"aws:ec2:stop-instances"}}' --tags Key1=Val1` — expect 200 with a generated experimentTemplate.id and .arn (currently HTTP 501 NotImplemented).
3. `aws fis get-experiment-template --id <id>` — expect the same description, actions, stopConditions, roleArn, tags and arn that were created.
4. `aws fis list-experiment-templates --max-results 1` — expect the template in the summary list, with nextToken paging when more than one exists.
5. `aws fis start-experiment --experiment-template-id <id>` — expect 200 with a new experiment id/arn, experimentTemplateId == <id>, state.status in {initiating, running} and the template's actions/targets/stopConditions copied.
6. `aws fis get-experiment --id <experimentId>` — expect the persisted experiment with its current state.status.
7. `aws fis stop-experiment --id <experimentId>` — expect state.status stopping/stopped and endTime set; re-run get-experiment and expect the stopped status to persist.
8. `aws fis delete-experiment-template --id <id>` — expect 200 returning the deleted template; then get-experiment-template on the same id must return ResourceNotFoundException (HTTP 404) and list-experiment-templates must not include it.
9. `aws fis get-experiment --id EXPdoesnotexist` — expect ResourceNotFoundException 404, not 501.
