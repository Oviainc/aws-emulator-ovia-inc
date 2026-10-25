Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one
Issue-Key: acm__apigateway|DeleteDomainName|NotImplemented
Issue-Commit: 3fffece81be5f9cdcba63f22f8252b41b774e64d

# API Gateway DeleteDomainName is still an unimplemented 501 scaffold stub, so a custom domain can never be deleted (new blocker now that ACM ImportCertificate answers)

> _Note: this issue was identified against the code base at commit `3fffece81be5f9cdcba63f22f8252b41b774e64d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomainName

## Description
ONE root cause.

Cause 1 of 1: apigateway DeleteDomainName answers HTTP 501 NotImplemented.
AWS: Smithy com.amazonaws.apigateway#DeleteDomainName — DELETE /domainnames/{domainName} with input domainName (required) and optional domainNameId, success HTTP 202 with no body ('Deletes the DomainName resource'). Its modeled errors are BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429) and UnauthorizedException (401); deleting a domain name that does not exist is a 404 NotFoundException, and deleting one that still has a dependent base-path mapping is a 409 ConflictException. There is no circumstance in which AWS answers 5xx for a well-formed DeleteDomainName.
Emulator: HTTP 501 with Error.Code 'NotImplemented' and message 'operation DeleteDomainName is scaffolded but not implemented in this emulator'. Confirmed in the checkout: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_domain_name.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker (line 4) and its handle() ends with Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDomainName")) (line 184), with SUCCESS_STATUS wired to the modeled 202.
Why: a custom domain, once created, can never be removed. Every client or test that provisions one leaks it, a re-run collides with the leftover on CreateDomainName's duplicate conflict, and the ACM certificate the domain pins can never be released — so ACM DeleteCertificate keeps refusing it as in use. 501 is a server fault to every SDK, so the caller retries and backs off against what is really a permanent condition instead of surfacing a client error.
Fix: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_domain_name.rs — implement handle() (202 on success, NotFoundException for an unknown domainName, ConflictException while a base-path mapping or access association still depends on it) and delete the SCAFFOLDED marker line.
Related open issue: 20261023T151100Z-apigateway-api-gateway-putrestapi-overwrite-imports-paths-but-no.md lists this check id in its trace_ids, but its replay keys name 'ImportCertificate|NotImplemented' and 'CreateVpcEndpoint|NotImplemented'. ACM ImportCertificate now answers, so the check gets past that blocker and dies further along on DeleteDomainName — a cause that issue does not name and whose replay key it does not carry. This issue records the new blocker.
Traces: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one.

## Evidence
From the behavioral replay (suite simparity-behavioral-ga-prod-v1), acm__apigateway 0/1 passed.

acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteDomainName operation: operation DeleteDomainName is scaffolded but not implemented in this emulator. Expected: HTTP 202 with an empty body (and 404 NotFoundException for an unknown domain name, 409 ConflictException while a dependent mapping exists).

Source confirmation in the checkout: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_domain_name.rs line 4 '//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented ...' and line 184 'Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDomainName"))'.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2).
2. Import a self-signed certificate into ACM and apigateway.create_domain_name(domainName='<some fqdn>', regionalCertificateArn=<that arn>, endpointConfiguration={'types': ['REGIONAL']}).
3. apigateway.delete_domain_name(domainName='<that fqdn>') — observe HTTP 501 Error.Code 'NotImplemented' instead of HTTP 202 with an empty body.
4. apigateway.get_domain_name(domainName='<that fqdn>') still resolves, and acm.delete_certificate(CertificateArn=<that arn>) refuses the certificate as in use, because the domain can never be removed.
5. Re-replay the check id in trace_ids to confirm.
