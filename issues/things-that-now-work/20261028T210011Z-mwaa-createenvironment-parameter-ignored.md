Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# An idempotent CreateEnvironment replay silently discards the Tags it carries instead of applying or refusing them

_Filed: 2026-10-28T21:00:11Z_
_Operation: CreateEnvironment | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironment`'s `@idempotent` replay check compares only `EnvironmentSpec`, and `EnvironmentSpec` holds no tags at all (tags live on `EnvironmentRecord.tags`, written once at insert time). The handler therefore treats a create of an already-stored name whose `Tags` differ from the stored tag map as an identical replay: it returns the stored ARN with HTTP 200 and never reaches the `tags:` write, so the pairs the caller asked to associate are neither stored nor refused.

This contradicts the rule the same block states and enforces for every other member of the request: a create of a taken name whose members differ is refused with `ValidationException` rather than allowed to pass. The crate thus has two mutually exclusive treatments of "a create of a taken name whose members differ" — 400 for `DagS3Path`, `MaxWorkers`, `AirflowVersion`, … and a silent 200 for `Tags`. The 200 is the damaging one: the caller is told the create succeeded, and a later `ListTagsForResource` / `GetEnvironment` reports the OLD tag map, so a documented input member is dropped on a success path.

`CreateEnvironment` is one of the two writers of the very tag map `TagResource`'s own handler doc calls "the SAME tag map `GetEnvironment` and `ListTagsForResource` read"; on this path it skips that write while answering success. Either the differing `Tags` member must make the request a different create (refused, like every other member), or it must be applied to the stored map — but it must not be answered 200 and thrown away.

## Evidence

Model — `CreateEnvironmentInput$Tags` (smithy_query query="doc"): "The key-value tag pairs you want to associate to your environment. For example, \"Environment\": \"Staging\"."

The replay path, services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_environment.rs:408-430:
```
// The operation is `@idempotent`: replaying the SAME create of a name that is
// already stored IN THIS REGION answers with the stored ARN and writes nothing. A
// create of that name with different members is a different request, and is
// refused rather than allowed to clobber the stored record. (Tags are deliberately
// outside the comparison: `TagResource` / `UntagResource` own them after the
// create.)
...
if let Some(existing) = store.environment(&region, &input.name) {
    if existing.spec == spec && !existing.is_deleting() {
        return Ok(CreateEnvironmentOutput {
            arn: Some(existing.arn.clone()),
        });
    }
    return Err(validation(format!(
        "An environment named {} already exists",
        input.name
    )));
}
```
The compared value carries no tags — `EnvironmentSpec`, services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:504-532, has members `execution_role_arn … logging_configuration` and no tag field, while the tag map is a separate field of the record (state.rs:812-813: `/// A `BTreeMap` so tags read back in a stable order.` / `pub tags: BTreeMap<String, String>,`).

The only place tags are written, create_environment.rs:463 — unreachable from the early return above:
```
            tags: input.tags.clone().unwrap_or_default().into_iter().collect(),
```

The other writer of that same map states the rule this path breaks — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/tag_resource.rs:5-6:
```
//! Resolves `ResourceArn` back to the stored environment and writes the requested pairs
//! into the SAME tag map `GetEnvironment` and `ListTagsForResource` read.
```

Test coverage: no existing test pins this path. `an_idempotent_replay_returns_the_stored_arn_and_a_different_create_is_refused` (tests/all/environment_lifecycle.rs:819-867) replays the byte-identical body (`harness.create(&case)` twice, tests/all/environment_lifecycle.rs:826) and builds its "different create" row by changing `DagS3Path` only (environment_lifecycle.rs:840-846: `body.as_object_mut().expect("an object body").insert("DagS3Path".into(), json!("somewhere/else"));`). A replay that differs only in `Tags` is exercised by neither assertion.

## Steps to Reproduce

1. PUT /environments/MyMWAAEnvironment with a valid body including `"Tags": {"Team": "data"}` — note the returned `Arn`.
2. GET /tags/{that Arn} — `Tags` reports `{"Team": "data"}`.
3. Re-send the identical PUT /environments/MyMWAAEnvironment body with only the tag map changed, e.g. `"Tags": {"Team": "platform", "CostCenter": "42"}`.
4. Observe HTTP 200 carrying the stored `Arn` — the handler took the replay branch at create_environment.rs:421 because `EnvironmentSpec` carries no tags.
5. GET /tags/{that Arn} again (and GET /environments/MyMWAAEnvironment): `Tags` still reports `{"Team": "data"}`. The `Tags` member of the accepted request was neither applied nor refused, while the same request with any other member changed (e.g. `DagS3Path`) is refused with 400 `ValidationException` "An environment named MyMWAAEnvironment already exists".
