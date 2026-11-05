Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.create-ca-crl-configuration-requires-valid-bucket acm-pca.create-certificate-authority-invalid-key-algorithm-rejected
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.create-ca-crl-configuration-requires-valid-bucket|run#9|AssertionError|assert code_b == _EXPECTED_CRL_REJECTION, (
Issue-Key: acm-pca.create-certificate-authority-invalid-key-algorithm-rejected|run#7|AssertionError|assert message == (
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# ACM PCA CreateCertificateAuthority validation: an enabled CrlConfiguration with no S3BucketName is refused as InvalidArgsException instead of ValidationException, and the keyAlgorithm enum value set includes SM2

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateAuthority

## Description
TWO net-new root causes in CreateCertificateAuthority's request validation. Neither failing check is marked ALREADY FILED or OWNED. The open issue 20261029T190010Z-acm-pca-constraint-violation-messages-omit-aws-s.md covers a different defect in the same formatter (the missing '1 validation error detected: ' prefix and enum ordering on other operations) and does not carry either of these replay keys; cause 2 below is about the CONTENTS of the keyAlgorithm value set, not its order.

Cause 1 of 2: an enabled CrlConfiguration submitted with no S3BucketName is refused with the service-specific InvalidArgsException instead of the front-door ValidationException naming the missing member.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models CreateCertificateAuthority errors as InvalidArgsException, InvalidPolicyException, InvalidTagException and LimitExceededException, with no revocation-specific error shape, and models CrlConfiguration.S3BucketName as OPTIONAL (S3BucketName3To255, @length 3..255) while Enabled is @required — so the "enabled CRL needs a bucket" rule is not expressible in the model and is recorded behavior only (REAL-AWS VERIFIED us-west-2 in the check). The recording shows AWS refuses every defective RevocationConfiguration with the unmodeled generic ValidationException: for a missing bucket, HTTP 400 ValidationException with "1 validation error detected: Value null or empty at 's3BucketName' failed to satisfy constraint: Member must not be null or empty." The fixed text is the front-door template ("<n> validation error detected: Value null or empty at '<member>' failed to satisfy constraint: Member must not be null or empty."); the variable parts are the violation count and the member's wire name (s3BucketName). The modeled service-specific codes are reserved for defects that get PAST that validation layer (a reserved 'aws:' tag prefix is InvalidTagException, an RSA/ECDSA mismatch is InvalidArgsException), and no CA is created. The sibling break in the same check — an enabled CRL against a bucket that does not exist — already answers ValidationException "The S3 bucket you provided does not exist." correctly in the emulator, which is what isolates this cause to the missing-member path.
Emulator: answers HTTP 400 with code InvalidArgsException for the missing-bucket body, so the error class is a service-specific one AWS reserves for semantic argument faults and the message never names the offending member or the constraint.
Why: a client branching on ValidationException (a schema fault — not retryable, member named, repairable automatically) versus InvalidArgsException (a semantic argument fault) takes the wrong branch, and the response does not say which member was missing, so a caller assembling a RevocationConfiguration cannot tell what to fill in.
Fix: CreateCertificateAuthority's request validation — classify a missing/empty S3BucketName under an enabled CrlConfiguration in the front-door constraint stage that emits ValidationException with the null-or-empty template for the member, ahead of the semantic InvalidArgsException checks.
Traces: acm-pca.create-ca-crl-configuration-requires-valid-bucket (check line 141, break B); handler services/service-com-amazonaws-acmpca-acmprivateca/src/ops/create_certificate_authority.rs and the service's shared request-validation layer.

Cause 2 of 2: the keyAlgorithm enum-constraint message advertises SM2, which AWS's front door does not list.
AWS: Smithy models CertificateAuthorityConfiguration.KeyAlgorithm as an enum whose declared values are [RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87, SM2]; the diagnostic text is recorded behavior only. The recording shows an out-of-set value refused with HTTP 400 ValidationException whose message is "Value at 'certificateAuthorityConfiguration.keyAlgorithm' failed to satisfy constraint: Member must satisfy enum value set: [RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87]" — the bracketed set is fixed text and does NOT include SM2 (the region's front door does not offer it), while the variable part is the dotted member path taken from the request. No CA is created, and a supported KeyAlgorithm still succeeds.
Emulator: same status, same code, same member path, but the bracketed set ends "..., ML_DSA_87, SM2]" — it renders the pinned model's full value list rather than the set the service advertises.
Why: the bracketed set is the actionable part of the message: a client (or a human) repairing the request reads SM2 as an accepted value, submits it, and is refused in production; any test that compares the refusal text fails on a value list that production never emits.
Fix: the enum-constraint message builder used by CreateCertificateAuthority's validation — render the service's advertised value set for KeyAlgorithm rather than the raw model enum, so model values the front door does not accept are not advertised.
Traces: acm-pca.create-certificate-authority-invalid-key-algorithm-rejected (check line 47); the shared constraint-violation formatter reached from services/service-com-amazonaws-acmpca-acmprivateca/src/ops/create_certificate_authority.rs.

## Evidence
Cause 1 — acm-pca.create-ca-crl-configuration-requires-valid-bucket, CreateCertificateAuthority with RevocationConfiguration={'CrlConfiguration': {'Enabled': True, 'ExpirationInDays': 7}} (no S3BucketName):
  expected: HTTP 400, code ValidationException, message "1 validation error detected: Value null or empty at 's3BucketName' failed to satisfy constraint: Member must not be null or empty."
  actual:   HTTP 400, code 'InvalidArgsException' — AssertionError: CRL enabled with no S3BucketName must be refused with ValidationException, got 'InvalidArgsException'.
  Control in the same check: an enabled CRL naming a nonexistent bucket already returns ValidationException 'The S3 bucket you provided does not exist.' (passes), and a DISABLED CrlConfiguration on the same body is accepted.

Cause 2 — acm-pca.create-certificate-authority-invalid-key-algorithm-rejected, CreateCertificateAuthority with CertificateAuthorityConfiguration.KeyAlgorithm='AES_256' (client-side parameter validation disabled so the value reaches the service):
  expected message: "Value at 'certificateAuthorityConfiguration.keyAlgorithm' failed to satisfy constraint: Member must satisfy enum value set: [RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87]"
  actual message:   "Value at 'certificateAuthorityConfiguration.keyAlgorithm' failed to satisfy constraint: Member must satisfy enum value set: [RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87, SM2]"
  diverging part: the trailing ', SM2' inside the fixed bracketed set. Status 400 and code ValidationException match; the positive control (KeyAlgorithm RSA_2048) succeeds.

## Steps to Reproduce
Cause 1: CreateCertificateAuthority(CertificateAuthorityConfiguration={'KeyAlgorithm':'RSA_2048','SigningAlgorithm':'SHA256WITHRSA','Subject':{'CommonName':'sim2real-crl-b'}}, CertificateAuthorityType='ROOT', RevocationConfiguration={'CrlConfiguration':{'Enabled':True,'ExpirationInDays':7}}) — no S3BucketName. Expect HTTP 400 with code ValidationException and the message "1 validation error detected: Value null or empty at 's3BucketName' failed to satisfy constraint: Member must not be null or empty.", and no CA created (ListCertificateAuthorities must not show the CommonName). For contrast, the same body with 'S3BucketName' set to a syntactically legal bucket that does not exist must stay ValidationException 'The S3 bucket you provided does not exist.', and ExpirationInDays=5001 must be the range violation.

Cause 2: with botocore parameter validation disabled (client._client_config.parameter_validation = False), CreateCertificateAuthority(CertificateAuthorityConfiguration={'KeyAlgorithm':'AES_256','SigningAlgorithm':'SHA256WITHRSA','Subject':{'CommonName':'sim2real-badkey'}}, CertificateAuthorityType='ROOT'). Expect HTTP 400 ValidationException whose bracketed enum value set ends at ML_DSA_87, with no SM2; then confirm KeyAlgorithm='RSA_2048' is accepted and the rejected CommonName exists on no CA.
