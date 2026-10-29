Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: bd5cf0434f64f19b5e9fa5ffd9f78f8fdf0b4a2e

# RevokeSigningProfile labels an unknown profileVersion with the ProfileNotFound code the crate reserves for a profile that does not exist

_Filed: 2026-10-29T09:41:53Z_
_Operation: RevokeSigningProfile | Class: error-response_

> _Note: this issue was identified against the code base at commit `bd5cf0434f64f19b5e9fa5ffd9f78f8fdf0b4a2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `RevokeSigningProfile` refuses a request because the profile has no such VERSION, it answers the 404 with `code: "ProfileNotFound"` — the code this crate defines and documents as meaning "No profile of that name" — even though the profile itself was found one line earlier. The same condition on the sibling handler `AddProfilePermission` (a `profileVersion` the profile has never had) is answered with `code: "ResourceNotFoundException"`, the rule the crate's own permissions module states for a 404 about a SUB-resource of a profile, and that side is pinned by a test.

So one crate answers the identical condition ("profile X exists but has no version V") with two different machine-readable discriminators, and the one `RevokeSigningProfile` picks is the code the crate itself documents as a different condition. A client that branches on `code` — which is exactly the rationale every code site in this crate gives — is told by RevokeSigningProfile that the profile does not exist, so a create-if-absent / re-put branch keyed on `ProfileNotFound` fires for a profile that is present and Active.

The handler comment asserts its own rule ("Both are the same condition to a caller, which is why they carry the one code"), which directly contradicts the permissions module's rule for the same sub-resource. One of the two has to go; the AddProfilePermission side is the one a test pins with a stated reason, the RevokeSigningProfile side is pinned by nothing (its test matches on `__type`, not on `code`).

## Evidence

Side A — RevokeSigningProfile uses the profile-not-found code for a missing VERSION. services/service-com-amazonaws-signer-wallabyservice/src/ops/revoke_signing_profile.rs:158-170:
```
/// The modelled 404 for a profile, or a profile version, this account holds none
/// of.
///
/// `code` names the condition a client branches on: the profile resource the
/// request named — a name, or a version of that name — does not exist. Both are
/// the same condition to a caller, which is why they carry the one code.
fn not_found(message: String) -> RevokeSigningProfileError {
    RevokeSigningProfileErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
        code: Some(crate::state::ERROR_CODE_PROFILE_NOT_FOUND.to_string()),
    })
```
and the version branch, revoke_signing_profile.rs:238-245:
```
        // `profileVersion` names WHICH version of the profile is revoked, so a
        // version this profile has never had names no revocable resource.
        if !profile.versions.contains(&input.profile_version) {
            return Err(not_found(format!(
                "Signing profile {} has no version {}",
                input.profile_name, input.profile_version
            )));
```
That constant's own documentation says it means something else — src/state.rs:54-55:
```
/// No profile of that name (or not one this account holds).
pub const ERROR_CODE_PROFILE_NOT_FOUND: &str = "ProfileNotFound";
```

Side B — the crate's rule for a 404 about a profile sub-resource, src/state/permissions.rs:20-23:
```
/// A resource the request's own values named — a statement id, a profile version —
/// does not exist: the modelled shape's own name, which is all a 404 for a
/// sub-resource Signer publishes no code for can say.
pub const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";
```
implemented for the identical condition at src/ops/add_profile_permission.rs:139-153:
```
/// The modelled 404 for a `profileVersion` the profile has never had: a
/// permission statement names the version it applies to, so a version that does
/// not exist is a resource this grant could not be written against.
fn profile_version_not_found(
    profile_name: &str,
    profile_version: &str,
) -> AddProfilePermissionError {
    ...
        code: Some(crate::state::permissions::ERROR_CODE_NOT_FOUND.to_string()),
```
and note add_profile_permission.rs:131-137 uses `ERROR_CODE_PROFILE_NOT_FOUND` only for an absent PROFILE — the two conditions are deliberately told apart there.

Test coverage: the AddProfilePermission side is pinned with a stated reason — tests/all/profile_permissions.rs:48-50 ("The error `code` a 404 for a sub-resource of the profile carries — the modelled shape's own name") asserted for the rows "a version never minted" / "another profile's version" at tests/all/profile_permissions.rs:1007-1038. The RevokeSigningProfile side is NOT pinned: tests/all/profile_revocation_and_identity.rs:600-658 (`revoking_an_unknown_profile_or_version_is_refused`) asserts only `answer.error_code()`, which reads `self.body["__type"]` (tests/all/profile_revocation_and_identity.rs:79-82), i.e. the error SHAPE, not the `code` member.

## Steps to Reproduce

1. PUT /signing-profiles/Rev_Probe with body {"platformId": "AWSLambda-SHA384-ECDSA"} — note the returned profileVersion.
2. PUT /signing-profiles/Rev_Probe/revoke with body {"profileVersion": "ZZZZZZZZZZ", "reason": "because", "effectiveTime": 1700000000} — a version the profile has never had.
3. Observe HTTP 404 with body code "ProfileNotFound", the code src/state.rs:54-55 documents as "No profile of that name", although GET /signing-profiles/Rev_Probe still returns the Active profile.
4. POST /signing-profiles/Rev_Probe/permissions with body {"profileVersion": "ZZZZZZZZZZ", "action": "signer:GetSigningProfile", "principal": "123456789012", "statementId": "s1"} — the same condition on a sibling handler.
5. Observe HTTP 404 with body code "ResourceNotFoundException": one crate, one condition, two codes a client branches on.
