Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# An update that does not resend cmafPackage leaves the stored CMAF child manifest names and URLs derived from the OLD ManifestName

_Filed: 2026-10-27T02:20:48Z_
_Operation: UpdateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateOriginEndpoint` resolves the endpoint's effective `ManifestName` for the request (the value sent, or the service default when the request omits it), re-derives the endpoint's own playback `Url` from it, and writes it onto the record. But it only re-materialises the CMAF package — the one place the child HLS manifests' inherited `manifestName` and their derived `url` are computed (`crate::state::stored_cmaf_package`) — in the branch where the REQUEST carried a packaging configuration. When the request names no packaging configuration (a legal request the handler explicitly supports: "a request that names no packaging configuration leaves the stored one in place"), the stored `cmafPackage.hlsManifests[]` entries are left untouched, so a child that was stored by inheritance keeps the PREVIOUS effective `ManifestName` and a `url` built from it.

Result for a CMAF endpoint: after `PUT /origin_endpoints/{Id}` that sends a new `manifestName` (or that omits `manifestName`, which resets the endpoint to the `index` default) and no `cmafPackage`, the stored record reports `manifestName: "replay"` with `url` ending in `/replay/index.m3u8`, while its child manifest still reports `manifestName: "stream"` and a `url` ending in `/stream/index.m3u8` — a child URL on a path the endpoint no longer serves, and a defaulted child name that is no longer "the manifestName for the OriginEndpoint". The wrong values are PERSISTED: `DescribeOriginEndpoint` and `ListOriginEndpoints` render the same stored record afterwards.

This is the residual of the already-fixed inheritance gap (20261024T211643Z): the fix threaded the endpoint's effective name into `stored_cmaf_package`, which repairs every write path that re-materialises the package, but the no-packaging update path never calls that helper at all, so the inherited value is never refreshed when the endpoint's own `ManifestName` changes underneath it.

## Evidence

Smithy model states the child's value IS the endpoint's: `smithy_query(query="doc",shape="HlsManifest",member="ManifestName")` -> "An optional short string appended to the end of the OriginEndpoint URL. If not specified, defaults to the manifestName for the OriginEndpoint." (the same sentence on `HlsManifestCreateOrUpdateParameters$ManifestName`). `HlsManifest` is the shape projected back on every read, so the rule governs the stored/projected value, not just the write.

The crate states the same rule for itself — services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:41-44: "A CMAF child manifest's own `ManifestName` documents a different default — \"defaults to the manifestName for the OriginEndpoint\" — so a child inherits the endpoint's effective value (see [`stored_cmaf_package`]) and reaches this literal only through an endpoint that took it itself."

The handler breaks it on one branch — services/service-com-amazonaws-mediapackage-mediapackage/src/ops/update_origin_endpoint.rs:302-305 resolves the new effective name:
```
        let manifest_name = input
            .manifest_name
            .take()
            .unwrap_or_else(|| crate::state::DEFAULT_MANIFEST_NAME.to_string());
```
:329-341 re-materialises the children ONLY when the request carried a package (`record.cmaf_package = input.cmaf_package.as_ref().map(|params| crate::state::stored_cmaf_package(params, &region, &channel_id, &id, &manifest_name))`), while :344-347 takes the other branch without touching the stored package at all:
```
            // An endpoint always packages its channel in exactly one format, so a request
            // that names no packaging configuration leaves the stored one in place: no
            // request can leave an endpoint with nothing to package.
            None => match record.packaging_kind() {
```
and :372-375 then moves the endpoint's own URL and name forward anyway:
```
        // The playback URL is re-derived from the effective `ManifestName` resolved above
        // whenever it or the packaging format changes.
        record.url = crate::state::playback_url(&region, &channel_id, &id, &manifest_name, kind);
        record.manifest_name = Some(manifest_name);
```
The child name/URL are computed nowhere else: services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2607-2642 (`stored_cmaf_package`, `manifest.manifest_name.clone().unwrap_or_else(|| endpoint_manifest_name.to_string())` plus `cmaf_manifest_url(..., &manifest_name)`) is the only producer, and `apply_origin_endpoint_defaults` (state.rs:1655-1660) only fills a child's AD/playlist defaults, never its name or url.

Test coverage: the wrong behaviour is NOT pinned, so no test has to change. tests/all/cmaf_child_manifest_name.rs:301-307 always sends `cmafPackage` on the update (`let mut request = json!({ "id": id, "cmafPackage": { "hlsManifests": case.children } });`), and tests/all/mutation_persistence.rs:739-748 exercises "keeps the stored packaging when the request names none" against an HLS fixture only (`json!({ "description": "configuration only" })` over `endpoint_create_body()`'s hlsPackage), so the CMAF + no-packaging combination is untested.

## Steps to Reproduce

1. POST /channels with {"id": "c1"} -> 200.
2. POST /origin_endpoints with {"id": "oe", "channelId": "c1", "manifestName": "stream", "cmafPackage": {"hlsManifests": [{"id": "a"}]}} -> 200. The child inherits the endpoint's name: $.cmafPackage.hlsManifests[0].manifestName == "stream" and its url ends in /stream/index.m3u8.
3. PUT /origin_endpoints/oe with {"id": "oe", "manifestName": "replay"} (no cmafPackage) -> 200. The endpoint reports manifestName "replay" and a url ending in /replay/index.m3u8, but $.cmafPackage.hlsManifests[0].manifestName is still "stream" and the child url still ends in /stream/index.m3u8.
4. GET /origin_endpoints/oe and GET /origin_endpoints?channelId=c1 -> the same stale child name and url, confirming the value is persisted.
5. PUT /origin_endpoints/oe with {"id": "oe", "description": "configuration only"} (no manifestName, no cmafPackage) -> 200. The endpoint falls back to manifestName "index" (url .../index/index.m3u8) while the child still reports "stream": an unnamed child no longer carries "the manifestName for the OriginEndpoint" the model documents.
