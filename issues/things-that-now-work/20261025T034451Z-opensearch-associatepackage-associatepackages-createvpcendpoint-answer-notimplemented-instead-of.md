Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.associate-package-nonexistent-domain-rejected opensearch.associate-packages-batch-duplicate-name-collapsed opensearch.vpc-endpoint-error-semantics opensearch.package-scope-invalid-account-rejected opensearch.server-side-service-software-rollback-window-enforced-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.associate-package-nonexistent-domain-rejected|run#14|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearch.associate-packages-batch-duplicate-name-collapsed|run#15|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: opensearch.vpc-endpoint-error-semantics|run#6|AssertionError|assert code == 'ValidationException', f'CreateVpcEndpoint with an unknown domain should be rejected with ValidationException, got {code}'
Issue-Key: opensearch.package-scope-invalid-account-rejected|run#9|AssertionError|assert message == expected, (
Issue-Key: opensearch.server-side-service-software-rollback-window-enforced-rejected|run#7|AssertionError|assert rollback_opts.get("Description") == (
Issue-Commit: 7904814320df70b9478f23cd4eaae300bbcca259

# opensearch: AssociatePackage/AssociatePackages/CreateVpcEndpoint answer NotImplemented instead of modeled rejections, plus two payload/validation-text divergences (5 causes)

> _Note: this issue was identified against the code base at commit `7904814320df70b9478f23cd4eaae300bbcca259`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociatePackage

## Description
Five net-new root causes in the Amazon OpenSearch Service control plane, all surfaced by behavioral checks that the replay report left unmarked (no ALREADY FILED / OWNED marker), i.e. no open issue currently carries their replay keys. Causes 1-3 are operations that answer a non-modeled `NotImplemented` where AWS answers a modeled rejection; open stub issues name those operations but do not carry these checks' keys nor the error semantics those checks pin, so they are recorded here as the behavioral requirement. Causes 4-5 are response-content divergences on operations that DO answer.

=== Cause 1 of 5: AssociatePackage against a nonexistent domain answers NotImplemented instead of 400 ValidationException ===
Failing check: opensearch.associate-package-nonexistent-domain-rejected
AWS: Smithy: AssociatePackage (POST /2021-01-01/packages/associate/{PackageID}/{DomainName}) models ValidationException (client, httpError 400) and ResourceNotFoundException among its errors; `NotImplemented` is not a modeled error of this operation. Recorded behavior: a request naming an AVAILABLE package and a syntactically valid DomainName that does not exist is refused with ValidationException and message 'The domain <name> does not exist.' (example text), and the package keeps zero domain associations (ListDomainsForPackage returns an empty list).
Emulator: the call is refused with error code 'NotImplemented' ('operation ... is scaffolded but not implemented'), so neither the refusal class nor the effect-absence can be observed.
Why: a client that distinguishes a bad request (ValidationException, do not retry, fix the domain name) from a service outage/unsupported call cannot do so; SDK error handling branches on the code, and NotImplemented is not in this operation's modeled error set at all, so typed catch blocks fall through to generic failure.
Fix: the AssociatePackage handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/associate_package.rs) plus its domain/package state lookup and validation: resolve the DomainName in domain state and return ValidationException when absent, leaving package associations untouched. Related open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md covers other CreateDomain validation gaps and does not cover this operation.
Traces: opensearch.associate-package-nonexistent-domain-rejected.

=== Cause 2 of 5: AssociatePackages with the same PackageID listed twice answers NotImplemented instead of 400 ValidationException ===
Failing check: opensearch.associate-packages-batch-duplicate-name-collapsed
AWS: Smithy: AssociatePackages (POST /2021-01-01/packages/associateMultiple, required PackageList + DomainName) models ValidationException (client, 400) and ConflictException; `NotImplemented` is not modeled. Recorded behavior: a batch whose PackageList repeats one PackageID is rejected as a whole with ValidationException and creates no association (the domain still lists no packages), while the same package listed once in the same call succeeds and returns exactly one DomainPackageDetails entry.
Emulator: the batch call is refused with code 'NotImplemented', so the all-or-nothing batch semantics and the single-entry positive control are unobservable.
Why: callers that batch-associate packages get an unmodeled failure for every call, and cannot tell a malformed batch (fixable client-side) from an unimplemented/unavailable API; partial-application semantics of the batch are untested and unspecified in the emulator.
Fix: the AssociatePackages handler (.../src/ops/associate_packages.rs) and its batch validation: reject duplicate PackageIDs in PackageList with ValidationException before any association is written, and apply the batch atomically otherwise.
Traces: opensearch.associate-packages-batch-duplicate-name-collapsed.

=== Cause 3 of 5: CreateVpcEndpoint with an unknown domain ARN answers NotImplemented instead of 400 ValidationException, blocking the VPC-endpoint error-semantics contract ===
Failing check: opensearch.vpc-endpoint-error-semantics
AWS: Smithy: CreateVpcEndpoint (POST /2021-01-01/opensearch/vpcEndpoints, required DomainArn + VpcOptions) models ValidationException (client, 400); `NotImplemented` is not modeled. Recorded behavior: a well-formed DomainArn for a domain that does not exist, with a real subnet id, is refused with ValidationException whose message is the deliberately non-committal "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." (example text). The same check then requires, downstream, that DescribeVpcEndpoints for an unknown endpoint id returns 200 with an empty VpcEndpoints list and one VpcEndpointErrors entry carrying ErrorCode 'ENDPOINT_NOT_FOUND', that DeleteVpcEndpoint on an unknown id raises ResourceNotFoundException, and that ListVpcEndpoints does not fabricate the id - none of which was reached.
Emulator: CreateVpcEndpoint answers code 'NotImplemented', aborting the check at its first assertion.
Why: VPC-endpoint clients cannot distinguish "no such domain / not permitted" from an unavailable API, and the split error channel (in-payload VpcEndpointErrors on read vs. a typed exception on mutation) is a behavior the emulator does not exhibit at all.
Fix: the CreateVpcEndpoint handler (.../src/ops/create_vpc_endpoint.rs) and the VPC-endpoint state/validation layer, including the DescribeVpcEndpoints VpcEndpointErrors serializer and DeleteVpcEndpoint not-found path. The open stub issue 20261024T055421Z-opensearch-createvpcendpoint-stub.md names the operation only; it does not carry this check's replay key nor the error-channel semantics above.
Traces: opensearch.vpc-endpoint-error-semantics.

=== Cause 4 of 5: UpdatePackageScope validation error for a malformed PackageUserList member is constructed with the wrong shape (count, member path, constraint set) ===
Failing check: opensearch.package-scope-invalid-account-rejected
AWS: Smithy: UpdatePackageScope models ValidationException (client, 400); PackageUserList is a list of PackageUser with @length min 6 max 12 and @pattern ^[0-9]{12}$|^GLOBAL$. The refusal message is built from the request and the member's constraints, so its construction rule is the requirement: ONE validation error is reported, attributed to the list member `packageUserList`, rendering the offending value as the list ('[not-an-account-id]') and aggregating ALL of the member's constraints in a single clause - 'Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 6, Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$]'. The error code (ValidationException) is correct in the emulator; the construction is not.
Emulator: emits '2 validation errors detected', one per violated constraint, attributed to an indexed path 'packageUserList.1.member' with the bare scalar value, and omits the min-length constraint from the enumerated set.
Why: clients and test harnesses that parse the AWS validation-message format (error count, member path, aggregated constraint list) to report which input member failed get a different member path and a different error count for the same bad request; the emulator's per-constraint expansion also leaks an internal index path that AWS does not use for this list.
Fix: the shared input-validation/error-rendering layer for opensearch (the UpdatePackageScope validation path and the constraint-violation formatter): aggregate all constraints of a list member into one violation attributed to the list member name, and render the value as the list.
Traces: opensearch.package-scope-invalid-account-rejected.

=== Cause 5 of 5: RollbackServiceSoftwareUpdate declines with the wrong RollbackServiceSoftwareOptions.Description ===
Failing check: opensearch.server-side-service-software-rollback-window-enforced-rejected
AWS: Smithy: RollbackServiceSoftwareUpdate (POST /2021-01-01/opensearch/serviceSoftwareUpdate/rollback) returns 200 with RollbackServiceSoftwareOptions. Recorded behavior: on a settled domain with no update in flight the service answers 200 and declines in the payload with RollbackAvailable=false and Description 'No previous software version available for rollback. Please contact AWS Support for assistance.' This is a response payload member, not an error message. RollbackAvailable=false is already correct in the emulator; only the Description member diverges.
Emulator: returns Description 'No service software rollback is available for this domain.'
Why: the Description member is the only machine-readable explanation of WHY the rollback was declined (no prior version available vs. a change in progress); a client or operator runbook that keys off it, or asserts the documented decline reason, reads a reason the service never emits.
Fix: the RollbackServiceSoftwareUpdate handler / its ServiceSoftwareOptions state (the rollback-decline branch that populates RollbackServiceSoftwareOptions.Description).
Traces: opensearch.server-side-service-software-rollback-window-enforced-rejected.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, opensearch 8/140 passed.

Cause 1 - opensearch.associate-package-nonexistent-domain-rejected [mismatch]:
  AssertionError: AssociatePackage against a nonexistent domain: expected ValidationException, got 'NotImplemented'
  Expected: code ValidationException (HTTP 400), message example 'The domain <name> does not exist.', then ListDomainsForPackage == [].

Cause 2 - opensearch.associate-packages-batch-duplicate-name-collapsed [mismatch]:
  AssertionError: expected ValidationException, got 'NotImplemented'
  Expected: AssociatePackages(PackageList=[{PackageID:X},{PackageID:X}]) -> ValidationException (400), no association created; same package listed once -> 200 with exactly one DomainPackageDetails entry.

Cause 3 - opensearch.vpc-endpoint-error-semantics [mismatch]:
  AssertionError: CreateVpcEndpoint with an unknown domain should be rejected with ValidationException, got NotImplemented
  Expected: ValidationException (400), message example "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain."; then DescribeVpcEndpoints(unknown id) -> 200, VpcEndpoints == [], one VpcEndpointErrors entry with ErrorCode 'ENDPOINT_NOT_FOUND'; DeleteVpcEndpoint(unknown id) -> ResourceNotFoundException; ListVpcEndpoints must not contain the id.

Cause 4 - opensearch.package-scope-invalid-account-rejected [mismatch]:
  Expected (AWS construction): "1 validation error detected: Value '[not-an-account-id]' at 'packageUserList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 6, Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$]"
  Actual (emulator): "2 validation errors detected: Value 'not-an-account-id' at 'packageUserList.1.member' failed to satisfy constraint: Member must have length less than or equal to 12; Value 'not-an-account-id' at 'packageUserList.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$"
  Code matched: ValidationException. Model: PackageUser @length(min 6, max 12), @pattern ^[0-9]{12}$|^GLOBAL$.

Cause 5 - opensearch.server-side-service-software-rollback-window-enforced-rejected [mismatch]:
  AssertionError: unexpected rollback decline description: 'No service software rollback is available for this domain.'
  Expected RollbackServiceSoftwareOptions.Description: 'No previous software version available for rollback. Please contact AWS Support for assistance.' (RollbackAvailable=false matched).

## Steps to Reproduce
Replay these behavioral checks against the emulator (all service token `opensearch`):

1. opensearch.associate-package-nonexistent-domain-rejected - create an S3 bucket with a dictionary object, CreatePackage(PackageType=TXT-DICTIONARY) and poll to AVAILABLE, then AssociatePackage(PackageID=<id>, DomainName=<valid-but-nonexistent>). Observe the error code.
2. opensearch.associate-packages-batch-duplicate-name-collapsed - create a domain and an AVAILABLE package, then AssociatePackages(DomainName=<domain>, PackageList=[{PackageID:X},{PackageID:X}]). Observe the error code; then repeat with the package listed once.
3. opensearch.vpc-endpoint-error-semantics - create a VPC and a subnet, then CreateVpcEndpoint(DomainArn=arn:aws:es:<region>:<account>:domain/<nonexistent>, VpcOptions={SubnetIds:[<subnet>]}). Observe the error code; then DescribeVpcEndpoints / DeleteVpcEndpoint / ListVpcEndpoints with a bogus 'aos-...' endpoint id.
4. opensearch.package-scope-invalid-account-rejected - with an AVAILABLE package, call UpdatePackageScope(PackageID=<id>, Operation='ADD', PackageUserList=['not-an-account-id']) and compare the full ValidationException message construction.
5. opensearch.server-side-service-software-rollback-window-enforced-rejected - CreateDomain (t3.small.search, 1 node, gp3 10GiB), wait until active, then RollbackServiceSoftwareUpdate(DomainName=<domain>) and read RollbackServiceSoftwareOptions.Description from the 200 response.
