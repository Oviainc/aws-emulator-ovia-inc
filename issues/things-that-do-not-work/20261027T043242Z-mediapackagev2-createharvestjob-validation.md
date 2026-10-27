Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# CreateHarvestJob accepts a harvest job against an origin endpoint of a MULTIVIEW channel instead of refusing it

_Filed: 2026-10-27T04:32:42Z_
_Operation: CreateHarvestJob | Class: validation_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateHarvestJob resolves the containment chain group -> channel -> endpoint purely to prove each level exists; it never reads the parent channel's `input_type`, so a harvest job is created and persisted against an endpoint whose channel is a MULTIVIEW channel. The service names exactly this refusal: `ValidationExceptionType` carries `MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB`, and `CreateHarvestJob` declares `ValidationException` (400).

This is a path that skips a helper the crate already has and that its siblings already call: `state::Channel::is_multiview()` exists and is documented as "the one answer every operation that has nothing to act on such a channel branches on"; `PutChannelPolicy` and `ResetChannelState` both branch on it and refuse with the matching `MULTIVIEW_*_NOT_ALLOWED` discriminator. CreateHarvestJob has the channel in hand on the line before it takes the endpoint and does not ask.

## Evidence

Smithy: `com.amazonaws.mediapackagev2#ValidationExceptionType` enumerates `MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB` (alongside `MULTIVIEW_CHANNEL_POLICY_NOT_ALLOWED` and `MULTIVIEW_RESET_NOT_ALLOWED`, the two this crate DOES implement), and `CreateHarvestJob` declares `{"shape": "ValidationException", "errorKind": "client", "httpError": 400}`.

The crate's single branch point, services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:913-919:
  "/// Decided from the stored `InputType`, which is materialised at create time and
   /// immutable, so it is the one answer every operation that has nothing to act on such
   /// a channel branches on — rather than each handler inferring it from a different member.
   pub fn is_multiview(&self) -> bool {"

Siblings applying it — src/ops/put_channel_policy.rs:216-230:
  "// A multiview channel takes no resource policy of its own ... Which channels
   // those are is decided in ONE place, `crate::state::Channel::is_multiview`, so
   // every operation that has nothing to do on such a channel agrees on it.
   if channel.is_multiview() {"
and src/ops/reset_channel_state.rs:209-222:
  "// A channel that composites other channels receives no ingest of its own, so
   // there is no ingest state for a reset to refresh — the case the model's
   // `MULTIVIEW_RESET_NOT_ALLOWED` validation type names.
   if channel.is_multiview() {"

The skip — src/ops/create_harvest_job.rs:306-318:
  "let Some(channel) = group.channels.get_mut(&input.channel_name) else { ... ResourceKind::Channel.not_found() ... };
   let Some(endpoint) = channel.origin_endpoints.get_mut(&input.origin_endpoint_name) else { ... };"
— the channel binding is used only to reach `origin_endpoints`; `channel.input_type` / `channel.is_multiview()` is never consulted anywhere in the handler (the only request-vs-store check is `names_unpublished_manifest(&input.harvested_manifests, endpoint)` at create_harvest_job.rs:322).

Test coverage: grep of tests/all for `MULTIVIEW` shows multiview rows for channel policy (channel_policy.rs:672-688), reset (channel_reset_multiview.rs:202), delete (delete_channel_multiview_source.rs) and configuration validation — none for harvest jobs; harvest_job_idempotency.rs / harvest_job_refusal_text.rs / harvest_job_tagging.rs contain no multiview case, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateChannelGroup `g`.
2. CreateChannel `g`/`src` with `{"InputType": "CMAF"}`.
3. CreateChannel `g`/`mv` with `{"InputType": "MULTIVIEW", "MultiviewConfiguration": {"AvailableSources": ["src"], "AvailableLayouts": [...]}}`.
4. CreateOriginEndpoint `g`/`mv`/`e` with `ContainerType` CMAF and one `HlsManifests` entry named `index`.
5. CreateHarvestJob `g`/`mv`/`e` with `HarvestedManifests.HlsManifests=[{"ManifestName":"index"}]`, a valid `ScheduleConfiguration` and `Destination`. Observe HTTP 200 and a QUEUED job persisted under a MULTIVIEW channel, where the service names `ValidationException` / `MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB` for this combination; GetHarvestJob then reports it.
