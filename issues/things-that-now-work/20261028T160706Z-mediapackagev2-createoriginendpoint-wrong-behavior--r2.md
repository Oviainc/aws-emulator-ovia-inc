Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# An HLS/low-latency-HLS manifest submitted without ChildManifestName is stored and reported with none, although the model states the service applies a default

_Filed: 2026-10-28T16:07:06Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other server-applied manifest setting is materialised at create time from "the submitted value or the service's default" and persisted, so the create response, a later `GetOriginEndpoint` and the `ListOriginEndpoints` projection all report the same concrete value. `ChildManifestName` is the one exception: `crate::state::stored_hls_manifest` and `crate::state::stored_low_latency_hls_manifest` copy `submitted.child_manifest_name` through verbatim, so a manifest the request sent without one is stored with none and every later read of the endpoint reports a manifest configuration with no `ChildManifestName`.

The model says the service supplies one. `CreateHlsManifestConfiguration$ChildManifestName`'s documentation states the rule outright: "If you don't enter a value, MediaPackage uses the default manifest name, index, with an added suffix to distinguish it from the manifest name." `GetHlsManifestConfiguration$ChildManifestName` repeats it on the response side. The handler therefore reports an endpoint that is missing a member the model says the service materialises, and a client that reads `ChildManifestName` off a create/read response to build a child playlist URL gets nothing.

The same copy-through sits on the update path (`UpdateOriginEndpoint` rebuilds the whole manifest set through the same two helpers), so no write ever fills the member in. `CreateOriginEndpoint` is the first operation whose handler calls the helpers.

This is distinct from the open `CreateOriginEndpoint`/validation issue (which asks for REFUSALS — DRM settings, duplicate manifest names, the parent channel's InputType): nothing here is about rejecting input, only about a documented service-applied value never being computed.

## Evidence

Smithy, `CreateHlsManifestConfiguration$ChildManifestName` (smithy_query doc, services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json):
```
A short string that's appended to the endpoint URL. The child manifest name creates a
unique path to this endpoint. If you don't enter a value, MediaPackage uses the default
manifest name, index, with an added suffix to distinguish it from the manifest name.
```

The handler copies the absent member through instead — src/state.rs:1984-2016 (`stored_hls_manifest`):
```
/// The stored form of a submitted HLS manifest: everything the caller asked for, plus
/// the URL the service computes for it and the manifest settings the service applies to
/// what the caller left out ([`DEFAULT_MANIFEST_WINDOW_SECONDS`],
/// [`DEFAULT_URI_PATH_TYPE`], and child-manifest URL encoding off).
pub fn stored_hls_manifest(
    submitted: CreateHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetHlsManifestConfiguration {
    GetHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: submitted.child_manifest_name,          // <- src/state.rs:1995
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        ...
        url_encode_child_manifest: Some(
            submitted
                .url_encode_child_manifest
                .unwrap_or(DEFAULT_URL_ENCODE_CHILD_MANIFEST),
        ),
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
    }
}
```
The helper's own doc enumerates the three settings it applies and `ChildManifestName` is not among them; the sibling `stored_low_latency_hls_manifest` has the identical line at src/state.rs:2028. There is no default constant for the member either — src/state.rs:128-165 declares `DEFAULT_MANIFEST_WINDOW_SECONDS`, `DEFAULT_URI_PATH_TYPE` and `DEFAULT_URL_ENCODE_CHILD_MANIFEST` and nothing for the child name.

The crate states the rule it breaks — src/state.rs:26-32 (module header):
```
//! The same holds for the members the SERVICE applies rather than the caller: a channel's
//! input type and output locking mode, its switching and CMSD-header blocks, an origin
//! endpoint's segment settings, startover window and URI/stream-name modes, and every
//! manifest configuration's window, URI path type and child-manifest URL encoding are
//! materialised ONCE, at create time, from the submitted value or the service's default,
//! and stored - so the create response, a later read and the list projection all report
//! the same concrete value.
```

The callers that persist the result — src/ops/create_origin_endpoint.rs:447-460:
```
        // Each submitted manifest is stored in its reported (`Get*`) form, URL included,
        // so the URL a client reads later is the one computed here.
        let hls_manifests: Vec<_> = input
            .hls_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_hls_manifest(manifest, at))
            .collect();
```
and src/ops/update_origin_endpoint.rs:477-488, which rebuilds the set through the same helpers.

Test coverage leaves the case open: the suite that pins exactly this class of member asserts only the three settings the helper does apply — tests/all/server_materialised_members.rs:227 and 239-241:
```
                    "HlsManifests": [{"ManifestName": "index"}],
...
                ("/HlsManifests/0/ManifestWindowSeconds", json!(60)),
                ("/HlsManifests/0/UriPathType", json!("LEAF")),
                ("/HlsManifests/0/UrlEncodeChildManifest", json!(false)),
```
No row names `ChildManifestName`, and the assertions are per-JSON-pointer equality (tests/all/server_materialised_members.rs:457-463), so nothing pins its absence. The only test that mentions the member always supplies it (tests/all/refusal_text_and_entity_tags.rs:770-785).

## Steps to Reproduce

1. Boot the emulator; POST /channelGroup with {"ChannelGroupName":"g"} and POST /channelGroup/g/channel with {"ChannelName":"c"}.
2. POST /channelGroup/g/channel/c/originEndpoint with {"OriginEndpointName":"e","ContainerType":"TS","HlsManifests":[{"ManifestName":"index"}]} — note the manifest carries no ChildManifestName.
3. Observe the 200 response: HlsManifests[0] reports ManifestWindowSeconds 60, UriPathType LEAF and UrlEncodeChildManifest false (all server-applied), but NO ChildManifestName at all.
4. GET /channelGroup/g/channel/c/originEndpoint/e — the stored manifest still reports no ChildManifestName, and GET /channelGroup/g/channel/c/originEndpoint reports the same in the list projection.
5. Compare with the model: CreateHlsManifestConfiguration$ChildManifestName says "If you don't enter a value, MediaPackage uses the default manifest name, index, with an added suffix to distinguish it from the manifest name" — so the member should be materialised at create time and reproduced byte for byte by the read, exactly as the window, URI path type and child-manifest URL encoding beside it are.
6. Repeat with "LowLatencyHlsManifests":[{"ManifestName":"ll"}] for the same result, and with PUT /channelGroup/g/channel/c/originEndpoint/e (UpdateOriginEndpoint) to confirm no write ever fills the member in.
