Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signer.sign-payload-on-canceled-profile-rejected signer.revoke-signing-profile-blocks-signing-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer.sign-payload-on-canceled-profile-rejected|run#8|AssertionError|assert code == "ValidationException", f"unexpected code: {code!r}"
Issue-Key: signer.revoke-signing-profile-blocks-signing-rejected|run#10|AssertionError|assert code == "ValidationException", f"unexpected code: {code!r}"
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# SignPayload does not refuse a profile that is not Active: Canceled and Revoked profiles get 501 NotImplemented instead of a 400 ValidationException naming the state

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignPayload

## Description
Two net-new causes, both in the signer SignPayload data-plane path. They are grounded in two failing behavioral checks that the replay report did NOT mark as covered; neither cause is stated by the open stub issue 20261024T054020Z-signer-signpayload-stub.md (which only asks for the operation to be written and explicitly declares the stub's 501 "correct while unimplemented"), so the lifecycle-state refusal contract SignPayload owes is recorded nowhere today. Naming that issue for cross-reference: whoever implements the handler must also satisfy the two refusals below.

Cause 1 of 2: SignPayload against a Canceled signing profile is not refused with the modeled client error.
AWS: SignPayload (POST /signing-jobs/with-payload, success 200) models ValidationException (HTTP 400) and ResourceNotFoundException (HTTP 404) among its errors — Smithy: SignPayload errors = AccessDeniedException(403), InternalServiceErrorException(500), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400); inputs profileName (ProfileName, len 2..64, pattern ^[a-zA-Z0-9_]{2,}$), payload (len 1..4096), payloadFormat all required. The model does not describe the lifecycle rule, so for the rule itself this is recorded behavior only: after CancelSigningProfile, GetSigningProfile settles the profile to status "Canceled" and a subsequent SignPayload on it is refused with error code "ValidationException" and the message "Signing profile with name: <profileName> is in state Canceled" — fixed text "Signing profile with name: " + the profileName taken verbatim from the request + " is in state " + the profile's current status from service state ("Canceled" here). No signing job is created and the profile's status is unchanged by the refused call.
Emulator: the call returns error code "NotImplemented" (the scaffold's HTTP 501 from `Err(crate::scaffolded::Scaffolded::not_implemented("SignPayload"))`) for every SignPayload request, Canceled profile or not. The check asserted the code and read 'NotImplemented'.
Why: a client that relies on SignPayload refusing a canceled profile with a retriable-never 400 ValidationException instead sees a 501 server-class error. SDK retry/error handling treats the two differently (501 is not a modeled error of this operation at all, so a typed catch for ValidationException never fires), and a caller cannot distinguish "this profile is no longer usable" from "the service is broken". Worse, the check's negative-space guarantee — that a canceled profile can never sign again — is untested, so an implementation that signs on a Canceled profile would pass unnoticed.
Fix: the SignPayload handler and its validation: load the named profile from signer state, and before minting a job reject any profile whose status is not Active with the modeled ValidationException carrying the message template above.
Traces: signer.sign-payload-on-canceled-profile-rejected (also asserts the positive control: a fresh Active profile's SignPayload returns a jobId).

Cause 2 of 2: SignPayload against a Revoked signing profile is not refused either, so revocation is not permanent for the data plane.
AWS: same modeled surface as above; rule itself is recorded behavior only. After RevokeSigningProfile(profileName, profileVersion, reason, effectiveTime), GetSigningProfile settles the profile to status "Revoked", and SignPayload on it is refused with error code "ValidationException" and message "Signing profile with name: <profileName> is in state Revoked" — same template as Cause 1, with the state word taken from the profile's stored status ("Revoked"). The refused call leaves the profile Revoked and creates no job.
Emulator: error code "NotImplemented" (HTTP 501) again, so revocation has no effect on the signing path.
Why: revocation is a security control; a client that revokes a profile to stop further signing gets no evidence the control took effect, and the 501 masks whether the emulator would have signed. Code-signing flows that assert "revoked profiles cannot sign" cannot be exercised against the emulator at all.
Fix: same component as Cause 1 — the SignPayload handler's pre-flight state check must read the profile's lifecycle status (Active / Canceled / Revoked) from signer state and refuse non-Active profiles; the status word in the message comes from state, not from a constant.
Traces: signer.revoke-signing-profile-blocks-signing-rejected (also asserts the positive control: a fresh Active profile signs and DescribeSigningJob reports Succeeded).

## Evidence
Check signer.sign-payload-on-canceled-profile-rejected:
  expected: ClientError code "ValidationException", message "Signing profile with name: <profileName> is in state Canceled"
  actual:   AssertionError: unexpected code: 'NotImplemented'  (check_signer_sign_payload_on_canceled_profile_rejected.py line 42, `assert code == "ValidationException"`)

Check signer.revoke-signing-profile-blocks-signing-rejected:
  expected: ClientError code "ValidationException", message "Signing profile with name: <profileName> is in state Revoked"
  actual:   AssertionError: unexpected code: 'NotImplemented'  (check_signer_revoke_signing_profile_blocks_signing_rejected.py line 51, `assert code == "ValidationException"`)

Both checks reached the SignPayload call only after PutSigningProfile and the status poll succeeded (GetSigningProfile did report Canceled / Revoked), so the divergence is isolated to SignPayload's response. Source of the 501: services/service-com-amazonaws-signer-wallabyservice/src/ops/sign_payload.rs line 171 returns Scaffolded::not_implemented("SignPayload"); modeled errors available on that channel are listed at lines 100-106 (ValidationException included). Smithy model consulted: services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json, operation SignPayload.

## Steps to Reproduce
1. Boot the signer emulator.
2. Canceled case: PutSigningProfile(profileName=<p1>, platformId="Notation-OCI-SHA384-ECDSA"); CancelSigningProfile(profileName=<p1>); poll GetSigningProfile(profileName=<p1>) until status == "Canceled".
3. Call SignPayload(profileName=<p1>, payload=<notary payload JSON bytes>, payloadFormat="application/vnd.cncf.notary.payload.v1+json"). Observe error code "NotImplemented" (HTTP 501); expected HTTP 400 "ValidationException" with message "Signing profile with name: <p1> is in state Canceled".
4. Revoked case: PutSigningProfile(profileName=<p2>, platformId="Notation-OCI-SHA384-ECDSA"); RevokeSigningProfile(profileName=<p2>, profileVersion=<from PutSigningProfile>, reason=<any>, effectiveTime=<now>); poll GetSigningProfile until status == "Revoked".
5. Call SignPayload(profileName=<p2>, ...) as in step 3. Observe "NotImplemented"; expected HTTP 400 "ValidationException" with message "Signing profile with name: <p2> is in state Revoked".
6. Positive control for both: PutSigningProfile on a fresh profile and SignPayload against it must return a jobId whose DescribeSigningJob eventually reports status "Succeeded".
