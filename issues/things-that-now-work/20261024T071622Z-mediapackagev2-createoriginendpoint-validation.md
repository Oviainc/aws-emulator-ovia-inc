Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateOriginEndpoint never validates DRM/encryption settings, duplicate manifest names, or the parent channel's InputType

_Filed: 2026-10-24T07:16:22Z_
_Operation: CreateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateOriginEndpoint validates exactly two request-level rules — the manifest-count minimum (NUM_MANIFESTS_LOW) and the container/manifest-family table (TS_CONTAINER_TYPE_WITH_DASH_MANIFEST and friends). Every other cross-member rule the model declares for an origin endpoint is unenforced, so the emulator answers 200 and persists an endpoint AWS refuses with 400 ValidationException. Three families of rule are missing:

(1) Segment.Encryption is stored verbatim with no checks at all. The model's ValidationExceptionType enumerates ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH (e.g. a TsEncryptionMethod on a CMAF container, or an IsmEncryptionMethod on a TS container), CENC_IV_INCOMPATIBLE, ENCRYPTION_CONTRACT_UNENCRYPTED / ENCRYPTION_CONTRACT_SHARED / ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE, DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE, CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1, plus the SpekeKeyProvider rules INVALID_ROLE_ARN / ROLE_ARN_INVALID_FORMAT / ROLE_ARN_LENGTH_OUT_OF_RANGE / ROLE_ARN_NOT_ASSUMABLE and the key-server URL rules URL_INVALID / URL_SCHEME / URL_USER_INFO / URL_PORT / URL_UNKNOWN_HOST / URL_LOOPBACK_ADDRESS / URL_LOCAL_ADDRESS. Expected: a SpekeKeyProvider whose RoleArn is 'not-an-arn' or whose Url is 'ftp://localhost:22/x' is a 400 ValidationException with the matching discriminator and no endpoint is created. Actual: 200, endpoint created, the garbage is echoed by GetOriginEndpoint.

(2) Duplicate manifest names are accepted. The model declares MANIFEST_NAME_COLLISION. Two HlsManifests entries with the same ManifestName — or an HLS and a low-latency HLS manifest sharing one name — are stored and each is given an identical egress Url by crate::state::stored_hls_manifest, so the endpoint publishes two manifests at the same URL. Expected: 400 ValidationException / MANIFEST_NAME_COLLISION.

(3) The parent channel's InputType is never consulted. The handler resolves the channel only to prove it exists; it does not read channel.input_type. So an endpoint created under a MULTIVIEW-input channel may carry a low-latency HLS manifest, an MSS manifest, an ISM container or a startover window (MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST, MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST, MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER, MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED), and ForceEndpointErrorConfiguration is accepted on an endpoint of an HLS-input channel even though the model names ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION for exactly that combination. UpdateOriginEndpoint shares the same two checks and the same blind spots.

## Evidence

Spec: com.amazonaws.mediapackagev2#ValidationExceptionType enumerates ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH, CENC_IV_INCOMPATIBLE, ENCRYPTION_CONTRACT_*, INVALID_ROLE_ARN, ROLE_ARN_INVALID_FORMAT, URL_* , MANIFEST_NAME_COLLISION, ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION, MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST, MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST, MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER, MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED; CreateOriginEndpoint declares ValidationException (400). Segment.Encryption -> SpekeKeyProvider{Url, RoleArn, EncryptionContractConfiguration} is a modeled member.
Handler: services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_origin_endpoint.rs — the only pre-store checks are crate::state::manifest_set_violation (line ~315), tag bounds (~350) and the count ceiling (~420); input.segment is passed straight to crate::state::materialized_segment (~481) and input.force_endpoint_error_configuration is assigned unexamined (~488); channel.input_type is never read.
State: grep of src/state.rs for 'encryption|speke|role_arn' finds only the generated type/enum tables — no validator; crate::state::container_manifest_conflict (~2279) covers container-vs-family only.
Tests: grep of tests/ for 'encryption|speke|collision' finds nothing; no test asserts any of these refusals.

## Steps to Reproduce

1. CreateChannelGroup g, then CreateChannel g/c with InputType HLS.
2. CreateOriginEndpoint g/c/e with ContainerType CMAF, one HlsManifests entry, and Segment.Encryption.SpekeKeyProvider = {Url: 'ftp://127.0.0.1:22/keys', RoleArn: 'not-an-arn', EncryptionContractConfiguration: {PresetSpeke20Audio: 'PRESET_AUDIO_1', PresetSpeke20Video: 'PRESET_VIDEO_1'}} and EncryptionMethod.TsEncryptionMethod = 'AES_128'. Observe 200 and a created endpoint; AWS answers 400 ValidationException (ROLE_ARN_INVALID_FORMAT / URL_SCHEME / ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH).
3. CreateOriginEndpoint g/c/e2 with ContainerType CMAF and HlsManifests = [{ManifestName: 'index'}, {ManifestName: 'index'}]. Observe 200 with two manifests carrying the same Url; AWS answers 400 ValidationException / MANIFEST_NAME_COLLISION.
4. CreateOriginEndpoint g/c/e3 with ContainerType TS, one HlsManifests entry and ForceEndpointErrorConfiguration = {EndpointErrorConditions: ['STALE_MANIFEST']} under the HLS-input channel. Observe 200; AWS answers 400 ValidationException / ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION.
