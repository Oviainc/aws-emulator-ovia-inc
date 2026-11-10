Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.revoke-root-ca-self-signed-certificate-rejected acm-pca.revoke-certificate-duplicate-revoke-already-processed acm-pca.revoke-certificate-not-ready-fails
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.revoke-root-ca-self-signed-certificate-rejected|run#21|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: acm-pca.revoke-certificate-duplicate-revoke-already-processed|run#7|AssertionError|assert root_code == "InvalidRequestException", (
Issue-Key: acm-pca.revoke-certificate-not-ready-fails|run#7|AssertionError|assert code == "InvalidStateException", f"expected InvalidStateException, got {code!r}"
Issue-Commit: 05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d

# ACM PCA RevokeCertificate refusal semantics are absent: self-signed-root revoke, duplicate revoke and non-revocable-CA-state all answer NotImplemented (3 causes)

> _Note: this issue was identified against the code base at commit `05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeCertificate

## Description
Three net-new root causes on acm-pca `RevokeCertificate`. All three are reached through the operation's handler `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/revoke_certificate.rs`, which is still an unmodified scaffold and answers HTTP 501 `NotImplemented` ("operation RevokeCertificate is scaffolded but not implemented in this emulator") to every request.

RELATED, NOT COVERED: the open issue `20261029T193511Z-acm-pca-revokecertificate-stub.md` ("Implement acm-pca RevokeCertificate: unimplemented scaffold stub") asks for the operation to be implemented, but its `Issue-Trace-Ids` carry only `acm-pca.e2e-ca-lifecycle-issue-revoke-audit-teardown`, `acm-pca.e2e-issue-and-revoke-certificate-reflected-in-audit-report`, `acm-pca.revoke-already-revoked-certificate-rejected`, `acm-pca.revoke-certificate-already-revoked-conflict` and `acm-pca.server-side-crl-config-persisted-and-enforced`. The three checks below are NOT in that list and are not replayed by that issue, and the refusal classification each one pins down is not described there. Each of the three refusal contracts below must hold once the handler exists; a handler that revokes unconditionally would still fail all three.

Cause 1 of 3 — revoking a root CA's own self-signed certificate must be refused as InvalidRequestException.
AWS: Smithy: `RevokeCertificate` (input `CertificateAuthorityArn` @required, `CertificateSerial` @required, `RevocationReason` @required; output is Unit) models `InvalidRequestException` as a client error with httpError 400. Recorded behavior: when `CertificateSerial` is the serial of the CA's OWN self-signed root certificate (the one imported by `ImportCertificateAuthorityCertificate`), AWS answers HTTP 400 `InvalidRequestException` with the message `Self-signed certificate can not be revoked.` — that sentence is entirely fixed text, it interpolates no request or state value. The CA's state must be left untouched: an audit report of the same CA must still show the root certificate with no `revokedAt`, while an end-entity certificate of the same CA revokes normally (the checks' positive control).
Emulator: the call is refused with HTTP 501 `NotImplemented`, so the error code observed is `NotImplemented` where `InvalidRequestException` is required (`AssertionError: expected InvalidRequestException, got NotImplemented`).
Why: a client that distinguishes "this certificate may never be revoked" from other refusals cannot do so — the 501 is not a modeled error of the operation, so botocore raises an unmodeled `ClientError` and retry/branching logic written against `InvalidRequestException` never runs. A caller trying to revoke a compromised CA root gets no actionable classification at all.
Fix: the `RevokeCertificate` handler and its validation — it must recognise the serial of the CA's own imported CA certificate and refuse it with the modeled `InvalidRequestException` and AWS's fixed sentence, before touching revocation state.
Traces: acm-pca.revoke-root-ca-self-signed-certificate-rejected, acm-pca.revoke-certificate-duplicate-revoke-already-processed.

Cause 2 of 3 — a first revoke of a leaf must succeed with no revocation configuration, and a second revoke of the same serial must be refused as RequestAlreadyProcessedException.
AWS: Smithy: `RevokeCertificate` models `RequestAlreadyProcessedException` as a client error with httpError 400 and returns an empty (Unit) output on success. Recorded behavior: revoking an end-entity certificate of a CA that has NO `RevocationConfiguration` at all (no CRL, no OCSP) succeeds with HTTP 200 and an empty body — revocation is simply recorded and not published anywhere. Replaying the SAME `CertificateAuthorityArn`/`CertificateSerial` pair is then refused with HTTP 400 `RequestAlreadyProcessedException` and an EMPTY message (`Error.Message` is the empty string; there is no fixed text and no interpolated value). The rejected duplicate must have no effect: the recorded revocation reason stays the one from the first, successful call.
Emulator: both calls are refused with HTTP 501 `NotImplemented`; there is no revocation state at all, so neither the success nor the already-processed refusal can be observed.
Why: revocation is not idempotent in AWS and clients rely on `RequestAlreadyProcessedException` to recognise a replay they already performed (a retried or duplicated workflow step). Against this emulator the operation never succeeds, so revocation never becomes durable state, and nothing downstream (CRL, OCSP, audit report) can ever reflect a revoked serial.
Fix: the `RevokeCertificate` handler plus the service's certificate state — record (serial, reason, revokedAt) per CA and classify a repeat of an already-revoked serial as `RequestAlreadyProcessedException` with an empty message, leaving the stored reason unchanged.
Traces: acm-pca.revoke-certificate-duplicate-revoke-already-processed.

Cause 3 of 3 — revoking against a CA that is not in a revocable state must be refused as InvalidStateException, with the CA's status unchanged.
AWS: Smithy: `RevokeCertificate` models `InvalidStateException` as a client error with httpError 400. Recorded behavior: with the CA in a non-revocable resting state (the check drives a never-activated ROOT CA to `DELETED` via `DeleteCertificateAuthority` with `PermanentDeletionTimeInDays=7`), `RevokeCertificate` is refused with HTTP 400 `InvalidStateException` and the message `The certificate authority <CertificateAuthorityArn> is not in the correct state to have issued certificates.` — fixed text except for `<CertificateAuthorityArn>`, which is the ARN taken verbatim from the request. The refused call must not mutate the CA: a following `DescribeCertificateAuthority` must report the same `Status` as before the call.
Emulator: the call is refused with HTTP 501 `NotImplemented` (`AssertionError: expected InvalidStateException, got 'NotImplemented'`), so neither the state gate nor the message exists.
Why: a client cannot tell "wrong CA lifecycle state" from "unknown serial", "bad ARN" or a genuine server fault; code that waits for a CA to become ACTIVE and retries on `InvalidStateException` sees an unmodeled 501 instead and treats a precondition failure as an outage.
Fix: the `RevokeCertificate` handler's state gate — check the CA's status before any revocation work and refuse non-revocable states with the modeled `InvalidStateException` and AWS's wording, interpolating the request's CA ARN; the refusal path must leave the stored CA record untouched.
Traces: acm-pca.revoke-certificate-not-ready-fails.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (acm-pca is bootstrapped; 61/125 checks pass, so the service is serving).

1) acm-pca.revoke-root-ca-self-signed-certificate-rejected [mismatch]
   check_acm_pca_revoke_root_ca_self_signed_certificate_rejected.py, line 93:
   assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
   AssertionError: expected InvalidRequestException, got NotImplemented
   Expected: HTTP 400, Error.Code = InvalidRequestException, Error.Message = "Self-signed certificate can not be revoked." (wholly fixed text).
   Actual:   HTTP 501, Error.Code = NotImplemented, Error.Message = "operation RevokeCertificate is scaffolded but not implemented in this emulator".

2) acm-pca.revoke-certificate-duplicate-revoke-already-processed [mismatch]
   check_acm_pca_revoke_certificate_duplicate_revoke_already_processed.py, line 35:
   assert root_code == "InvalidRequestException"
   AssertionError: unexpected error code revoking the root CA's self-signed cert: 'NotImplemented'
   Same check further requires: first leaf revoke -> HTTP 200 empty body on a CA with no RevocationConfiguration; second revoke of that serial -> HTTP 400 Error.Code = RequestAlreadyProcessedException with Error.Message = "" (empty); audit report shows the root serial with no revokedAt and the leaf revoked exactly once with the first call's reason.
   Actual: every RevokeCertificate call answers HTTP 501 NotImplemented, so no revocation state is created.

3) acm-pca.revoke-certificate-not-ready-fails [mismatch]
   check_acm_pca_revoke_certificate_not_ready_fails.py, line 90:
   assert code == "InvalidStateException", f"expected InvalidStateException, got {code!r}"
   AssertionError: expected InvalidStateException, got 'NotImplemented'
   Expected: HTTP 400, Error.Code = InvalidStateException, Error.Message = "The certificate authority {CertificateAuthorityArn} is not in the correct state to have issued certificates." (fixed text; the ARN is the request's CertificateAuthorityArn), and DescribeCertificateAuthority still reports the pre-call Status (DELETED).
   Actual:   HTTP 501, Error.Code = NotImplemented.

Handler: services/service-com-amazonaws-acmpca-acmprivateca/src/ops/revoke_certificate.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Scaffolded::not_implemented(..)`.

Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json): RevokeCertificate input = {CertificateAuthorityArn @required (Arn, length 5..200), CertificateSerial @required (String128, length 0..128), RevocationReason @required (enum)}, output = Unit, errors = ConcurrentModificationException, InvalidArnException, InvalidRequestException, InvalidStateException, LimitExceededException, RequestAlreadyProcessedException, RequestFailedException, RequestInProgressException, ResourceNotFoundException (all client, httpError 400).

## Steps to Reproduce
Cause 1 and 2:
1. Boot the acm-pca emulator.
2. CreateCertificateAuthority (CertificateAuthorityType=ROOT, RSA_2048/SHA256WITHRSA, no RevocationConfiguration); wait for Status=PENDING_CERTIFICATE.
3. GetCertificateAuthorityCsr, IssueCertificate with TemplateArn arn:aws:acm-pca:::template/RootCACertificate/V1, GetCertificate, ImportCertificateAuthorityCertificate; wait for Status=ACTIVE. Note the root certificate's serial.
4. Call RevokeCertificate with that root serial and RevocationReason=UNSPECIFIED.
   Expected HTTP 400 InvalidRequestException "Self-signed certificate can not be revoked."; observed HTTP 501 NotImplemented.
5. IssueCertificate for an end-entity CSR, GetCertificate, note its serial. Call RevokeCertificate on that serial — expected HTTP 200 with an empty body; observed HTTP 501 NotImplemented.
6. Call RevokeCertificate again with the same serial — expected HTTP 400 RequestAlreadyProcessedException with an empty Error.Message; observed HTTP 501 NotImplemented.

Cause 3:
1. CreateCertificateAuthority (ROOT, UsageMode=SHORT_LIVED_CERTIFICATE), wait for Status=PENDING_CERTIFICATE.
2. DeleteCertificateAuthority with PermanentDeletionTimeInDays=7; wait until DescribeCertificateAuthority reports Status=DELETED.
3. Call RevokeCertificate on that CA ARN with any CertificateSerial (e.g. "0a:1b:2c:3d:4e:5f:60:71") and RevocationReason=KEY_COMPROMISE.
   Expected HTTP 400 InvalidStateException "The certificate authority <that ARN> is not in the correct state to have issued certificates." and an unchanged CA Status; observed HTTP 501 NotImplemented.

Replay shortcut: re-run acm-pca.revoke-root-ca-self-signed-certificate-rejected, acm-pca.revoke-certificate-duplicate-revoke-already-processed and acm-pca.revoke-certificate-not-ready-fails.
