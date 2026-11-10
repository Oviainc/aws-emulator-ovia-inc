Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates
Issue-Key: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# acm__apigateway__s3: the mutual-TLS REGIONAL custom-domain scenario is still unreachable — a DNS-validated ACM certificate never reaches ISSUED after its validation CNAME is published (1 cause)

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCertificate

## Description
One net-new root cause (Cause 1 of 1), grounded in one failing behavioral check that exhausts its whole wall clock inside its ACM setup, before any API Gateway custom domain, truststore or TLS handshake is exercised.

Cause 1 of 1: ACM never carries a DNS-validated certificate for a name in the account's own public hosted zone from PENDING_VALIDATION to ISSUED after the requester publishes exactly the CNAME DescribeCertificate handed out.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) RequestCertificate{DomainName required -> DomainNameString @length 1..253, ValidationMethod} -> CertificateArn; DescribeCertificate -> CertificateDetail.Status of CertificateStatus [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED], with per-domain DomainValidation.ValidationStatus (PENDING_VALIDATION | SUCCESS | FAILED) and DomainValidation.ResourceRecord (the CNAME record the requester adds to its DNS database for domain validation); CertificateDetail.FailureReason of the FailureReason enum documented as existing only when Status is FAILED. AWS behavior: while the challenge CNAME is absent the certificate stays PENDING_VALIDATION (becoming VALIDATION_TIMED_OUT only after 72h); once the published CNAME resolves to the handed-out value, ValidationStatus becomes 'SUCCESS' and Status becomes 'ISSUED'. 'FAILED' is reserved for a definite refusal and must be explained by a modeled FailureReason — a correctly published CNAME for a sub-name of the account's own public hosted zone is not one of those cases.

Emulator: the certificate never reaches ISSUED. The check's _issue_public_certificate helper calls acm.request_certificate(DomainName='<label>.<baseline apex>', ValidationMethod='DNS'), polls DescribeCertificate for DomainValidationOptions[0].ResourceRecord, CREATEs exactly that Name/Type/Value in the tagged public zone via route53.change_resource_record_sets, then polls DescribeCertificate for Status == 'ISSUED' with a 300s budget — and the check burns its entire wall clock instead ("Error: Check timed out after 60.0s"). A previous replay of this same check reached the poll's own deadline and reported "Last result: 'FAILED'", i.e. the emulator settles this certificate on terminal FAILED with no modeled FailureReason rather than issuing it. Hypothesis: the DNS-validation resolver that should match the published CNAME against the hosted zone and drive PENDING_VALIDATION -> ISSUED never matches, and the state machine's only non-pending outcome is FAILED.

Why: every scenario that needs an AWS-issued, DNS-validated certificate is unreachable, and a terminal FAILED is worse than a stall — a client or IaC tool that correctly waits on Status sees a definite, non-retryable failure for a certificate AWS would have issued, with no FailureReason to explain it, so it aborts or tears down instead of proceeding. Concretely it blocks this check's whole subject: an API Gateway REGIONAL custom domain with mutualTlsAuthentication pointing at an S3-hosted truststore must serve 200 (with the stage's mock body) to a SigV4-signed request presenting a client certificate the truststore trusts, must refuse a request presenting NO client certificate at the TLS layer (a reset, a closed connection, or a certificate-required alert — never an HTTP response), must refuse a certificate signed by an untrusted CA with HTTP 403, body {"message":"Forbidden"} and header x-amzn-ErrorType: ForbiddenException, and must keep serving the trusted client afterwards. None of that is exercised.

Fix: the ACM DNS-validation state machine in the acm service crate — the RequestCertificate handler's validation driver (the resolver that checks the published challenge CNAME against Route 53) and the CertificateDetail projection DescribeCertificate reads. A matching published record must take the per-domain ValidationStatus to 'SUCCESS' and Status to 'ISSUED', and the state machine must never settle on 'FAILED' without a modeled FailureReason.

Traces: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates.

=== Related open issues that do NOT carry this replay key ===
20261109T132410Z-acm-apigateway-s3-a-dns-validated-acm-certificate.md names this same check id, but keys it to the helper's _poll_until AssertionError ("Last result: 'FAILED'"); the check no longer fails that way — it now exhausts its wall clock without ever reaching that assertion — so the suite reports it as unowned and the defect is still live. If that filing was treated as fixed, this is the same stage still refusing to issue: the fix left the certificate unable to reach ISSUED. 20261029T093731Z-acm-dns-validated-certificate-issuance-is-unreachable-the.md covers hosted-zone discoverability under the 'acm' heading, and 20261101T214920Z-appsync-custom-domain-checks-are-all-blocked-upstream.md records the same observable for AppSync; neither is filed under this service heading nor carries this check id.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: acm__apigateway__s3 0/1 (0%), 1 failed.

[mismatch] acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates
  Error: Check timed out after 60.0s

The blocking step is the check's ACM setup helper _issue_public_certificate(ctx, acm, route53, zone_id, domain):
  cert_arn = acm.request_certificate(DomainName='<label>.<baseline apex>', ValidationMethod='DNS')['CertificateArn']
  _poll_until(... describe_certificate(...)['Certificate'].get('DomainValidationOptions',[{}])[0].get('ResourceRecord') is not None, timeout_s=60, interval_s=3)
  route53.change_resource_record_sets(HostedZoneId=<baseline zone>, Changes=[{'Action':'CREATE','ResourceRecordSet':{'Name':record['Name'],'Type':record['Type'],'TTL':60,'ResourceRecords':[{'Value':record['Value']}]}}])
  _poll_until(lambda: describe_certificate(...)['Certificate']['Status'], lambda s: s == 'ISSUED', timeout_s=300, interval_s=10, description="the certificate for '<domain>' reaching ISSUED via DNS validation")
Expected (AWS): PENDING_VALIDATION -> ISSUED (and DomainValidationOptions[0].ValidationStatus 'SUCCESS') once the published CNAME matches. Actual: the ISSUED status is never observed and the check exhausts its wall clock; a prior replay of this same check reported the poll's own failure as "not met within 300s. Last result: 'FAILED'".

Unreached as a consequence: apigw.create_domain_name(domainName=<domain>, endpointConfiguration={'types':['REGIONAL']}, regionalCertificateArn=<cert>, securityPolicy='TLS_1_2', mutualTlsAuthentication={'truststoreUri':'s3://<bucket>/truststore.pem','truststoreVersion':<version>}) reaching domainNameStatus 'AVAILABLE' and publishing a regionalDomainName; then (1) a SigV4-signed GET presenting a truststore-trusted client certificate served HTTP 200 with the stage's mock body; (2) a request with no client certificate refused at the TLS layer (connection reset, closed with no response, or a certificate-required alert — never an HTTP status); (3) a client certificate from an untrusted CA refused with HTTP 403, body {"message":"Forbidden"} and header x-amzn-ErrorType: ForbiddenException; (4) the trusted client served 200 again afterwards.

Per-run values (the generated label and baseline apex, the bucket name, object VersionId, certificate/API ids, the mock body's marker) are not requirements; the requirements are the modeled status tokens ('ISSUED', per-domain 'SUCCESS') and, downstream, the 403 + {"message":"Forbidden"} + x-amzn-ErrorType: ForbiddenException refusal envelope and the TLS-layer refusal with no HTTP response.

Smithy consulted: acm RequestCertificate, DescribeCertificate, CertificateStatus enum, FailureReason enum, DomainValidation.{ValidationStatus,ResourceRecord}.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay suite simparity-behavioral-aws-v1; acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates reproduces as "Check timed out after 60.0s".
2. Resolve the account's tagged public hosted zone (route53 ListHostedZones + ListTagsForResource) and take its apex and zone id.
3. acm.request_certificate(DomainName='<label>.<apex>', ValidationMethod='DNS'); poll acm.describe_certificate until DomainValidationOptions[0].ResourceRecord appears.
4. route53.change_resource_record_sets(HostedZoneId=<zone>, Changes=[CREATE of exactly that ResourceRecord Name/Type/Value, TTL 60]).
5. Poll acm.describe_certificate for Status. Observed: 'ISSUED' never appears (a prior run observed it settle on 'FAILED' with no FailureReason). Expected (AWS): DomainValidationOptions[0].ValidationStatus 'SUCCESS' and Status 'ISSUED'.
6. With an ISSUED certificate, create the REGIONAL custom domain with mutualTlsAuthentication over an S3 truststore object and a base path mapping to a deployed MOCK stage, wait for domainNameStatus 'AVAILABLE', then verify the three mTLS outcomes: trusted client certificate -> 200 with the stage body; no client certificate -> refusal at the TLS layer with no HTTP response; untrusted-CA client certificate -> 403 with body {"message":"Forbidden"} and x-amzn-ErrorType: ForbiddenException; then the trusted client served 200 again.
7. Re-replay the check id in trace_ids to confirm the fix.
