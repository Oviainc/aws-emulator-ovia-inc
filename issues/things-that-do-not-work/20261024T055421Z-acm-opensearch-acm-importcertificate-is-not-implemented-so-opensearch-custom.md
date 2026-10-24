Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected acm__opensearch.half-specified-custom-endpoint-rejected
Issue-Key: acm__opensearch|ImportCertificate|NotImplemented
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# ACM ImportCertificate is not implemented, so OpenSearch custom-endpoint certificate validation cannot be exercised

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
One root cause.

Cause 1 of 1: ACM ImportCertificate returns NotImplemented.
AWS: ACM ImportCertificate accepts a Certificate/PrivateKey (optionally CertificateChain) and returns 200 with the new certificate's CertificateArn. Smithy: acm ImportCertificate -> ImportCertificateResponse{CertificateArn}; modeled errors are LimitExceededException, InvalidTagException, TooManyTagsException, TagPolicyException, InvalidParameterException, InvalidArnException, RequestInProgressException, ResourceNotFoundException. There is no NotImplemented error in the operation's error set and no 501 in its wire contract.
Emulator: the call fails with error code `NotImplemented` and the message 'operation ImportCertificate is scaffolded but not implemented in this emulator'. Both checks abort on the very first ACM call, before any OpenSearch request is made, so no CreateDomain custom-endpoint behavior is graded at all.
Why: a client that imports a certificate and then references its ARN from another service (here OpenSearch DomainEndpointOptions.CustomEndpointCertificateArn) cannot proceed. Any workflow whose first step is 'bring your own certificate' is blocked, and the whole class of cross-service certificate validation (does the certificate's SAN cover the requested custom hostname?) is unreachable.
Fix: the ImportCertificate handler in the ACM service crate, plus the certificate store it must write so DescribeCertificate/the ARN can be resolved by other services.
Traces: acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected, acm__opensearch.half-specified-custom-endpoint-rejected.

## Evidence
Expected: ImportCertificate -> HTTP 200 with {"CertificateArn": "arn:aws:acm:<region>:<account>:certificate/<id>"}.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation: operation ImportCertificate is scaffolded but not implemented in this emulator.

Both failing checks raise at the same point: the helper `_import_cert(ctx, alb_dns)` executed before any opensearch client call.

## Steps to Reproduce
1. Boot the emulator.
2. acm = boto3.client('acm'); acm.import_certificate(Certificate=<self-signed leaf PEM>, PrivateKey=<key PEM>) with a SAN covering an ALB DNS name.
3. Observe error code NotImplemented instead of a 200 carrying CertificateArn.
4. Replay acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected and acm__opensearch.half-specified-custom-endpoint-rejected.
