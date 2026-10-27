Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# A CMAF package carrying two child HLS manifests with the same Id is stored instead of being refused

_Filed: 2026-10-27T04:29:02Z_
_Operation: CreateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `CmafPackage`'s child manifest `Id` is documented as unique within the OriginEndpoint, but nothing in this crate enforces it. `CreateOriginEndpoint` (and `UpdateOriginEndpoint`, through the same shared helpers) accepts `cmafPackage.hlsManifests: [{"id":"a"}, {"id":"a"}]` with HTTP 200 and stores both entries.

The crate validates this list in three other ways — entry count, per-entry string lengths (`bound_origin_endpoint_members`), a minimum of one manifest (`require_cmaf_hls_manifests`) and the per-entry duration floors (`bound_origin_endpoint_durations`) — so the list IS judged; the uniqueness rule the model states for the member is the one rule missing. The crate already enforces the identical model sentence for the resource `Id` ("The ID must be unique within the region"), refusing a second create with `resource_already_exists`, so the rule exists in this crate for one id and is skipped for the other.

The consequence is worse than a merely lenient accept: the stored child `Url` is derived from `stable_label(&[channel_id, endpoint_id, manifest_id, region])`, so two children sharing an id are stored with the SAME derived playback `Url` and the same `ManifestName` — two entries in `cmafPackage.hlsManifests` that every later `DescribeOriginEndpoint`/`ListOriginEndpoints` reports as indistinguishable duplicates, which a client keying children by their (documented-unique) id cannot address apart.

## Evidence

Model (kind A) — `smithy_query(query="shape",shape="com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters")`, member `Id`:

  "The ID of the manifest. The ID must be unique within the OriginEndpoint and it cannot be changed after it is created."

`UnprocessableEntityException` (httpError 422, "The parameters sent in the request are not valid") is the operation's modelled client-input error.

Handler — the complete set of pre-store checks for CreateOriginEndpoint, none of which looks at child ids as a set: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_origin_endpoint.rs:214-252` (`check_created_id`, `bound_chars`, `bound_tags`, `bound_origin_endpoint_members`, `bound_origin_endpoint_durations`, `check_origin_endpoint_members`).

The helpers those calls reach, which judge the manifest list by size and count only:
- `src/state.rs:678-700` — `bound_entries("cmafPackage.hlsManifests", manifests.len(), MAX_CMAF_HLS_MANIFESTS)` then per-entry `bound_part_chars(... "an id", &manifest.id, MAX_STRING_CHARS)`.
- `src/state.rs:939-953` — `require_cmaf_hls_manifests`: "Refuse a `CmafPackage` that publishes no child manifest at all" (count `< MIN_CMAF_HLS_MANIFESTS` only).
- `src/state.rs:962-981` — `check_origin_endpoint_members` = whitelist + ARNs + `require_cmaf_hls_manifests`, and nothing else.

In-crate corroboration (kind B) — the same model sentence IS enforced for the resource id, in this very handler: `src/ops/create_origin_endpoint.rs:299-307`

  // "The ID must be unique within the region": a second create on an id already in
  // use in this Region is refused, never an overwrite.
  if state.has_origin_endpoint(&region, &input.id) { return Err(crate::state::resource_already_exists(...)) }

Duplicate ids collapse to one URL: `src/state.rs:2600-2612` — `cmaf_manifest_url` builds `manifest_label = stable_label(&[channel_id, endpoint_id, manifest_id, region])`, and `src/state.rs:2631-2659` (`stored_cmaf_package`) maps every entry through it, so two entries with `manifest.id == "a"` are stored with identical `url` and identical `manifestName`.

Same helpers on the update path, so one fix covers both writes: `src/ops/update_origin_endpoint.rs:251-279` calls the identical `bound_origin_endpoint_members` / `check_origin_endpoint_members`.

Test coverage: no test sends two children with the same id. `tests/all/origin_endpoint_contents.rs:788-832` (`a_cmaf_package_at_or_above_the_manifest_minimum_is_stored_and_read_back`) builds its list as `(0..count).map(|index| json!({ "id": format!("m-{index}") }))` — distinct ids only; `tests/all/channel_lifecycle.rs:939` builds `(0..51)` distinct ids for the count bound; `tests/all/cmaf_child_manifest_name.rs` and `tests/all/cmaf_child_manifest_name_follows_the_endpoint.rs:211-236` use distinct ids (`"a"`, `"primary"`). A repo-wide grep for `duplicate` in `src/` returns only the resource-id rule. So the path is uncovered, not deliberately pinned.

## Steps to Reproduce

1. Boot the emulator and point an SDK/curl at it in one Region.
2. POST /channels with {"id":"c1"}.
3. POST /origin_endpoints with {"id":"oe1","channelId":"c1","cmafPackage":{"hlsManifests":[{"id":"a"},{"id":"a"}]}} -> observe HTTP 200; the model states the manifest ID "must be unique within the OriginEndpoint", so this request names no storable configuration.
4. GET /origin_endpoints/oe1 -> the response carries two cmafPackage.hlsManifests entries with the same "id", the same "manifestName" and the same derived "url", which no client can tell apart.
5. Repeat with PUT /origin_endpoints/oe1 and the same body -> the duplicate pair is accepted there too (same shared helpers).
