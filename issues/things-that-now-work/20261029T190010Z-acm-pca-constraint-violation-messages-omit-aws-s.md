Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-parameter-shape-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-parameter-shape-validation-errors
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|CreateCertificateAuthority|4xx->4xx|$.message
Issue-Key: acm-pca|RevokeCertificate|4xx->4xx|$.message
Issue-Key: acm-pca|IssueCertificate|4xx->4xx|$.message
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# ACM PCA constraint-violation messages omit AWS's '1 validation error detected:' prefix and list enum value sets in the wrong order

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateAuthority

## Description
THREE net-new causes, all in one place: the acm-pca front-door constraint-violation message builder. Every enum-constraint refusal has the right HTTP status (400) and the right error code, but the message text is assembled differently from AWS: the count prefix is missing on all three, and on two of them the enum value set is enumerated in the Smithy model's declaration order instead of the order AWS's front door emits. No open acm-pca issue covers these: the open behavioral issue 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md is about PutPolicy/DeletePolicy/authorization, and 20261022T143159Z-acm-pca-private-ca-lifecycle-is-unusable-issuecertificate.md is about CSR/issuance lifecycle.

Cause 1 of 3: CreateCertificateAuthority's invalid-enum refusal message is missing AWS's leading validation-error count.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models CreateCertificateAuthority.CertificateAuthorityType as required, targeting the enum CertificateAuthorityType with values [ROOT, SUBORDINATE]; the model does not state the diagnostic wording, so for the text this is recorded behavior only. The recording shows AWS refuses an out-of-set value with HTTP 400 and a message whose FIXED text is the standard front-door template "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]" — i.e. the count prefix ("1 validation error detected: ") is part of the message, and the VARIABLE parts are the number of violations, the member's wire name taken from the request (certificateAuthorityType) and the enum's value list.
Emulator: answers 400 with the same error class but the message starts at "Value at 'certificateAuthorityType' failed to satisfy constraint: ..." — the entire "1 validation error detected: " prefix is absent. The value set itself matches here ([ROOT, SUBORDINATE]).
Why: SDK and CLI users, and every test that asserts on a refusal, match AWS's template; a message that drops the count prefix makes the emulator's refusal unrecognizable to anything that parses or asserts the standard front-door text, and hides how many violations were detected when several members are bad at once.
Fix: the shared constraint-violation/ValidationException message formatter in the acm-pca service crate's request-validation layer must emit the count prefix ("<n> validation error detected: ") ahead of the per-member clauses for every operation, not just this one.
Traces: acm-pca-parameter-shape-validation-errors#task#1.

Cause 2 of 3: RevokeCertificate's invalid-RevocationReason refusal both drops the count prefix and lists the enum value set in model-declaration order rather than AWS's order.
AWS: Smithy models RevokeCertificate.RevocationReason as required, targeting enum RevocationReason; the message text is recorded behavior only. The recording shows HTTP 400 with the fixed front-door template plus the count prefix, and the bracketed value set rendered in AWS's own order: [AFFILIATION_CHANGED, CESSATION_OF_OPERATION, A_A_COMPROMISE, PRIVILEGE_WITHDRAWN, SUPERSEDED, UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE]. The fixed text is the template and the bracketed set; the variable parts are the violation count and the member path taken from the request (revocationReason).
Emulator: message is "Value at 'revocationReason' failed to satisfy constraint: Member must satisfy enum value set: [UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE, AFFILIATION_CHANGED, SUPERSEDED, CESSATION_OF_OPERATION, PRIVILEGE_WITHDRAWN, A_A_COMPROMISE]" — no count prefix, and the set is emitted in the Smithy shape's declaration order (which the model confirms is exactly UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE, AFFILIATION_CHANGED, SUPERSEDED, CESSATION_OF_OPERATION, PRIVILEGE_WITHDRAWN, A_A_COMPROMISE), so the emulator is clearly rendering the model order.
Why: the bracketed set is fixed text, not a per-run value, so a client that asserts on the refusal message — or a human reading it — sees a different allowed-value listing than production returns for the same bad request; the formatter cannot be trusted to be byte-comparable for any enum member.
Fix: same formatter/validation layer — the enum value set must be rendered in the service's published order per enum rather than the model's member order, and the count prefix added.
Traces: acm-pca-parameter-shape-validation-errors#task#4.

Cause 3 of 3: IssueCertificate's invalid Validity.Type refusal has the same two defects on a nested member path.
AWS: Smithy models IssueCertificate.Validity as required (structure Validity whose Type targets enum ValidityPeriodType); message text is recorded behavior only. The recording shows HTTP 400 with the count prefix and the set in AWS's order: [END_DATE, MONTHS, YEARS, DAYS, ABSOLUTE]. The member path in the message is the dotted path of the offending member taken from the request (validity.type) — that part is variable, the template and the value list are fixed.
Emulator: "Value at 'validity.type' failed to satisfy constraint: Member must satisfy enum value set: [END_DATE, ABSOLUTE, DAYS, MONTHS, YEARS]" — again no count prefix, and again exactly the Smithy declaration order for ValidityPeriodType.
Why: identical client impact to cause 2, and it confirms the defect is in the shared formatter rather than in one operation's handler — every enum constraint in the service is affected, including nested ones.
Fix: same component; fixing the formatter must cover nested member paths too.
Traces: acm-pca-parameter-shape-validation-errors#task#6.

## Evidence
Cause 1 — fixture acm-pca-parameter-shape-validation-errors step task#1, CreateCertificateAuthority, status 400 expected and 400 actual (only $.message diverges):
  expected message: "1 validation error detected: Value at 'certificateAuthorityType' failed to satisfy constraint: Member must satisfy enum value set: [ROOT, SUBORDINATE]"
  actual message:   "Value at 'certificateAuthorityType' failed to satisfy constraint: Member must satisfy enum value set: [ROOT, SUBORDINATE]"
  diverging part: the fixed prefix "1 validation error detected: " (count variable, rest fixed).

Cause 2 — same fixture, step task#4, RevokeCertificate, 400 vs 400:
  expected message: "1 validation error detected: Value at 'revocationReason' failed to satisfy constraint: Member must satisfy enum value set: [AFFILIATION_CHANGED, CESSATION_OF_OPERATION, A_A_COMPROMISE, PRIVILEGE_WITHDRAWN, SUPERSEDED, UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE]"
  actual message:   "Value at 'revocationReason' failed to satisfy constraint: Member must satisfy enum value set: [UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE, AFFILIATION_CHANGED, SUPERSEDED, CESSATION_OF_OPERATION, PRIVILEGE_WITHDRAWN, A_A_COMPROMISE]"
  diverging parts: missing count prefix; bracketed set emitted in Smithy declaration order (model RevocationReason values = UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE, AFFILIATION_CHANGED, SUPERSEDED, CESSATION_OF_OPERATION, PRIVILEGE_WITHDRAWN, A_A_COMPROMISE).

Cause 3 — same fixture, step task#6, IssueCertificate, 400 vs 400:
  expected message: "1 validation error detected: Value at 'validity.type' failed to satisfy constraint: Member must satisfy enum value set: [END_DATE, MONTHS, YEARS, DAYS, ABSOLUTE]"
  actual message:   "Value at 'validity.type' failed to satisfy constraint: Member must satisfy enum value set: [END_DATE, ABSOLUTE, DAYS, MONTHS, YEARS]"
  diverging parts: missing count prefix; bracketed set in model order (model ValidityPeriodType values = END_DATE, ABSOLUTE, DAYS, MONTHS, YEARS).

Status and error class are correct in all three; only the message body diverges.

## Steps to Reproduce
All three are single calls against a freshly booted emulator, no setup state required (they are front-door constraint refusals).

Cause 1: CreateCertificateAuthority with a valid CertificateAuthorityConfiguration (e.g. KeyAlgorithm RSA_2048, SigningAlgorithm SHA256WITHRSA, Subject CommonName set) and CertificateAuthorityType set to a value outside [ROOT, SUBORDINATE]. Expect HTTP 400 whose Error.Message begins with "1 validation error detected: ".

Cause 2: RevokeCertificate(CertificateAuthorityArn=<any well-formed acm-pca CA ARN>, CertificateSerial=<any serial>, RevocationReason=<value outside the enum>). Expect HTTP 400 and compare the whole message, including the order of the bracketed value set, against AWS's: [AFFILIATION_CHANGED, CESSATION_OF_OPERATION, A_A_COMPROMISE, PRIVILEGE_WITHDRAWN, SUPERSEDED, UNSPECIFIED, KEY_COMPROMISE, CERTIFICATE_AUTHORITY_COMPROMISE].

Cause 3: IssueCertificate(CertificateAuthorityArn=<any well-formed CA ARN>, Csr=<any blob>, SigningAlgorithm=SHA256WITHRSA, Validity={Value:1, Type:<value outside the enum>}). Expect HTTP 400, message with the count prefix and the set ordered [END_DATE, MONTHS, YEARS, DAYS, ABSOLUTE].

Replay: acm-pca-parameter-shape-validation-errors steps task#1, task#4, task#6.
