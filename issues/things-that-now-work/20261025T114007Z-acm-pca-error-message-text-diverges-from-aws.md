Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.ca-soft-delete-restore-window-and-refused-transitions acm-pca.list-cas-invalid-next-token-is-not-silent-restart
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.ca-soft-delete-restore-window-and-refused-transitions|run#23|AssertionError|assert msg_tag == _DELETED_STATE_MESSAGE, (
Issue-Key: acm-pca.list-cas-invalid-next-token-is-not-silent-restart|run#7|AssertionError|assert code == ("InvalidNextTokenException", _INVALID_TOKEN_MESSAGE), (
Issue-Commit: a50ad8772dd6764223f30d84c82087abb6d16928

# ACM PCA error-message text diverges from AWS in two places: the DELETED-state refusal message and the invalid-NextToken refusal message

> _Note: this issue was identified against the code base at commit `a50ad8772dd6764223f30d84c82087abb6d16928`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagCertificateAuthority

## Description
This issue bundles 2 distinct net-new root causes in the acm-pca emulator. Both are error-message-text divergences: in each case the emulator picks the right HTTP status and the right error code, but emits message prose that is not what AWS emits for that error class. An error message is part of the contract — it is what a human operator reads and what a client that discriminates sub-cases of a single error code matches on — so wording that differs from AWS is a defect even when the code is right.

Neither cause is covered by the four open acm-pca issues. 20261022T143159Z-acm-pca-request-validation-and-ca-creation-contract.md covers ValidationException message SHAPE on CreateCertificateAuthority, CRL config validation and the idempotency token — not the DELETED-state guard and not pagination. 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md covers PutPolicy/DeletePolicy/GetPolicy diagnostics only. 20261022T143159Z-acm-pca-private-ca-lifecycle-is-unusable-issuecertificate.md covers IssueCertificate, the CSR and import. 20261025T011542Z-acm-pca-cloudformation-rejects-every-aws-acmpca-resource-type-as.md covers CloudFormation. The replay report marked neither of the two checks below as owned.

=== Cause 1 of 2: the InvalidStateException message for a mutation refused against a soft-deleted (DELETED) CA is the wrong text ===
Failing check: acm-pca.ca-soft-delete-restore-window-and-refused-transitions

AWS: DeleteCertificateAuthority is a SOFT delete. Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json): DeleteCertificateAuthority.PermanentDeletionTimeInDays is an integer with @range min 7 max 30, the authority stays addressable (DescribeCertificateAuthority models only InvalidArnException and ResourceNotFoundException and still resolves the ARN), and TagCertificateAuthority models InvalidStateException (client error, HTTP 400) exactly for this case; the shape's own doc is only the generic class description "The state of the private CA does not allow this action to occur." The runtime message is not in the model, so for the message itself this is recorded behavior only: the check records, as verified against real AWS in us-west-2, that a mutation attempted against a CA in the DELETED state is refused with code InvalidStateException, HTTP 400, and the message
    "The certificate authority is in the DELETED state and must be restored to complete this action."
That whole string is FIXED text for this error class — it carries no value from the request or from state (no ARN, no id, no timestamp, no day count), so it is reproducible verbatim. Note that the sibling refusal on the same deleted authority has its own distinct fixed text: UpdateCertificateAuthority(Status=...) against a DELETED CA answers InvalidStateException with "The certificate authority must be in the ACTIVE or DISABLED state to be updated", and the emulator already matches that one — so the two refusals are not interchangeable prose and must not be collapsed onto one shared string.

Emulator: the code and status are right (the check's code assertion passed), but TagCertificateAuthority against a DELETED CA answers the message
    "The certificate authority is in the DELETED state, which does not allow this action. Restore it first."
The assertion that fired is: "TagCertificateAuthority on a DELETED CA message: 'The certificate authority is in the DELETED state, which does not allow this action. Restore it first.'" This reads like the generic class doc rephrased with an imperative appended, rather than the specific per-operation refusal text.

Why: an operator debugging a failed tag-on-restore sees text no AWS document or runbook contains, and any client or test that discriminates the several InvalidStateException sub-cases of acm-pca by message — which is the only way to tell "must be restored" apart from "must be ACTIVE or DISABLED", since both are the same code and status — takes the wrong branch or fails to match at all. Because the emulator's variant also drops the phrase "must be restored to complete this action", a substring match on the documented wording finds nothing.

Fix: the state-guard error construction on the DELETED branch of the acm-pca handler — wherever the soft-delete/DELETED-state refusal message is built for the tag path (and any other mutation that shares it). Give the DELETED-state refusal its own exact fixed string instead of deriving one from the generic InvalidStateException description, and keep it distinct from the UpdateCertificateAuthority ACTIVE-or-DISABLED text that is already correct.

Traces: acm-pca.ca-soft-delete-restore-window-and-refused-transitions (assertion at check line 287, after the check successfully created the CA, drove it to PENDING_CERTIFICATE, soft-deleted it with PermanentDeletionTimeInDays=7, confirmed Status=DELETED and a RestorableUntil inside the requested window, and then attempted the refused tag). Everything up to the message comparison matched, so this is a message-only divergence on an otherwise working soft-delete path.

=== Cause 2 of 2: the InvalidNextTokenException message for ListCertificateAuthorities is the exception's doc string, not AWS's runtime message ===
Failing check: acm-pca.list-cas-invalid-next-token-is-not-silent-restart

AWS: Smithy: ListCertificateAuthorities takes NextToken (@length min 1 max 43739) and models exactly one error, InvalidNextTokenException (client error, HTTP 400). A NextToken the service never minted is REFUSED rather than ignored, and the emulator does refuse it with the right code and status (the over-permissiveness half of the check — that the call must not return HTTP 200 with page one — passed). For the message the model is not the oracle: the model's string for InvalidNextTokenException is its API-reference DESCRIPTION of the error class, "The token specified in the NextToken argument is not valid. Use the token returned from your previous call to ListCertificateAuthorities." The recorded runtime message is
    "The NextToken you provided is invalid. Use a token returned by ListCertificateAuthorities."
That string is entirely fixed text for this error class, with no request- or state-derived parts — the rejected token value is deliberately not echoed. Hypothesis: the emulator is emitting the Smithy doc string verbatim as the runtime message, which is why the two differ in exactly the way a doc-to-message shortcut would.

Emulator: ListCertificateAuthorities(MaxResults=1, NextToken=<never-minted token>) returns code InvalidNextTokenException with the message "The token specified in the NextToken argument is not valid. Use the token returned from your previous call to ListCertificateAuthorities." The same wrong text is returned for both rejected token forms the check exercises — a well-formed base64 payload the service never minted, and a string that is not base64 at all — so this is one shared message construction, not two.

Why: a client or operator matching the documented runtime wording ("The NextToken you provided is invalid") never matches, so pagination-token failures are misattributed; and because the divergence is message-only while the code is correct, it is invisible to any check that asserts the code alone and surfaces only in message-sensitive callers.

Fix: the error construction for the NextToken validation path of the acm-pca ListCertificateAuthorities handler. The message must be the recorded runtime text for this error class, not the exception shape's documentation string; if the serializer is defaulting InvalidNextTokenException's message from the model doc, that default is the thing to replace with the operation's own fixed text.

Traces: acm-pca.list-cas-invalid-next-token-is-not-silent-restart (assertion at check line 63 on the base64 bogus token; the identical message is also what the non-base64 token path at line 71 would report, and the leaked-page assertion at line 84 passed, confirming the refusal itself is real).

## Evidence
Cause 1 — acm-pca.ca-soft-delete-restore-window-and-refused-transitions [mismatch]
Operation: TagCertificateAuthority against a CA in the DELETED state.
Expected (AWS, recorded us-west-2): HTTP 400, Code InvalidStateException, Message "The certificate authority is in the DELETED state and must be restored to complete this action." (all fixed text; no request- or state-derived parts).
Actual (emulator): HTTP 400, Code InvalidStateException (correct), Message "The certificate authority is in the DELETED state, which does not allow this action. Restore it first."
Replay assertion: `assert msg_tag == _DELETED_STATE_MESSAGE` at check_acm_pca_ca_soft_delete_restore_window_and_refused_transitions.py:287 ->
  AssertionError: TagCertificateAuthority on a DELETED CA message: 'The certificate authority is in the DELETED state, which does not allow this action. Restore it first.'
Control in the same check that PASSED and bounds the defect to the message: the error code assertion (InvalidStateException) passed; Describe on the deleted CA resolved with the same Arn and CommonName; RestorableUntil was present, a datetime, and inside the requested 7-day window; and the sibling refusal UpdateCertificateAuthority(Status=DISABLED) matched its own distinct AWS text "The certificate authority must be in the ACTIVE or DISABLED state to be updated".
Smithy: TagCertificateAuthority models InvalidStateException (client, httpError 400); the shape's doc is only the class description "The state of the private CA does not allow this action to occur.", so the runtime wording above is recorded behavior, not modeled.

Cause 2 — acm-pca.list-cas-invalid-next-token-is-not-silent-restart [mismatch]
Operation: ListCertificateAuthorities(MaxResults=1, NextToken=<token the service never minted>).
Expected (AWS, recorded): HTTP 400, Code InvalidNextTokenException, Message "The NextToken you provided is invalid. Use a token returned by ListCertificateAuthorities." (all fixed text; the rejected token is not echoed).
Actual (emulator): HTTP 400, Code InvalidNextTokenException (correct), Message "The token specified in the NextToken argument is not valid. Use the token returned from your previous call to ListCertificateAuthorities." — which is byte-for-byte the Smithy doc string of com.amazonaws.acmpca#InvalidNextTokenException, i.e. the error class's API-reference description rather than its runtime message.
Replay assertion: `assert code == ("InvalidNextTokenException", _INVALID_TOKEN_MESSAGE)` at check_acm_pca_list_cas_invalid_next_token_is_not_silent_restart.py:63 ->
  AssertionError: a NextToken acm-pca never minted must be refused with InvalidNextTokenException, got ('InvalidNextTokenException', 'The token specified in the NextToken argument is not valid. Use the token returned from your previous call to ListCertificateAuthorities.')
Controls in the same check that PASSED and bound the defect to the message: ListCertificateAuthorities(MaxResults=1) returned at most one authority, and the decisive over-permissiveness assertion — the invalid-token call must NOT return HTTP 200 with a page — held, so the refusal itself is correct and only its text diverges.
Smithy: ListCertificateAuthorities errors = [InvalidNextTokenException] (client, httpError 400); NextToken @length 1..43739.

## Steps to Reproduce
Cause 1 (TagCertificateAuthority DELETED-state message):
1. CreateCertificateAuthority with CertificateAuthorityType=ROOT, UsageMode=SHORT_LIVED_CERTIFICATE, CertificateAuthorityConfiguration={KeyAlgorithm: RSA_2048, SigningAlgorithm: SHA256WITHRSA, Subject: {CommonName: <any>}}.
2. Poll DescribeCertificateAuthority until Status == PENDING_CERTIFICATE.
3. DeleteCertificateAuthority(CertificateAuthorityArn=<arn>, PermanentDeletionTimeInDays=7); poll DescribeCertificateAuthority until Status == DELETED.
4. TagCertificateAuthority(CertificateAuthorityArn=<arn>, Tags=[{Key: k, Value: v}]).
5. Read Error.Code and Error.Message of the ClientError. Code is InvalidStateException (correct); Message is "The certificate authority is in the DELETED state, which does not allow this action. Restore it first." where AWS returns "The certificate authority is in the DELETED state and must be restored to complete this action."
6. For contrast, UpdateCertificateAuthority(CertificateAuthorityArn=<arn>, Status=DISABLED) on the same deleted CA and observe that its message already matches AWS — confirming only the DELETED-state tag text is wrong.
Or replay the check directly: acm-pca.ca-soft-delete-restore-window-and-refused-transitions.

Cause 2 (ListCertificateAuthorities invalid-NextToken message):
1. ListCertificateAuthorities(MaxResults=1) to confirm the listing works.
2. ListCertificateAuthorities(MaxResults=1, NextToken=base64("sim2real-behav-acm-pca-bogus-cursor")) — a well-formed base64 token the service never minted.
3. Read Error.Code and Error.Message: Code is InvalidNextTokenException (correct); Message is "The token specified in the NextToken argument is not valid. Use the token returned from your previous call to ListCertificateAuthorities." where AWS returns "The NextToken you provided is invalid. Use a token returned by ListCertificateAuthorities."
4. Repeat with NextToken="not-a-token-at-all" (not base64) and observe the same wrong message, showing one shared construction.
Or replay the check directly: acm-pca.list-cas-invalid-next-token-is-not-silent-restart.
