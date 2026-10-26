Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# RevokeSigningProfile overwrites an existing revocation record instead of leaving the first one standing

_Filed: 2026-10-26T08:19:15Z_
_Operation: RevokeSigningProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RevokeSigningProfile` calls `SigningProfileRecord::revoke` unconditionally, so a second (or third) revoke of an ALREADY-revoked profile rewrites the stored `revocationRecord` wholesale: `revocationEffectiveFrom`, `revokedAt`, `revokedBy` and `statusReason` are all replaced by the latest caller's values. Because `revocationEffectiveFrom` is the instant after which signatures stop being trusted, a later `effectiveTime` moves that boundary FORWARD and silently restores trust in signatures that the first revocation had already placed outside it; the identity and the time of the original revocation are lost, so every later `GetSigningProfile` read attributes the revocation to the most recent caller.

This contradicts the rule the same crate implements for the other revocation path it owns. `SigningJobRecord::revoke` guards with `if self.is_revoked() { return; }` and the doc above it states the rule explicitly; the crate's own test module for `RevokeSignature` pins it. The profile record even carries the helper this path would need — `SigningProfileRecord::is_revoked` (state.rs:226) — and that helper has no caller anywhere in the crate (`grep is_revoked` finds only its definition, the job helper, and the unrelated `list_signing_jobs_types` field), i.e. the profile path simply skips a check the crate already has.

## Evidence

The job path records a revocation ONCE — services/service-com-amazonaws-signer-wallabyservice/src/state.rs:379-386:
```
    /// A revocation is a fact about a signature that has already stopped being
    /// trusted, so it is recorded ONCE: revoking an already-revoked job leaves
    /// the first record standing rather than restating when, why and by whom
    /// trust ended.
    pub fn revoke(&mut self, reason: &str, revoked_by: Option<String>, now: SystemTime) {
        if self.is_revoked() {
            return;
        }
```
and that rule is pinned by this crate's tests — tests/all/signature_revocation.rs:15-16:
```
//! * a revocation is recorded ONCE: revoking an already-revoked job leaves the
//!   first record standing;
```
The profile path has no such guard — src/state.rs:273-287:
```
    pub fn revoke(
        &mut self,
...
        self.status = PROFILE_STATUS_REVOKED.to_string();
        self.status_reason = Some(reason.to_string());
        self.revocation_record = Some(SigningProfileRevocationRecord {
            revocation_effective_from: Some(effective_time),
            revoked_at: Some(to_timestamp(now)),
            revoked_by,
        });
```
and the handler calls it unconditionally — src/ops/revoke_signing_profile.rs:244:
```
        profile.revoke(&input.reason, input.effective_time, revoked_by, now);
```
The helper the check would use exists and is never called — src/state.rs:225-228:
```
    /// Whether this profile has been revoked.
    pub fn is_revoked(&self) -> bool {
        self.status == PROFILE_STATUS_REVOKED
    }
```
Why the overwritten field matters, from the model: `smithy_query(query="doc", shape="RevokeSigningProfileRequest", member="effectiveTime")` — "A timestamp for when revocation of a Signing Profile should become effective. Signatures generated using the signing profile after this timestamp are not trusted."

Test coverage: tests/all/profile_revocation_and_identity.rs covers a single revoke per profile (`revoke_records_the_transition_and_every_read_reports_it`, `revoking_an_unknown_profile_or_version_is_refused`); no test exercises a second revoke of an already-revoked profile, so nothing pins the current overwrite.

## Steps to Reproduce

1. PUT /signing-profiles/Revoke_Twice with a body naming a catalog platformId (e.g. {"platformId": "AWSLambda-SHA384-ECDSA"}) and note the returned profileVersion.
2. PUT /signing-profiles/Revoke_Twice/revoke with {"profileVersion": <version>, "reason": "key compromised", "effectiveTime": 1000000000} — answers 204.
3. GET /signing-profiles/Revoke_Twice and record revocationRecord.revocationEffectiveFrom / revokedAt / revokedBy and statusReason.
4. PUT /signing-profiles/Revoke_Twice/revoke again with {"profileVersion": <same version>, "reason": "second call", "effectiveTime": 2000000000} — answers 204 as well.
5. GET /signing-profiles/Revoke_Twice: revocationRecord now reports the SECOND call's revocationEffectiveFrom (2000000000), its revokedAt/revokedBy and statusReason "second call"; the first revocation's effective instant and revoker are gone, so signatures issued between the two instants are no longer covered by any recorded revocation.
6. Compare with the job path: RevokeSignature twice on one job leaves the first record standing (tests/all/signature_revocation.rs:291).
