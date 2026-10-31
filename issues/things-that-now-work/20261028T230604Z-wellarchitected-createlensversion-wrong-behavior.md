Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc

# CreateLensVersion fingerprints the RAW Optional IsMajorVersion, so a retry that spells the default explicitly is refused 409 instead of replaying the publish it already made

_Filed: 2026-10-28T23:06:04Z_
_Operation: CreateLensVersion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLensVersion` builds its `@idempotencyToken` replay fingerprint over the raw `input.is_major_version` (an `Option<bool>`), but the version it STORES is `input.is_major_version.unwrap_or(false)`. An omitted `IsMajorVersion` and an explicit `IsMajorVersion: false` therefore publish an IDENTICAL version record, yet they hash to different fingerprints (serde renders `None` as `null` and `Some(false)` as `false` — the crate states exactly that rendering rule in `src/ops/update_agent_profile.rs:217-221`).

Consequence: after a publish under token T that omitted `IsMajorVersion` has completed successfully (200, `{LensArn, LensVersion}`), the identical retry under token T that spells the member `false` — the same lens, the same `LensVersion`, the same stored `PublishedLensVersion` — is answered `409 ConflictException` ("the token was already spent on a request with different parameters") instead of the replayed success the token exists to give. A success is turned into an error for a request that asks for exactly what the original request already did.

The crate owns the opposite convention and states it for the sibling creates: a fingerprint is taken over the NORMALISED values that the create would record, so two requests that would record the same resource are the same request. `CreateProfile` says so in as many words and builds the record before the fingerprint; `CreateWorkload` fingerprints `account_ids`, `applications`, `pillar_priorities` and `tags` AFTER their `unwrap_or_default()` / defaulting, so omitting a defaulted list and sending its default value are the same request there. `CreateLensVersion` skips that normalisation for the one member it defaults itself.

## Evidence

Model rule being implemented — `CreateLensVersionInput$ClientRequestToken` (`smithy_query(query="shape", shape="ClientRequestToken")`): "A unique case-sensitive string used to ensure that this request is idempotent (executes only once). … If you retry a request with the same client request token and the same parameters after the original request has completed successfully, the result of the original request is returned." `IsMajorVersion` is a plain optional boolean with no `@default` (`smithy_query(query="operation", operation="CreateLensVersion")`: `"IsMajorVersion": {"target": "IsMajorVersion", "type": "boolean"}`), so an omitted member really does reach the handler as `None`.

Side 1 — the fingerprint is over the raw option: `src/ops/create_lens_version.rs:180-184` — `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "IsMajorVersion": &input.is_major_version, "LensAlias": &input.lens_alias, "LensVersion": &input.lens_version, }));`

Side 1b — yet the stored record collapses the two spellings: `src/ops/create_lens_version.rs:268-275` — `record.versions.insert(input.lens_version.clone(), crate::state::PublishedLensVersion { content, // "Set to true if this new major lens version": a request that omits the // member is not publishing a major version. is_major_version: input.is_major_version.unwrap_or(false), published_at: now, });` — so omitted and `false` publish the identical version. The mismatch path is the 409: `src/ops/create_lens_version.rs:196-205` — `if token.fingerprint != fingerprint { return Err(CreateLensVersionErrorModeled::ConflictException(crate::state::idempotency_token_conflict(…)).into()); }`

Side 2 — the sibling creates that apply the normalisation, with the rule stated: `src/ops/create_profile.rs:204-207` — "The record is built before the fingerprint so the fingerprint covers the NORMALIZED questions ([`crate::state::ProfileRecord::question_fingerprint`]): two requests that would record the same profile are the same request, however their question list was ordered." `src/ops/create_workload.rs:317-363` — `let profile_arns = input.profile_arns.unwrap_or_default(); … let account_ids = input.account_ids.unwrap_or_default(); let applications = input.applications.unwrap_or_default(); let pillar_priorities = match input.pillar_priorities { Some(priorities) if !priorities.is_empty() => priorities, _ => crate::state::default_pillar_priorities(), };` followed by `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "AccountIds": &account_ids, "Applications": &applications, … "PillarPriorities": &pillar_priorities, "ProfileArns": &profile_arns, … }));` — every defaulted member is fingerprinted AFTER defaulting.

Test coverage that must stay green, and does: `tests/all/custom_lens_versions.rs:494-545` (`a_publish_is_idempotent_per_client_request_token`) builds its rows as `(case, version, IsMajorVersion, same-request?)` = `("same-parameters", "1.0", Some(true), true)`, `("different-version", "2.0", Some(true), false)`, `("different-is-major", "1.0", Some(false), false)`, `("is-major-omitted", "1.0", None, false)` — the FIRST publish of every row sends `Some(true)` (`:510`), so normalising the member before fingerprinting leaves `Some(false)` and `None` both differing from `true` and all four rows keep their current status. No row exercises omitted-then-`false`.

## Steps to Reproduce

1. ImportLens a custom lens document and wait for the import to land (crate::state::LENS_IMPORT_DURATION)
2. POST /lenses/<lens ARN>/versions with body {"LensVersion": "1.0", "ClientRequestToken": "tok-1"} — IsMajorVersion omitted. Observe 200 and a version published with is_major_version = false (src/ops/create_lens_version.rs:274)
3. Retry the same publish with the member spelled explicitly: POST /lenses/<lens ARN>/versions with {"LensVersion": "1.0", "IsMajorVersion": false, "ClientRequestToken": "tok-1"}
4. Observe 409 ConflictException from the fingerprint mismatch at src/ops/create_lens_version.rs:196-205, instead of the replayed 200 {LensArn, LensVersion: "1.0"} the ClientRequestToken rule quoted above promises — although the second request would publish exactly the record the first one did
5. Compare with CreateWorkload, where the same shape of retry (omitting AccountIds vs sending []) replays, because the fingerprint is taken over the defaulted value (src/ops/create_workload.rs:322 and :333)
