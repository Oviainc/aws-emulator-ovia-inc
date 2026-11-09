Issue-Source: trace-sweep-divergence
Issue-Trace: directory-service-certificate-and-assessment-lookup-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directory-service-certificate-and-assessment-lookup-errors ds-madl-traj-register-certificate-mistakes
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|DescribeCertificate|4xx->4xx|$.message
Issue-Key: ds|DescribeADAssessment|4xx->4xx|$.message
Issue-Commit: 8339e0b79c5a3f0b6ab10000bb256c51d9f94d77

# ValidationException message omits regex anchors for certificateId and assessmentId patterns

> _Note: this issue was identified against the code base at commit `8339e0b79c5a3f0b6ab10000bb256c51d9f94d77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
This bundle has 2 causes, the same anchor-stripping defect on the certificate and assessment identifier patterns. The emulator returns the right status (400) and the right error type (ValidationException) but renders the modeled regular expression without its `^` and `$` anchors.

Message template for this error class (AWS wording): `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. Everything but `<value>` (the rejected request value), `<memberPath>` (the input member's path) and `<pattern>` is fixed text; `<pattern>` must be the Smithy @pattern trait string VERBATIM. The recorded values 'ac-01234567890123456789012345678901' and 'da-12345678901234567890' are the recordings' request values filling the template, not part of it.

Cause 1 of 2 — DescribeCertificate / certificateId.
  AWS: Smithy model services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json — shape com.amazonaws.directoryservice#CertificateId has pattern `^c-[0-9a-f]{10}$`; a violating value is refused 400 ValidationException with the template above carrying `^c-[0-9a-f]{10}$`.
  Emulator: 400 ValidationException with the same sentence but the pattern printed as `c-[0-9a-f]{10}`.
  Why: the message is a requirement of the error class; a client asserting on AWS's sentence fails, and the regular expression the message publishes is unanchored, so anything derived from it accepts identifiers AWS rejects.
  Fix: the shared request-validation layer that formats @pattern failures for ds inputs (DescribeCertificate's input validation path) — emit the @pattern value with its anchors.
  Traces: directory-service-certificate-and-assessment-lookup-errors#task#1 (cluster also covers #task#2 and ds-madl-traj-register-certificate-mistakes#task#8).

Cause 2 of 2 — DescribeADAssessment / assessmentId.
  AWS: same model — shape com.amazonaws.directoryservice#AssessmentId has pattern `^da-[0-9a-f]{18}$`; a violating value is refused 400 ValidationException with the template above carrying `^da-[0-9a-f]{18}$`.
  Emulator: 400 ValidationException with the pattern printed as `da-[0-9a-f]{18}`.
  Why: as above — the published constraint differs from AWS's and is unanchored.
  Fix: same shared @pattern message formatter, reached through DescribeADAssessment's input validation.
  Traces: directory-service-certificate-and-assessment-lookup-errors#task#3.

Not covered by any open issue: 20261103T100504Z-ds-directory-service-certificate-and-directory-lookups-unvalidated-shortname.md owns other certificate-lookup steps (RegisterCertificate oCSPUrl wording, CertificateDoesNotExistException classification) but carries the replay key of neither step filed here, and 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md concerns validation that is skipped entirely rather than mis-rendered.

## Evidence
Replay of trace set simparity-wire-aws-v1 (service ds), verdict mismatch, expected_status 400 / actual_status 400, diverging key $.message.

Cause 1 — fixture directory-service-certificate-and-assessment-lookup-errors step task#1, DescribeCertificate (cluster affected_count 3: also task#2 and ds-madl-traj-register-certificate-mistakes#task#8):
  expected $.message = "1 validation error detected: Value 'ac-01234567890123456789012345678901' at 'certificateId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^c-[0-9a-f]{10}$"
  actual   $.message = "1 validation error detected: Value 'ac-01234567890123456789012345678901' at 'certificateId' failed to satisfy constraint: Member must satisfy regular expression pattern: c-[0-9a-f]{10}"

Cause 2 — fixture directory-service-certificate-and-assessment-lookup-errors step task#3, DescribeADAssessment:
  expected $.message = "1 validation error detected: Value 'da-12345678901234567890' at 'assessmentId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^da-[0-9a-f]{18}$"
  actual   $.message = "1 validation error detected: Value 'da-12345678901234567890' at 'assessmentId' failed to satisfy constraint: Member must satisfy regular expression pattern: da-[0-9a-f]{18}"

Model evidence: smithy_query shape CertificateId -> {"pattern": "^c-[0-9a-f]{10}$"}; shape AssessmentId -> {"pattern": "^da-[0-9a-f]{18}$"}. Status and error type match; only the pattern text inside the message diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Call ds DescribeCertificate with a CertificateId violating `^c-[0-9a-f]{10}$` (e.g. 'ac-01234567890123456789012345678901') and ds DescribeADAssessment with an AssessmentId violating `^da-[0-9a-f]{18}$` (e.g. 'da-12345678901234567890').
3. Observe 400 ValidationException whose $.message ends in `c-[0-9a-f]{10}` / `da-[0-9a-f]{18}`.
4. AWS ends the same sentence with `^c-[0-9a-f]{10}$` / `^da-[0-9a-f]{18}$`. Re-replay fixture directory-service-certificate-and-assessment-lookup-errors steps task#1 and task#3 to confirm a fix.
