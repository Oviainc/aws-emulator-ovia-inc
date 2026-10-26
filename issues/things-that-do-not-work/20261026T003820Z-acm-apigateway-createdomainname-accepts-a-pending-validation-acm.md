Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one
Issue-Key: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one|_refusal#23|AssertionError|raise AssertionError(
Issue-Commit: 4d43e6f6a9670b66172d8d2a614a41a4fbd2c288

# acm__apigateway: CreateDomainName accepts a PENDING_VALIDATION ACM certificate for a REGIONAL custom domain instead of refusing with BadRequestException (1 cause)

> _Note: this issue was identified against the code base at commit `4d43e6f6a9670b66172d8d2a614a41a4fbd2c288`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Cause 1 of 1: apigateway's CreateDomainName does not gate `regionalCertificateArn` on the referenced ACM certificate's state. A certificate still in PENDING_VALIDATION is accepted and a custom domain is created, where AWS refuses the call. Failing check: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one.

AWS: API Gateway requires the certificate named by `regionalCertificateArn` to be in ACM status ISSUED at the moment of the create. Smithy (com.amazonaws.apigateway, CreateDomainName): input member `regionalCertificateArn` (String), `endpointConfiguration` (EndpointConfiguration), `securityPolicy` (SecurityPolicy); modeled errors BadRequestException (httpError 400), ConflictException (409), LimitExceededException (429), TooManyRequestsException (429), UnauthorizedException (401); success is HTTP 201. Smithy (com.amazonaws.acm): CertificateStatus distinguishes PENDING_VALIDATION from ISSUED. The model does not itself state the cross-service precondition, so for the refusal itself this is recorded behavior only: the recording shows HTTP 400 with error code `BadRequestException` and a message whose fixed text is "Certificate is not in an ISSUED state, and cannot be used" — it names the certificate STATE and carries no variable part (notably NOT the certificate ARN; the check was recorded against real AWS and corrected on exactly that point, H2 in the check header). That wording is what keeps this refusal distinguishable from the malformed-ARN refusal (a format failure, covered by apigateway/check_create_domain_name_malformed_certificate_arn_rejected.py) and from an authorization refusal.

Emulator: the identical CreateDomainName call with the PENDING_VALIDATION certificate SUCCEEDS. The replay fails at beat 3 with "AssertionError: the call was expected to be REFUSED and instead succeeded; an over-permissive target must not be able to pass this check by accepting the request" — the emulator never raises a ClientError, so no status, code or message is produced to compare. The surrounding beats show the emulator is otherwise on the right path: beat 1's positive control (the same call with an ISSUED imported certificate) succeeds and echoes `regionalCertificateArn`, so the handler stores the ARN without ever resolving it against ACM state.

Why: a client relying on AWS's behavior loses the only guard that stops a non-servable custom domain from being created. On AWS, requesting a DNS-validated certificate and wiring it into a custom domain before validation completes fails fast at CreateDomainName with a 400 that names the state, so deployment tooling (CloudFormation-style ordering, CI promotion gates, scripts that poll ACM before attaching) learns it must wait for ISSUED. Against the emulator the create returns 201 and the domain is listed by GetDomainNames, so the ordering bug is masked in test and only appears in production. Code-path coverage is lost too: negative tests that assert the BadRequestException, and tests that distinguish a state refusal from a malformed-ARN refusal by message, both pass vacuously.

Fix: the CreateDomainName handler's validation in the apigateway crate (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_domain_name.rs) — before any domain record is stored, resolve the certificate referenced by `regionalCertificateArn` (and the EDGE equivalent `certificateArn`) in ACM state and reject with BadRequestException / HTTP 400 and the message above unless its status is ISSUED; the refusal must be ordered after ARN-format validation so a malformed ARN still produces the existing format refusal, and it must leave no domain behind (beat 4 asserts GetDomainNames does not list the domain after the refused create). This is distinct from open issue 20261025T210904Z-acm-apigatewayv2-a-regional-custom-domain-never-presents.md, which reports the absent custom-domain TLS data plane in apigatewayv2; that issue is about serving the certificate, this one is about refusing an unusable one at the control plane in apigateway v1, and neither covers the other.

Traces: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one (mismatch — AssertionError at _refusal, the refused call succeeded).

## Evidence
Expected (AWS): with an ACM certificate whose DescribeCertificate reports Status=PENDING_VALIDATION, the call
  apigateway:CreateDomainName(domainName=<domain>, regionalCertificateArn=<pending-arn>, endpointConfiguration={types:[REGIONAL]}, securityPolicy=TLS_1_2)
is refused with HTTP 400, Error.Code = "BadRequestException", and Error.Message containing the fixed text "Certificate is not in an ISSUED state, and cannot be used" (no variable parts — the message names the state, not the ARN). GetDomainNames must not list <domain> afterwards. The identical call with an ISSUED certificate must succeed (HTTP 201) and echo regionalCertificateArn.

Actual (emulator): the call with the PENDING_VALIDATION certificate returns success — no ClientError at all. Replay verdict: mismatch, with
  File "check_acm__apigateway_regional_custom_domain_refuses_a_pending_certificate_then_serves_the_issued_one.py", line 458, in run
    error = _refusal(lambda: _create(pending_arn))
  AssertionError: the call was expected to be REFUSED and instead succeeded; an over-permissive target must not be able to pass this check by accepting the request
Because nothing was raised, the three downstream assertions are never reached: code == "BadRequestException", "not in an ISSUED state" in the message, and the absent-effect read (<domain> not among GetDomainNames). The positive control immediately before it passed — the same create with the ISSUED imported certificate returned a record whose regionalCertificateArn equalled the ARN supplied — which localises the defect to the missing certificate-state precondition rather than to CreateDomainName generally.

## Steps to Reproduce
1. Mint a self-signed root CA and a leaf carrying DNS SAN = <domain> (a random label under a parent you control), then acm:ImportCertificate the leaf and poll acm:DescribeCertificate until Status=ISSUED -> issued_arn.
2. acm:RequestCertificate(DomainName=<domain>, ValidationMethod="DNS", IdempotencyToken=<token>) and do NOT validate it -> pending_arn. Confirm acm:DescribeCertificate reports Status=PENDING_VALIDATION.
3. Positive control: apigateway:CreateDomainName(domainName=<domain>, regionalCertificateArn=issued_arn, endpointConfiguration={"types":["REGIONAL"]}, securityPolicy="TLS_1_2"). Expect HTTP 201 with regionalCertificateArn == issued_arn. Then apigateway:DeleteDomainName(domainName=<domain>) and poll GetDomainNames until <domain> is gone, so the next call is a true CREATE.
4. Re-read acm:DescribeCertificate(CertificateArn=pending_arn) and assert Status is still PENDING_VALIDATION.
5. Issue the IDENTICAL CreateDomainName call, changing only regionalCertificateArn to pending_arn. Expect a ClientError: HTTP 400, Code="BadRequestException", Message containing "Certificate is not in an ISSUED state, and cannot be used". Observed: the call succeeds and a domain is created.
6. apigateway:GetDomainNames and assert <domain> is absent (the refused create must leave nothing behind).
7. Restore: repeat step 3's call with issued_arn and expect it to succeed again, proving the gate is state-specific and not a blanket refusal.
