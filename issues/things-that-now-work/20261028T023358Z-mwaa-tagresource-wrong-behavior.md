Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# Tag and Apache Airflow writes are applied to an environment whose teardown has already started

_Filed: 2026-10-28T02:33:58Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate's state module documents `DELETING` as a status in which an environment "still reads back, but nothing can be applied to it". `UpdateEnvironment` honours that rule (it refuses any patch unless the record has settled into `AVAILABLE`), and `CreateEnvironment` honours it too (it refuses to replay a create of a name whose record `is_deleting()`). The three remaining writers do not consult the status at all:

* `TagResource` inserts into `stored.tags` of a record in `DELETING`;
* `UntagResource` removes from `stored.tags` of a record in `DELETING`;
* `InvokeRestApi` runs its write methods (`POST`/`PATCH`/`DELETE` on `/variables` and `/pools`) against `environment.airflow` of a record in `DELETING`.

So a caller that issues `DeleteEnvironment` and then `TagResource` gets HTTP 200, and `ListTagsForResource`/`GetEnvironment` report the new tag map for an environment that is being torn down — a change "applied" to a record the same handler chain says can take none, which then silently disappears when the clock reaps the record. The three paths skip a state gate the crate already implements for its other two writers.

## Evidence

The rule, in this crate's own state module — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:880-884:
```
/// Whether the teardown of this environment has already been started, so it is on its
/// way out: its name is still taken and it still reads back, but nothing can be
/// applied to it and the clock owes its removal.
pub fn is_deleting(&self) -> bool {
    self.status == STATUS_DELETING
```
The sibling writers that enforce it:
* src/ops/update_environment.rs:277-282 — `if !stored.accepts_update() { return Err(validation(crate::state::update_not_allowed_message(&input.name, &stored.status))); }` (with `accepts_update` = `self.status == STATUS_AVAILABLE && self.pending.is_empty()`, state.rs:876-878).
* src/ops/create_environment.rs:420-430 — `if existing.spec == spec && !existing.is_deleting() { .. } return Err(validation(format!("An environment named {} already exists", input.name)));`

The writers that skip it:
* src/ops/tag_resource.rs:174-201 — `let stored = store.environment_by_arn_mut(&region, &resource_arn)` … `for (key, value) in input.tags { stored.tags.insert(key, value); }` — no status test anywhere in the handler.
* src/ops/untag_resource.rs:172-182 — `for key in &input.tag_keys { stored.tags.remove(key); }`.
* src/ops/invoke_rest_api.rs:229-241 — `let Some(environment) = store.environment_mut(&region, &input.name) else { .. };` … `let answer = serve(environment, &call);`, whose write arms (e.g. `set_variable`, src/ops/invoke_rest_api.rs:481-496: `metadata.variables.insert(key.clone(), requested);`) mutate the record unconditionally.

Test coverage: tests/all/environment_deletion_teardown.rs:400-453 (`an_environment_being_torn_down_accepts_no_change_and_keeps_its_name`, with the comment "Nothing can be applied to a record on its way out") exercises only `UpdateEnvironment` and `CreateEnvironment`; no test in tests/all calls `TagResource`, `UntagResource` or `InvokeRestApi` against an environment in `DELETING`, so nothing pins today's accepting behaviour.

## Steps to Reproduce

1. Boot the emulator and point an mwaa client at it.
2. CreateEnvironment(Name='Teardown', ExecutionRoleArn='arn:aws:iam::000000000000:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket', DagS3Path='dags', NetworkConfiguration={'SubnetIds':['subnet-a','subnet-b'],'SecurityGroupIds':['sg-1']}, Tags={'Team':'a'}) and poll GetEnvironment until Status == 'AVAILABLE'.
3. DeleteEnvironment(Name='Teardown') and immediately GetEnvironment(Name='Teardown') — Status is 'DELETING'.
4. UpdateEnvironment(Name='Teardown', MaxWorkers=11) — refused with 400 ValidationException naming the DELETING status (the documented rule).
5. TagResource(ResourceArn='arn:aws:airflow:us-east-1:000000000000:environment/Teardown', Tags={'Added':'while-deleting'}) — observe HTTP 200, and ListTagsForResource now reports the new key on an environment that is tearing down.
6. UntagResource(..., tagKeys=['Team']) — observe HTTP 200 and the key removed from the same record.
7. InvokeRestApi(Name='Teardown', Method='POST', Path='/variables', Body={'key':'k','value':'v'}) — observe HTTP 200 and the variable readable through a following GET, all on a record the handler chain documents as accepting nothing.
