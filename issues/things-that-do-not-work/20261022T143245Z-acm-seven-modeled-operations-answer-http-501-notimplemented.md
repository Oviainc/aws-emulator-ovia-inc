Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.export-non-exportable-certificate-types-rejected acm.op-contract-exportCertificate acm.tag-resource-nonexistent-arn-rejected acm.tag-resource-nonexistent-resource-rejected acm.untag-resource-nonexistent-arn-rejected acm.renew-certificate-concurrent-request-in-progress acm.describe-nonexistent-acme-account-rejected acm.list-acme-endpoints-throttling-enforced acm.get-credentials-nonexistent-binding-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.export-non-exportable-certificate-types-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException for a default AMAZON_ISSUED cert, got {code!r}"
Issue-Key: acm.op-contract-exportCertificate|run#7|AssertionError|assert code_pending == "ResourceNotFoundException", (
Issue-Key: acm.tag-resource-nonexistent-arn-rejected|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: acm.tag-resource-nonexistent-resource-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: acm.untag-resource-nonexistent-arn-rejected|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: acm.renew-certificate-concurrent-request-in-progress|run#10|AssertionError|assert all(code == "ResourceNotFoundException" for code in codes), (
Issue-Key: acm.describe-nonexistent-acme-account-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: acm.list-acme-endpoints-throttling-enforced|run#11|AssertionError|assert all(code == "ThrottlingException" for code in throttled_codes), (
Issue-Key: acm.get-credentials-nonexistent-binding-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# ACM: seven modeled operations answer HTTP 501 NotImplemented (scaffold stubs)

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportCertificate

## Description
Seven operations the ACM Smithy model declares are still scaffold stubs: their handler returns `crate::scaffolded::Scaffolded::not_implemented(..)`, so every call answers HTTP 501 with error code `NotImplemented` instead of the modeled success shape or the modeled error. Each one is a separate handler file and therefore a separate cause. (The suite already has filed stub issues for ImportCertificate, CreateAcmeEndpoint, PutAccountConfiguration, GetAccountConfiguration, RemoveTagsFromCertificate, ResendValidationEmail, RevokeCertificate, SearchCertificates, UpdateCertificateOptions and ListCertificateDomainValidations — those are NOT repeated here. The seven below are not covered by any open issue.)

Cause 1 of 7: ExportCertificate.
AWS: Smithy ACM (awsJson1_1) models ExportCertificate(CertificateArn required, Passphrase required @sensitive len 4..128) -> {Certificate, CertificateChain, PrivateKey}, errors InvalidArnException, RequestInProgressException, ResourceNotFoundException, ThrottlingException, ValidationException (all HTTP 400). Exporting a default AMAZON_ISSUED (non-exportable) certificate is refused with 400 ValidationException; exporting a PENDING_VALIDATION certificate is likewise a modeled 400.
Emulator: HTTP 501, Error.Code 'NotImplemented', message 'operation ExportCertificate is scaffolded but not implemented in this emulator'.
Why: a client cannot distinguish "this certificate may not be exported" from "the service is broken"; retry logic treats 501 as a server fault and SDK error handling never sees the modeled ValidationException.
Fix: the operation's handler — services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs (replace the not_implemented Err and delete the SCAFFOLDED marker).
Traces: acm.export-non-exportable-certificate-types-rejected, acm.op-contract-exportCertificate.

Cause 2 of 7: TagResource.
AWS: Smithy models TagResource(ResourceArn required, Tags required TagList len 1..50) -> Unit, errors ResourceNotFoundException, ServiceQuotaExceededException, ValidationException. Tagging a non-existent ACM resource ARN is refused with 400 ResourceNotFoundException.
Emulator: HTTP 501 'NotImplemented'.
Why: tag-management tooling sees a server error on every call, and the negative contract (not-found) is unobservable.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/tag_resource.rs.
Traces: acm.tag-resource-nonexistent-arn-rejected, acm.tag-resource-nonexistent-resource-rejected.

Cause 3 of 7: UntagResource.
AWS: Smithy models UntagResource(ResourceArn, TagKeys) -> Unit, errors ResourceNotFoundException, ValidationException; an unknown resource ARN is 400 ResourceNotFoundException.
Emulator: HTTP 501 'NotImplemented'.
Why: the same as Cause 2 for tag removal.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/untag_resource.rs.
Traces: acm.untag-resource-nonexistent-arn-rejected.

Cause 4 of 7: RenewCertificate.
AWS: Smithy models RenewCertificate(CertificateArn required) -> Unit, errors InvalidArnException, RequestInProgressException, ResourceNotFoundException, ValidationException. Concurrent renews of an ARN that does not exist each answer 400 ResourceNotFoundException.
Emulator: HTTP 501 'NotImplemented' for all calls.
Why: renewal automation cannot tell a missing certificate from an outage, and the RequestInProgressException concurrency contract is untestable.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/renew_certificate.rs.
Traces: acm.renew-certificate-concurrent-request-in-progress.

Cause 5 of 7: DescribeAcmeAccount.
AWS: Smithy models DescribeAcmeAccount(AcmeEndpointArn, AccountUrl) -> DescribeAcmeAccountResponse, errors AccessDeniedException, InternalServerException, ResourceNotFoundException, ThrottlingException, ValidationException; an unknown ACME account/endpoint answers 400 ResourceNotFoundException.
Emulator: HTTP 501 'NotImplemented'.
Why: ACME account introspection is unavailable and the not-found contract is lost.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/describe_acme_account.rs.
Traces: acm.describe-nonexistent-acme-account-rejected.

Cause 6 of 7: ListAcmeEndpoints.
AWS: Smithy models ListAcmeEndpoints(MaxResults, NextToken) -> ListAcmeEndpointsResponse, errors AccessDeniedException, InternalServerException, ThrottlingException, ValidationException; a burst of calls is either served or refused with ThrottlingException, never 501.
Emulator: HTTP 501 'NotImplemented' (the check observed the rejected-call code set {'NotImplemented'} where only 'ThrottlingException' is admissible).
Why: endpoint enumeration is impossible, and clients that back off only on ThrottlingException retry-storm or fail outright.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/list_acme_endpoints.rs.
Traces: acm.list-acme-endpoints-throttling-enforced.

Cause 7 of 7: GetAcmeExternalAccountBindingCredentials.
AWS: Smithy models GetAcmeExternalAccountBindingCredentials(AcmeExternalAccountBindingArn) -> credentials, errors AccessDeniedException, InternalServerException, ResourceNotFoundException, ThrottlingException, ValidationException; an unknown binding ARN answers 400 ResourceNotFoundException.
Emulator: HTTP 501 'NotImplemented'.
Why: EAB credential retrieval — the only way an ACME client gets its key id/MAC key — is unavailable, and the not-found contract is lost.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs.
Traces: acm.get-credentials-nonexistent-binding-rejected.

## Evidence
All seven surfaced as HTTP 501 / Error.Code 'NotImplemented' where a 200 or a modeled 400 is required.
- acm.export-non-exportable-certificate-types-rejected: AssertionError: expected ValidationException for a default AMAZON_ISSUED cert, got 'NotImplemented'.
- acm.op-contract-exportCertificate: AssertionError: expected ResourceNotFoundException for pending cert, got NotImplemented.
- acm.tag-resource-nonexistent-arn-rejected: AssertionError: expected ResourceNotFoundException for non-existent resource ARN, got 'NotImplemented'.
- acm.tag-resource-nonexistent-resource-rejected: AssertionError: expected ResourceNotFoundException, got NotImplemented.
- acm.untag-resource-nonexistent-arn-rejected: AssertionError: expected ResourceNotFoundException for non-existent resource ARN, got 'NotImplemented'.
- acm.renew-certificate-concurrent-request-in-progress: AssertionError: expected all 5 concurrent renew_certificate calls to fail with ResourceNotFoundException, got ['NotImplemented' x5].
- acm.describe-nonexistent-acme-account-rejected: AssertionError: expected ResourceNotFoundException for unknown ACME account/endpoint, got 'NotImplemented'.
- acm.list-acme-endpoints-throttling-enforced: AssertionError: expected only ThrottlingException among rejected calls, got {'NotImplemented'}.
- acm.get-credentials-nonexistent-binding-rejected: AssertionError: expected ResourceNotFoundException for unknown binding, got 'NotImplemented'.
Source confirmation in the checkout: services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs and .../tag_resource.rs both carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and end their handle() with Err(crate::scaffolded::Scaffolded::not_implemented(...)).

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Call each of ExportCertificate, TagResource, UntagResource, RenewCertificate, DescribeAcmeAccount, ListAcmeEndpoints, GetAcmeExternalAccountBindingCredentials with any well-formed input (e.g. acm.export_certificate(CertificateArn=<an existing AMAZON_ISSUED cert>, Passphrase=b'Sim2RealPass123')).
3. Observe HTTP 501 with Error.Code 'NotImplemented' instead of the modeled 200 or modeled 400 error.
4. Re-replay the nine checks in trace_ids to confirm.
