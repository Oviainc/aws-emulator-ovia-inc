Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-ca-not-ready-operation-guards
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-ca-not-ready-operation-guards
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|RevokeCertificate|4xx->4xx|$.__type
Issue-Key: acm-pca|RevokeCertificate|4xx->4xx|$.message
Issue-Commit: 70957ec00c701454466831515708b974b6725531

# ACM PCA RevokeCertificate refuses a not-ready CA with InvalidStateException where AWS reports ResourceNotFoundException for the unknown certificate serial

> _Note: this issue was identified against the code base at commit `70957ec00c701454466831515708b974b6725531`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeCertificate

## Description
One net-new root cause on acm-pca `RevokeCertificate`: the operation's CA-status gate is applied too broadly and ahead of the certificate lookup, so a revoke aimed at a CA that is not in a revocable resting state is classified as a CA-state refusal instead of an unknown-certificate refusal.

Cause 1 of 1: RevokeCertificate against a not-ready CA must be refused as ResourceNotFoundException naming the certificate, not as InvalidStateException naming the CA.

AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json): `RevokeCertificate` input = {CertificateAuthorityArn @required (Arn, length 5..200, pattern ^arn:[\w+=/,.@-]+:acm-pca:...), CertificateSerial @required (String128, length 0..128), RevocationReason @required (enum)}, output = Unit; it models BOTH `InvalidStateException` ("The state of the private CA does not allow this action to occur.") and `ResourceNotFoundException` ("A resource such as a private CA, S3 bucket, certificate, audit report, or policy cannot be found."), each a client error with httpError 400. The model does not state which gate runs first; the recorded response does: for a CA in a not-ready resting state (the fixture's `ca-not-ready` guard CA) carrying a serial no certificate of that CA holds, AWS answers HTTP 400 with `__type` = `ResourceNotFoundException` and message `Could not find specified certificate.` — that sentence is entirely fixed text and interpolates nothing from the request or from state. In other words AWS resolves the named certificate before it judges the CA's status for this operation, so the refusal a caller sees is about the certificate the request named.

Emulator: the same step is answered HTTP 400 with `__type` = `InvalidStateException` and message `The certificate authority <CertificateAuthorityArn> is not in the correct state to have issued certificates.` (fixed text with the request's CA ARN interpolated — in the recorded run `arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/f92fd328-7c01-417c-9161-fdd344faf0f2`, which is a per-run value and not part of the requirement). Both the error code and the whole message diverge; only the 400 status matches.

Why: a client cannot distinguish "that serial names no certificate of this CA" from "this CA is in the wrong lifecycle state". The two refusals drive opposite recovery paths: `ResourceNotFoundException` is terminal for that serial and the caller must re-resolve the certificate (GetCertificate / audit report), whereas `InvalidStateException` reads as a precondition that will clear on its own, so retry-until-ACTIVE loops written against AWS spin forever against this emulator on a request that can never succeed. Error-code-keyed branching and any assertion on the message text both take the wrong branch.

Fix: the operation's handler and its ordering of gates — services/service-com-amazonaws-acmpca-acmprivateca/src/ops/revoke_certificate.rs resolves the CA, then rejects any status outside `crate::state::REVOCABLE_STATUSES` with `invalid_state(not_in_state_to_have_issued_message(&ca_arn))` BEFORE it ever looks the serial up with `record.certificate_with_serial_mut(..)`. The certificate lookup (and its `CERTIFICATE_NOT_FOUND_MESSAGE` refusal) must run for a CA in a not-ready state so the unknown serial is reported as the modeled `ResourceNotFoundException` with AWS's fixed sentence; keep the CA-state refusal only where the recording shows it. The refusal path must still mutate nothing.

RELATED, NOT COVERED: the open issue `20261030T020554Z-acm-pca-revokecertificate-refusal-semantics-are-absent-self.md` describes RevokeCertificate refusal semantics, but it was filed against a scaffold that answered 501 for every call, its Issue-Trace-Ids are behavioral check ids only (acm-pca.revoke-root-ca-self-signed-certificate-rejected, acm-pca.revoke-certificate-duplicate-revoke-already-processed, acm-pca.revoke-certificate-not-ready-fails), and it carries no wire fixture key. Its cause 3 asks precisely for the InvalidStateException the emulator NOW returns, so satisfying it does not surface — and can actively mask — the ordering defect this wire step pins down. This divergence is reported by the replay as owned by no open issue.

## Evidence
Replay of wire fixture corpus simparity-wire-aws-v1 against the emulator (emulator_up = true, bootstrap_signal = false; acm-pca is bootstrapped and serving — 76/87 steps passed).

Fixture acm-pca-ca-not-ready-operation-guards, step task#9, operation RevokeCertificate, verdict = mismatch, expected_status 400, actual_status 400:
  $.__type:   value_mismatch (expected 'ResourceNotFoundException', got 'InvalidStateException')
  $.message:  value_mismatch (expected 'Could not find specified certificate.',
                              got 'The certificate authority arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/f92fd328-7c01-417c-9161-fdd344faf0f2 is not in the correct state to have issued certificates.')

Requirement: HTTP 400, Error.Code = ResourceNotFoundException, Error.Message = "Could not find specified certificate." (wholly fixed text, no interpolation).
Actual:      HTTP 400, Error.Code = InvalidStateException, Error.Message = "The certificate authority {CertificateAuthorityArn} is not in the correct state to have issued certificates." — fixed text plus the request's CA ARN; the ARN above is that run's generated CA id and is not itself a requirement.

Code: services/service-com-amazonaws-acmpca-acmprivateca/src/ops/revoke_certificate.rs lines 225-229 (the REVOCABLE_STATUSES gate returning invalid_state(..)) precede lines 248-253 (the serial lookup returning not_found(CERTIFICATE_NOT_FOUND_MESSAGE)).

Smithy: RevokeCertificate errors include both InvalidStateException and ResourceNotFoundException, client kind, httpError 400 (defaulted).

## Steps to Reproduce
1. Boot the emulator and replay wire fixture `acm-pca-ca-not-ready-operation-guards`.
2. The fixture creates a private CA and leaves it in a not-ready resting state (never activated / no imported CA certificate), then at step `task#9` calls RevokeCertificate with that CA's ARN, a CertificateSerial that names no certificate of the CA, and a RevocationReason.
3. Observe the response: expected HTTP 400 `ResourceNotFoundException` with message "Could not find specified certificate."; the emulator returns HTTP 400 `InvalidStateException` with "The certificate authority <CA ARN> is not in the correct state to have issued certificates."

Replay shortcut: re-replay acm-pca-ca-not-ready-operation-guards#task#9.
