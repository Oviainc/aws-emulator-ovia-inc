Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-dns-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-dns-lifecycle acm-certificate-domain-validation-and-tagging acm-certificate-export-renew-eligibility-edge-cases acm-request-certificate-email-validation-resend acm-update-certificate-options-validation-method acm-export-certificate-pending-validation-error acm-renew-certificate-not-eligible-error acm-request-certificate-options-reflected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.DomainValidationOptions[*].ValidationDomain
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.ExtendedKeyUsages
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Issuer
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.KeyAlgorithm
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.KeyUsages
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Options
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.SignatureAlgorithm
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Subject
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.DomainValidationOptions[*].ValidationEmails[*]
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Options.Export
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# DescribeCertificate omits modeled certificate metadata and mis-forms KeyAlgorithm

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
The emulator's CertificateDetail projection is incomplete: several members that AWS always populates for an AMAZON_ISSUED certificate are absent, one enum is serialized in the wrong lexical form, and one list is fabricated. Five distinct causes, all in the DescribeCertificate response shaping.

Cause 1 of 5 - DNS certificate detail is missing most X.509/metadata members.
AWS: Smithy CertificateDetail declares Subject, Issuer, SignatureAlgorithm, KeyUsages (KeyUsageList), ExtendedKeyUsages (ExtendedKeyUsageList), Options (CertificateOptions) and DomainValidation.ValidationDomain. The recording shows AWS returning all of them for a requested public certificate: Issuer='Amazon', Subject='CN=<domain>', SignatureAlgorithm='SHA256WITHRSA', KeyUsages=[], ExtendedKeyUsages=[], Options={CertificateTransparencyLoggingPreference:'ENABLED', Export:'DISABLED'}, and DomainValidationOptions[i].ValidationDomain equal to each entry's DomainName.
Emulator: HTTP 200, but $.Certificate.Subject, .Issuer, .SignatureAlgorithm, .KeyUsages, .ExtendedKeyUsages, .Options and DomainValidationOptions[0..1].ValidationDomain are all absent.
Why: a client reading cert.Options.Export, cert.Issuer or cert.KeyUsages gets null/None and either crashes or mis-decides (e.g. concludes export is unavailable because Options is missing rather than DISABLED). ValidationDomain being absent breaks email-validation tooling that derives the notification domain from it.
Fix: the DescribeCertificate response serializer / CertificateDetail projection in the ACM service crate - these members must be populated from certificate state, with the empty lists emitted as [] rather than omitted.

Cause 2 of 5 - KeyAlgorithm uses the model enum name instead of the DescribeCertificate wire form.
AWS: recorded behavior only - DescribeCertificate returns $.Certificate.KeyAlgorithm='RSA-2048' (hyphen). Smithy KeyAlgorithm enumerates RSA_1024/RSA_2048/.../EC_prime256v1, which is the form used on the request side; the recording shows the describe projection emitting the hyphenated spelling.
Emulator: returns 'RSA_2048'.
Why: clients that compare the describe output against the hyphenated value AWS returns (or display it) see an unknown algorithm string.
Fix: KeyAlgorithm serialization in the DescribeCertificate projection.

Cause 3 of 5 - same omissions on an email/export edge-case certificate, plus fabricated ValidationEmails.
AWS: the recorded DescribeCertificate response carries no ValidationEmails entries for the domain validation option (and where present elsewhere it is the empty list), together with Issuer/Subject/SignatureAlgorithm/KeyUsages/ExtendedKeyUsages/Options as in Cause 1 and KeyAlgorithm='RSA-2048'.
Emulator: emits five synthesized addresses in $.Certificate.DomainValidationOptions[0].ValidationEmails (administrator@, hostmaster@, postmaster@, webmaster@, admin@ of the requested domain) and still omits the members above.
Why: a client asserting on the validation-email list sees addresses AWS did not report, so email-validation flows diverge in both directions (extra entries here, missing metadata there).
Fix: the same projection - do not synthesize ValidationEmails; populate it only from recorded/derived validation state.

Cause 4 of 5 - identical omissions on the certificate described before an ExportCertificate/RenewCertificate error path.
AWS: as Cause 1 - ValidationDomain, Issuer, Subject, SignatureAlgorithm, KeyUsages=[], ExtendedKeyUsages=[], Options present; KeyAlgorithm='RSA-2048'.
Emulator: all absent; KeyAlgorithm='RSA_2048'.
Why: the pre-condition read a client makes before export/renew returns a certificate that looks unpopulated.
Fix: same projection.

Cause 5 of 5 - Options submitted on RequestCertificate are only partly reflected.
AWS: Smithy CertificateOptions has members CertificateTransparencyLoggingPreference and Export; the recording shows Options.Export='DISABLED' and Issuer='Amazon' on describe after a RequestCertificate that carried Options.
Emulator: $.Certificate.Options.Export and $.Certificate.Issuer are absent (Options itself is present in this case but incomplete).
Why: a client that sets Options on request and reads them back to confirm cannot see the Export preference it just set.
Fix: carry CertificateOptions through RequestCertificate into certificate state and emit every member in the describe projection.

Traces: acm-certificate-dns-lifecycle#task#7 (representative); acm-certificate-domain-validation-and-tagging#task#2; acm-certificate-export-renew-eligibility-edge-cases#task#2 (also acm-request-certificate-email-validation-resend#task#2, acm-update-certificate-options-validation-method#task#2 and #task#4); acm-export-certificate-pending-validation-error#task#2 (also acm-renew-certificate-not-eligible-error#task#2); acm-request-certificate-options-reflected#task#2.

## Evidence
Expected vs actual, HTTP 200 on both sides.
acm-certificate-dns-lifecycle#task#7: $.Certificate.DomainValidationOptions[0].ValidationDomain expected 'sim2real-probe-acm-acmdns-d530a20b.example.com', got None; [1].ValidationDomain expected 'www.sim2real-probe-acm-acmdns-d530a20b.example.com', got None; ExtendedKeyUsages expected [], got None; Issuer expected 'Amazon', got None; KeyAlgorithm expected 'RSA-2048', got 'RSA_2048'; KeyUsages expected [], got None; Options expected {'CertificateTransparencyLoggingPreference': 'ENABLED', 'Export': 'DISABLED'}, got None; SignatureAlgorithm expected 'SHA256WITHRSA', got None; Subject expected 'CN=sim2real-probe-acm-acmdns-d530a20b.example.com', got None.
acm-certificate-domain-validation-and-tagging#task#2: Issuer expected 'Amazon', got None; Options expected {'CertificateTransparencyLoggingPreference': 'ENABLED', 'Export': 'DISABLED'}, got None.
acm-certificate-export-renew-eligibility-edge-cases#task#2: ValidationEmails[0..4] extra (expected None, got 'administrator@...', 'hostmaster@...', 'postmaster@...', 'webmaster@...', 'admin@...'); plus the Issuer/Subject/SignatureAlgorithm/KeyUsages/ExtendedKeyUsages/Options omissions and KeyAlgorithm 'RSA-2048' -> 'RSA_2048'. Cluster covers 4 steps.
acm-export-certificate-pending-validation-error#task#2: ValidationDomain, ExtendedKeyUsages, Issuer, KeyUsages, Options, SignatureAlgorithm, Subject all missing; KeyAlgorithm 'RSA-2048' -> 'RSA_2048'. Cluster covers 2 steps.
acm-request-certificate-options-reflected#task#2: Issuer expected 'Amazon', got None; Options.Export expected 'DISABLED', got None.
Smithy: CertificateDetail members Subject/Issuer/SignatureAlgorithm/KeyUsages/ExtendedKeyUsages/Options; DomainValidation.ValidationDomain; CertificateOptions{CertificateTransparencyLoggingPreference, Export}; enum KeyAlgorithm values RSA_1024..EC_secp521r1.

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator (replay_wire_corpus).
2. RequestCertificate for a domain (DNS or EMAIL validation, with or without Options), then DescribeCertificate on the returned CertificateArn.
3. Compare the $.Certificate object with the recorded response: Subject, Issuer, SignatureAlgorithm, KeyUsages, ExtendedKeyUsages, Options and DomainValidationOptions[*].ValidationDomain are absent, KeyAlgorithm comes back 'RSA_2048' instead of 'RSA-2048', and ValidationEmails contains five synthesized addresses AWS did not return.
4. Named steps: acm-certificate-dns-lifecycle#task#7, acm-certificate-domain-validation-and-tagging#task#2, acm-certificate-export-renew-eligibility-edge-cases#task#2, acm-export-certificate-pending-validation-error#task#2, acm-request-certificate-options-reflected#task#2.
