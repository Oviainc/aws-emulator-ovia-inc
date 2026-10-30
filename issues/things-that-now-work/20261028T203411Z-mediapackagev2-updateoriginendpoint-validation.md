Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# UpdateOriginEndpoint replaces the manifest set without consulting the endpoint's stored harvest jobs, leaving jobs that harvest manifests the endpoint no longer publishes

_Filed: 2026-10-28T20:34:11Z_
_Operation: UpdateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An origin endpoint's harvest jobs are persisted ON the endpoint record (`OriginEndpoint::harvest_jobs`) and each one names, per family, the `ManifestName`s it is harvesting. `CreateHarvestJob` exists to keep that reference sound: it refuses a job that names any manifest the endpoint does not publish, with 400 `ValidationException` / `HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT`.

`UpdateOriginEndpoint` then replaces the WHOLE manifest set of the same endpoint — `endpoint.hls_manifests`, `low_latency_hls_manifests`, `dash_manifests`, `mss_manifests` are each overwritten from the request (a family the request omits becomes empty) — and never looks at `endpoint.harvest_jobs`. The handler's own doc header enumerates everything it refuses (three immutable members, the manifest-set rules, the ETag precondition); the stored harvest jobs are not among them, and `names_unpublished_manifest`, the predicate the crate already has for exactly this question, is not called.

So a successful update can leave the store in the state `CreateHarvestJob` is written to prevent: a QUEUED or IN_PROGRESS harvest job whose `HarvestedManifests` name manifests the endpoint no longer publishes. `GetHarvestJob` / `ListHarvestJobs` keep reporting those names while `GetOriginEndpoint` reports a disjoint manifest set, and re-creating the very same job is refused 400 — the read side and the write side of one invariant disagree. The crate already refuses a write that would break a stored cross-reference elsewhere: `DeleteChannel` refuses to delete a channel a multiview channel still lists as an available source, via `crate::state::multiview_source_in_use()` (409). `UpdateOriginEndpoint` declares `ConflictException` in the model for precisely this class of outcome.

## Evidence

The invariant one handler enforces — `services/.../src/ops/create_harvest_job.rs:322-333`:

    // A harvest job exports manifests the endpoint actually publishes, so each
    // requested manifest name is resolved against the endpoint's own manifests.
    if names_unpublished_manifest(&input.harvested_manifests, endpoint) {
        return Err(CreateHarvestJobErrorModeled::ValidationException(
            crate::state::validation_failed(
                "harvestedManifests",
                UNPUBLISHED_MANIFEST_REQUIREMENT,
                crate::state::VALIDATION_HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT,
            ),
        )
        .into());
    }

and the rule it states, `create_harvest_job.rs:165-167`:

    const UNPUBLISHED_MANIFEST_REQUIREMENT: &str = "A harvest job can harvest only the \
        manifests its origin endpoint publishes. Verify the manifest names against the \
        manifests of the origin endpoint and then try again";

The same invariant broken by the sibling writer — `services/.../src/ops/update_origin_endpoint.rs:477-500`:

        endpoint.hls_manifests = input
            .hls_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_hls_manifest(manifest, at))
            .collect();
        endpoint.low_latency_hls_manifests = input
            .low_latency_hls_manifests
            .unwrap_or_default()
            ...

Nothing between the lookup (`update_origin_endpoint.rs:402-410`) and that write reads `endpoint.harvest_jobs`, and the handler's own list of refusals (`update_origin_endpoint.rs:11-22`) names only: "a member the model documents as immutable after creation", "a manifest set the new `ContainerType` cannot serve, no manifest at all, or more manifests than ...", "an `ETag` precondition".

The jobs demonstrably survive the update: `state.rs:913-924` scrubs them out of the endpoint's fingerprint — "The harvest jobs are left out for the same reason the attached policy is: a sub-resource commits versions of its own" — so they stay in `endpoint.harvest_jobs` with their old `HarvestedManifests`.

The crate's own precedent for refusing a write that would break a stored cross-reference, `services/.../src/ops/delete_channel.rs:187-201`: "a channel cannot be deleted while a multiview channel of the same group still lists it as an available source, because the deletion would leave that reference pointing at nothing" -> `crate::state::multiview_source_in_use()`.

Model: `UpdateOriginEndpoint` declares `ConflictException` (HTTP 409) — "Updating or deleting this resource can cause an inconsistent state."

Test coverage: `tests/all/update_origin_endpoint.rs` and `tests/all/update_roundtrip.rs` exercise manifest replacement, `tests/all/rejection_rule_and_negative_paths.rs` (`harvest_jobs_run_their_lifecycle_out_of_persisted_state`, line ~1173) exercises the harvest-job lifecycle — no test combines the two, so nothing pins the current outcome.

## Steps to Reproduce

1. CreateChannelGroup g; CreateChannel g/c; CreateOriginEndpoint g/c/e with ContainerType TS and HlsManifests=[{"ManifestName":"index"}].
2. CreateHarvestJob on g/c/e with HarvestedManifests.HlsManifests=[{"ManifestName":"index"}] and a schedule window; observe 200, job stored QUEUED.
3. UpdateOriginEndpoint g/c/e with ContainerType TS and HlsManifests=[{"ManifestName":"other"}]; observe 200 and the endpoint now publishing only "other".
4. GetHarvestJob on the job: it still reports HarvestedManifests.HlsManifests=[{"ManifestName":"index"}], a manifest GetOriginEndpoint no longer lists.
5. CreateHarvestJob again with HarvestedManifests.HlsManifests=[{"ManifestName":"index"}]: refused 400 ValidationException / HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT — the state the update just created is one the create refuses to produce.
