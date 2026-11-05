Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.delete-policy-idempotent-no-existing-policy acm-pca.delete-policy-on-disabled-ca-is-noop acm-pca.concurrent-delete-policy-conflict acm-pca.delete-policy-after-ca-deleted-invalid-state acm-pca.delete-policy-no-policy-attached-not-found acm-pca.delete-policy-on-pending-ca-is-noop acm-pca.delete-policy-wrong-ca-state-fails acm-pca.put-policy-disallowed-action-rejected acm-pca.put-policy-duplicate-conflicting-principal-blocked acm-pca.put-policy-invalid-principal acm-pca.put-policy-invalid-principal-account-rejected
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|DeletePolicy|ResourceNotFoundException
Issue-Key: acm-pca.concurrent-delete-policy-conflict|run#15|AssertionError|assert len(oks) == 2, f"expected both racing DeletePolicy calls to succeed, got oks={oks} errors={errors}"
Issue-Key: acm-pca.put-policy-disallowed-action-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: acm-pca.put-policy-duplicate-conflicting-principal-blocked|_error#6|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded with {result!r}")
Issue-Key: acm-pca.put-policy-invalid-principal|_err#5|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded with {result!r}")
Issue-Key: acm-pca.put-policy-invalid-principal-account-rejected|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# ACM PCA resource-policy surface: DeletePolicy is not delete-if-exists (two distinct 404 paths) and PutPolicy accepts a disallowed action and a nonexistent principal

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
FOUR net-new root causes in the acm-pca resource-policy surface (DeletePolicy / PutPolicy). None of the eleven failing checks below is marked ALREADY FILED or OWNED in this replay. Related open issues exist and a fixer should read them, but neither carries these checks' replay keys: 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md (PutPolicy generic diagnostic, DeletePolicy race, IAM explicit deny) and 20261029T190010Z-acm-pca-resource-policy-wire-contract-diverges-a.md (Deny effect accepted, GetPolicy re-serialization, truncated InvalidPolicy tail, DeletePolicy delete-if-exists over the wire). Causes 1 and 2 here are two DIFFERENT 404 paths inside DeletePolicy, and causes 3 and 4 are two over-permissive PutPolicy acceptance paths that no open issue describes.

Cause 1 of 4: DeletePolicy on an existing CA that holds no resource policy is refused with ResourceNotFoundException instead of succeeding.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models DeletePolicy with ResourceArn (Arn, @required, @length 5..200) and output Unit, errors ConcurrentModificationException, InvalidArnException, InvalidStateException, LockoutPreventedException, RequestFailedException, ResourceNotFoundException (all 400). The modeled documentation states the rule explicitly: "If there is no policy attached to the private CA, this action will return successful." So the requirement is delete-if-exists: HTTP 200 with an empty body whenever the AUTHORITY named by ResourceArn exists, with ResourceNotFoundException reserved for the authority itself being absent. The checks additionally record (REAL-AWS verified) that repeating the call, and racing two of them, both succeed.
Emulator: answers HTTP 400 {"__type":"ResourceNotFoundException","message":"Could not find policy for specified resource."} whenever the CA has no attached policy. The whole sentence is fixed text the emulator reuses from GetPolicy; nothing in it comes from the request. In the concurrency check one racer returned ok and the other that code. The handler's own module comment (services/service-com-amazonaws-acmpca-acmprivateca/src/ops/delete_policy.rs, lines 5-16) states this 404 is intentional and cites acm-pca.delete-policy-on-pending-ca-is-noop as the evidence — but that check asserts the OPPOSITE (it calls delete_policy on a policy-less CA and requires no exception), so the handler is grounded in a misreading of it.
Why: idempotent delete is the retry and teardown pattern; a client retrying after a timeout, or two cleanup paths running concurrently, gets a hard 400 for an operation that already succeeded. Worse, ResourceNotFoundException is the same class that means "this CA does not exist", so the caller can conclude the authority is gone and skip the rest of its cleanup.
Fix: DeletePolicy's handler and policy state — resolve the authority from ResourceArn, then remove any attached policy and return success unconditionally; raise ResourceNotFoundException only when no such authority exists.
Traces: acm-pca.delete-policy-idempotent-no-existing-policy, acm-pca.delete-policy-on-disabled-ca-is-noop, acm-pca.delete-policy-after-ca-deleted-invalid-state, acm-pca.concurrent-delete-policy-conflict; handler services/service-com-amazonaws-acmpca-acmprivateca/src/ops/delete_policy.rs.

Cause 2 of 4: DeletePolicy reports a CA that EXISTS as a missing CA when that CA is not in an activated state (PENDING_CERTIFICATE, or soft-deleted DELETED inside its restoration window).
AWS: same model entry as cause 1. Recorded behavior: policy operations are not gated on CA lifecycle state as long as the CA resource exists — DeletePolicy against a PENDING_CERTIFICATE CA and against a soft-deleted (DELETED) CA both return HTTP 200 with an empty body, and the ResourceNotFoundException message "Could not find specified CA" is reserved for a well-formed ARN that names no CA at all (the checks assert exactly that contrast, with the bogus-ARN half of the same check expecting that code and message).
Emulator: answers HTTP 400 {"__type":"ResourceNotFoundException","message":"Could not find specified CA"} for a CA that was created moments earlier and is readable through DescribeCertificateAuthority. The message is fixed text and asserts the authority is absent while it exists; this is a different code path from cause 1 (different message, state-dependent) and must be fixed with it.
Why: the response claims the CA does not exist. A teardown or a RAM-sharing client reading that will stop treating the authority as present, and the distinction the API relies on — absent policy vs absent authority — is destroyed for every CA that has not been activated.
Fix: the CA-resolution step DeletePolicy (and the policy state it reads) uses — resolve the authority by ARN across every lifecycle state including PENDING_CERTIFICATE, CREATING and the soft-deleted DELETED window, rather than only activated CAs.
Traces: acm-pca.delete-policy-no-policy-attached-not-found, acm-pca.delete-policy-on-pending-ca-is-noop, acm-pca.delete-policy-wrong-ca-state-fails; handler services/service-com-amazonaws-acmpca-acmprivateca/src/ops/delete_policy.rs.

Cause 3 of 4: PutPolicy accepts a policy document granting an acm-pca action that is not allowed for a certificate-authority resource policy.
AWS: Smithy models PutPolicy with ResourceArn (@required) and Policy (AWSPolicy, @required, @length 1..81920), output Unit, and InvalidPolicyException among its errors ("The resource policy is invalid or is missing a required statement"). Which documents are refused is recorded behavior only: a statement granting a non-shareable management-plane action (the checks use acm-pca:DeleteCertificateAuthority) is refused with HTTP 400 InvalidPolicyException and the message "InvalidPolicy: Input policy containing action that is not allowed for resource type acm-pca:CertificateAuthority" — that entire sentence is fixed text, including the resource-type token; nothing is taken from the request. No policy is attached as a result.
Emulator: answers HTTP 200 with an empty body (content-length 2) and attaches the document.
Why: an over-permissive answer on a security surface. A caller validating that only the shareable read/issue actions can be granted on a private CA passes against the emulator and fails in production, and any later GetPolicy serves a resource policy real ACM PCA could never hold, so access-evaluation logic is exercised against an impossible configuration.
Fix: PutPolicy's policy-document validation — classify each statement's Action against the allow-list for resource type acm-pca:CertificateAuthority before mutating state, and refuse with InvalidPolicyException carrying AWS's fixed sentence, leaving the CA's policy untouched.
Traces: acm-pca.put-policy-disallowed-action-rejected (check line 44/102), acm-pca.put-policy-duplicate-conflicting-principal-blocked (check line 42/135); handler services/service-com-amazonaws-acmpca-acmprivateca/src/ops/put_policy.rs.

Cause 4 of 4: PutPolicy accepts a policy whose Principal names an AWS account that does not exist.
AWS: same model entry; the rule is recorded behavior only (REAL-AWS verified in the checks): ACM PCA resolves the principal and refuses a document naming a nonexistent account with HTTP 400 InvalidPolicyException and the message "InvalidPolicy: Principal not found." — the whole sentence is fixed text, the offending account id is NOT echoed. Nothing is attached, and the refusal is independent of CA state (the check performs it on a DISABLED CA and then accepts a valid Organizations-sharing document on the same CA).
Emulator: answers HTTP 200 with an empty body and attaches the policy.
Why: cross-account sharing configured against a typo'd or decommissioned account id silently "works" on the emulator; a client has no way to exercise the principal-validation branch, and the stored policy grants an identity that cannot exist.
Fix: PutPolicy's policy-document validation — resolve each statement's Principal (account ids and principal ARNs) against the accounts the emulator knows and refuse an unresolvable one with InvalidPolicyException "InvalidPolicy: Principal not found." before any state mutation.
Traces: acm-pca.put-policy-invalid-principal (check line 131), acm-pca.put-policy-invalid-principal-account-rejected (check line 39); handler services/service-com-amazonaws-acmpca-acmprivateca/src/ops/put_policy.rs.

## Evidence
Cause 1 — acm-pca.delete-policy-idempotent-no-existing-policy / acm-pca.delete-policy-on-disabled-ca-is-noop / acm-pca.delete-policy-after-ca-deleted-invalid-state:
  expected: DeletePolicy(ResourceArn=<existing CA with no policy>) -> HTTP 200, empty body.
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeletePolicy operation: Could not find policy for specified resource.
  acm-pca.concurrent-delete-policy-conflict: expected both racing DeletePolicy calls to succeed; actual oks=[('ok', None)] errors=[('error', 'ResourceNotFoundException')].

Cause 2 — acm-pca.delete-policy-no-policy-attached-not-found / acm-pca.delete-policy-on-pending-ca-is-noop / acm-pca.delete-policy-wrong-ca-state-fails:
  expected: DeletePolicy against a CA that exists but is PENDING_CERTIFICATE, or soft-deleted (DELETED within its restoration window) -> HTTP 200, empty body; "Could not find specified CA" only for a well-formed ARN naming no CA (that half of the same checks passes).
  actual:   botocore.errorfactory.ResourceNotFoundException: ... when calling the DeletePolicy operation: Could not find specified CA — for a CA that DescribeCertificateAuthority reports in that same check.

Cause 3 — acm-pca.put-policy-disallowed-action-rejected:
  expected: HTTP 400 ('InvalidPolicyException', 'InvalidPolicy: Input policy containing action that is not allowed for resource type acm-pca:CertificateAuthority'); GetPolicy afterwards still ('ResourceNotFoundException', 'Could not find policy for specified resource.').
  actual:   HTTP 200, body {} — AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2'}}. Same in acm-pca.put-policy-duplicate-conflicting-principal-blocked (succeeded with None).

Cause 4 — acm-pca.put-policy-invalid-principal / acm-pca.put-policy-invalid-principal-account-rejected (Principal {"AWS": "999999999999"}):
  expected: HTTP 400 code InvalidPolicyException, message 'InvalidPolicy: Principal not found.'; no policy attached.
  actual:   HTTP 200, empty body — the policy was attached.

Status/code are right only where noted; in causes 3 and 4 the emulator refuses nothing at all.

## Steps to Reproduce
Setup: acmpca.create_certificate_authority(CertificateAuthorityConfiguration={'KeyAlgorithm':'RSA_2048','SigningAlgorithm':'SHA256WITHRSA','Subject':{'CommonName':'sim2real-pca'}}, CertificateAuthorityType='ROOT'); poll DescribeCertificateAuthority until PENDING_CERTIFICATE; where an ACTIVE CA is needed, run the documented GetCertificateAuthorityCsr -> IssueCertificate(TemplateArn=arn:aws:acm-pca:::template/RootCACertificate/V1) -> GetCertificate -> ImportCertificateAuthorityCertificate sequence.

Cause 1: on a CA that has no policy attached, call DeletePolicy(ResourceArn=<ca-arn>) — it must return HTTP 200 with an empty body; call it a second time, also 200. Then PutPolicy a valid Allow document and fire two DeletePolicy calls concurrently (2 workers): both must succeed and a following GetPolicy must be the call that reports ResourceNotFoundException 'Could not find policy for specified resource.'.

Cause 2: on a freshly created, still PENDING_CERTIFICATE CA call DeletePolicy(ResourceArn=<ca-arn>) -> must be 200. Then DeleteCertificateAuthority(PermanentDeletionTimeInDays=7), poll to DELETED, and call DeletePolicy again -> must still be 200. For contrast, DeletePolicy with <ca-arn-prefix>/00000000-0000-0000-0000-000000000000 must be the one answering ResourceNotFoundException 'Could not find specified CA'.

Cause 3: on an ACTIVE CA, PutPolicy(ResourceArn=<ca-arn>, Policy=<Allow document, Principal {'AWS': <own account id>}, Action ['acm-pca:DeleteCertificateAuthority'], Resource <ca-arn>>) -> must be HTTP 400 InvalidPolicyException with the message 'InvalidPolicy: Input policy containing action that is not allowed for resource type acm-pca:CertificateAuthority', and GetPolicy must still report no policy. A document granting only allow-listed actions (e.g. acm-pca:DescribeCertificateAuthority / acm-pca:IssueCertificate / acm-pca:GetCertificate) must be accepted.

Cause 4: PutPolicy on the same CA with Principal {'AWS': '999999999999'} (a nonexistent account) and an allow-listed action -> must be HTTP 400 InvalidPolicyException 'InvalidPolicy: Principal not found.' with nothing attached, while the same document naming the caller's own account is accepted.
