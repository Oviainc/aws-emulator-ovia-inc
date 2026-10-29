Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e172822ed36ddbdf21d6296d81819842e39b78b2

# Packaging-block strings and lists (spekeKeyProvider url/resourceId/systemIds, segmentPrefix, constantInitializationVector, utcTimingUri) are stored with no size bound, against the crate's own stated invariant

_Filed: 2026-10-28T20:36:36Z_
_Operation: UpdateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `e172822ed36ddbdf21d6296d81819842e39b78b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`bound_origin_endpoint_members` is the one function the origin-endpoint create and the update both call to refuse "the caller-sized members an OriginEndpoint create and update share" (its own doc), and `state.rs`'s module header states the invariant it exists to uphold: "one request cannot decide how much memory its record occupies" and "every caller-supplied string capped".

The function bounds exactly four things: `description`, `manifestName`, the `whitelist` (count + each entry) and `cmafPackage.hlsManifests` (count + each entry's `id` and `manifestName`). Every OTHER caller-supplied string and list inside a packaging block is stored verbatim on the record and echoed by every later projection with no bound at all:

* `<hls|dash|cmaf|mss>Package.encryption.spekeKeyProvider.url` (`@required` string)
* `...spekeKeyProvider.resourceId` (`@required` string)
* `...spekeKeyProvider.systemIds` (`@required` list<string> — neither the entry count nor any entry's length is bounded)
* `...spekeKeyProvider.roleArn` / `.certificateArn` and `authorization.cdnIdentifierSecret` / `.secretsRoleArn` (shape-checked by `check_arn_member`, which imposes no length bound: `Requirement::admits` only parses the ARN and compares the vendor/resource type, so `arn:aws:iam::123456789012:role/<megabytes>` is admitted)
* `cmafPackage.segmentPrefix`, `hlsPackage.encryption.constantInitializationVector`, `cmafPackage.encryption.constantInitializationVector`, `dashPackage.utcTimingUri`
* `hlsPackage.adTriggers` / `dashPackage.periodTriggers` entry counts

The crate's sibling create proves this is the bound it means to apply: `CreateHarvestJob` runs `bound_chars(..., MAX_STRING_CHARS)` over EVERY caller-supplied string of its request, including `s3Destination.roleArn` — the same class of member (an ARN string it then shape-checks) that the origin-endpoint writers leave unbounded. So one `CreateOriginEndpoint`/`UpdateOriginEndpoint` can decide how much memory its record occupies, and `MAX_ORIGIN_ENDPOINTS` (20,000) stops being the finite product the module header claims.

Filed against `UpdateOriginEndpoint` because the gap lives in the shared helper both writers call (`bound_origin_endpoint_members`, whose doc says "an update must not be able to grow a record past what a create of it would have refused") and the `CreateOriginEndpoint`/`validation` pair already carries an unrelated gap. Both call sites are cited below.

## Evidence

The invariant the code breaks — `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:95-100`:

    // The per-record caller-sized collections and strings are bounded too, with
    // `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not
    // valid"), so one request cannot decide how much memory its record occupies. The product
    // is finite: at most MAX_CHANNELS channels and MAX_ORIGIN_ENDPOINTS endpoints, each with
    // at most MAX_TAGS tags, MAX_WHITELIST_ENTRIES whitelist entries and
    // MAX_CMAF_HLS_MANIFESTS child CMAF manifests, every caller-supplied string capped.

The complete set of members it actually bounds — `src/state.rs:653-702` (nothing about speke, segmentPrefix, the initialization vector, utcTimingUri or the trigger lists):

    /// Refuse the caller-sized members an OriginEndpoint create and update share.
    ///
    /// Both handlers call this, so the bounds one enforces cannot drift from the other's: an
    /// update must not be able to grow a record past what a create would have refused. [...]
    pub fn bound_origin_endpoint_members(
        description: Option<&str>,
        manifest_name: Option<&str>,
        whitelist: Option<&[String]>,
        cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    ) -> Result<(), ServiceError> {
        for (member, value) in [("description", description), ("manifestName", manifest_name)] { ... }
        if let Some(whitelist) = whitelist { ... }
        if let Some(manifests) = cmaf_package.and_then(|package| package.hls_manifests.as_ref()) { ... }
        Ok(())
    }

The sibling create that DOES bound every string of its request, including the ARN it then shape-checks — `src/ops/create_harvest_job.rs:225-234`:

    for (member, value) in [
        ("originEndpointId", &input.origin_endpoint_id),
        ("startTime", &input.start_time),
        ("endTime", &input.end_time),
        ("s3Destination.bucketName", &input.s3_destination.bucket_name),
        ("s3Destination.manifestKey", &input.s3_destination.manifest_key),
        ("s3Destination.roleArn", &input.s3_destination.role_arn),
    ] {
        crate::state::bound_chars(member, value, crate::state::MAX_STRING_CHARS)?;
    }

The ARN check that imposes no length bound — `services/common/src/arn.rs:259-262`:

    pub fn admits(self, value: &str) -> bool {
        parse_of(value, self.vendor)
            .is_some_and(|arn| self.resource_type.is_none_or(|kind| arn.resource_type() == kind))
    }

The two call sites that leave the rest unbounded, under comments claiming the opposite — `src/ops/create_origin_endpoint.rs:206-228` ("Bound what the caller may make this record cost, before anything is stored: the store copies these collections and strings verbatim, so one request must not decide how much memory its record occupies.") and `src/ops/update_origin_endpoint.rs:247-256` ("Bound what the caller may make this record cost, before anything is written, with the same function the create bounds its request with").

The unbounded members are stored verbatim and echoed: `src/ops/create_origin_endpoint.rs:365-374` (`hls_package: input.hls_package.clone()`, `dash_package`, `cmaf_package_parameters`, `mss_package`, `authorization`) and `src/ops/update_origin_endpoint.rs:330-333`; rendered back by `OriginEndpointRecord::to_origin_endpoint` (`src/state.rs:1893-1913`). `grep` over `src/` for `system_ids|resource_id|segment_prefix|constant_initialization_vector|utc_timing_uri` matches only the generated struct fields (`src/ops/types/shared_types.rs:98,105,116,118,169,199,227`) and `stored_cmaf_package`'s `segment_prefix` default (`src/state.rs:2738-2742`) — no bound anywhere.

Test coverage: `tests/all/origin_endpoint_contents.rs` round-trips the packaging blocks; `tests/all/origin_endpoint_durations.rs` covers the duration floor; no test in the crate sends an oversized speke member, so adding the bound through the existing `bound_chars`/`bound_entries` + `MAX_STRING_CHARS` leaves every current test green.

## Steps to Reproduce

1. CreateChannel with Id="speke-channel".
2. POST /origin_endpoints with Id="unbounded", ChannelId="speke-channel" and dashPackage = { "encryption": { "spekeKeyProvider": { "url": "https://keys.example.com/" + "a".repeat(5_000_000), "resourceId": "a".repeat(5_000_000), "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"; 100_000], "roleArn": "arn:aws:iam::123456789012:role/" + "a".repeat(5_000_000) } } }.
3. Observe HTTP 200: the endpoint is created and the whole request is stored, although `description` or `manifestName` of the same request would have been refused with 422 past MAX_STRING_CHARS (1024) and `CreateHarvestJob` refuses the identically documented `s3Destination.roleArn` past the same bound.
4. GET /origin_endpoints/unbounded — the multi-megabyte speke block is echoed back from the store, so the record kept it.
5. Repeat as PUT /origin_endpoints/unbounded (UpdateOriginEndpoint) with the same body: accepted too, even though `bound_origin_endpoint_members`' doc says "an update must not be able to grow a record past what a create of it would have refused".
6. Also accepted: cmafPackage.segmentPrefix, hlsPackage.encryption.constantInitializationVector and dashPackage.utcTimingUri of arbitrary length, and hlsPackage.adTriggers with 1,000,000 entries.
