Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-endpoint-directory-url-immutable-server-generated acm.acme-endpoint-directory-url-managed-by-service acm.describe-acme-endpoint-returns-server-managed-urls acm.create-acme-domain-validation-generates-server-managed-dns-record acm.domain-validation-optional-field-empty-vs-absent-preserved acm.create-acme-domain-validation-for-unowned-endpoint-denied acm.concurrent-update-acme-domain-validation-no-lost-update acm.deploy-acme-endpoint-via-cloudformation-behaves
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.acme-endpoint-directory-url-immutable-server-generated|_assert_directory_url#7|AssertionError|assert _DIRECTORY_PATH_RE.fullmatch(parts.path) and not parts.query and not parts.fragment, (
Issue-Key: acm.acme-endpoint-directory-url-managed-by-service|_assert_directory_url#6|AssertionError|assert _DIRECTORY_PATH_RE.fullmatch(parts.path) and not parts.query and not parts.fragment, (
Issue-Key: acm.describe-acme-endpoint-returns-server-managed-urls|_assert_directory_url#11|AssertionError|assert _DIRECTORY_PATH_RE.fullmatch(parts.path) and not parts.query and not parts.fragment, (
Issue-Key: acm.create-acme-domain-validation-generates-server-managed-dns-record|run#9|AssertionError|assert first.get("PrevalidationType") == "DNS", (
Issue-Key: acm.domain-validation-optional-field-empty-vs-absent-preserved|run#17|AssertionError|assert described["PrevalidationType"] == "DNS", (
Issue-Key: acm.create-acme-domain-validation-for-unowned-endpoint-denied|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code} ({message!r})"
Issue-Key: acm.concurrent-update-acme-domain-validation-no-lost-update|run#13|AssertionError|assert codes == ["ValidationException", "ValidationException"], (
Issue-Key: acm|CreateStack|ValidationError
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# ACM ACME surface diverges: directory URL path, PrevalidationType value, cross-account endpoint rejection, the VALIDATING update guard, and the CloudFormation AcmeEndpoint resource type (5 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAcmeEndpoint

## Description
Five distinct root causes in ACM's ACME (acme-endpoint / acme-domain-validation) surface. Each is grounded in behavioral checks that pass on real AWS. Fix all five before submitting.

Cause 1 of 5: the server-minted ACME directory URL carries an extra '/acme' path segment.
AWS: Smithy AcmeEndpoint.EndpointUrl -> String, doc '<p>The URL of the ACME endpoint.</p>' (CreateAcmeEndpoint's request shape has NO URL-shaped member, so the value is purely service-computed). The model does not state the path, so: recorded behavior only - the URL is 'https://acm-acme-enroll.<region>.<domain>/<endpoint-id>/directory', i.e. the host part is acm-acme-enroll.<region>.<domain> and the path is exactly '/<id>/directory' with nothing before the id and no query or fragment.
Emulator: the host is correct but the path has an extra leading segment - '/acme/<uuid>/directory' (e.g. 'https://acm-acme-enroll.us-west-2.amazonaws.com/acme/2a23b7fd-df89-4e4c-9506-225c7c2f9d3d/directory'). The value is otherwise stable across Describes and unique per endpoint.
Why: an ACME client (certbot, lego, acme.sh) is handed this URL verbatim as its directory endpoint and derives every subsequent resource URL from its path; a client or test that parses the ACM-shaped path, or any fixture that matches it, diverges on every endpoint the emulator mints.
Fix: the EndpointUrl generator in the ACME-endpoint state/serializer of the ACM service crate (the value DescribeAcmeEndpoint/ListAcmeEndpoints/CreateAcmeEndpoint project).
Traces: acm.acme-endpoint-directory-url-immutable-server-generated, acm.acme-endpoint-directory-url-managed-by-service, acm.describe-acme-endpoint-returns-server-managed-urls.

Cause 2 of 5: AcmeDomainValidation.PrevalidationType is projected as the model's enum token instead of the value AWS returns on the wire.
AWS: Smithy AcmeDomainValidation.PrevalidationType -> PrevalidationType, an enum whose only declared value is 'DNS_PREVALIDATION'. Recorded behavior, however, is that a DNS-prevalidated domain validation comes back with PrevalidationType 'DNS' (the two behavioral checks below assert exactly that against real AWS), so the recording - not the enum token - is the wire requirement here; the model's token is the stale name.
Emulator: DescribeAcmeDomainValidation / CreateAcmeDomainValidation projections return PrevalidationType 'DNS_PREVALIDATION'.
Why: every client that branches on PrevalidationType ('DNS') takes the wrong branch and cannot find the DNS challenge record it is supposed to publish; the whole prevalidation flow silently falls through.
Fix: the AcmeDomainValidation response projection/serializer for the PrevalidationType member (emit the wire value AWS emits).
Traces: acm.create-acme-domain-validation-generates-server-managed-dns-record, acm.domain-validation-optional-field-empty-vs-absent-preserved.

Cause 3 of 5: CreateAcmeDomainValidation against an AcmeEndpointArn in ANOTHER account is refused with the wrong error class.
AWS: Smithy CreateAcmeDomainValidation declares both ResourceNotFoundException and ValidationException (both HTTP 400) plus AccessDeniedException (403). AcmeEndpointArn is constrained by pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$, so the account segment is part of the input the service validates. Recorded behavior: an ARN whose account segment is not the caller's own is rejected with HTTP 400 ValidationException (a request-validation verdict - the caller is told the ARN is not addressable by them, not that the resource is missing).
Emulator: HTTP 400 ResourceNotFoundException, message 'Could not find ACME endpoint arn:aws:acm:us-west-2:111122223333:acme-endpoint/<id>.' - i.e. the handler resolves the ARN in the local account's store, misses, and reports not-found without ever checking the account segment.
Why: the codes mean opposite things to a client - ResourceNotFoundException invites create-then-retry or treats the endpoint as deleted, while ValidationException says the ARN is malformed/not yours and must not be retried. Cross-account isolation also becomes untestable: nothing distinguishes 'someone else's endpoint' from 'no such endpoint'.
Fix: input validation in the CreateAcmeDomainValidation handler - compare the ARN's account segment to the caller's account and raise ValidationException before the state lookup.
Traces: acm.create-acme-domain-validation-for-unowned-endpoint-denied.

Cause 4 of 5: UpdateAcmeDomainValidation answers 501 NotImplemented, so the VALIDATING wrong-state guard is unobservable and concurrent updates have no defined outcome.
AWS: Smithy UpdateAcmeDomainValidation(AcmeDomainValidationArn required, PrevalidationOptions) -> Unit, @idempotent, errors AccessDeniedException (403), ConflictException, InternalServerException (500), ResourceNotFoundException, ThrottlingException, ValidationException (400). Recorded behavior: while the domain validation is still in Status VALIDATING the call is refused outright with HTTP 400 ValidationException, and two racing updates carrying mutually exclusive DomainScope values are BOTH refused with that same code, leaving PrevalidationDetails exactly at its pre-update value (no partial/torn write, no non-deterministic winner).
Emulator: both racing calls answer HTTP 501 with Error.Code 'NotImplemented' ('operation UpdateAcmeDomainValidation is scaffolded but not implemented in this emulator').
Why: a client cannot distinguish 'this resource is in the wrong state for an update' from 'the service is broken'; 501 is treated as a server fault and retried, and the state machine's no-lost-update guarantee cannot be relied on.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/update_acme_domain_validation.rs - implement the handler and its VALIDATING-state guard (reject with ValidationException, mutate nothing). An already-filed stub issue names this operation and file; this cause adds the state-machine contract that must hold once it is implemented, so close both together.
Traces: acm.concurrent-update-acme-domain-validation-no-lost-update.

Cause 5 of 5: CloudFormation does not know the AWS::CertificateManager::AcmeEndpoint resource type.
AWS: recorded behavior - CreateStack accepts a template whose Resources contain Type 'AWS::CertificateManager::AcmeEndpoint' (Properties AuthorizationBehavior / CertificateAuthority / Contact), drives the real CreateAcmeEndpoint on create and DeleteAcmeEndpoint on stack deletion, and the endpoint is describable through the ACM data plane while ACTIVE and genuinely gone after DELETE_COMPLETE.
Emulator: CreateStack is refused with HTTP 400 ValidationError, message 'Template format error: Unrecognized resource types: [AWS::CertificateManager::AcmeEndpoint]' - the stack is never created, so nothing downstream runs.
Why: any template that provisions an ACME endpoint cannot be deployed at all, and an infrastructure-as-code test that passes on AWS fails at template parse time.
Fix: register the AWS::CertificateManager::AcmeEndpoint resource type in the CloudFormation resource-type registry and wire its create/delete to ACM's CreateAcmeEndpoint/DeleteAcmeEndpoint (note DeleteAcmeEndpoint is itself a scaffold stub with its own open issue, 20261023T004835Z-acm-deleteacmeendpoint-stub.md).
Traces: acm.deploy-acme-endpoint-via-cloudformation-behaves.

## Evidence
Cause 1: acm.acme-endpoint-directory-url-immutable-server-generated and acm.acme-endpoint-directory-url-managed-by-service and acm.describe-acme-endpoint-returns-server-managed-urls - AssertionError: EndpointUrl 'https://acm-acme-enroll.us-west-2.amazonaws.com/acme/2a23b7fd-df89-4e4c-9506-225c7c2f9d3d/directory' must have the path /<id>/directory and nothing after it (checker regex /[0-9a-fA-F-]+/directory applied with fullmatch to the URL path). The host and scheme assertions passed; only the extra '/acme' segment diverges.
Cause 2: acm.create-acme-domain-validation-generates-server-managed-dns-record - AssertionError: unexpected PrevalidationType: 'DNS_PREVALIDATION' full={'AcmeDomainValidationArn': 'arn:aws:acm:us-west-2:000000000000:acme-endpoint/51f800eb-.../acme-domain-validation/0b9929b1-...', 'PrevalidationType': 'DNS_PREVALIDATION', 'PrevalidationDetails': {'DnsPrevalidation': {...}}, 'Status': 'VALIDATING', ...}; acm.domain-validation-optional-field-empty-vs-absent-preserved - AssertionError: unexpected PrevalidationType, full record: {... 'PrevalidationType': 'DNS_PREVALIDATION' ...} (expected 'DNS' in both).
Cause 3: acm.create-acme-domain-validation-for-unowned-endpoint-denied - AssertionError: expected ValidationException, got ResourceNotFoundException ('Could not find ACME endpoint arn:aws:acm:us-west-2:111122223333:acme-endpoint/2b260b3a-a1db-47b2-98dd-b80925898547.').
Cause 4: acm.concurrent-update-acme-domain-validation-no-lost-update - AssertionError: expected both concurrent updates against a VALIDATING domain validation to be rejected with ValidationException, got ['NotImplemented', 'NotImplemented'].
Cause 5: acm.deploy-acme-endpoint-via-cloudformation-behaves - botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CertificateManager::AcmeEndpoint].
Smithy citations (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): AcmeEndpoint{EndpointUrl: String}; AcmeDomainValidation{PrevalidationType, PrevalidationDetails, Status}; PrevalidationType enum values ['DNS_PREVALIDATION']; CreateAcmeDomainValidation errors AccessDeniedException(403)/ConflictException/InternalServerException(500)/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException and AcmeEndpointArn pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$; UpdateAcmeDomainValidation -> Unit, @idempotent, errors incl. ValidationException and ConflictException.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', Contact='NOT_REQUIRED', CertificateAuthority={'PublicCertificateAuthority': {}}), wait for Status ACTIVE, then acm.describe_acme_endpoint(AcmeEndpointArn=<arn>) and inspect AcmeEndpoint.EndpointUrl - its path is '/acme/<id>/directory' instead of '/<id>/directory'.
3. Cause 2: acm.create_acme_domain_validation(AcmeEndpointArn=<arn>, DomainName=<d>, PrevalidationOptions={'DnsPrevalidation': {}}) then acm.describe_acme_domain_validation(AcmeDomainValidationArn=<dv arn>) - PrevalidationType is 'DNS_PREVALIDATION' where AWS returns 'DNS'.
4. Cause 3: acm.create_acme_domain_validation(AcmeEndpointArn='arn:aws:acm:<region>:111122223333:acme-endpoint/<uuid>', DomainName=<d>, PrevalidationOptions={'DnsPrevalidation': {}}) from an account other than 111122223333 - the error is ResourceNotFoundException, not ValidationException.
5. Cause 4: create an ACME endpoint and a domain validation, then fire two concurrent acm.update_acme_domain_validation calls with different DomainScope values at the VALIDATING resource - both answer HTTP 501 NotImplemented.
6. Cause 5: cloudformation.create_stack with Resources {'Endpoint': {'Type': 'AWS::CertificateManager::AcmeEndpoint', 'Properties': {...}}} - CreateStack is refused with ValidationError / Unrecognized resource types.
7. Re-replay the 8 checks listed in trace_ids.
