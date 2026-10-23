Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate acm__elbv2.acm-certificate-in-use-by-a-listener-cannot-be-deleted-rejected acm__elbv2.acm-inuseby-clears-only-after-the-last-listener-detaches acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected acm__elbv2.https-listener-without-a-certificate-rejected acm__elbv2.live-listener-certificate-rotation-swaps-the-presented-leaf-with-no-dropped-request cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui elbv2.add-listener-certificates-and-describe elbv2.add-listener-certificates-malformed-arn-rejected elbv2.deploy-trust-store-cfn-association-enforced elbv2.e2e-mtls-trust-store-listener-flow elbv2.e2e-mtls-truststore-revocation-flow elbv2.elbv2-https-listener-with-cert elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected elbv2.https-to-http-redirect-rejected elbv2.listener-certificate-add-describe-remove-lifecycle elbv2.listener-certificate-empty-vs-absent-distinction elbv2.remove-listener-certificate-removes-entry elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni
Issue-Key: acm__elbv2|ImportCertificate|NotImplemented
Issue-Key: cognito-idp__elbv2|ImportCertificate|NotImplemented
Issue-Key: elbv2|ImportCertificate|NotImplemented
Issue-Key: elbv2__s3|ImportCertificate|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# ACM ImportCertificate 501 stub makes the whole ELBv2 HTTPS / listener-certificate / mTLS surface unreachable (21 checks)

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: acm ImportCertificate answers HTTP 501 NotImplemented, so no check that needs a server certificate can even reach ELBv2.

AWS: Smithy (com-amazonaws-acm-certificatemanager) models ImportCertificate with required Certificate (blob, @length 1..32768) and PrivateKey (blob, @length 1..5120, @sensitive), optional CertificateChain and Tags, output CertificateArn (@pattern ^arn:...:acm:...), and client errors ConflictException / InvalidArnException / InvalidParameterException / InvalidTagException / LimitExceededException / ResourceNotFoundException / TagPolicyException / TooManyTagsException / ValidationException (all 400). A well-formed self-signed RSA-2048 leaf + key is imported successfully and returns a certificate ARN usable as an ELBv2 listener Certificates[].CertificateArn.

Emulator: every ImportCertificate call is answered `NotImplemented` — "operation ImportCertificate is scaffolded but not implemented in this emulator" (HTTP 501). 501/NotImplemented is not a modeled ACM error, so botocore surfaces it as an unretriable ClientError and the check dies on its first setup call.

Why: a client cannot bring its own certificate at all. Every ALB/NLB TLS path collapses: creating an HTTPS listener, AddListenerCertificates / RemoveListenerCertificates / DescribeListenerCertificates ordering and empty-vs-absent semantics, mTLS trust stores bound to a listener, HTTPS->HTTP redirect rejection, access logs recording the negotiated TLS/SNI and chosen_cert_arn, ALB authenticate-cognito redirects, and ACM's own InUseBy / delete-while-in-use behaviour. 21 replayed checks never exercise the behaviour they grade.

Fix: implement the ImportCertificate handler in the acm service crate (its ops module) plus the certificate state it writes — the imported cert must be retrievable via DescribeCertificate (Type=IMPORTED, Status=ISSUED) and referenceable by ARN from elbv2 listeners, and InUseBy must list attached listener ARNs.

Related already-open issues that do NOT carry these replay keys and so do not cover them: 20261022T143245Z-acm-importcertificate-stub.md (the bare stub), 20261023T125319Z-acm-importcertificate-501-stub-makes-the-whole-apigatewayv2.md (same stub, apigatewayv2 surface), 20261022T235108Z-ec2-the-whole-ec2-client-vpn-and-enclave-certificate.md.

Traces: the 21 failing checks listed in trace_ids; each one's first failure is the same ClientError on ImportCertificate.

## Evidence
Expected: ImportCertificate(Certificate=<PEM leaf>, PrivateKey=<PEM PKCS#8 key>[, CertificateChain=<PEM chain>]) -> HTTP 200 with {"CertificateArn": "arn:aws:acm:<region>:<account>:certificate/<id>"}.
Actual: HTTP 501, Error.Code=NotImplemented, message (example) "operation ImportCertificate is scaffolded but not implemented in this emulator".
Observed in the replay as: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation.

## Steps to Reproduce
1. Mint a self-signed RSA-2048 certificate/key pair locally (any X.509 leaf is enough).
2. Call acm ImportCertificate with Certificate=<leaf PEM> and PrivateKey=<key PEM>.
3. Observe HTTP 501 / NotImplemented instead of a 200 carrying CertificateArn.
4. Re-replay e.g. acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate or elbv2.elbv2-https-listener-with-cert.
