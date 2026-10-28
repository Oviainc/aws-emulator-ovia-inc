Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a24b8bb497f295cf2fd83b42be71d5dbb9740df2

# Packaging combinations the model forbids are accepted: adMarkers=DATERANGE with programDateTimeIntervalSeconds 0, and encryptionContractConfiguration with key rotation on

_Filed: 2026-10-28T16:03:48Z_
_Operation: CreateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `a24b8bb497f295cf2fd83b42be71d5dbb9740df2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`check_origin_endpoint_members` is the crate's single place where an origin endpoint's packaging members are judged by CONTENT (it is called by both CreateOriginEndpoint and UpdateOriginEndpoint, and its own doc says it refuses "an OriginEndpoint create or update whose access-control or packaging members name something this service could not act on"). It enforces four rules: the whitelist CIDR rule, the ARN requirements, the CMAF manifest minimum, and child-manifest id uniqueness. Two conditional requirements the model states IN PROSE are enforced nowhere, so requests the model says are invalid are accepted and stored.

(a) adMarkers = DATERANGE with no programDateTimeIntervalSeconds. The member's own documentation says that for the DATERANGE option "you must set a programDateTimeIntervalSeconds value that is greater than 0". Nothing checks the pair. Worse, the crate's own default-fill then ACTIVELY produces the forbidden combination: `fill_hls_package`/`fill_hls_manifest` fill an absent `programDateTimeIntervalSeconds` with `DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS` = 0, so a create carrying `hlsPackage: {adMarkers: "DATERANGE"}` is stored — and reported by the create response, DescribeOriginEndpoint and every ListOriginEndpoints entry — as DATERANGE with an interval of 0. The same holds for each child manifest of a CMAF package, whose `AdMarkers` carries the identical sentence. An explicit `programDateTimeIntervalSeconds: 0` with DATERANGE is accepted too.

(b) encryptionContractConfiguration with key rotation enabled. `EncryptionContractConfiguration`'s documentation states "You must disable key rotation for this endpoint by setting keyRotationIntervalSeconds to 0" (and that the block "can be used for DASH or CMAF endpoints that use SPEKE 2.0"). The member exists on the generated `SpekeKeyProvider` and is stored and echoed back, but no code in the crate ever reads it: a request carrying `encryptionContractConfiguration` together with `keyRotationIntervalSeconds: 900` — or inside an `hlsPackage`/`mssPackage` encryption block — is accepted and persisted. `bound_origin_endpoint_durations` is the only code that looks at `keyRotationIntervalSeconds`, and it only refuses values below the shared floor of 0.

Both are exactly the class of unmodelled, service-enforced rule this crate already implements for other members (the startover-window minimum, the CMAF manifest minimum, the whitelist CIDR rule), with the refusal builder (`member_invalid` -> 422 `UnprocessableEntityException` keyed by the member path) already in place.

## Evidence

Model, `smithy_query(query="doc", shape="com.amazonaws.mediapackage#HlsPackage", member="AdMarkers")` — the same sentence is on `com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters$AdMarkers`:

    "DATERANGE" inserts EXT-X-DATERANGE tags to signal ad and program transition events
    in HLS and CMAF manifests. For this option, you must set a programDateTimeIntervalSeconds value
    that is greater than 0.

Model, `smithy_query(query="doc", shape="com.amazonaws.mediapackage#EncryptionContractConfiguration")`:

    Note the following considerations when using encryptionContractConfiguration:
    encryptionContractConfiguration can be used for DASH or CMAF endpoints that use SPEKE 2.0. SPEKE 2.0 relies on the CPIX 2.3 specification.
    You must disable key rotation for this endpoint by setting keyRotationIntervalSeconds to 0.

The complete content-rule set, `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:1018-1038` — no adMarkers rule, no encryption-contract rule:

    pub fn check_origin_endpoint_members(
        whitelist: Option<&[String]>,
        authorization: Option<&Authorization>,
        hls_package: Option<&HlsPackage>,
        dash_package: Option<&DashPackage>,
        cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
        mss_package: Option<&MssPackage>,
    ) -> Result<(), ServiceError> {
        check_whitelist(whitelist)?;
        for (member, value, requirement) in origin_endpoint_arn_members(...) {
            check_arn_member(&member, value, requirement)?;
        }
        require_cmaf_hls_manifests(cmaf_package)?;
        require_unique_cmaf_hls_manifest_ids(cmaf_package)
    }

The fill that materialises the forbidden pair, `src/state.rs:1601-1603` (and the identical child-manifest copy at `src/state.rs:1635-1637`):

    package.program_date_time_interval_seconds = package
        .program_date_time_interval_seconds
        .or(Some(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS));

with `src/state.rs:1476`: `pub const DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS: i32 = 0;`

The only code that reads `keyRotationIntervalSeconds`, `src/state.rs:1296-1302` (floor only, `MIN_DURATION_SECONDS = 0`):

                    (
                        "encryption.keyRotationIntervalSeconds",
                        package
                            .encryption
                            .as_ref()
                            .and_then(|encryption| encryption.key_rotation_interval_seconds),
                    ),

The encryption-contract member is stored but never judged — `grep -n 'encryption_contract|encryptionContract'` over the crate matches exactly one line, the generated field `src/ops/types/shared_types.rs:115`:

    pub encryption_contract_configuration: Option<EncryptionContractConfiguration>,

Call sites that would have refused: `src/ops/create_origin_endpoint.rs:245-252` and `src/ops/update_origin_endpoint.rs:273-280`, both calling `crate::state::check_origin_endpoint_members(...)` under the comment "a whitelist entry that is not an address range, an ARN that names no resource of the documented service, a CMAF package that publishes no child manifest and one that publishes two children under a single id are each refused before anything is stored".

Test coverage: `tests/all/origin_endpoint_defaults.rs:253-271` ("hls, all supplied") is the only place DATERANGE appears in the crate, and it pairs it with `"programDateTimeIntervalSeconds": 60` — the VALID combination, so it stays green. `tests/all/origin_endpoint_durations.rs:101-114, 166-172` exercise `programDateTimeIntervalSeconds` and `encryption.keyRotationIntervalSeconds` only against the negative floor. No test in the crate sends `encryptionContractConfiguration` at all.

## Steps to Reproduce

1. CreateChannel with Id="ad-channel".
2. CreateOriginEndpoint with Id="daterange", ChannelId="ad-channel", hlsPackage = { "adMarkers": "DATERANGE" } and no programDateTimeIntervalSeconds.
3. Observe HTTP 200 and a stored endpoint whose hlsPackage reports adMarkers DATERANGE with programDateTimeIntervalSeconds 0 (GET /origin_endpoints/daterange returns the same), although the member's documentation states that for DATERANGE you must set a programDateTimeIntervalSeconds greater than 0.
4. Repeat with cmafPackage = { "hlsManifests": [{"id": "a", "adMarkers": "DATERANGE"}] } for the same result on a child manifest.
5. CreateOriginEndpoint with Id="contract", ChannelId="ad-channel" and cmafPackage.encryption = { "keyRotationIntervalSeconds": 900, "spekeKeyProvider": { url, resourceId, systemIds, roleArn, "encryptionContractConfiguration": {"presetSpeke20Audio": "PRESET-AUDIO-1", "presetSpeke20Video": "PRESET-VIDEO-1"} } }.
6. Observe HTTP 200 and the endpoint stored with both the encryption contract and a 900-second key rotation, although the shape's documentation says key rotation must be disabled (keyRotationIntervalSeconds 0) when encryptionContractConfiguration is used.
