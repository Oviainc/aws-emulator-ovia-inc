Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.create-ca-subject-field-contract-limits-rejected acm-pca.create-ca-tag-contract-limits-rejected acm-pca.create-certificate-authority-invalid-key-algorithm-rejected acm-pca.tag-certificate-authority-invalid-tag-key-rejected acm-pca.list-permissions-invalid-arn-rejected acm-pca.create-ca-crl-configuration-requires-valid-bucket acm-pca.create-certificate-authority-idempotency-token-mismatch acm-pca.deploy-certificate-authority-cfn-issues-certificate acm-pca.deploy-cloudformation-ca-issues-certificate acm-pca.deploy-certificate-authority-cfn-then-issue-certificate acm-pca.ca-arn-validation-precedes-resource-lookup
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.create-ca-subject-field-contract-limits-rejected|run#6|AssertionError|assert msg_a == (
Issue-Key: acm-pca.create-ca-tag-contract-limits-rejected|run#7|AssertionError|assert msg_key == (
Issue-Key: acm-pca.create-certificate-authority-invalid-key-algorithm-rejected|run#7|AssertionError|assert message == (
Issue-Key: acm-pca.tag-certificate-authority-invalid-tag-key-rejected|run#7|AssertionError|assert message == (
Issue-Key: acm-pca.list-permissions-invalid-arn-rejected|run#5|AssertionError|assert refusal == (
Issue-Key: acm-pca.create-ca-crl-configuration-requires-valid-bucket|_err#5|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and created {arn!r}")
Issue-Key: acm-pca.create-certificate-authority-idempotency-token-mismatch|run#13|AssertionError|assert arn_b != arn, repr((arn, arn_b))
Issue-Key: acm-pca|DescribeCertificateAuthority|ValidationException
Issue-Key: acm-pca.deploy-certificate-authority-cfn-then-issue-certificate|<genexpr>#11|KeyError|ca_arn = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "CertificateAuthorityArn")
Issue-Key: acm-pca.ca-arn-validation-precedes-resource-lookup|run#6|AssertionError|assert gone_msg == "Could not find specified CA", (
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# ACM PCA request-validation and CA-creation contract diverges: ValidationException message shape, CRL config accepted unvalidated, idempotency-token dedupe, not-found message, and CFN CertificateAuthority Ref/Outputs (6 causes)

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateAuthority

## Description
Six further root causes on acm-pca, independent of the certificate-path issue filed alongside this one.

=== Cause 1 of 6: ValidationException messages are not AWS-shaped ===
AWS: acm-pca refuses constraint violations with the generic, unmodeled ValidationException (400) whose message is built from the request in a fixed form: "<N> validation error[s] detected: Value at '<member path>' failed to satisfy constraint: <constraint>" — the offending VALUE is not echoed; the regex in a pattern constraint is quoted UNANCHORED (e.g. [A-Za-z]{2}, arn:[\w+=/,.@-]+:acm-pca:... , ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)); when one member violates two constraints they are joined with '; ' in pattern-then-length order; and an ENUM violation carries no "<N> validation error detected:" prefix at all ("Value at 'certificateAuthorityConfiguration.keyAlgorithm' failed to satisfy constraint: Member must satisfy enum value set: [...]"). The member paths are the wire (lowerCamel) paths with 1-based list indices, e.g. tags.1.member.key, certificateAuthorityConfiguration.subject.customAttributes.1.member.objectIdentifier. Recorded behavior (REAL-AWS verified in the checks); the pinned model carries the constraints themselves (ASN1Subject country = CountryCodeString @length 2..2 @pattern ^[A-Za-z]{2}$, commonName @length ..64, generationQualifier @length ..3, Tag.key @length ..128 @pattern, Arn @pattern) but not the message form.
Emulator: same code (ValidationException, 400) and same constraint detection, but the message echoes the value ("Value 'USA' at '...'"), quotes the pattern ANCHORED with ^...$, orders a double violation length-then-pattern, and prefixes enum violations with "1 validation error detected:".
Why: the message is the only place AWS reports WHICH member and WHICH bound failed, and clients, CLIs and test suites parse it. A caller matching AWS's documented form mis-parses every acm-pca constraint refusal; echoing the submitted value additionally leaks the rejected input back into logs for members that may be sensitive.
Fix: the service's shared request-validation/serialization layer for acm-pca (the validation stage feeding ValidationException, not any one operation handler) — drop the value echo, strip the regex anchors when quoting a pattern, order multi-violation messages pattern-then-length, and omit the count prefix for enum violations. NOTE for the fixer: the enum message also lists SM2, which IS present in the pinned model's KeyAlgorithm enum (RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87, SM2) while the recorded AWS list omits it; that list difference is model-vs-region and is NOT claimed as a defect here — only the prefix/value-echo/anchor/ordering rules are.
Traces: acm-pca.create-ca-subject-field-contract-limits-rejected, acm-pca.create-ca-tag-contract-limits-rejected, acm-pca.create-certificate-authority-invalid-key-algorithm-rejected, acm-pca.tag-certificate-authority-invalid-tag-key-rejected, acm-pca.list-permissions-invalid-arn-rejected.

=== Cause 2 of 6: CreateCertificateAuthority accepts a RevocationConfiguration naming a nonexistent S3 bucket ===
AWS: a defective RevocationConfiguration is refused outright with 400 ValidationException and no CA is created — including the semantic S3 lookup failure (recorded message "The S3 bucket you provided does not exist.") and the range violation ("1 validation error detected: Value at 'revocationConfiguration.crlConfiguration.expirationInDays' failed to satisfy constraint: Member must have value less than or equal to 5000"). Recorded behavior, REAL-AWS verified us-west-2; the Smithy model lists only InvalidArgsException, InvalidPolicyException, InvalidTagException, LimitExceededException for CreateCertificateAuthority and no revocation-specific error, so AWS uses the unmodeled generic ValidationException here.
Emulator: the call SUCCEEDS and mints a CA (200 + CertificateAuthorityArn) with CrlConfiguration{Enabled:true, S3BucketName:<nonexistent>, ExpirationInDays:7}.
Why: over-permissive. A client that mis-configures CRL publication gets a CA that looks healthy and silently never publishes a CRL, instead of the create-time refusal AWS gives; the misconfiguration is only discovered when revocation checking fails in production.
Fix: the CreateCertificateAuthority handler's RevocationConfiguration validation (services/service-com-amazonaws-acmpca-acmprivateca/src/ops/create_certificate_authority.rs) must validate the CRL block — bucket existence/permission and the modeled ranges — and refuse with ValidationException before any CA is registered.
Traces: acm-pca.create-ca-crl-configuration-requires-valid-bucket.

=== Cause 3 of 6: CreateCertificateAuthority dedupes on IdempotencyToken even when the configuration differs ===
AWS: reusing an IdempotencyToken with a MATERIALLY DIFFERENT CertificateAuthorityConfiguration does not raise IdempotencyParameterMismatchException and does not replace or mutate the first CA — it creates a second, wholly independent CA with its own distinct ARN, each keeping its own configuration. (Reusing the token with the IDENTICAL configuration IS idempotent and returns the original ARN.) Recorded behavior only; Smithy marks CreateCertificateAuthority @idempotent with IdempotencyToken @length 1..36.
Emulator: the second call, with a different KeyAlgorithm (RSA_4096 vs RSA_2048) and a different Subject.CommonName, returns the FIRST CA's ARN — the token alone is the dedupe key, so only one CA exists.
Why: a client that reuses a token across different CA configurations (a common retry/templating bug, and the reason this behavior was pinned) believes it created two CAs and holds two handles that are the same resource. Describe on the second handle returns the first CA's configuration, and work targeted at CA B silently lands on CA A.
Fix: the CreateCertificateAuthority handler's idempotency key must include the request's CertificateAuthorityConfiguration (replay only on an identical configuration, otherwise create a new CA).
Traces: acm-pca.create-certificate-authority-idempotency-token-mismatch.

=== Cause 4 of 6: a CloudFormation-created AWS::ACMPCA::CertificateAuthority does not resolve to the CA ARN ===
AWS: Ref on an AWS::ACMPCA::CertificateAuthority resource returns the certificate-authority ARN (arn:aws:acm-pca:<region>:<account>:certificate-authority/<uuid>), which is what a stack Output exporting the CA carries and what DescribeCertificateAuthority accepts (Smithy: CertificateAuthorityArn is Arn @length 5..200 @pattern ^arn:...:acm-pca:...$). Recorded behavior only for the CFN resource provider.
Emulator: the stack Output value is a generated logical/physical NAME (e.g. of the form "<stack-name>-Ca-<suffix>"), so the very next call fails: 400 ValidationException "1 validation error detected: Value '<that name>' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*$".
Why: no infrastructure-as-code path can hand a CA to anything else. Every template that Refs/exports a private CA — to issue a certificate, to attach a policy, to wire it into ACM — produces an unusable identifier, and the breakage surfaces as a validation error on an unrelated operation.
Fix: the CloudFormation resource provider for AWS::ACMPCA::CertificateAuthority must set the resource's physical id / Ref value to the created CA's ARN (and GetAtt Arn accordingly).
Traces: acm-pca.deploy-certificate-authority-cfn-issues-certificate, acm-pca.deploy-cloudformation-ca-issues-certificate.

=== Cause 5 of 6: stack Output entries for the CA stack omit OutputValue ===
AWS: DescribeStacks returns each Outputs entry as {OutputKey, OutputValue, (Description, ExportName)} — OutputValue is always present for a stack in CREATE_COMPLETE. Recorded behavior only.
Emulator: after the ACMPCA stack reaches CREATE_COMPLETE, an Outputs entry with the expected OutputKey ('CertificateAuthorityArn') has NO OutputValue member at all — reading it raises KeyError 'OutputValue'.
Why: any client reading a stack output crashes or sees an absent value rather than the resource it exported; there is no way to discover the deployed CA from the stack.
Fix: the CloudFormation DescribeStacks output serializer / stack-output resolution must emit OutputValue for every resolved output (and the resolution of a Ref to the ACMPCA resource must produce a value at all — see cause 4).
Traces: acm-pca.deploy-certificate-authority-cfn-then-issue-certificate.

=== Cause 6 of 6: ResourceNotFoundException for an unknown CA ARN does not carry AWS's message ===
AWS: a syntactically valid CertificateAuthorityArn that identifies no CA is refused 400 ResourceNotFoundException with the fixed message "Could not find specified CA" (and the analogous fixed "Could not find policy for specified resource." / "Could not find specified Audit Report." for the policy and audit-report lookups). Recorded behavior only — the message is a service constant, not request-derived; the model only documents the shape ("A resource such as a private CA, S3 bucket, certificate, audit report, or policy cannot be found."). The status and code themselves already match.
Emulator: returns ResourceNotFoundException with a constructed, per-request sentence: 'the certificate authority <arn> was not found'.
Why: lowest severity of this bundle — the code and status are right, so SDK error handling works. It matters only for callers and golden tests that match AWS's constant not-found text, and the same constructed form is likely used for the other acm-pca not-found constants.
Fix: the not-found error construction shared by the acm-pca CA/policy/audit-report lookups should emit AWS's constant messages rather than interpolating the ARN.
Traces: acm-pca.ca-arn-validation-precedes-resource-lookup (the ARN-before-lookup ordering this check also grades is already correct; only the missing-CA message diverges).

## Evidence
Cause 1: acm-pca.create-ca-subject-field-contract-limits-rejected — got "2 validation errors detected: Value 'USA' at 'certificateAuthorityConfiguration.subject.country' failed to satisfy constraint: Member must have length less than or equal to 2; Value 'USA' at '...subject.country' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z]{2}$"; expected "2 validation errors detected: Value at 'certificateAuthorityConfiguration.subject.country' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z]{2}; Value at '...subject.country' failed to satisfy constraint: Member must have length less than or equal to 2". acm-pca.create-ca-tag-contract-limits-rejected — got "1 validation error detected: Value 'kkk…' at 'tags.1.member.key' …"; expected "1 validation error detected: Value at 'tags.1.member.key' …". acm-pca.create-certificate-authority-invalid-key-algorithm-rejected — got "1 validation error detected: Value 'AES_256' at 'certificateAuthorityConfiguration.keyAlgorithm' failed to satisfy constraint: Member must satisfy enum value set: [RSA_2048, …, SM2]"; expected no count prefix and no value echo. acm-pca.tag-certificate-authority-invalid-tag-key-rejected — got "… Value 'bad#tag!' at 'tags.1.member.key' … pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$"; expected "… Value at 'tags.1.member.key' … pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)". acm-pca.list-permissions-invalid-arn-rejected — got ('ValidationException', "1 validation error detected: Value 'not-an-acm-pca-arn' at 'certificateAuthorityArn' … pattern: ^arn:…$"); expected the same without the value and without ^…$.

Cause 2: acm-pca.create-ca-crl-configuration-requires-valid-bucket — AssertionError: real AWS rejects this call, but it SUCCEEDED and created 'arn:aws:acm-pca:us-west-2:<acct>:certificate-authority/<uuid>' (CreateCertificateAuthority with CrlConfiguration{Enabled:true, S3BucketName:<absent bucket>, ExpirationInDays:7}); expected 400 ValidationException.

Cause 3: acm-pca.create-certificate-authority-idempotency-token-mismatch — AssertionError at `assert arn_b != arn`: ('arn:aws:acm-pca:us-west-2:<acct>:certificate-authority/60900b80-…', 'arn:aws:acm-pca:us-west-2:<acct>:certificate-authority/60900b80-…') — identical ARNs for RSA_2048/CN-a and RSA_4096/CN-b sharing one IdempotencyToken.

Cause 4: acm-pca.deploy-certificate-authority-cfn-issues-certificate — botocore ClientError (ValidationException) calling DescribeCertificateAuthority: Value 'sim2real-pca-cfn-4fa26dcb0ced-Ca-54A57D56DB05' at 'certificateAuthorityArn' failed to satisfy constraint: … ^arn:…acm-pca…$ (the value came from the stack output 'CaArn'). acm-pca.deploy-cloudformation-ca-issues-certificate — same, Value 'sim2real-acmpca-cfn-28a1cf89ea40-Ca-08FF2C7F73F6'.

Cause 5: acm-pca.deploy-certificate-authority-cfn-then-issue-certificate — KeyError: 'OutputValue' at `next(o["OutputValue"] for o in outputs if o["OutputKey"] == "CertificateAuthorityArn")` over describe_stacks(...)['Stacks'][0]['Outputs'] of a CREATE_COMPLETE stack.

Cause 6: acm-pca.ca-arn-validation-precedes-resource-lookup — AssertionError: DescribeCertificateAuthority missing-CA message: 'the certificate authority arn:aws:acm-pca:us-west-2:<acct>:certificate-authority/d0904488-127f-411c-a2d0-b289cadd9abc was not found'; expected ResourceNotFoundException message "Could not find specified CA".

## Steps to Reproduce
Cause 1: acmpca.create_certificate_authority(CertificateAuthorityConfiguration={'KeyAlgorithm':'RSA_2048','SigningAlgorithm':'SHA256WITHRSA','Subject':{'CommonName':'x','Country':'USA'}}, CertificateAuthorityType='ROOT') -> compare the ValidationException message to AWS's form; repeat with a 129-char tag key, with KeyAlgorithm='AES_256', with tag_certificate_authority(Tags=[{'Key':'bad#tag!','Value':'x'}]), and with list_permissions(CertificateAuthorityArn='not-an-acm-pca-arn').
Cause 2: acmpca.create_certificate_authority(..., RevocationConfiguration={'CrlConfiguration':{'Enabled':True,'S3BucketName':'<a bucket that does not exist>','ExpirationInDays':7}}) -> observe 200 + CertificateAuthorityArn instead of 400 ValidationException.
Cause 3: call create_certificate_authority twice with the SAME IdempotencyToken but different CertificateAuthorityConfiguration (RSA_2048/CN-a then RSA_4096/CN-b) -> both responses carry the same CertificateAuthorityArn; AWS returns two distinct ARNs.
Cause 4/5: cloudformation.create_stack with a template declaring an AWS::ACMPCA::CertificateAuthority and an Output exporting {'Ref': <that resource>}; wait for CREATE_COMPLETE; read describe_stacks(...)['Stacks'][0]['Outputs'] -> the entry either lacks OutputValue (cause 5) or carries a generated name rather than the CA ARN, which DescribeCertificateAuthority then rejects with ValidationException (cause 4).
Cause 6: acmpca.describe_certificate_authority(CertificateAuthorityArn=<well-formed ARN for a random uuid>) -> ResourceNotFoundException whose message is a constructed sentence rather than "Could not find specified CA".
Replay the ids in trace_ids to confirm.
