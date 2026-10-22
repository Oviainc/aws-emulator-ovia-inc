Issue-Source: trace-sweep-divergence
Issue-Trace: codeguru-security-account-encryption-configuration
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguru-security-account-encryption-configuration
Issue-Shape-Id: com.amazonaws.codegurusecurity#AwsCodeGuruSecurity
Issue-Key: codeguru-security|UpdateAccountConfiguration|4xx->2xx|$.message
Issue-Key: codeguru-security|UpdateAccountConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeguru-security|UpdateAccountConfiguration|4xx->2xx|status
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# UpdateAccountConfiguration returns 200 instead of ResourceNotFoundException when no account configuration exists

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountConfiguration

## Description
CodeGuru Security's UpdateAccountConfiguration does not create an account encryption configuration out of nothing: when the caller's account has no existing account configuration, the operation fails with its modeled not-found error rather than succeeding. The emulator instead treats the update as unconditionally successful, so the error branch of this operation is unreachable.

Cause 1 of 1: UpdateAccountConfiguration on an account with no existing account configuration.
  AWS:      Smithy: UpdateAccountConfiguration (PUT /updateAccountConfiguration), input/output member `encryptionConfig` (target EncryptionConfig, required); modeled error ResourceNotFoundException with @httpError 404, documented as "The resource specified in the request was not found." The recorded response for this step is HTTP 404 with header `x-amzn-errortype: ResourceNotFoundException` and a body carrying a `message` member (recorded example: "No existing account configuration found."). So the requirement is the 404 status and the ResourceNotFoundException error code plus a populated `message`; the exact recorded wording is only an example.
  Emulator: Returns HTTP 200 for the same request. No `x-amzn-errortype` response header is emitted and the response body has no `message` member — i.e. the request is reported as a successful configuration update.
  Why:      An SDK client sees success where AWS signals a missing resource. Clients that branch on ResourceNotFoundException to first provision an account configuration (or to surface a setup error) never take that branch, so they believe the KMS encryption configuration was applied when it was not. Error handling and retry/provision logic around account-level encryption setup cannot be exercised at all, and the emulator reports a stronger security posture (customer-managed encryption configured) than is actually stored.
  Fix:      The UpdateAccountConfiguration handler in the codeguru-security service: before applying `encryptionConfig`, look up the caller's account configuration in service state and, when absent, return the modeled ResourceNotFoundException (404, `x-amzn-errortype: ResourceNotFoundException`, body with a `message`) instead of serializing a 200 success. The error serializer must emit the errortype header for this operation's modeled client errors.
  Traces:   codeguru-security-account-encryption-configuration#task#2 (expected 404 / ResourceNotFoundException, got 200).

No existing open issue covers this operation (no open codeguru-security issues are filed).

## Evidence
Replay of fixture codeguru-security-account-encryption-configuration, step task#2 (operation UpdateAccountConfiguration), verdict=mismatch.

Expected (recorded AWS response):
  status: 404
  headers: x-amzn-errortype: ResourceNotFoundException
  body: $.message present (recorded example: 'No existing account configuration found.')

Actual (emulator):
  status: 200
  headers: x-amzn-errortype missing
  body: $.message missing (None)

Replay detail verbatim:
  status: expected 404, got 200
  $.message: missing (expected 'No existing account configuration found.', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)

Model support: smithy_query operation=UpdateAccountConfiguration lists ResourceNotFoundException (errorKind client, httpError 404) among its modeled errors; http binding is PUT /updateAccountConfiguration with success code 200. Replay summary: steps_total=5, steps_passed=4, steps_mismatched=1, score_pct=80.0, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from mainline (/__admin/health answers 200; data listener accepting).
2. Replay wire fixture 'codeguru-security-account-encryption-configuration' from trace set simparity-wire-aws-v1.
3. Let the fixture reach step task#2, which issues PUT /updateAccountConfiguration with an `encryptionConfig` payload for an account that has no existing account configuration (the preceding steps of the fixture do not create one).
4. Observe the response: the emulator answers HTTP 200 with an encryptionConfig-shaped success body and no x-amzn-errortype header.
5. Expected: HTTP 404 with header x-amzn-errortype: ResourceNotFoundException and a body containing a `message` member (recorded example: 'No existing account configuration found.').
