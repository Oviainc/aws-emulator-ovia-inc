Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-dns-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-dns-lifecycle acm-certificate-domain-validation-and-tagging acm-export-certificate-pending-validation-error acm-renew-certificate-not-eligible-error acm-request-certificate-dns-lifecycle acm-certificate-email-with-inline-tags acm-update-certificate-options-validation-method acm-osc-public-cert-for-alb-dns-name acm-request-public-certificate-dns-validation-lifecycle
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.Status
Issue-Key: acm|DescribeCertificate|2xx->2xx|$.Certificate.DomainValidationOptions[*].ValidationStatus
Issue-Related: 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md
Issue-Related: 20261102T085352Z-acm-requestcertificate-for-a-long-aws-owned-elb.md
Issue-Commit: 6c534d81bd78f33a29d39ee989f5affa38b22780

# ACM requested certificates never settle terminally: DescribeCertificate stays PENDING_VALIDATION with no FailureReason (5 causes)

> _Note: this issue was identified against the code base at commit `6c534d81bd78f33a29d39ee989f5affa38b22780`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Five net-new wire-replay root causes on the ACM AMAZON_ISSUED certificate state machine and the CertificateDetail projection DescribeCertificate serves for a certificate that cannot be validated. All five are DescribeCertificate mismatch clusters whose replay keys are carried by no open issue (the replay reports owned_by = [] for each). Each cause below must be closed before this issue is submitted; the four causes beyond the representative are named in trace_ids.

The shared fault: a public certificate requested for a name ACM cannot validate for the account is parked in Status 'PENDING_VALIDATION' forever, so no terminal state and no modeled FailureReason is ever produced, and the per-domain and per-certificate projections that depend on the terminal state are wrong as well.

=== Cause 1 of 5: DescribeCertificate never reports terminal Status FAILED / FailureReason ADDITIONAL_VERIFICATION_REQUIRED (fixture acm-certificate-dns-lifecycle, step task#7) ===

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) RequestCertificate{DomainName required -> DomainNameString @length 1..253 @pattern ^(\*\.)?(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$, ValidationMethod} -> CertificateArn; DescribeCertificate -> CertificateDetail, whose Status targets CertificateStatus [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED] and whose FailureReason targets FailureReason [NO_AVAILABLE_CONTACTS, ADDITIONAL_VERIFICATION_REQUIRED, DOMAIN_NOT_ALLOWED, INVALID_PUBLIC_DOMAIN, DOMAIN_VALIDATION_DENIED, CAA_ERROR, PCA_*, SLR_NOT_FOUND, OTHER], documented as existing "only when the certificate status is FAILED". Recorded behavior for the general case: a DNS-validated public certificate requested for a name ACM will not issue for (a non-validatable public label) does not stay pending — within seconds it settles into the terminal Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', and stays there. Those are modeled enum tokens; the ARN, account id, generated label and timestamps of any one recording fill the template and are not part of it.

Emulator: DescribeCertificate answers 200 with Certificate.Status 'PENDING_VALIDATION' and no Certificate.FailureReason at all, on every poll — the replay reports "$.Certificate.Status: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')", "$.Certificate.FailureReason: missing (expected 'ADDITIONAL_VERIFICATION_REQUIRED', got None)" and "poll: not in ['FAILED'] after 60 attempt(s)".

Why: a client relying on AWS's behavior hangs instead of failing. The standard pattern (and the certificate-issued waiter) polls DescribeCertificate until Status is terminal; against the emulator the caller cannot distinguish "still validating" from "will never issue", and code that branches on FailureReason reads nothing for a field AWS guarantees once Status is FAILED. Every negative scenario that needs a terminal managed certificate as its precondition becomes unreachable.

Fix: the ACM certificate state machine in the acm service crate — the RequestCertificate handler's validation/issuance driver and the stored certificate state that DescribeCertificate projects. A requested AMAZON_ISSUED certificate whose names ACM cannot validate must settle into Status 'FAILED' with a populated modeled FailureReason, hold no Serial, and remain terminal.

Traces: acm-certificate-dns-lifecycle#task#7.

=== Cause 2 of 5: the per-domain DomainValidationOptions[].ValidationStatus never follows the certificate into FAILED (fixture acm-certificate-domain-validation-and-tagging, step task#2) ===

AWS: Smithy DomainValidation.ValidationStatus targets DomainStatus, documented as PENDING_VALIDATION | SUCCESS | FAILED, inside CertificateDetail.DomainValidationOptions ("information about the initial validation of each domain name that occurs as a result of the RequestCertificate request"). Recorded behavior: when the certificate reaches Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', the entry for each requested domain carries ValidationStatus 'FAILED' — the per-domain status and the certificate status agree.

Emulator: Certificate.Status is 'PENDING_VALIDATION' (never 'FAILED'), Certificate.FailureReason is absent, and DomainValidationOptions[0].ValidationStatus is 'PENDING_VALIDATION' where 'FAILED' is required. This cluster covers 4 fixtures (acm-certificate-domain-validation-and-tagging, acm-export-certificate-pending-validation-error, acm-renew-certificate-not-eligible-error, acm-request-certificate-dns-lifecycle), i.e. every downstream refusal that is gated on a validation failure reads a pending certificate instead.

Why: a client that inspects per-domain validation to decide which name blocked issuance sees 'PENDING_VALIDATION' for a domain whose validation definitively failed, so retry/alerting logic keyed on the per-domain status never fires, and operations whose eligibility depends on a terminal certificate (export, renew) take the wrong branch.

Fix: the same ACM validation driver, plus the DomainValidation projection in the DescribeCertificate response builder — the per-domain ValidationStatus must be derived from the validation outcome and move to 'FAILED' with the certificate.

Traces: acm-certificate-domain-validation-and-tagging#task#2 (also acm-export-certificate-pending-validation-error#task#2, acm-renew-certificate-not-eligible-error#task#2, acm-request-certificate-dns-lifecycle#task#2).

=== Cause 3 of 5: DescribeCertificate keeps emitting DomainValidation.ValidationEmails for an EMAIL-validated certificate that is no longer awaiting an email challenge (fixture acm-certificate-email-with-inline-tags, step task#2) ===

AWS: Smithy DomainValidation.ValidationEmails -> ValidationEmailList, documented as "A list of email addresses that ACM used to send domain validation emails." Recorded behavior only: for an EMAIL-validated certificate in the terminal failed state, the recorded DescribeCertificate response carries NO ValidationEmails member on the domain-validation entry (the member is absent, not an empty list), alongside Status 'FAILED', FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED' and the per-domain ValidationStatus 'FAILED'. Hypothesis (not stated by the model): ACM exposes the contacted addresses only while an email challenge is live, and drops the member once the request is no longer awaiting one.

Emulator: it serves a fixed, synthesized five-address list on the domain-validation entry (the admin/administrator/hostmaster/postmaster/webmaster@<domain> set) where the member must be absent — the replay reports five "$.Certificate.DomainValidationOptions[0].ValidationEmails[n]: extra (expected None, got ...)" entries — while Status stays 'PENDING_VALIDATION' and FailureReason is missing, as in cause 1. The per-domain addresses themselves are derived from the request's domain and are not the divergence; the presence of the member on a non-pending certificate is.

Why: a client that treats a non-empty ValidationEmails as "an email challenge is outstanding, re-send or wait for approval" is told to keep waiting on a certificate that AWS has already given up on, and a consumer that diffs the DescribeCertificate shape against AWS sees an extra member on every email-validated certificate.

Fix: the DomainValidation projection in the acm crate's DescribeCertificate response builder (and the state it reads) — emit ValidationEmails only while the EMAIL challenge is outstanding, and omit the member otherwise.

Traces: acm-certificate-email-with-inline-tags#task#2 (also acm-update-certificate-options-validation-method#task#2, #task#4).

=== Cause 4 of 5: DomainValidation.ValidationDomain drops the requested name's wildcard label (fixture acm-osc-public-cert-for-alb-dns-name, step task#3) ===

AWS: Smithy DomainValidation.ValidationDomain -> DomainNameString @length 1..253 with the SAME @pattern ^(\*\.)?... as the request's DomainName, i.e. the leading wildcard label "*." is an explicitly modeled part of a valid value. Recorded behavior: for a certificate requested with a wildcard first domain, the domain-validation entry's ValidationDomain echoes the requested name verbatim, wildcard label included, and the certificate settles Status 'FAILED' / FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED' with per-domain ValidationStatus 'FAILED'. The specific zone in the recording is a value from the request, not a requirement; the requirement is that the wildcard label survives the echo.

Emulator: ValidationDomain is emitted with the "*." label stripped (the replay reports expected '*.us-west-2.elb.amazonaws.com', got 'us-west-2.elb.amazonaws.com'), and as in cause 1 Status stays 'PENDING_VALIDATION' with FailureReason missing and per-domain ValidationStatus 'PENDING_VALIDATION'. Note: the replay classifies the ValidationDomain line as "host_not_own" because the recorded value names a real-AWS host; it is nevertheless an echo of the request's own DomainName and therefore deterministic — the Status/FailureReason half of this cluster is grounded independently of it.

Why: a client that correlates a certificate's domain-validation entries back to the names it requested cannot match a wildcard request to its entry, because the key it asked for ("*.<zone>") is not the key it gets back; ResendValidationEmail and DNS-challenge bookkeeping both take ValidationDomain/Domain as request input, so a round-trip of the emulator's own value is not the value AWS accepts.

Fix: the DomainValidation projection in the acm crate's DescribeCertificate/RequestCertificate path — echo the requested domain name unmodified (wildcard label included) into DomainName/ValidationDomain instead of normalizing it away.

Traces: acm-osc-public-cert-for-alb-dns-name#task#3.

=== Cause 5 of 5: a requested DNS-validated certificate used as scenario setup never leaves PENDING_VALIDATION, so the whole fixture's subject is never reached (fixture acm-request-public-certificate-dns-validation-lifecycle, step setup#2) ===

AWS: as cause 1 — DescribeCertificate.Certificate.Status reaches a terminal CertificateStatus for a request ACM cannot validate, within seconds, rather than remaining 'PENDING_VALIDATION' indefinitely.

Emulator: the setup's DescribeCertificate poll is still answering Status 'PENDING_VALIDATION' after 60 attempts ("poll: not in ['FAILED'] after 60 attempt(s)"), so the fixture's setup never establishes its precondition. This is the same non-settling state machine observed through a setup step rather than a task step, and it is what makes the downstream refusal in this fixture untestable.

Why: any caller (or IaC/waiter) that blocks on a terminal certificate status spins to its own timeout; a scenario whose precondition is a settled certificate can never start.

Fix: the ACM validation/issuance driver in the acm service crate, as in cause 1 — the state machine must take a terminal transition for a non-validatable request instead of parking the certificate.

Traces: acm-request-public-certificate-dns-validation-lifecycle#setup#2.

=== Related open issues that do NOT cover these replays ===
20261110T152935Z-acm-certificate-state-machine-never-settles-a-non.md reports the same never-settling observable, but against the BEHAVIORAL corpus and keyed to check ids (acm.acm-reimport-into-an-amazon-issued-certificate-arn-rejected, acm.dns-validated-certificate-issues-under-a-delegated-zone); it carries none of the five wire fixture/step keys above. 20261102T085352Z-acm-requestcertificate-for-a-long-aws-owned-elb.md owns only step task#1 of acm-osc-public-cert-for-alb-dns-name (the missing 64-octet RequestCertificate refusal); the replay reports task#3 of that fixture as unowned, which is cause 4 here. 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md owns the ListCertificateDomainValidations/SearchCertificates/imported-certificate projection clusters, and 20261109T125518Z-acm-listcertificatedomainvalidations-omits-requestedvalidationconfiguration-for-email-validated-certificates.md owns the EMAIL RequestedValidationConfiguration cluster — neither carries a DescribeCertificate Status/FailureReason/ValidationEmails/ValidationDomain key. None of these closes any of the five causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): 79 fixtures replayed, 348/393 steps passed (88.5%), 44 mismatched, 16 divergence clusters — all acm. 11 clusters are marked OWNED by open issues and skipped; the 5 unowned clusters are the causes above.

Cause 1 — [mismatch] acm DescribeCertificate, fixture acm-certificate-dns-lifecycle step task#7, expected 200 / actual 200:
  $.Certificate.FailureReason: missing (expected 'ADDITIONAL_VERIFICATION_REQUIRED', got None)
  $.Certificate.Status: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.Status: poll: not in ['FAILED'] after 60 attempt(s) (expected 'FAILED', got 'PENDING_VALIDATION')

Cause 2 — [mismatch] acm DescribeCertificate, fixture acm-certificate-domain-validation-and-tagging step task#2, 200/200, affected_count 4 (also acm-export-certificate-pending-validation-error#task#2, acm-renew-certificate-not-eligible-error#task#2, acm-request-certificate-dns-lifecycle#task#2):
  $.Certificate.DomainValidationOptions[0].ValidationStatus: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.FailureReason: missing (expected 'ADDITIONAL_VERIFICATION_REQUIRED', got None)
  $.Certificate.Status: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.Status: poll: not in ['FAILED'] after 60 attempt(s)

Cause 3 — [mismatch] acm DescribeCertificate, fixture acm-certificate-email-with-inline-tags step task#2, 200/200, affected_count 3 (also acm-update-certificate-options-validation-method#task#2 and #task#4):
  $.Certificate.DomainValidationOptions[0].ValidationEmails[0..4]: extra (expected None, got 'admin@<domain>', 'administrator@<domain>', 'hostmaster@<domain>', 'postmaster@<domain>', 'webmaster@<domain>')
  $.Certificate.DomainValidationOptions[0].ValidationStatus: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.FailureReason: missing (expected 'ADDITIONAL_VERIFICATION_REQUIRED', got None)
  $.Certificate.Status: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')

Cause 4 — [mismatch] acm DescribeCertificate, fixture acm-osc-public-cert-for-alb-dns-name step task#3, 200/200:
  $.Certificate.DomainValidationOptions[0].ValidationStatus: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.FailureReason: missing (expected 'ADDITIONAL_VERIFICATION_REQUIRED', got None)
  $.Certificate.Status: value_mismatch (expected 'FAILED', got 'PENDING_VALIDATION')
  $.Certificate.DomainValidationOptions[0].ValidationDomain: host_not_own (expected '*.us-west-2.elb.amazonaws.com', got 'us-west-2.elb.amazonaws.com')
  $.Certificate.Status: poll: not in ['FAILED'] after 24 attempt(s)

Cause 5 — [mismatch] acm DescribeCertificate, fixture acm-request-public-certificate-dns-validation-lifecycle step setup#2, 200/200:
  $.Certificate.Status: poll: not in ['FAILED'] after 60 attempt(s) (expected None, got 'PENDING_VALIDATION')

Per-run values in these observations (certificate ARNs, the generated sim2real-probe-acm-* labels and hex suffixes, CreatedAt/IssuedAt, CNAME challenge names and values, the account id) are NOT requirements. The requirements are: the modeled enum tokens Status 'FAILED' plus a populated FailureReason ('ADDITIONAL_VERIFICATION_REQUIRED' for a name needing manual verification), the per-domain ValidationStatus 'FAILED' agreeing with it, the absence of the ValidationEmails member on a certificate no longer awaiting an email challenge, and a ValidationDomain that echoes the requested domain name including its '*.' wildcard label.

Smithy consulted (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): DescribeCertificate (-> CertificateDetail; errors InvalidArnException/ResourceNotFoundException/ValidationException, client, httpError 400), RequestCertificate, CertificateStatus enum [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED], FailureReason enum (incl. ADDITIONAL_VERIFICATION_REQUIRED), CertificateDetail.{Status, FailureReason, DomainValidationOptions, Serial, Type}, DomainValidation.{DomainName, ValidationStatus -> DomainStatus, ValidationEmails, ValidationDomain, ResourceRecord, ValidationMethod}.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay trace set simparity-wire-aws-v1. All five clusters reproduce as acm DescribeCertificate mismatches at: acm-certificate-dns-lifecycle#task#7, acm-certificate-domain-validation-and-tagging#task#2, acm-certificate-email-with-inline-tags#task#2, acm-osc-public-cert-for-alb-dns-name#task#3, acm-request-public-certificate-dns-validation-lifecycle#setup#2.

Causes 1, 2 and 5 directly:
2. acm.request_certificate(DomainName='<non-validatable-label>.example.com', ValidationMethod='DNS'); keep the CertificateArn.
3. Poll acm.describe_certificate(CertificateArn=<arn>). Observed: Certificate.Status stays 'PENDING_VALIDATION' indefinitely (60+ polls), Certificate.FailureReason absent, DomainValidationOptions[0].ValidationStatus 'PENDING_VALIDATION'. Expected (AWS): within seconds Status 'FAILED', FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', per-domain ValidationStatus 'FAILED', no Serial, Type 'AMAZON_ISSUED'.

Cause 3:
4. acm.request_certificate(DomainName='<label>.example.com', ValidationMethod='EMAIL') and poll acm.describe_certificate. Observed: DomainValidationOptions[0].ValidationEmails is always populated with the synthesized admin/administrator/hostmaster/postmaster/webmaster@<domain> list while Status stays 'PENDING_VALIDATION'. Expected (AWS): once the certificate is no longer awaiting its email challenge (Status 'FAILED', FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', per-domain ValidationStatus 'FAILED'), the ValidationEmails member is absent from the entry.

Cause 4:
5. acm.request_certificate(DomainName='*.<some-zone>', ValidationMethod='DNS') and read DomainValidationOptions[0].ValidationDomain from acm.describe_certificate. Observed: the leading '*.' label is stripped. Expected (AWS): the requested name is echoed verbatim, wildcard label included (DomainNameString's pattern admits '^(\*\.)?').

6. Re-replay ALL FIVE fixture#step keys (the representative acm-certificate-dns-lifecycle#task#7 plus the four in trace_ids) and confirm every one passes before closing; closing any subset leaves the others live.
