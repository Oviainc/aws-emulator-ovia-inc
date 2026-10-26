Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-update-domain-name-single-winner appsync.deploy-domain-name-cfn-association-resolves appsync.domain-api-association-lifecycle appsync.domain-name-api-association-lifecycle appsync.domain-reassociation-conflict-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ImportCertificate|NotImplemented
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# ACM ImportCertificate is an unimplemented scaffold stub, blocking every AppSync custom-domain-name check

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
Cause 1 of 1: ACM ImportCertificate returns NotImplemented, so no certificate ARN can be obtained and every AppSync custom-domain-name behavior (CreateDomainName, domain/API association, reassociation conflict, concurrent UpdateDomainName, CFN AWS::AppSync::DomainName) is unreachable. The failing checks are AppSync checks (hence the appsync service token taken from their check ids), but the unimplemented operation lives in the ACM service crate.

AWS: ACM ImportCertificate accepts a PEM certificate/private key (and optional chain) and returns a CertificateArn, which AppSync CreateDomainName then requires as `certificateArn`. Recorded behavior only for the specific PEM material these checks import; the contract at issue is simply that ImportCertificate succeeds and returns a usable certificate ARN rather than an unmodeled error.
Emulator: the call fails with error code `NotImplemented` and the message "operation ImportCertificate is scaffolded but not implemented in this emulator" — i.e. the operation is a scaffold stub. `NotImplemented` is not in ACM's modeled error set for ImportCertificate, so it is also not a code any SDK client can handle.
Why: a client cannot set up any AppSync custom domain, and five distinct AppSync behaviors cannot be exercised at all. The failure also masks whatever the AppSync domain-name operations themselves do (see the companion issue about those stubs), so the two must be fixed together to make these checks meaningful.
Fix: the ACM ImportCertificate handler — services/service-com-amazonaws-acm-certificatemanager/src/ops/import_certificate.rs — and the certificate state it must record so that DescribeCertificate/the AppSync domain-name path can resolve the returned ARN. No open issue names this operation.
Traces: appsync.concurrent-update-domain-name-single-winner, appsync.deploy-domain-name-cfn-association-resolves, appsync.domain-api-association-lifecycle, appsync.domain-name-api-association-lifecycle, appsync.domain-reassociation-conflict-rejected.

## Evidence
Expected: ImportCertificate returns a CertificateArn usable as AppSync CreateDomainName `certificateArn`. Actual, in all five checks at the setup call: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation: operation ImportCertificate is scaffolded but not implemented in this emulator`. None of these five checks is marked ALREADY FILED/OWNED in the replay report, and no open issue names ImportCertificate.

## Steps to Reproduce
1. Call acm ImportCertificate(Certificate=<PEM>, PrivateKey=<PEM key>) — it fails with error code NotImplemented.
2. Consequently appsync CreateDomainName(domainName=..., certificateArn=<arn from step 1>) can never be reached, which is where all five checks stop.
