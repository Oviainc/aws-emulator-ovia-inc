Issue-Source: trace-sweep-divergence
Issue-Trace: signer-lambda-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-lambda-profile-lifecycle signer-profile-revocation-workflow signer-cross-account-profile-permissions signer-error-paths-jobs-and-permissions signer-notation-signpayload-revoke
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.platform
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.profileToken
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.signatureValidityPeriod
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.tags
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.revocationRecord
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.status
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.statusReason
Issue-Key: signer|PutSigningProfile|2xx->2xx|$.tags
Issue-Key: signer|CancelSigningProfile|2xx->2xx|status
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Signer profile responses drop modeled members and never record revocation state

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSigningProfile

## Description
Five distinct signing-profile defects on the same fault: the profile the emulator stores and serializes is a reduced version of the AWS one — members that PutSigningProfile accepted are not persisted or not emitted, revocation is not recorded in profile state at all, and the cancel response uses the wrong success status. A fixer must close ALL FIVE causes below before submitting.

Cause 1 of 5: GetSigningProfile omits profileToken, signatureValidityPeriod and platform.
AWS: Smithy GetSigningProfileResponse models `signatureValidityPeriod` (SignatureValidityPeriod {value: Integer, type: ValidityType}); the recording also carries `profileToken` and `platform`, which the current model does not list — recorded behavior only for those two. The recording returns all three for a profile created with a signatureValidityPeriod of 135 MONTHS on platform AWSLambda-SHA384-ECDSA: `signatureValidityPeriod` echoes the value and unit supplied to PutSigningProfile, `platform` echoes the platform id, and `profileToken` is an opaque server-generated id (the recorded UUID is an example — only its presence and shape are a requirement).
Emulator: all three members are absent from the 200 body (got None for each) although the profile was created with that validity period and platform.
Why: a client reading back a profile it just created cannot see the validity period or platform it configured, so round-trip/verify flows and anything keyed on profileToken break even though the write appeared to succeed.
Fix: GetSigningProfile handler/serializer plus the profile record in state — persist signatureValidityPeriod and platform from PutSigningProfile and mint a profileToken at creation.
Traces: signer-lambda-profile-lifecycle#task#2 (also signer-lambda-profile-lifecycle#teardown#9)

Cause 2 of 5: the same GetSigningProfile omission also drops `tags`.
AWS: Smithy GetSigningProfileResponse models `tags` (TagMap). The recording returns `tags` as a present (here empty) map alongside profileToken, signatureValidityPeriod and platform.
Emulator: `$.tags` is absent entirely rather than present-and-empty, in addition to the three members of Cause 1.
Why: a client cannot distinguish 'profile has no tags' from 'this emulator does not report tags', so tag reconciliation loops re-tag on every pass.
Fix: GetSigningProfile serializer — always emit the modeled `tags` map.
Traces: signer-profile-revocation-workflow#task#3

Cause 3 of 5: RevokeSigningProfile does not change profile state — GetSigningProfile still reports Active with no revocation record.
AWS: Smithy GetSigningProfileResponse models `status` (SigningProfileStatus enum Active|Canceled|Revoked), `statusReason` (String) and `revocationRecord` (SigningProfileRevocationRecord {revocationEffectiveFrom, revokedAt, revokedBy}); RevokeSigningProfile @required members are profileName, profileVersion, reason, effectiveTime. After a successful revoke the recording returns status 'Revoked', statusReason echoing the request's reason ('rotate-signing-key' in the recording), and a populated revocationRecord (its timestamps and revokedBy principal are environment-specific — presence and shape are the requirement).
Emulator: status stays 'Active' through 15 polls, and statusReason and revocationRecord are absent, so the revoke is accepted and then discarded.
Why: the single most important state transition of this service is invisible. A client that polls for Revoked never converges, and every rule that depends on revoked state (notably CancelSigningProfile's ProfileRevoked refusal, filed separately) cannot fire.
Fix: RevokeSigningProfile handler + profile state — record status/statusReason/revocationRecord on revoke, and have GetSigningProfile serialize them.
Traces: signer-profile-revocation-workflow#task#5

Cause 4 of 5: PutSigningProfile's 200 response omits the `tags` object.
AWS: recorded behavior only — Smithy PutSigningProfileResponse models arn/profileVersion/profileVersionArn, while the recording's 200 body additionally carries `tags` holding the profile's resource identity: `resourceArn` (the signing-profile ARN derived from the request's profileName, e.g. arn:aws:signer:<region>:<account>:/signing-profiles/<name>) and an opaque `internalId` (shape-only, not asserted).
Emulator: `$.tags` is absent from the create response (got None).
Why: callers that take the resourceArn straight from the create response to tag or reference the new profile get nothing and must re-derive the ARN themselves; the recorded flows use this value in the following step.
Fix: PutSigningProfile handler/serializer — emit the tags object, with resourceArn built from the request and a generated internalId. Region/account/name in the ARN are environment-derived, not fixed values.
Traces: signer-cross-account-profile-permissions#setup#1 (also signer-error-paths-jobs-and-permissions#task#1, signer-notation-signpayload-revoke#setup#2)

Cause 5 of 5: CancelSigningProfile answers 200 instead of 204 No Content.
AWS: Smithy models CancelSigningProfile as DELETE /signing-profiles/{profileName} with output Unit and http code 200, but the RECORDING shows 204 on success — take the recorded status: the corpus asserts 204 with no body. (Note the model/recording disagreement explicitly; the wire-accurate value for this corpus is 204.)
Emulator: returns 200.
Why: a client or SDK that treats only 204 as the success signal for the delete, or that tries to parse a body on 200, mis-reads a successful cancel.
Fix: CancelSigningProfile handler — return the 204 success status for the empty (Unit) output.
Traces: signer-cross-account-profile-permissions#teardown#6 (also signer-error-paths-jobs-and-permissions#teardown#10, signer-lambda-profile-lifecycle#teardown#8, signer-notation-signpayload-revoke#teardown#7)

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0 — not reset artefacts).

Cause 1 — signer-lambda-profile-lifecycle#task#2, GetSigningProfile, 200 vs 200: $.profileToken missing; $.signatureValidityPeriod missing (expected {'value': 135, 'type': 'MONTHS'}); $.platform missing (expected 'AWSLambda-SHA384-ECDSA'). Cluster also covers signer-lambda-profile-lifecycle#teardown#9.
Cause 2 — signer-profile-revocation-workflow#task#3, GetSigningProfile, 200 vs 200: $.profileToken, $.signatureValidityPeriod, $.tags (expected {}) and $.platform all missing.
Cause 3 — signer-profile-revocation-workflow#task#5, GetSigningProfile, 200 vs 200: $.status value_mismatch (expected 'Revoked', got 'Active') and 'poll: not in [Revoked] after 15 attempt(s)'; $.statusReason missing (expected 'rotate-signing-key'); $.revocationRecord missing (expected {revocationEffectiveFrom, revokedAt, revokedBy: arn:aws:iam::<account>:user/...}); plus the Cause 1/2 member omissions.
Cause 4 — signer-cross-account-profile-permissions#setup#1, PutSigningProfile, 200 vs 200: $.tags missing (expected {'resourceArn': 'arn:aws:signer:<region>:<account>:/signing-profiles/<profileName>', 'internalId': shaped-not-asserted}). Cluster also covers signer-error-paths-jobs-and-permissions#task#1 and signer-notation-signpayload-revoke#setup#2.
Cause 5 — signer-cross-account-profile-permissions#teardown#6, CancelSigningProfile: status expected 204, got 200. Cluster also covers signer-error-paths-jobs-and-permissions#teardown#10, signer-lambda-profile-lifecycle#teardown#8, signer-notation-signpayload-revoke#teardown#7.

Model evidence (smithy_query on services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json): GetSigningProfileResponse members include signatureValidityPeriod, tags (TagMap), status (SigningProfileStatus = Active|Canceled|Revoked), statusReason, revocationRecord (SigningProfileRevocationRecord{revocationEffectiveFrom, revokedAt, revokedBy}); RevokeSigningProfile requires profileName, profileVersion, reason, effectiveTime; CancelSigningProfile is DELETE /signing-profiles/{profileName} with output Unit.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Causes 1-2: PUT /signing-profiles/<name> with platformId AWSLambda-SHA384-ECDSA and signatureValidityPeriod {value:135,type:MONTHS}, then GET /signing-profiles/<name> -> 200 body has no profileToken, no signatureValidityPeriod, no platform and no tags (fixtures signer-lambda-profile-lifecycle step task#2 and signer-profile-revocation-workflow step task#3).
3. Cause 3: for that profile call PUT /signing-profiles/<name>/revoke with profileVersion, reason 'rotate-signing-key' and effectiveTime, then poll GET /signing-profiles/<name> -> status remains 'Active' for 15 attempts and statusReason/revocationRecord never appear (fixture signer-profile-revocation-workflow, step task#5).
4. Cause 4: inspect the 200 body of the PUT /signing-profiles/<name> create call itself -> no `tags` object with resourceArn/internalId (fixture signer-cross-account-profile-permissions, step setup#1).
5. Cause 5: DELETE /signing-profiles/<name> -> emulator answers 200; the corpus records 204 with no body (fixture signer-cross-account-profile-permissions, step teardown#6).
