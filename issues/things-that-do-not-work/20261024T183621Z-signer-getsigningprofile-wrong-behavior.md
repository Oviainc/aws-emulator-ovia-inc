Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Signing-profile state is not Region-scoped: a profile created in one Region is readable and mutable through every other Region's endpoint

_Filed: 2026-10-24T18:36:21Z_
_Operation: GetSigningProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS Signer is a regional service: a signing profile created through the us-west-2 endpoint exists only in us-west-2, its ARN is `arn:aws:signer:us-west-2:<account>:/signing-profiles/<name>`, and `GetSigningProfile` for that name against the eu-west-1 endpoint answers HTTP 404 `ResourceNotFoundException` because no such profile exists in that Region.

Expected: profiles are keyed by (Region, profileName). A read or mutation signed for a Region other than the one a profile was created in finds nothing — `GetSigningProfile` → 404 `ResourceNotFoundException`, `ListSigningProfiles` omits it, `CancelSigningProfile`/`RevokeSigningProfile`/`StartSigningJob`/`AddProfilePermission` → 404.

Actual: the store holds one global `profiles: BTreeMap<String, SigningProfileRecord>` keyed by `profileName` only. `crate::state::request_region(ctx.parts())` is read in exactly one place — `PutSigningProfile`, to build the ARN that is then frozen on the record — and no read or mutation consults the request's Region again. So `GetSigningProfile` signed for eu-west-1 returns HTTP 200 for a profile created in us-west-2, and the body it returns advertises `arn`/`profileVersionArn` in us-west-2 — an answer in which the resource's own ARN contradicts the endpoint that served it. `ListSigningProfiles` likewise lists every Region's profiles from any Region, a second `PutSigningProfile` on the same name from another Region re-versions the existing record (keeping the original Region's ARN), and cancel/revoke/StartSigningJob all act cross-Region.

The emulator already treats the Region as part of a profile's identity on the tagging path — `profile_for_tagging` requires `profile.arn == resourceArn`, so a tagging call carrying another Region's ARN is correctly refused `NotFoundException` — which makes the divergence self-inconsistent: the same profile is "not in eu-west-1" for TagResource/ListTagsForResource and "in eu-west-1" for GetSigningProfile.

## Evidence

Spec/well-established semantics: the model's `awsService` gives endpointPrefix `signer` with per-Region endpoints, and Signer profile ARNs embed the Region (`crate::state::profile_arn` builds `arn:aws:signer:<region>:<account>:/signing-profiles/<name>`); a regional AWS resource is not visible through another Region's endpoint, and `GetSigningProfile` models `ResourceNotFoundException` (HTTP 404) for "A specified resource could not be found". Handler/state: src/state.rs — `ComAmazonawsSignerWallabyserviceState.profiles` is a `BTreeMap<String, SigningProfileRecord>` keyed by name with no Region dimension, and the `SigningProfileRecord` doc says the ARN is "stored rather than recomputed per read, so a profile created by a request signed for one region keeps reporting that region's ARN"; src/ops/get_signing_profile.rs lines ~188-211 look up `store.profiles.get(&input.profile_name)` after checking only `profileOwner`, with `let _ = ctx;` discarding the request parts the Region would come from; src/ops/list_signing_profiles.rs lines ~166-250 iterate the whole map with no Region filter (also `let _ = ctx;`). Contrast: src/state.rs `profile_for_tagging`/`profile_for_tagging_mut` filter on `profile.arn == resource_arn`, which is Region-sensitive. Test coverage: tests/all/resource_tags.rs case "the profile's name in ANOTHER region" (line ~608) asserts only the tagging refusal; no test in tests/all/* issues a GetSigningProfile or ListSigningProfiles signed for a second Region, so the cross-Region read is unasserted in either direction.

## Steps to Reproduce

1. Send a SigV4-signed PUT /signing-profiles/Region_Scoped with {"platformId": "AWSLambda-SHA384-ECDSA"} whose credential scope is `.../us-west-2/signer/aws4_request` (Host: signer.us-west-2.amazonaws.com). Note the returned arn `arn:aws:signer:us-west-2:<account>:/signing-profiles/Region_Scoped`.
2. Send GET /signing-profiles/Region_Scoped signed for `.../eu-west-1/signer/aws4_request` (Host: signer.eu-west-1.amazonaws.com). Observe HTTP 200 reporting `arn`/`profileVersionArn` in us-west-2, instead of HTTP 404 ResourceNotFoundException.
3. Send GET /signing-profiles signed for eu-west-1. Observe the us-west-2 profile in `profiles`, instead of an empty list.
4. Send GET /tags/arn:aws:signer:eu-west-1:<account>:/signing-profiles/Region_Scoped — this one IS refused 404 NotFoundException, showing the tagging path scopes by Region while the profile reads do not.
5. Send PUT /signing-profiles/Region_Scoped/revoke (or DELETE /signing-profiles/Region_Scoped) signed for eu-west-1 and observe it mutates the us-west-2 profile.
