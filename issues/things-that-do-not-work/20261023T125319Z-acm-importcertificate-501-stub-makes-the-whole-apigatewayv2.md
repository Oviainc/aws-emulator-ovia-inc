Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it apigatewayv2.api-mapping-duplicate-path-rejected apigatewayv2.api-mapping-key-collision-rejected apigatewayv2.api-mapping-key-conflict-rejected apigatewayv2.api-mapping-key-non-ascii-rejected apigatewayv2.api-mapping-lifecycle apigatewayv2.api-mapping-lifecycle-crud apigatewayv2.api-mapping-path-special-chars-round-trip apigatewayv2.concurrent-api-mapping-same-path-conflict apigatewayv2.concurrent-api-mapping-update-conflict apigatewayv2.concurrent-domain-name-update-conflict apigatewayv2.create-api-mapping-duplicate-path-conflict apigatewayv2.create-api-mapping-malformed-path-rejected apigatewayv2.create-domain-name-duplicate-bad-request apigatewayv2.create-routing-rule-missing-condition-rejected apigatewayv2.delete-domain-name-twice-not-idempotent-404 apigatewayv2.deploy-api-mapping-cfn-behaves apigatewayv2.deploy-domain-name-cfn-behaves apigatewayv2.domain-name-lifecycle apigatewayv2.domain-name-lifecycle-crud-transitions apigatewayv2.domain-name-mtls-ownership-verification-enforced apigatewayv2.domain-name-security-policy-server-enforced apigatewayv2.e2e-custom-domain-mapping-invoke-flow apigatewayv2.list-domain-names-reflects-created apigatewayv2.list-routing-rules-reflects-created apigatewayv2.put-routing-rule-nonexistent-not-found apigatewayv2.routing-rule-lifecycle-crud apigatewayv2.routing-rule-priority-numeric-precision-preserved apigatewayv2.routing-rule-priority-numeric-precision-round-trip apigatewayv2.routing-rule-priority-out-of-range-rejected
Issue-Key: acm__apigatewayv2|ImportCertificate|NotImplemented
Issue-Key: apigatewayv2|ImportCertificate|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# ACM ImportCertificate 501 stub makes the whole apigatewayv2 custom-domain / API-mapping / routing-rule surface unreachable (30 checks)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
Cause 1 of 1: ACM `ImportCertificate` answers NotImplemented, and every apigatewayv2 custom-domain behaviour depends on it.

AWS: ACM `ImportCertificate` (Smithy: ImportCertificate -> ImportCertificateResponse.CertificateArn; modeled errors ResourceNotFoundException, LimitExceededException, InvalidTagException, TooManyTagsException, TagPolicyException, InvalidParameterException, InvalidArnException) imports a caller-supplied certificate/private key pair and returns the new certificate ARN. There is no NotImplemented error in the operation's error list, and no AWS code path returns one. Every apigatewayv2 check that needs a REGIONAL custom domain mints a self-signed certificate with openssl, imports it with ImportCertificate, and passes the returned ARN to CreateDomainName.

Emulator: the first ImportCertificate call fails with HTTP error code `NotImplemented` and message `operation ImportCertificate is scaffolded but not implemented in this emulator`, so the check dies before it reaches any apigatewayv2 assertion. This single stub is the FIRST blocker for 30 behavioural checks in this suite, covering four otherwise-unexercised apigatewayv2 areas: DomainName lifecycle/CRUD/mTLS/security-policy, ApiMapping create/update/delete and its path-collision rules, RoutingRule CRUD and priority handling, and the CFN deploy paths for domains and mappings.

Why: a client that imports a certificate and then builds a regional custom domain (the normal way an HTTP/WebSocket API gets a stable hostname) cannot get past step one — it sees a 501-class error for a modeled, always-available control-plane call. Because this stub fails first, the whole downstream apigatewayv2 custom-domain surface is also unverified; note that `CreateDomainName`/`UpdateDomainName`/`DeleteDomainName` are separately-filed stubs, so clearing ImportCertificate is necessary but not sufficient for these checks to pass.

Related already-open issue: `20261022T143245Z-acm-importcertificate-stub.md` names the same operation but carries none of the replay keys below, so re-replaying it would not re-run any of these 30 apigatewayv2 checks; this issue records that linkage. `20261022T235108Z-ec2-the-whole-ec2-client-vpn-and-enclave-certificate.md` notes the same stub from the EC2 side.

Fix: implement the ImportCertificate handler and its state in the acm service crate (certificate import: accept Certificate/PrivateKey/CertificateChain, mint and return a CertificateArn, make the imported certificate visible to DescribeCertificate/ListCertificates and usable as an apigatewayv2 DomainName CertificateArn).

Traces: the 30 failing check ids in trace_ids; all show the identical final line `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation`.

## Evidence
Expected (AWS): ImportCertificate returns 200 with a CertificateArn such as arn:aws:acm:<region>:<account>:certificate/<id>, which CreateDomainName then accepts.

Actual (emulator): HTTP error, Error.Code=`NotImplemented`, Error.Message=`operation ImportCertificate is scaffolded but not implemented in this emulator` (message quoted as the recorded example). Observed identically as the first failing call in 30 checks, e.g.
- acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it
- apigatewayv2.domain-name-lifecycle
- apigatewayv2.api-mapping-lifecycle-crud
- apigatewayv2.routing-rule-lifecycle-crud

None of these checks was marked ALREADY FILED or OWNED by the replay report.

## Steps to Reproduce
1. Generate a self-signed certificate + key (openssl req -x509 ... with a SAN).
2. Call acm:ImportCertificate(Certificate=<pem>, PrivateKey=<key>).
3. Observe NotImplemented instead of a CertificateArn.
4. Re-replay the checks listed in trace_ids to confirm the fix.
