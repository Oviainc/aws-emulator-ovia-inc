Issue-Source: trace-sweep-divergence
Issue-Trace: signer-notation-signpayload-revoke
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-notation-signpayload-revoke signer-profile-revocation-workflow
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|PutSigningProfile|2xx->4xx|$.arn
Issue-Key: signer|PutSigningProfile|2xx->4xx|$.profileVersion
Issue-Key: signer|PutSigningProfile|2xx->4xx|$.profileVersionArn
Issue-Key: signer|PutSigningProfile|2xx->4xx|$.tags
Issue-Key: signer|PutSigningProfile|2xx->4xx|status
Issue-Key: signer|CancelSigningProfile|2xx->4xx|status
Issue-Key: signer|RevokeSigningProfile|2xx->2xx|$headers.content-length
Issue-Key: signer|GetSigningProfile|2xx->2xx|$.revocationRecord.revokedBy
Issue-Commit: 1ef06ca7a44534aa441d9eadf70f05d4ca480a0d

# Signer signing-profile lifecycle responses diverge from AWS across create, cancel and revoke

> _Note: this issue was identified against the code base at commit `1ef06ca7a44534aa441d9eadf70f05d4ca480a0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSigningProfile

## Description
Four distinct causes in the signer signing-profile lifecycle (create -> revoke -> cancel -> read). All four must be closed before this issue is resolved.

Cause 1 of 4: PutSigningProfile rejects a valid create request (fixture signer-notation-signpayload-revoke, step setup#2).
AWS: Smithy PutSigningProfile is PUT /signing-profiles/{profileName}, http code 200, output PutSigningProfileResponse {arn, profileVersion, profileVersionArn}. Only profileName (@httpLabel, @length 2..64, @pattern ^[a-zA-Z0-9_]{2,}$) and platformId are @required; signingMaterial (SigningMaterial{certificateArn}) is NOT required, so a container/notation-platform profile created without signingMaterial is a well-formed request and AWS answers 200 with the profile ARN and a 10-character profileVersion (@length 10, @pattern ^[a-zA-Z0-9]{10}$) plus profileVersionArn. The only modeled 400 is ValidationException.
Emulator: answers 400 and returns no body at all - arn, profileVersion, profileVersionArn (and the tags the recording carries) are all absent.
Why: profile creation is the first call of every signing workflow, so a client cannot create a signing profile at all; every later call in the workflow (revoke, cancel, get) then operates on a profile that does not exist, and the whole signing path is unusable against the emulator.
Fix: the PutSigningProfile handler's input validation (it is rejecting a request whose only optional member, signingMaterial, is absent) and the success path that persists the profile and serializes arn/profileVersion/profileVersionArn.

Cause 2 of 4: CancelSigningProfile reports the profile does not exist (fixture signer-notation-signpayload-revoke, step teardown#7).
AWS: Smithy CancelSigningProfile is DELETE /signing-profiles/{profileName} with an empty (Unit) output; the recording shows status 204 with no body for a profile that exists. ResourceNotFoundException/404 is modeled only for a profile that is absent.
Emulator: answers 404 for the profile the same fixture created earlier in the run.
Why: a client cannot delete/cancel a signing profile it owns, and a 404 here is indistinguishable from a genuinely missing profile, so cleanup and idempotent teardown logic break.
Hypothesis: secondary to Cause 1 - the profile was never created because setup#2 was rejected, so the cancel finds no state. It must be re-replayed after Cause 1 is fixed and, if it then still 404s, fixed in the CancelSigningProfile handler's state lookup.
Fix: the CancelSigningProfile handler's state lookup / the shared signing-profile store keyed by profileName.

Cause 3 of 4: the empty RevokeSigningProfile response omits the Content-Length header (fixture signer-profile-revocation-workflow, step task#4).
AWS: recorded behavior only - RevokeSigningProfile (PUT /signing-profiles/{profileName}/revoke, Unit output) answers with an empty body and the recording carries a Content-Length response header. The status itself matches (204 expected, 204 returned).
Emulator: returns 204 with no Content-Length header at all.
Why: clients and intermediaries that frame a bodyless response by Content-Length (rather than by status alone) can hang or treat the response as truncated; the header value itself (0) is not the requirement, its presence on an empty-body response is.
Fix: the REST response serializer's empty-body/no-output path shared by the signer operations with Unit output.

Cause 4 of 4: GetSigningProfile reports a synthesized revoker identity in revocationRecord.revokedBy (fixture signer-profile-revocation-workflow, step task#5).
AWS: Smithy SigningProfileRevocationRecord.revokedBy is documented as "The identity of the revoker" - the principal that called RevokeSigningProfile, derived from the request's SigV4 credentials. The recording shows a stable IAM user principal ARN (example: arn:aws:iam::000000000000:user/emulator); the literal name is an example, the requirement is that revokedBy be the caller principal the emulator resolves for the signed request, the same identity it reports elsewhere for those credentials, and stable across runs.
Emulator: returns arn:aws:iam::000000000000:user/EMULATOR37FD40F9290E - a per-boot synthesized user name (random hex suffix) rather than the principal resolved from the revoking request's credentials.
Why: audit/compliance clients read revokedBy to attribute a revocation to a principal; a value invented per boot cannot be correlated with the caller, is not stable between runs, and breaks assertions that compare revokedBy against the identity used to make the call.
Fix: the caller-identity resolution used when the RevokeSigningProfile handler builds the revocation record (it should use the request's resolved principal, not a generated placeholder) and the GetSigningProfile serializer that echoes it.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 49 steps, 19 passed, score 38.8%).

Cause 1 - signer PutSigningProfile, fixture signer-notation-signpayload-revoke step setup#2:
  expected status 200, actual 400
  $.arn: missing (expected an arn:aws:signer:...:/signing-profiles/<profileName> value, got None)
  $.profileVersion: missing (expected a shaped 10-char version, got None)
  $.profileVersionArn: missing (expected <profile arn>/<profile_version>, got None)
  $.tags: missing (expected a tag map, got None)
  divergence_key: service=signer|PutSigningProfile|status=200->400|keys=$.arn,$.profileVersion,$.profileVersionArn,$.tags,status
  Smithy: PutSigningProfile http PUT /signing-profiles/{profileName} code 200; required members profileName, platformId only; signingMaterial optional; 400 only via ValidationException.

Cause 2 - signer CancelSigningProfile, fixture signer-notation-signpayload-revoke step teardown#7:
  expected status 204, actual 404
  divergence_key: service=signer|CancelSigningProfile|status=204->404|keys=status
  Smithy: CancelSigningProfile DELETE /signing-profiles/{profileName}, output Unit; ResourceNotFoundException/404 modeled only for an absent profile.

Cause 3 - signer RevokeSigningProfile, fixture signer-profile-revocation-workflow step task#4:
  status matched (204/204) but $headers.content-length: missing (expected present, got None)
  divergence_key: service=signer|RevokeSigningProfile|status=204->204|keys=$headers.content-length

Cause 4 - signer GetSigningProfile, fixture signer-profile-revocation-workflow step task#5:
  status matched (200/200) but $.revocationRecord.revokedBy: value_mismatch
  expected 'arn:aws:iam::000000000000:user/emulator' (example of the caller principal), got 'arn:aws:iam::000000000000:user/EMULATOR37FD40F9290E'
  divergence_key: service=signer|GetSigningProfile|status=200->200|keys=$.revocationRecord.revokedBy
  Smithy: SigningProfileRevocationRecord.revokedBy = "The identity of the revoker".

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture signer-notation-signpayload-revoke, step setup#2 - PutSigningProfile PUT /signing-profiles/<profileName> for a notation/container platform with no signingMaterial. Observe 400 with an empty body instead of 200 with arn/profileVersion/profileVersionArn.
4. Cause 2: same fixture, step teardown#7 - CancelSigningProfile DELETE /signing-profiles/<profileName> for the profile created in step 3. Observe 404 instead of 204.
5. Cause 3: fixture signer-profile-revocation-workflow, step task#4 - RevokeSigningProfile PUT /signing-profiles/<profileName>/revoke. Observe 204 with no Content-Length response header.
6. Cause 4: same fixture, step task#5 - GetSigningProfile GET /signing-profiles/<profileName> after the revoke. Observe revocationRecord.revokedBy is a per-boot synthesized principal (user/EMULATOR<hex>) rather than the principal resolved from the revoking request's credentials.
7. Re-replay all four fixture#step bindings after fixing: signer-notation-signpayload-revoke#setup#2, signer-notation-signpayload-revoke#teardown#7, signer-profile-revocation-workflow#task#4, signer-profile-revocation-workflow#task#5.
