Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# PutSigningProfile on an existing name erases the profile's revocation record, so GetRevocationStatus reports previously revoked versions as not revoked

_Filed: 2026-10-28T15:18:56Z_
_Operation: PutSigningProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `PutSigningProfile` on a name the region already holds REPLACES the stored `SigningProfileRecord` while deliberately keeping the profile's identity (same ARN, same `created_at`, same `profile_token`) and its whole version history (`versions` is cloned from the existing profile and the new version pushed onto it). The replacement record is built with `status: Active`, `status_reason: None` and `revocation_record: None`, so a re-put of a profile that `RevokeSigningProfile` had moved to `Revoked` silently DISCARDS the revocation fact: the instant trust ended, who ended it and why are gone, while every version that revocation covered is still listed in `versions` and still resolves through the unchanged profile ARN.

The consequence is observable on a security decision path. `GetRevocationStatus` answers for a `profileVersionArn` by asking `profile.version_revoked_at(version, signature_timestamp)`, which consults `self.revocation_record`; with the record erased it returns `false`, so a version ARN that answered "revoked" before the re-put answers "nothing the request named is revoked" after it — the "this signature is fine" answer — for the very signatures the revocation had untrusted. `GetSigningProfile` likewise stops reporting the revocation it previously reported.

This directly contradicts the rule the crate states for the same stored field one level down: `SigningProfileRecord::revoke` refuses to even UPDATE an existing record precisely so that trust is not restored, yet the put path removes the record outright. (This is not the duplicate-name refusal of open issue 20261023T031331Z: that one is about whether a second put is accepted at all; this is about the revocation state the accepted put destroys while keeping the profile's identity and version list.)

## Evidence

The write that loses the state — services/service-com-amazonaws-signer-wallabyservice/src/ops/put_signing_profile.rs:284-337:
```
        let existing = store.profile(&region, &input.profile_name);
        let created_at = existing.map_or(now, |profile| profile.created_at);
        let mut versions = existing
            .map(|profile| profile.versions.clone())
            .unwrap_or_default();
...
            // `statusReason` reports why a profile is not active, so an active
            // one carries none — and an active one has not been revoked, so it
            // carries no revocation record either.
            status_reason: None,
            revocation_record: None,
```
followed by `store.insert_profile(&region, record);` (put_signing_profile.rs:342), whose own contract is "Store `record` as `region`'s profile of that name, replacing whatever that region held under it" (src/state.rs:573-579).

The rule the same crate states for that field — src/state.rs:285-290 (`SigningProfileRecord::revoke`):
```
    /// A revocation is a fact about trust that has already ended, so it is
    /// recorded ONCE: revoking an already-revoked profile leaves the first record
    /// standing rather than restating when, from which instant, why and by whom
    /// trust ended. Moving `revocation_effective_from` forward would otherwise
    /// restore trust in signatures the first revocation had already placed after
    /// it.
```

The read that goes wrong — src/state.rs:320-337 (`version_revoked_at`): "`RevokeSigningProfile` revokes the PROFILE — it moves the profile itself to `Revoked` and records ONE revocation record on it — so the revocation covers every version the profile has had", implemented as
```
        if !self.versions.iter().any(|held| held == version) {
            return false;
        }
        let Some(record) = self.revocation_record.as_ref() else {
            return false;
        };
```
and called from src/ops/get_revocation_status.rs:249-256, which pushes the version ARN into `revokedEntities` only when `profile.version_arn(profile_version) == input.profile_version_arn && profile.version_revoked_at(...)`. Because `arn` and `versions` survive the re-put unchanged, the ARN still matches and only the record is missing, so the entity is reported as NOT revoked.

Test coverage: tests/all/cancel_profile_status_transitions.rs:216-217 and :282-288 pin only the STATUS after a revoke-then-re-put (`Setup::RevokedThenReput` → "likewise `Active` again", `status_before: "Active"`); no test in tests/all/ asserts that the revocation record is gone after a re-put (grep for `re-put|reput` across tests/all shows only version/status/tagging assertions), and tests/all/profile_revocation_is_recorded_once.rs pins the opposite direction — a repeat revoke "leaves the first record standing".

## Steps to Reproduce

1. Boot the emulator and PUT /signing-profiles/Revoked_Then_Reput with {"platformId":"AWSLambda-SHA384-ECDSA"}; note the returned profileVersion V1 and profileVersionArn A1.
2. PUT /signing-profiles/Revoked_Then_Reput/revoke with {"profileVersion":"V1","reason":"key compromise","effectiveTime":<t0>} — answers 204.
3. GET /revocations?signatureTimestamp=<t0+1>&platformId=AWSLambda-SHA384-ECDSA&profileVersionArn=A1&jobArn=<any signer job ARN>&certificateHashes=aa — observe revokedEntities contains A1, and GET /signing-profiles/Revoked_Then_Reput reports status Revoked with a revocationRecord.
4. PUT /signing-profiles/Revoked_Then_Reput again with the same platformId (accepted today, minting version V2).
5. Repeat the GetRevocationStatus call from step 3 with the UNCHANGED profileVersionArn A1: revokedEntities is now empty — the revoked version reports as not revoked — and GET /signing-profiles/Revoked_Then_Reput reports no revocationRecord and no statusReason at all.
