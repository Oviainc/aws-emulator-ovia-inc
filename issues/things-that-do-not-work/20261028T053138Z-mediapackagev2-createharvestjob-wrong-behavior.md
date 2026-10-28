Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The HarvestJobName collision check is scoped to one origin endpoint, although the model says the name must be unique within the CHANNEL

_Filed: 2026-10-28T05:31:38Z_
_Operation: CreateHarvestJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateHarvestJob` does implement the duplicate-name refusal — `ConflictException` / `RESOURCE_ALREADY_EXISTS` via `crate::state::resource_already_exists()` — but it computes the collision over the addressed origin endpoint's job map alone (`endpoint.harvest_jobs.contains_key(&name)`). The model scopes the uniqueness one level higher: `CreateHarvestJobRequest$HarvestJobName` is documented "A name for the harvest job. This name must be unique within the channel."

So two origin endpoints of the SAME channel each accept a harvest job named `job-a`: the second create answers 200 and persists a second job under that name in the channel, where the service's rule makes the name already taken. The check is present and produces the wrong result because it is applied to the wrong collection — the sibling levels of the same crate scope their collision checks exactly as the model scopes them (`CreateChannel` checks `group.channels`, which the model scopes to the channel group; `CreateChannelGroup` checks the Region's groups, which the model scopes per Region).

Note this is a different defect from the open CreateHarvestJob validation issue (`20261027T043242Z-mediapackagev2-createharvestjob-validation`), which is about the missing MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB refusal and says nothing about name scoping.

## Evidence

Smithy, verbatim (`smithy_query(query="doc", shape="CreateHarvestJobRequest", member="HarvestJobName")`):
  "<p>A name for the harvest job. This name must be unique within the channel.</p>"

The handler, scoped to the endpoint — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_harvest_job.rs:394-399:
  "if endpoint.harvest_jobs.contains_key(&name) {
       return Err(CreateHarvestJobErrorModeled::ConflictException(
           crate::state::resource_already_exists(),
       )
       .into());
   }"
(the only collection consulted is `endpoint.harvest_jobs`, reached at create_harvest_job.rs:314-320; the channel binding above it is used only to reach `origin_endpoints`).

The generated-name path is endpoint-scoped for the same reason — create_harvest_job.rs:383-393:
  "// `HarvestJobName` is optional: with none given the service names the job itself,
   // from the endpoint it belongs to and the instant it was created, so the name is
   // unique within the endpoint and is reported back for the caller to address it."
— "unique within the endpoint" where the model says "unique within the channel".

How the siblings scope their own collision checks, each to the level the model names: src/ops/create_channel.rs:447-452 (`if group.channels.contains_key(&input.channel_name)`) and src/ops/create_channel_group.rs:281-286 (`if store.has_channel_group(&region, &input.channel_group_name)`).

Test coverage: tests/all/rejection_rule_and_negative_paths.rs:1312-1326 pins only the endpoint-scoped case and says so —
  "// A duplicate name on the same endpoint is the duplicate-create class."
followed by `assert_fixed_refusal(… 409, "ConflictException", ("ConflictExceptionType", "RESOURCE_ALREADY_EXISTS") …)`. No test in tests/all creates two same-named jobs under two endpoints of one channel, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateChannelGroup `g`, CreateChannel `g`/`c`.
2. CreateOriginEndpoint `g`/`c`/`e1` and `g`/`c`/`e2`, each with `ContainerType: TS` and `HlsManifests: [{"ManifestName": "index"}]`.
3. CreateHarvestJob on `g`/`c`/`e1` with `HarvestJobName: "job-a"`, `HarvestedManifests.HlsManifests=[{"ManifestName":"index"}]`, a valid `ScheduleConfiguration` and `Destination`. Observe 200.
4. CreateHarvestJob on `g`/`c`/`e2` with the SAME `HarvestJobName: "job-a"` and an equally valid body. Observe 200 and a second persisted job named `job-a` inside channel `c`, where the model says the name "must be unique within the channel" (expected: 409 ConflictException / RESOURCE_ALREADY_EXISTS, and nothing stored).
5. ListHarvestJobs on `g` now reports two items whose `HarvestJobName` is `job-a` under one channel.
