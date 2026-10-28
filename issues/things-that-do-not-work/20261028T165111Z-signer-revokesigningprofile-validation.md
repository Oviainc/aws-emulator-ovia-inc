Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1ea0d2fdb22c9ad1b73243a3510f3db1b8810ab9

# RevokeSigningProfile records a revocation on a platform published as not supporting revocation, which RevokeSignature refuses

_Filed: 2026-10-28T16:51:10Z_
_Operation: RevokeSigningProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `1ea0d2fdb22c9ad1b73243a3510f3db1b8810ab9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for revocation capability and applies it in only one of the two revocation paths.

`crate::state::platform_supports_revocation` is the crate's single judgement of whether a signature produced on a platform can be revoked, read off the catalog's published `revocationSupported`. `RevokeSignature` calls it and refuses with the modelled 400 `ValidationException` when the platform publishes `revocationSupported: false` (the two `AmazonFreeRTOS-*` catalog entries). `RevokeSigningProfile` — whose own modelled documentation says revoking the profile means "signatures generated using the signing profile after an effective start date are no longer valid" — never consults the catalog at all: it resolves the profile, checks the version exists, and writes the revocation record unconditionally.

The result is two contradictory answers for the same fact about the same platform. On `AmazonFreeRTOS-Default` (or `AmazonFreeRTOS-TI-CC3220SF`):
* `RevokeSignature` on a job of that profile → 400 `ValidationException` ("Revocation is not supported for platform …"), nothing recorded;
* `RevokeSigningProfile` on the profile itself → 204, a `SigningProfileRevocationRecord` persisted, `GetSigningProfile` reports status `Revoked`, and `GetRevocationStatus` then pushes that `profileVersionArn` into `revokedEntities` — i.e. the service reports a revoked entity on a platform where it has just told the same client revocation is impossible.

`GetRevocationStatus` is the security decision path the revocation records feed, so the two paths disagreeing there is the observable damage: whether a signature on a FreeRTOS platform can be revoked depends on which operation the caller happens to use. `RevokeSigningProfile` models `ValidationException` (HTTP 400), the same shape and status `RevokeSignature` already uses, so the refusal is expressible inside this operation's own modelled error set.

## Evidence

The sibling path APPLIES the rule — services/service-com-amazonaws-signer-wallabyservice/src/ops/revoke_signature.rs:228-240:
```
        // A platform published as not supporting revocation produces signatures
        // that cannot be revoked, so there is no revocation for this call to
        // record.
        if !crate::state::platform_supports_revocation(&job.platform_id) {
            let message = crate::state::revocation_unsupported_message(&job.platform_id);
            return Err(RevokeSignatureErrorModeled::ValidationException(
                ValidationException {
                    message: Some(message),
                    code: Some(crate::state::ERROR_CODE_VALIDATION.to_string()),
                },
            )
            .into());
        }
```

The helper it calls, and what the crate says that helper decides — src/state.rs:1427-1438:
```
/// Whether a signature produced on `platform_id` can be revoked — the platform's
/// published `revocationSupported` ("Indicates whether revocation is supported
/// for the platform", [`SigningPlatformEntry::revocation_supported`]), which is
/// how a client learns in advance whether revoking one of its signatures is
/// possible at all.
pub fn platform_supports_revocation(platform_id: &str) -> bool {
    signing_platform(platform_id).is_none_or(|entry| entry.revocation_supported)
}
```

The path that SKIPS it — src/ops/revoke_signing_profile.rs:232-252 (the whole body after the store lock; `profile.platform_id` is right there on the record and is never read):
```
        let Some(profile) = store.profile_mut(&region, &input.profile_name) else {
            return Err(not_found(format!(
                "profile with name {} does not exists",
                input.profile_name
            )));
        };
        // `profileVersion` names WHICH version of the profile is revoked, so a
        // version this profile has never had names no revocable resource.
        if !profile.versions.contains(&input.profile_version) {
            return Err(not_found(format!(
                "Signing profile {} has no version {}",
                input.profile_name, input.profile_version
            )));
        }

        profile.revoke(&input.reason, input.effective_time, revoked_by, now);

        Ok(RevokeSigningProfileOutput)
```

The platforms that publish the capability as absent — src/state.rs:1286-1311 (`AmazonFreeRTOS-Default` and `AmazonFreeRTOS-TI-CC3220SF`):
```
        platform_id: "AmazonFreeRTOS-Default",
...
        revocation_supported: false,
```

The rule this crate's own suite states for the capability — tests/all/signature_revocation.rs:17-20:
```
//! * a signature can be revoked exactly when the platform it was produced on
//!   publishes `revocationSupported` — the capability `GetSigningPlatform`
//!   reports — and a platform published without it refuses the call with the
//!   modelled 400 instead;
```

Where the inconsistent record becomes an answer — src/ops/get_revocation_status.rs:249-256:
```
        if profile_version_named.is_some_and(|(_, profile_name, profile_version)| {
            store.profile(&region, profile_name).is_some_and(|profile| {
                profile.version_arn(profile_version) == input.profile_version_arn
                    && profile.version_revoked_at(profile_version, &input.signature_timestamp)
            })
        }) {
            revoked_entities.push(input.profile_version_arn.clone());
        }
```

Model text for the operation (`smithy_query(query="doc", shape="RevokeSigningProfile")`): "Changes the state of a signing profile to REVOKED. This indicates that signatures generated using the signing profile after an effective start date are no longer valid." `RevokeSigningProfile` declares `ValidationException` / HTTP 400 (`smithy_query(query="operation", operation="RevokeSigningProfile")`), so the refusal fits its modelled error set.

Test coverage to be aware of: no test asserts a refusal here, and one suite incidentally pins today's acceptance — tests/all/cancel_profile_status_transitions.rs:281-288 uses `platform: "AmazonFreeRTOS-TI-CC3220SF"` for the `RevokedThenReput` row, and the seed at :304-311 asserts `revoked.status == 204` ("the seeding revoke must succeed"). That row's subject is the CANCEL transition, not revocation capability, and it states no reason tied to `revocationSupported`; a fix needs that row re-seeded on a revocation-supporting platform. tests/all/revocation_status.rs:366-374 already restricts its seeds to `entry.revocation_supported` platforms, so it is unaffected.

## Steps to Reproduce

1. Boot the emulator. PUT /signing-profiles/FreeRTOS_Revoke with {"platformId":"AmazonFreeRTOS-Default","signingMaterial":{"certificateArn":"arn:aws:acm:us-west-2:000000000000:certificate/abc"}} and note the returned profileVersion V1 and profileVersionArn A1.
2. PUT /signing-profiles/FreeRTOS_Revoke/revoke with {"profileVersion":"V1","reason":"key compromise","effectiveTime":<t0>} — observe 204 and a persisted revocation: GET /signing-profiles/FreeRTOS_Revoke reports status Revoked with a revocationRecord.
3. GET /revocations?signatureTimestamp=<t0+1>&platformId=AmazonFreeRTOS-Default&profileVersionArn=A1&jobArn=arn:aws:signer:us-west-2:000000000000:/signing-jobs/anything&certificateHashes=aa — observe revokedEntities contains A1, i.e. a revoked entity on a platform published with revocationSupported=false.
4. GET /signing-platforms/AmazonFreeRTOS-Default — observe "revocationSupported": false.
5. For contrast, start a signing job from that same profile and PUT /signing-jobs/{jobId}/revoke with {"reason":"key compromise"} — observe 400 ValidationException "Revocation is not supported for platform AmazonFreeRTOS-Default": the same capability refuses one revocation path and not the other.
