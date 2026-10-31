Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A ClientToken already spent on another origin endpoint of the same channel is read as Fresh, so a changed create under it mints a second job instead of the modelled mismatch

_Filed: 2026-10-29T01:46:15Z_
_Operation: CreateHarvestJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateHarvestJob` resolves its idempotency token against the harvest jobs of the ADDRESSED origin endpoint only, while a harvest job's identity namespace in this crate is the whole CHANNEL. The two scopes disagree, and the disagreement is reachable: a second, DIFFERENT create that reuses a token already spent on a job under a sibling endpoint of the same channel finds no stored job carrying that token, so `match_client_token` answers `Fresh` and the handler creates a second job holding the same `ClientToken`.

The handler's own module doc states the rule the code then breaks: a repeat under the same token is the SAME request, and one that changed any other member is the modelled `IDEMPOTENT_PARAMETER_MISMATCH` conflict. The request's fingerprint even includes `OriginEndpointName` (so a changed endpoint IS a changed request by the crate's own definition) — but that field can never be compared, because a job stored under another endpoint is never offered to the matcher. The result is a channel holding two harvest jobs minted by two different requests under one idempotency token, where the crate says the second must be refused 409.

The name half of the same decision was already moved to channel scope (`Channel::holds_harvest_job`), so this handler now judges the name over the channel and the token over one endpoint in the same call.

## Evidence

The rule the handler states — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_harvest_job.rs:7-10:
```
//! The operation is `@idempotent` and binds `ClientToken` with `@idempotencyToken`, so a
//! repeat under the same token is the SAME request: an unchanged one replays the job that
//! token already created and creates nothing, while one that changed any other member is
//! the modelled `IDEMPOTENT_PARAMETER_MISMATCH` conflict.
```

The fingerprint declares `OriginEndpointName` a covered member — src/ops/create_harvest_job.rs:344-347:
```
        let fingerprint = crate::state::request_fingerprint(&[
            input.channel_group_name.clone(),
            input.channel_name.clone(),
            input.origin_endpoint_name.clone(),
```

...but the candidate set the token is searched in is one endpoint's map — src/ops/create_harvest_job.rs:361-371:
```
        match crate::state::match_client_token(
            input.client_token.as_deref(),
            &fingerprint,
            endpoint.harvest_jobs.values().map(|job| {
                (
                    job.name.as_str(),
                    job.client_token.as_deref(),
                    job.fingerprint.as_str(),
                )
            }),
        ) {
```
and `match_client_token` answers `Fresh` when no offered sibling carries the token — src/state.rs:2285-2294:
```
    for (name, stored_token, stored_fingerprint) in stored {
        if stored_token == Some(token) {
            return if stored_fingerprint == fingerprint {
                TokenMatch::Replay(name.to_string())
            } else {
                TokenMatch::Mismatch
            };
        }
    }
    TokenMatch::Fresh
```

The other half of the SAME create decides over the whole channel — src/ops/create_harvest_job.rs:392-399:
```
            Some(chosen) => {
                if channel.holds_harvest_job(&chosen) {
```
with the crate's reason, src/state.rs:1099-1106:
```
    /// Every harvest job this channel holds, whichever of its origin endpoints owns it.
    ///
    /// A job record is persisted on the endpoint whose content it exports, but the model
    /// gives the job's NAME the CHANNEL's namespace
    /// (`CreateHarvestJobRequest$HarvestJobName`: "This name must be unique within the
    /// channel"), so this walk — not one endpoint's map — is the collection a job name is
    /// judged against.
```

No test pins today's behaviour: the mismatch table mutates body members only and never the addressed endpoint — tests/all/harvest_job_idempotency.rs:335-337:
```
/// Every member of the request the token does NOT cover, one row each: the names, the
/// description, the schedule, the destination, the harvested manifests and the tags.
const MUTATIONS: &[Mutation] = &[
```
(`create` in that suite always posts to one endpoint's collection), and tests/all/harvest_job_name_namespace.rs:94-107 (`create_job`) sends no `x-amzn-client-token` at all.

## Steps to Reproduce

1. Boot the emulator and create channel group `g`, channel `c`, and two origin endpoints `a` and `b` under `c`, each publishing an HLS manifest named `index`.
2. POST /channelGroup/g/channel/c/originEndpoint/a/harvestJob with header `x-amzn-client-token: T`, a valid ScheduleConfiguration, Destination, and HarvestedManifests.HlsManifests=[{ManifestName: "index"}]. Observe 200 and note the job name the service chose.
3. POST /channelGroup/g/channel/c/originEndpoint/b/harvestJob with the SAME header `x-amzn-client-token: T` and the same body. This is a different request (it addresses endpoint `b`, a member the crate's own fingerprint covers).
4. Observe 200 and a second job created, instead of the 409 ConflictException / IDEMPOTENT_PARAMETER_MISMATCH the handler doc states for a repeat under a spent token that changed a member.
5. GET /channelGroup/g/harvestJob?maxResults=100 — two jobs are listed in the channel, both minted under token T.
