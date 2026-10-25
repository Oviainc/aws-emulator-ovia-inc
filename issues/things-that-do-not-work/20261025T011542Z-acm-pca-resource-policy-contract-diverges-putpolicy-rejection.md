Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.ca-resource-policy-lockout-guard-classified acm-pca.concurrent-delete-policy-race-conflict acm-pca.put-policy-missing-permission-denied
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.ca-resource-policy-lockout-guard-classified|run#19|AssertionError|assert msg_json == f"InvalidPolicy: {malformed_body}", (
Issue-Key: acm-pca.concurrent-delete-policy-race-conflict|run#14|AssertionError|assert errors == [None, None], f"expected both racing DeletePolicy calls to succeed, got {errors}"
Issue-Key: acm-pca.put-policy-missing-permission-denied|_error#19|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# ACM PCA resource-policy contract diverges: PutPolicy rejection diagnostics collapse into one generic message, DeletePolicy is not delete-if-exists, and an explicitly-denied principal is allowed to PutPolicy

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPolicy

## Description
Three distinct net-new root causes in the acm-pca resource-policy surface (PutPolicy / DeletePolicy / GetPolicy). All three are grounded in failing behavioral checks that are NOT marked ALREADY FILED or OWNED. The two open acm-pca issues (20261022T143159Z-acm-pca-private-ca-lifecycle-is-unusable-issuecertificate.md and 20261022T143159Z-acm-pca-request-validation-and-ca-creation-contract.md) do not cover any of these: the lifecycle issue owns failures where the check never got past IssueCertificate/CSR (e.g. acm-pca.delete-policy-idempotent-no-existing-policy aborts with IssueCertificate InvalidStateException and therefore never exercises the DeletePolicy no-policy path), and the request-validation issue owns CreateCertificateAuthority/tagging validation.

Cause 1 of 3: PutPolicy refuses every bad policy document with the same generic message, so the malformed-document rule and the Deny-effect rule are indistinguishable.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model) models PutPolicy with ResourceArn (Arn, required) and Policy (AWSPolicy, required, @length 1..81920) and the errors ConcurrentModificationException, InvalidArnException, InvalidPolicyException, InvalidStateException, LockoutPreventedException, RequestFailedException, ResourceNotFoundException. The model does not state the diagnostic text, so for the text this is recorded behavior only: the check records (REAL-AWS VERIFIED us-west-2) that acm-pca rejects all three bad-document classes with HTTP 400 / InvalidPolicyException but makes the message RULE-SPECIFIC, and for the malformed classes the message is BUILT FROM THE REQUEST — it is the literal prefix "InvalidPolicy: " followed by the rejected document verbatim (both for a body that is not JSON at all and for a JSON body carrying no Statement). For a syntactically valid document containing a Deny statement the message is instead the rule name, "InvalidPolicy: Only the Allow effect is supported", i.e. the Deny document is NOT echoed. The requirement is therefore the derivation rule (echo the un-parseable document for a malformed body; name the effect rule for a Deny statement, and never the same string for both), not any one recorded sentence.
Emulator: every bad document yields one fixed sentence, 'The resource policy is invalid or is missing a required statement.' — it does not echo the submitted document and does not name the effect rule, so the malformed-body, missing-Statement and Deny-effect refusals are byte-identical.
Why: a client (or a human operator) cannot tell a typo in a policy document from a document that can never be accepted at all, and cannot locate WHICH document was rejected when several are submitted in a loop; retry/repair logic that branches on the diagnostic (repair the JSON vs. rewrite the effect) picks the wrong branch, and tooling that echoes the offending document back to the user loses it entirely.
Fix: PutPolicy's policy-document validation in the acm-pca service crate — the stage that classifies the document must carry the failing rule into the InvalidPolicyException message (echo the document for parse/shape failures, emit the effect-rule text for a non-Allow effect) instead of one constant string.
Traces: acm-pca.ca-resource-policy-lockout-guard-classified (assertion at check line 311: expected "InvalidPolicy: <document>", got 'The resource policy is invalid or is missing a required statement.').

Cause 2 of 3: DeletePolicy is not idempotent — deleting a policy that is not attached to an existing CA is refused with ResourceNotFoundException instead of succeeding.
AWS: Smithy models DeletePolicy with ResourceArn (required) and the errors ConcurrentModificationException, InvalidArnException, InvalidStateException, LockoutPreventedException, RequestFailedException, ResourceNotFoundException; the model does not say when each applies, so recorded behavior only: DeletePolicy has delete-if-exists semantics — against an EXISTING certificate authority that currently has no resource policy it returns success (HTTP 200, empty body), and ResourceNotFoundException is reserved for the AUTHORITY itself not existing. Consequently two DeletePolicy calls racing against the same CA both succeed (no ConcurrentModificationException loser, no not-found loser), and GetPolicy afterwards is the call that reports ResourceNotFoundException.
Emulator: with two concurrent DeletePolicy calls on the same CA, one succeeded and the other was refused with ResourceNotFoundException ([None, 'ResourceNotFoundException']). The emulator is looking the POLICY up and returning not-found when it is absent, which also means a plain retry of DeletePolicy on a CA with no policy fails.
Why: idempotent-delete retry is the normal client pattern (and the pattern every teardown path uses). A caller that retries after a timeout, or any two agents cleaning up concurrently, gets a hard 400 ResourceNotFoundException for an operation that succeeded; worse, the code is the same one that means "this CA does not exist", so the caller cannot distinguish an absent policy from an absent authority and may wrongly conclude the CA is gone.
Fix: DeletePolicy's handler/state in the acm-pca service crate — resolve the CA from the ARN (not-found only if the authority is absent), then remove the policy if present and return success unconditionally.
Traces: acm-pca.concurrent-delete-policy-race-conflict (assertion at check line 60: expected both racing DeletePolicy calls to succeed, got [None, 'ResourceNotFoundException']).

Cause 3 of 3: PutPolicy succeeds for a principal with an explicit IAM deny on acm-pca:PutPolicy — the permission-boundary rejection class is not implemented.
AWS: recorded behavior only (authorization is not expressed in the Smithy model): a request signed by an IAM user whose identity-based policy contains {"Effect":"Deny","Action":"acm-pca:PutPolicy","Resource":"*"} is refused with HTTP 400 AccessDeniedException, message built from the request in the standard form "User: <user-arn> is not authorized to perform: acm-pca:PutPolicy on resource: <ca-arn> with an explicit deny in an identity-based policy", and no policy is attached to the authority.
Emulator: the restricted principal's PutPolicy returned success (the check's _error helper reports the call succeeded and returned None), i.e. the request was authorized and the policy was attached despite the explicit deny.
Why: every rejection class the emulator does implement is an input-validation class; a caller testing least-privilege configuration, an explicit-deny guardrail or a permission-boundary regression sees it pass against the emulator and fail in production. An over-permissive answer is worse than a wrong error code — security-relevant test suites are validated green on a backend that authorizes everything.
Fix: request authorization for acm-pca operations (the signing-identity/IAM evaluation path consulted by PutPolicy's handler before it mutates state) must evaluate identity-based policies of the signing principal and return AccessDeniedException with the standard not-authorized message, leaving the authority's policy untouched.
Traces: acm-pca.put-policy-missing-permission-denied (assertion at check line 75/132: expected the call to be refused, but it succeeded and returned None).

## Evidence
Cause 1 — acm-pca.ca-resource-policy-lockout-guard-classified:
  expected (real AWS, us-west-2): code InvalidPolicyException, message 'InvalidPolicy: <the rejected document verbatim>' for a non-JSON body and for a JSON body with no Statement; message 'InvalidPolicy: Only the Allow effect is supported' for a document containing a Deny statement (document NOT echoed).
  actual (emulator): code InvalidPolicyException, message 'The resource policy is invalid or is missing a required statement.' for all three classes — identical strings, so the Deny rule is not a distinct rule.

Cause 2 — acm-pca.concurrent-delete-policy-race-conflict:
  expected: two concurrent DeletePolicy(ResourceArn=<ca>) -> both HTTP 200 success; a following GetPolicy -> 400 ResourceNotFoundException.
  actual: [None, 'ResourceNotFoundException'] — one racer refused with 400 ResourceNotFoundException.

Cause 3 — acm-pca.put-policy-missing-permission-denied:
  expected: 400 AccessDeniedException, message 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: acm-pca:PutPolicy on resource: <ca-arn> with an explicit deny in an identity-based policy'; no policy attached.
  actual: PutPolicy returned success for the denied principal (no error raised).

## Steps to Reproduce
Cause 1: CreateCertificateAuthority (ROOT, RSA_2048/SHA256WITHRSA, UsageMode SHORT_LIVED_CERTIFICATE); on the PENDING_CERTIFICATE CA call PutPolicy three times — (a) Policy='this-is-not-json', (b) Policy='{"Version":"2012-10-17"}' (no Statement), (c) a valid document whose single statement has Effect=Deny for arn:aws:iam::<account>:root on acm-pca:* — and compare the three Error.Message values: each must name its own rule (a and b echo the document after 'InvalidPolicy: ', c says 'InvalidPolicy: Only the Allow effect is supported').

Cause 2: on the same CA, PutPolicy a valid Allow document, then submit two DeletePolicy(ResourceArn=<ca-arn>) calls concurrently (ThreadPoolExecutor, 2 workers); both must return success. Equivalently: DeletePolicy twice in sequence on a CA that has no policy — the second must also succeed, while DeletePolicy on a well-formed ARN naming a non-existent CA must be ResourceNotFoundException.

Cause 3: iam.CreateUser; iam.PutUserPolicy with {"Effect":"Deny","Action":"acm-pca:PutPolicy","Resource":"*"}; iam.CreateAccessKey; build an acm-pca client from those credentials and call PutPolicy(ResourceArn=<ca-arn>, Policy=<valid Allow document>). It must be refused with AccessDeniedException and GetPolicy must still report ResourceNotFoundException.
