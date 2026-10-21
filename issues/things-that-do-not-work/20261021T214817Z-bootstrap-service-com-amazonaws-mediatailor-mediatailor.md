Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MediaTailor emulator

_Filed: 2026-10-21T21:48:17Z_
_Operation: service-com-amazonaws-mediatailor-mediatailor | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 48 handlers in `services/service-com-amazonaws-mediatailor-mediatailor/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no MediaTailor client can do anything at all. Implement a FIRST minimal, stateful slice: the playback-configuration lifecycle, which is MediaTailor's primary resource and the entry point of every ad-insertion workflow (restJson1, sigv4 name `mediatailor`).

### Happy path to implement (in order)

1. **PutPlaybackConfiguration** — `PUT /playbackConfiguration`. Input: `Name` (required), `AdDecisionServerUrl`, `VideoContentSourceUrl`, `SlateAdUrl`, `CdnConfiguration`, `DashConfiguration` (DashConfigurationForPut), `ManifestProcessingRules`, `AvailSuppression`, `Bumper`, `ConfigurationAliases`, `LivePreRollConfiguration`, `PersonalizationThresholdSeconds`, `TranscodeProfileName`, `InsertionMode` (default `STITCHED_ONLY`), `Tags`. Must create-or-replace the named configuration in the store and return the PERSISTED record, including the server-derived fields: `PlaybackConfigurationArn` (`arn:aws:mediatailor:<region>:<account>:playbackConfiguration/<Name>`), `PlaybackEndpointPrefix`, `SessionInitializationEndpointPrefix`, `HlsConfiguration.ManifestEndpointPrefix`, `DashConfiguration.ManifestEndpointPrefix` plus the echoed-back-from-state `MpdLocation`/`OriginManifestType`, `LogConfiguration.PercentEnabled` (0 by default), and `InsertionMode` defaulted when absent. Calling it twice with the same `Name` must update in place, not create a duplicate.
2. **GetPlaybackConfiguration** — `GET /playbackConfiguration/{Name}`. Must return exactly what was stored by the preceding Put (same ARN, same endpoint prefixes, same `Tags`, same nested config structures), NOT a canned or echoed body. An unknown `Name` must fail (404-class error), never a hollow 200 with an empty body.
3. **ListPlaybackConfigurations** — `GET /playbackConfigurations`. Must return `Items` built from the stored map (each item carrying `Name`, `PlaybackConfigurationArn`, `AdDecisionServerUrl`, `VideoContentSourceUrl`, `Tags`, endpoint prefixes), and must honour `MaxResults` (1..100) by truncating and emitting an opaque `NextToken` that a follow-up call resumes from; omit `NextToken` on the last page.
4. **DeletePlaybackConfiguration** — `DELETE /playbackConfiguration/{Name}` → 204. Must remove the record so a subsequent GetPlaybackConfiguration fails and ListPlaybackConfigurations no longer contains it.

Secondary (nice-to-have in the same slice, cheap once the store exists): **TagResource** / **ListTagsForResource** keyed by the configuration ARN, so tags written at Put time or via TagResource are readable back through `GET /tags/{ResourceArn}`.

All four operations must compute their responses from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` — no hardcoded bodies, no echoing the request back without storing it, no randomly generated ARNs that are not saved. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each handler you implement.

## State Model

In `src/state.rs` (`ComAmazonawsMediatailorMediatailorState`):

- `playback_configurations: BTreeMap<String /*Name*/, PlaybackConfigurationRecord>` — ordered by name so list pagination is deterministic.
- `PlaybackConfigurationRecord { name: String, arn: String, ad_decision_server_url: Option<String>, video_content_source_url: Option<String>, slate_ad_url: Option<String>, insertion_mode: String /*default "STITCHED_ONLY"*/, personalization_threshold_seconds: Option<i32>, transcode_profile_name: Option<String>, cdn_configuration: Option<CdnConfiguration>, dash_configuration: Option<DashConfiguration>, hls_configuration: Option<HlsConfiguration>, manifest_processing_rules: Option<ManifestProcessingRules>, avail_suppression: Option<AvailSuppression>, bumper: Option<Bumper>, configuration_aliases: Option<Map<String, Map<String,String>>>, live_pre_roll_configuration: Option<LivePreRollConfiguration>, log_configuration_percent_enabled: i32, playback_endpoint_prefix: String, session_initialization_endpoint_prefix: String, tags: BTreeMap<String,String> }`
- `tags_by_arn: BTreeMap<String /*ResourceArn*/, BTreeMap<String,String>>` — the single source of truth for TagResource/UntagResource/ListTagsForResource; the Put path seeds it from the request `Tags`.
- `list_tokens`: pagination cursors may be encoded as the last-returned `Name` (opaque base64) rather than stored, so no extra structure is required.

All other resources (channels, programs, source locations, live/VOD sources, prefetch schedules, functions, channel policies, alerts) stay 501 for now and will be filed as individual gap issues.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-mediatailor-mediatailor/src/ops/` matches all 48 operation files (every handler returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501), so the service holds zero state. Contracts cited from the crate's generated `OPERATIONS.md`: PutPlaybackConfiguration (`PUT /playbackConfiguration`, required `Name`, `InsertionMode` default `STITCHED_ONLY`, output carries `PlaybackConfigurationArn`, `PlaybackEndpointPrefix`, `SessionInitializationEndpointPrefix`, `HlsConfiguration`, `DashConfiguration`, `LogConfiguration`), GetPlaybackConfiguration (`GET /playbackConfiguration/{Name}`, @readonly), ListPlaybackConfigurations (`GET /playbackConfigurations`, @paginated inputToken=NextToken/outputToken=NextToken/items=Items, `MaxResults` range 1..100), DeletePlaybackConfiguration (`DELETE /playbackConfiguration/{Name}` → 204, @idempotent), ListTagsForResource/TagResource (`/tags/{ResourceArn}`, BadRequestException 400). Protocol `restJson1` per OPERATIONS.md 'Wire protocol'. No tests exist beyond the scaffold's `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI MediaTailor client at it.
2. `aws mediatailor put-playback-configuration --name demo --ad-decision-server-url https://ads.example.com/vast --video-content-source-url https://cdn.example.com/out/v1/master.m3u8 --tags Env=dev` — expect 200 with `PlaybackConfigurationArn` ending in `playbackConfiguration/demo` and populated `PlaybackEndpointPrefix`/`SessionInitializationEndpointPrefix`/`HlsConfiguration.ManifestEndpointPrefix`; currently 501.
3. `aws mediatailor get-playback-configuration --name demo` — expect the identical record (same ARN, same URLs, `Tags={Env=dev}`, `InsertionMode=STITCHED_ONLY`) read back from state.
4. `aws mediatailor list-playback-configurations --max-results 1` after creating two configurations — expect exactly one item plus a `NextToken`, and the second page to return the other item with no `NextToken`.
5. `aws mediatailor list-tags-for-resource --resource-arn <ARN from step 2>` — expect `{"Tags":{"Env":"dev"}}`.
6. `aws mediatailor delete-playback-configuration --name demo` — expect 204, then `get-playback-configuration --name demo` must fail with a 404-class error and `list-playback-configurations` must no longer list `demo`.
