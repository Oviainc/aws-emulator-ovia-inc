Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.ca-soft-delete-restore-window-and-refused-transitions
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.ca-soft-delete-restore-window-and-refused-transitions|run#25|AssertionError|assert code_upd == _EXPECTED_DELETED_STATE_REJECTION, (
Issue-Commit: df009dd79b3a670d3c0c53c12bb5e37a115b862c

# ACM PCA UpdateCertificateAuthority is unimplemented: every status/revocation update answers HTTP 501 NotImplemented instead of the modeled Unit success or InvalidStateException

> _Note: this issue was identified against the code base at commit `df009dd79b3a670d3c0c53c12bb5e37a115b862c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCertificateAuthority

## Description
This issue carries 1 net-new root cause in the acm-pca emulator.

Not covered by any existing issue. Of the four open acm-pca issues, 20261022T143159Z-acm-pca-private-ca-lifecycle-is-unusable-issuecertificate.md covers IssueCertificate/GetCertificateAuthorityCsr/import, 20261022T143159Z-acm-pca-request-validation-and-ca-creation-contract.md covers CreateCertificateAuthority validation and the idempotency token, 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md covers PutPolicy/DeletePolicy/GetPolicy, and 20261025T011542Z-acm-pca-cloudformation-rejects-every-aws-acmpca-resource-type-as.md covers CloudFormation; none names UpdateCertificateAuthority. The replay report did NOT mark this check as ALREADY FILED or OWNED, and there is no acm-pca-updatecertificateauthority-stub issue in either the open or the fixed set (unlike the ten sibling operations that do have one). This is also NOT a reopen: the one fixed issue that mentions this operation, 20261025T114007Z-acm-pca-error-message-text-diverges-from-aws.md, scoped its fix to TagCertificateAuthority's DELETED-state message (which now passes) and cited UpdateCertificateAuthority only as a control it believed already correct; the operation is in fact still a pristine scaffold.

=== Cause 1 of 1: UpdateCertificateAuthority is an unimplemented scaffold stub, so no CA status or revocation-configuration update is ever honored or refused ===
Failing check: acm-pca.ca-soft-delete-restore-window-and-refused-transitions

AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models UpdateCertificateAuthority with input CertificateAuthorityArn (target Arn, @required, @length 5..200, pattern ^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*$), optional RevocationConfiguration, and optional Status (enum CertificateAuthorityStatus: ACTIVE | CREATING | DELETED | DISABLED | EXPIRED | FAILED | PENDING_CERTIFICATE); output is smithy.api#Unit, i.e. HTTP 200 with an empty body on success. Its modeled errors are exactly ConcurrentModificationException, InvalidArgsException, InvalidArnException, InvalidPolicyException, InvalidStateException and ResourceNotFoundException, every one errorKind=client with httpError 400. There is no NotImplemented error and no 5xx in the operation's modeled error set, so a 501 is not a response this operation can legally produce. The operation doc states the state precondition directly: "Updates the status or configuration of a private certificate authority (CA). Your private CA must be in the ACTIVE or DISABLED state before you can update it. You can disable a private CA that is in the ACTIVE state or make a CA that is in the DISABLED state active again." For the runtime message the model is only the generic class description of InvalidStateException ("The state of the private CA does not allow this action to occur."), so the exact wording is recorded behavior only: the check records, as verified against real AWS in us-west-2, that UpdateCertificateAuthority(Status=DISABLED) against a CA whose status is outside {ACTIVE, DISABLED} — here a soft-deleted CA in the DELETED state — is refused with HTTP 400, Error.Code InvalidStateException and Error.Message "The certificate authority must be in the ACTIVE or DISABLED state to be updated". That whole string is FIXED text for this error class: it embeds no ARN, id, status name, timestamp or other value taken from the request or from state, so it is reproducible verbatim. It is also deliberately distinct from the sibling DELETED-state text TagCertificateAuthority returns ("The certificate authority is in the DELETED state and must be restored to complete this action."), so the two refusals must not be collapsed onto one shared string. Conversely, when the CA IS in ACTIVE or DISABLED the call must succeed with HTTP 200 and an empty body and must actually move the persisted status (ACTIVE -> DISABLED is the documented disable path and the prerequisite for DeleteCertificateAuthority).

Emulator: the handler is the generated scaffold, untouched. services/service-com-amazonaws-acmpca-acmprivateca/src/ops/update_certificate_authority.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker at line 4 and its `handle` body is a single `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateCertificateAuthority"))` at line 202, with the input bound only to silence the unused warning (line 192). Every UpdateCertificateAuthority request therefore answers HTTP 501 with Error.Code `NotImplemented`, regardless of the CA's status, the requested Status value or the RevocationConfiguration supplied. The replayed assertion is: `AssertionError: updating the status of a DELETED CA must be refused with InvalidStateException, got 'NotImplemented'`. Both halves of the contract are missing — the refusal (wrong code, wrong status, wrong message, and an unmodeled 5xx class) and the success path (no status transition is ever persisted, so ACTIVE -> DISABLED cannot be performed at all).

Why: UpdateCertificateAuthority is the only way a client changes a private CA's status, so this breaks the whole CA lifecycle rather than one error branch. A caller cannot disable an ACTIVE CA, which is the documented precondition for DeleteCertificateAuthority — so an ACTIVE CA becomes undeletable through the API and any teardown path that disables-then-deletes hangs or leaks the authority. A caller cannot re-enable a DISABLED CA. A caller cannot update RevocationConfiguration (CRL/OCSP) on an existing CA. On the negative side, a 501 is retried by SDK/client error classification as a transient server fault rather than branched on as the terminal 400 client error AWS returns, so retry loops spin on a condition that will never clear, and error handling written against InvalidStateException never matches because the code it sees is NotImplemented and the status is 501 rather than 400. The operation is also invisible as a stub today because the surrounding checks' teardown helpers swallow the exception from it.

Fix: implement the operation in its handler, services/service-com-amazonaws-acmpca-acmprivateca/src/ops/update_certificate_authority.rs — the only file to touch for it — reading and writing the CA record through crate::state::service_state, and delete the SCAFFOLDED marker line. It needs: ARN shape validation before resource lookup (InvalidArnException vs ResourceNotFoundException as distinct stages), the state gate admitting only CAs in ACTIVE or DISABLED and otherwise returning InvalidStateException with the fixed text above, persistence of the requested Status transition and of RevocationConfiguration, and the modeled Unit/HTTP 200 empty-body success response.

Traces: acm-pca.ca-soft-delete-restore-window-and-refused-transitions (assertion at check line 293). Everything the check does before that point passed — it created a ROOT/RSA_2048 CA, polled it to PENDING_CERTIFICATE, confirmed a live CA carries no RestorableUntil, tagged it, soft-deleted it with PermanentDeletionTimeInDays=7, confirmed Status=DELETED with the same Arn and CommonName and a RestorableUntil datetime inside the requested 7-day window, and saw TagCertificateAuthority correctly refused with InvalidStateException and AWS's exact message — so the divergence is bounded to UpdateCertificateAuthority itself.

## Evidence
Cause 1 — acm-pca.ca-soft-delete-restore-window-and-refused-transitions [mismatch]
Operation: UpdateCertificateAuthority(CertificateAuthorityArn=<arn>, Status="DISABLED") against a CA in the DELETED state.

Expected (AWS, recorded us-west-2 2026-09-07): HTTP 400, Error.Code InvalidStateException, Error.Message "The certificate authority must be in the ACTIVE or DISABLED state to be updated" — entirely fixed text for this error class, with no request- or state-derived parts (no ARN, status name, id or timestamp is interpolated).

Actual (emulator): HTTP 501, Error.Code "NotImplemented" — an error class the operation does not model at all. No InvalidStateException is reachable, and no success response is reachable either.

Replay assertion: `assert code_upd == _EXPECTED_DELETED_STATE_REJECTION` at check_acm_pca_ca_soft_delete_restore_window_and_refused_transitions.py:293 ->
  AssertionError: updating the status of a DELETED CA must be refused with InvalidStateException, got 'NotImplemented'

Source grounding (the stub itself): services/service-com-amazonaws-acmpca-acmprivateca/src/ops/update_certificate_authority.rs line 4 — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and asserts NOTHING about `UpdateCertificateAuthority`'s real behaviour."; line 192 — `let _ = (&input, ctx, &self.engine);`; line 202 — `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateCertificateAuthority"))`. The handler never reads or writes CA state, so neither the refusal nor the status transition exists.

Controls in the same check that PASSED and bound the defect to this one operation:
- CreateCertificateAuthority (ROOT, RSA_2048/SHA256WITHRSA, UsageMode=SHORT_LIVED_CERTIFICATE) succeeded and DescribeCertificateAuthority polled to Status=PENDING_CERTIFICATE.
- A live CA correctly carried no RestorableUntil member.
- TagCertificateAuthority on the live CA succeeded and ListTags reflected the tag.
- DeleteCertificateAuthority(PermanentDeletionTimeInDays=7) soft-deleted it; Describe still resolved the ARN with the same Arn and Subject.CommonName and Status=DELETED, and RestorableUntil was present, deserialized to a datetime, and fell inside the requested 7-day window.
- TagCertificateAuthority against the DELETED CA was correctly refused with HTTP 400 / InvalidStateException and AWS's exact message "The certificate authority is in the DELETED state and must be restored to complete this action." — i.e. the DELETED-state gate exists and is correct on the tag path, and only UpdateCertificateAuthority lacks it.

Smithy grounding (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json): UpdateCertificateAuthority input = {CertificateAuthorityArn: Arn @required @length 5..200 + ARN pattern, RevocationConfiguration, Status: CertificateAuthorityStatus enum}; output_shape = smithy.api#Unit with SUCCESS_STATUS 200; errors = [ConcurrentModificationException, InvalidArgsException, InvalidArnException, InvalidPolicyException, InvalidStateException, ResourceNotFoundException], all errorKind=client / httpError 400. No 5xx and no NotImplemented is modeled. Operation doc: "Your private CA must be in the ACTIVE or DISABLED state before you can update it." InvalidStateException's own doc is only the class description "The state of the private CA does not allow this action to occur.", so the runtime message quoted above is recorded behavior, not modeled text.

## Steps to Reproduce
Refusal path (what the check replays):
1. CreateCertificateAuthority with CertificateAuthorityType=ROOT, UsageMode=SHORT_LIVED_CERTIFICATE, CertificateAuthorityConfiguration={KeyAlgorithm: RSA_2048, SigningAlgorithm: SHA256WITHRSA, Subject: {CommonName: <any>}}.
2. Poll DescribeCertificateAuthority until Status == PENDING_CERTIFICATE.
3. DeleteCertificateAuthority(CertificateAuthorityArn=<arn>, PermanentDeletionTimeInDays=7); poll DescribeCertificateAuthority until Status == DELETED.
4. UpdateCertificateAuthority(CertificateAuthorityArn=<arn>, Status="DISABLED").
5. Observe HTTP 501 with Error.Code "NotImplemented". AWS answers HTTP 400, Error.Code InvalidStateException, Error.Message "The certificate authority must be in the ACTIVE or DISABLED state to be updated".
6. For contrast, TagCertificateAuthority on the same DELETED CA and observe that it IS correctly refused with 400 / InvalidStateException and its own distinct AWS message — the state gate is simply absent from the update path.

Success path (also unreachable, same stub):
1. Create a CA and drive it to ACTIVE (or use any CA whose Status is ACTIVE or DISABLED).
2. UpdateCertificateAuthority(CertificateAuthorityArn=<arn>, Status="DISABLED").
3. Observe HTTP 501 NotImplemented. AWS answers HTTP 200 with an empty body and the CA's persisted Status becomes DISABLED, which is the documented precondition for DeleteCertificateAuthority on an ACTIVE CA. Re-reading DescribeCertificateAuthority shows the status unchanged on the emulator.
4. Likewise UpdateCertificateAuthority(CertificateAuthorityArn=<arn>, RevocationConfiguration={CrlConfiguration:{Enabled:false}}) answers 501 instead of 200, so revocation configuration can never be changed after creation.

Or replay the check directly: acm-pca.ca-soft-delete-restore-window-and-refused-transitions. After fixing, re-replay that check and confirm it advances past line 293 (the update refusal), through the idempotent repeat-delete and RestoreCertificateAuthority assertions.
