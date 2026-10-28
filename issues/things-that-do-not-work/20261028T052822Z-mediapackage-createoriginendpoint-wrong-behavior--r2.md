Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# A CMAF encryption block sent without constantInitializationVector is stored and reported with none, although the model states MediaPackage creates it

_Filed: 2026-10-28T05:28:22Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CmafEncryption$ConstantInitializationVector` is the one encryption member whose documentation states what the SERVICE does when the caller omits it: MediaPackage creates the constant initialization vector. The crate never materialises it. `stored_cmaf_package` copies the caller's `Encryption` block through verbatim, and `apply_origin_endpoint_defaults` — the one function every write of an endpoint goes through, whose stated job is that "every optional configuration member this request left unset takes that member's default BEFORE the record is read back" — touches only the CMAF package's child manifests and its stream selection, never its encryption. So a CreateOriginEndpoint (and UpdateOriginEndpoint) carrying a `cmafPackage.encryption` with no `constantInitializationVector` is stored with the member absent, and the create response, DescribeOriginEndpoint and every ListOriginEndpoints entry report a CMAF encryption configuration with no IV at all.

The crate's own defaults section justifies leaving `Encryption` alone with a claim the model contradicts: it says the members left as sent are the ones for which neither the model, a recording nor the published configuration publishes a default. The model publishes one for exactly this member — and it publishes the opposite for the HLS sibling (`HlsEncryption$ConstantInitializationVector`: "When not specified the initialization vector will be periodically rotated"), which is why the CMAF sentence is specific rather than generic.

## Evidence

The model sentence (`smithy_query(query="doc", shape="com.amazonaws.mediapackage#CmafEncryption", member="ConstantInitializationVector")`):

    "An optional 128-bit, 16-byte hex value represented by a 32-character string, used in
     conjunction with the key for encrypting blocks. If you don't specify a value, then
     MediaPackage creates the constant initialization vector (IV)."

Contrast the HLS member, which documents no created value (`smithy_query(query="shape", shape="HlsEncryption")`): "A constant initialization vector for encryption (optional). When not specified the initialization vector will be periodically rotated."

What the crate does instead — `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2734-2745`, the stored CMAF package:

    CmafPackage {
        encryption: params.encryption.clone(),
        hls_manifests: Some(hls_manifests),
        segment_duration_seconds: params.segment_duration_seconds,
        segment_prefix: Some(
            params.segment_prefix.clone().unwrap_or_else(|| channel_id.to_string()),
        ),
        stream_selection: params.stream_selection.clone(),
    }

and `src/state.rs:1712-1717`, the only place a stored CMAF package is server-filled:

    if let Some(package) = record.cmaf_package.as_mut() {
        for manifest in package.hls_manifests.iter_mut().flatten() {
            fill_hls_manifest(manifest);
        }
        fill_stream_selection(&mut package.stream_selection);
    }

The justification the model contradicts, `src/state.rs:1415-1418`:

    // A member whose default neither the model, a recording nor the service's published
    // configuration publishes is left exactly as the request sent it — `Encryption`,
    // `HlsPackage$SegmentDurationSeconds` and the `MssPackage` window and segment members —
    // because inventing a value for it would be a fabrication, not a default.

The handler that relies on that fill, `src/ops/create_origin_endpoint.rs:386-391`:

    crate::state::rederive_cmaf_package(&mut record);
    // MediaPackage does not store the endpoint as the request wrote it: every optional
    // configuration member the request left unset is server-filled with that member's
    // default FIRST, so the stored record — and therefore every later projection of it
    // — carries the same member set whatever the request omitted.
    crate::state::apply_origin_endpoint_defaults(&mut record);

Test coverage: `tests/all/origin_endpoint_defaults.rs` and `tests/all/origin_endpoint_contents.rs` pin the filled CMAF members (child manifests, stream selection, segmentPrefix); neither asserts anything about `cmafPackage.encryption.constantInitializationVector`, so nothing pins the current absence.

## Steps to Reproduce

1. CreateChannel with Id="packaged-channel".
2. CreateOriginEndpoint with ChannelId="packaged-channel", Id="cmaf-endpoint" and a cmafPackage carrying encryption = { spekeKeyProvider: {url, resourceId, systemIds, roleArn} } and NO constantInitializationVector.
3. Read the create response body, then GET /origin_endpoints/cmaf-endpoint.
4. Observe cmafPackage.encryption carries no constantInitializationVector, although the member's documentation states that when the caller does not specify a value MediaPackage creates the constant initialization vector (which every later projection of the endpoint would then report).
