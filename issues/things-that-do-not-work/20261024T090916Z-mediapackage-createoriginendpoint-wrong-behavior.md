Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# DASH origin endpoints omit the live-manifest server defaults (manifestLayout, utcTiming, manifestWindowSeconds, minUpdatePeriodSeconds, suggestedPresentationDelaySeconds, segmentDurationSeconds)

_Filed: 2026-10-24T09:09:16Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MediaPackage does not store an OriginEndpoint as the request wrote it — every optional packaging member the request leaves unset is server-filled with that member's service default, stored, and returned on EVERY projection of the endpoint (the create response, DescribeOriginEndpoint, each entry of ListOriginEndpoints, and the UpdateOriginEndpoint response). For a `DashPackage` created with `"dashPackage": {}` the service fills the whole DASH configuration, including the live-manifest members:

- `manifestLayout` -> `FULL`
- `utcTiming` -> `NONE`
- `manifestWindowSeconds` -> 60
- `minUpdatePeriodSeconds` -> 15
- `suggestedPresentationDelaySeconds` -> 25
- `segmentDurationSeconds` -> 2

Actual: the emulator fills only SEVEN of the DashPackage members and leaves the six above absent from the body. `crate::state::fill_dash_package` (src/state.rs ~915-942) assigns exactly `ad_triggers`, `ads_on_delivery_restrictions`, `include_iframe_only_stream`, `min_buffer_time_seconds`, `period_triggers`, `profile` and `segment_template_format`; there is no `DEFAULT_DASH_MANIFEST_LAYOUT`, `DEFAULT_DASH_UTC_TIMING`, `DEFAULT_DASH_MANIFEST_WINDOW_SECONDS`, `DEFAULT_DASH_MIN_UPDATE_PERIOD_SECONDS`, `DEFAULT_DASH_SUGGESTED_PRESENTATION_DELAY_SECONDS` or `DEFAULT_DASH_SEGMENT_DURATION_SECONDS` constant anywhere in the crate. Because `apply_origin_endpoint_defaults` is the single write-path filler shared by CreateOriginEndpoint (create_origin_endpoint.rs line 317) and UpdateOriginEndpoint (update_origin_endpoint.rs line 327), the stored record is under-populated and every later read of a DASH endpoint reports a configuration AWS never returns.

Consequence: a client that reads DASH endpoint configuration back — to decide whether to update it, to render it in a UI, or to deserialize into a non-optional field — gets null where AWS always supplies a value. An enum parse of `manifestLayout`/`utcTiming` or an integer read of `manifestWindowSeconds` fails on the emulator only, and drift-detection tooling sees every DASH endpoint as unconfigured. This is the same fault the already-fixed issue `20261022T124635Z-mediapackage-origin-endpoint-projections-omit-defaulted-members.md` closed for the HLS path and for the endpoint-level members (origination / startoverWindowSeconds / timeDelaySeconds); the DASH live-only members were never part of that fix because the recording it was derived from covered HLS only.

## Evidence

Spec grounding: `smithy_query(query="shape",shape="com.amazonaws.mediapackage#DashPackage")` declares all six members — `ManifestLayout` (enum `ManifestLayout`, jsonName `manifestLayout`), `UtcTiming` (enum `UtcTiming`, `utcTiming`), `ManifestWindowSeconds` ("Time window (in seconds) contained in each manifest"), `MinUpdatePeriodSeconds` ("Minimum duration (in seconds) between potential changes to the ... MPD"), `SuggestedPresentationDelaySeconds` ("Duration (in seconds) to delay live content before presentation") and `SegmentDurationSeconds` — and the model publishes no `@default` on any DashPackage member, which is exactly why the crate's own default table is the thing that has to carry them.

Well-established service behaviour: MediaPackage server-fills the DASH packaging defaults published for a live DASH endpoint (manifest layout Full, UTC timing None, manifest window 60 s, minimum update period 15 s, suggested presentation delay 25 s, segment duration 2 s, min buffer time 30 s). The crate already accepts and implements this rule for the SAME shape — `fill_dash_package` fills `profile` = NONE, `segmentTemplateFormat` = NUMBER_WITH_TIMELINE, `periodTriggers` = [] and the numeric `minBufferTimeSeconds` = 30 — so there is no principled line on which AWS would populate `minBufferTimeSeconds` and `profile` but leave the four sibling live-manifest timings and the two sibling enums absent. The crate's own comment (src/state.rs ~747-760) explains the omission as "a member whose default neither the model nor a recording publishes is left exactly as the request sent it ... the DASH window/timing members", and names its source as a MediaPackage **VOD** recording — a service whose DashPackage has no live manifest window / min-update-period / suggested-presentation-delay / utcTiming members at all, so their absence from that recording is evidence about VOD, not about this service.

Handler/state: `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs` lines 915-942 (`fill_dash_package`, the complete assignment list) and 949-969 (`apply_origin_endpoint_defaults`, the single write-path filler); `src/ops/create_origin_endpoint.rs` line 317 and `src/ops/update_origin_endpoint.rs` line 327 call it; `OriginEndpointRecord::to_origin_endpoint` (state.rs ~1078) then projects the stored (under-populated) DashPackage into the create, describe, list and update responses unchanged.

Test coverage: `tests/all/origin_endpoint_defaults.rs` pins the DASH default table it has — `dash_package_defaults()` (lines 127-138) lists only the seven members the emulator fills, and `expected_members`/`assert_members` assert only that those are present with those values; no row asserts the six members above in either direction, so they are UNCOVERED rather than deliberately pinned. (Note for the fixer: the separate test `a_package_with_no_published_defaults_gains_no_invented_member` (lines 351-393) does pin `mssPackage` and a supplied `streamSelection` block as "stored exactly as sent"; this issue is deliberately scoped to DashPackage and does not ask for MssPackage or StreamSelection to change.)

## Steps to Reproduce

1. Boot the emulator and point an SDK/boto3 client at it.
2. mp.create_channel(Id="c1").
3. mp.create_origin_endpoint(Id="oe-dash", ChannelId="c1", DashPackage={}) -> observe HTTP 200 whose $.dashPackage carries adTriggers, adsOnDeliveryRestrictions, includeIframeOnlyStream, minBufferTimeSeconds, periodTriggers, profile and segmentTemplateFormat, but no manifestLayout, utcTiming, manifestWindowSeconds, minUpdatePeriodSeconds, suggestedPresentationDelaySeconds or segmentDurationSeconds.
4. mp.describe_origin_endpoint(Id="oe-dash") -> the same six members are absent from $.dashPackage.
5. mp.list_origin_endpoints(ChannelId="c1") -> the entry for oe-dash omits the same six members.
6. mp.update_origin_endpoint(Id="oe-dash", DashPackage={"minBufferTimeSeconds": 10}) -> the 200 response again omits the six members, so no write path ever materialises them.
7. Repeat step 3 supplying each of the six members explicitly (e.g. DashPackage={"manifestLayout": "COMPACT", "utcTiming": "HTTP-HEAD", "manifestWindowSeconds": 30, "minUpdatePeriodSeconds": 5, "suggestedPresentationDelaySeconds": 10, "segmentDurationSeconds": 4}) -> these are echoed back correctly, confirming the gap is the missing server fill and not the serializer.
