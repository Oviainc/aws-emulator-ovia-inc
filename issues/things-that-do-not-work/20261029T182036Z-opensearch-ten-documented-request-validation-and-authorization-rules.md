Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.overly-permissive-cluster-config-rules opensearch.overly-permissive-engine-version-rejected opensearch.overly-permissive-fgac-requires-encryption-and-https opensearch.overly-permissive-domain-endpoint-options-rules opensearch.overly-permissive-access-policy-rejected opensearch.start-migration-nonexistent-application-rejected opensearch.overly-permissive-migration-invalid-source-endpoint-rejected opensearch.start-migration-invalid-source-version-rejected opensearch.concurrent-start-migration-independent-domains opensearch.overly-permissive-attach-data-source-cross-account-rejected opensearch.overly-permissive-attach-data-source-unsupported-type-rejected opensearch.overly-permissive-package-missing-and-bad-source opensearch.get-package-version-history-cross-account-access-denied opensearch.get-package-version-history-cross-account-denied opensearch.rollback-service-software-update-no-update-in-progress-error opensearch.default-application-setting-managed opensearch.index-settings-unicode-analyzer-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.overly-permissive-cluster-config-rules|run#12|AssertionError|assert code == 'ValidationException', f'zone awareness with a single data node must be rejected, got {code}'
Issue-Key: opensearch.overly-permissive-engine-version-rejected|run#2|AssertionError|assert code == "ValidationException", (
Issue-Key: opensearch.overly-permissive-fgac-requires-encryption-and-https|_expect_rejection#2|AssertionError|raise AssertionError(
Issue-Key: opensearch.overly-permissive-domain-endpoint-options-rules|_expect_rejection#4|AssertionError|raise AssertionError(
Issue-Key: opensearch.overly-permissive-access-policy-rejected|run#13|AssertionError|assert code == 'ValidationException', f'a non-JSON AccessPolicies document must be rejected, got {code}'
Issue-Key: opensearch.start-migration-nonexistent-application-rejected|_error_detail#11|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {result!r}")
Issue-Key: opensearch.overly-permissive-migration-invalid-source-endpoint-rejected|_error#7|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearch.start-migration-invalid-source-version-rejected|_error#4|AssertionError|raise AssertionError("expected StartMigration to be rejected, but the call SUCCEEDED")
Issue-Key: opensearch.overly-permissive-attach-data-source-cross-account-rejected|_client_error#11|AssertionError|raise AssertionError(f"{what} must be rejected, but it succeeded: {resp!r}")
Issue-Key: opensearch.overly-permissive-attach-data-source-unsupported-type-rejected|_error#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearch.overly-permissive-package-missing-and-bad-source|_error#25|AssertionError|raise AssertionError(f'expected rejection, but the call SUCCEEDED and returned {result!r}')
Issue-Key: opensearch.get-package-version-history-cross-account-access-denied|_error#25|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: opensearch.rollback-service-software-update-no-update-in-progress-error|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearch.index-settings-unicode-analyzer-rejected|_sustained_error_code#19|AssertionError|raise AssertionError(
Issue-Related: 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md
Issue-Related: 20261029T080439Z-opensearch-2-net-new-behavioral-causes-startmigration-still.md
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# opensearch: ten documented request-validation and authorization rules are not enforced — CreateDomain, StartMigration, AttachDataSource, CreatePackage, GetPackageVersionHistory and RollbackServiceSoftwareUpdate accept what AWS refuses (10 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This issue bundles 10 root causes, all in the opensearch service. Every one is a missing refusal: the emulator answers HTTP 200 and mints state for a request real AWS refuses. The open issue 20261025T172549Z-acm-opensearch-opensearch-createdomain-does-not-validate-domainendpointoptions-custom-endpoints.md covers the ACM-certificate side of the custom-endpoint rules for other checks; cause 4 below is the incomplete-endpoint rule as hit by an opensearch-only check that was not marked owned by it. Each cause must be closed before submitting.

Cause 1 of 10: CreateDomain accepts ZoneAwarenessEnabled with a single data node.
AWS: Smithy: CreateDomain takes ClusterConfig and models ValidationException (400). ClusterConfig.ZoneAwarenessEnabled requires at least two data nodes (AWS distributes nodes across the configured availability zones), so InstanceCount=1 with ZoneAwarenessEnabled=true is refused 400 ValidationException naming the zone-awareness/instance-count rule; the variable parts are the requested instance count and zone-awareness setting.
Emulator: the domain is created, HTTP 200, DomainStatus returned with ClusterConfig echoed.
Why: a client receives a domain whose topology AWS would never build, so an invalid Terraform/CloudFormation/SDK configuration passes in test and fails in production.
Fix: CreateDomain's input validation (ClusterConfig cross-field rules).
Traces: opensearch.overly-permissive-cluster-config-rules.

Cause 2 of 10: CreateDomain accepts an EngineVersion that does not exist.
AWS: Smithy: CreateDomain.EngineVersion targets VersionString @pattern "^Elasticsearch_[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$". A value that matches the pattern but is not a version the service offers (ListVersions does not advertise it) is refused 400 ValidationException naming the unsupported version.
Emulator: `OpenSearch_99.99` is accepted and a domain is created.
Why: a client can create a domain pinned to a nonexistent engine, and every later version-dependent behaviour (feature gating, upgrade paths) is computed from a version that does not exist.
Fix: CreateDomain's input validation must check EngineVersion against the advertised version list (the same source ListVersions/GetCompatibleVersions read).
Traces: opensearch.overly-permissive-engine-version-rejected.

Cause 3 of 10: CreateDomain accepts fine-grained access control without the encryption and HTTPS preconditions.
AWS: Smithy: CreateDomain takes AdvancedSecurityOptions (AdvancedSecurityOptionsInput), EncryptionAtRestOptions, NodeToNodeEncryptionOptions and DomainEndpointOptions, and models ValidationException (400). AdvancedSecurityOptions.Enabled=true requires EncryptionAtRestOptions.Enabled=true, NodeToNodeEncryptionOptions.Enabled=true and DomainEndpointOptions.EnforceHTTPS=true; omitting any of them is refused 400 ValidationException naming the missing precondition.
Emulator: a domain with FGAC enabled and EncryptionAtRest absent is created (HTTP 200, DomainId returned).
Why: a client believes it has provisioned an FGAC domain that AWS would have refused as insecure, so a security-relevant precondition silently disappears from the test matrix.
Fix: CreateDomain's input validation (AdvancedSecurityOptions precondition rules).
Traces: opensearch.overly-permissive-fgac-requires-encryption-and-https.

Cause 4 of 10: CreateDomain accepts a half-specified DomainEndpointOptions custom endpoint.
AWS: Smithy: CreateDomain.DomainEndpointOptions carries CustomEndpointEnabled, CustomEndpoint and CustomEndpointCertificateArn; CreateDomain models ValidationException (400). CustomEndpointEnabled=true without CustomEndpoint (or without CustomEndpointCertificateArn) is refused 400 ValidationException naming the member that must be supplied.
Emulator: the domain is created with DomainEndpointOptions {EnforceHTTPS: true, CustomEndpointEnabled: true} and no CustomEndpoint.
Why: a client gets a domain claiming a custom endpoint that cannot exist, and the partial configuration round-trips through DescribeDomain as if valid.
Fix: CreateDomain's input validation (DomainEndpointOptions cross-field rules) — the same validator the custom-endpoint certificate checks need.
Traces: opensearch.overly-permissive-domain-endpoint-options-rules.

Cause 5 of 10: CreateDomain accepts an AccessPolicies value that is not a JSON document.
AWS: Smithy: CreateDomain.AccessPolicies targets PolicyDocument (@length 0..102400, @pattern ".*"); the pattern does not constrain the content, but AWS parses the document and refuses a non-JSON body with 400 ValidationException naming the policy as malformed.
Emulator: the call succeeds (no error raised at all) and the domain is created with the unparseable policy stored.
Why: a client stores garbage as a resource policy; nothing it later reads or enforces can be correct, and a malformed policy that AWS would reject at submit time is only discovered much later.
Fix: CreateDomain's (and UpdateDomainConfig's) AccessPolicies validation — parse the document before accepting it.
Traces: opensearch.overly-permissive-access-policy-rejected.

Cause 6 of 10: StartMigration validates nothing about the application or the migration source.
AWS: Smithy: StartMigration(applicationId @pattern "^[a-z0-9]{3,30}$", migrationOptions required) models ValidationException (400), ResourceNotFoundException (409), ConflictException (409) and AccessDeniedException (403). An applicationId that does not exist is refused ResourceNotFoundException; a migrationOptions.source whose endpoint/ARN is not a legal migration source, and a source engine version that cannot be migrated, are refused ValidationException naming the offending member.
Emulator: every such call returns HTTP 200 with {"migrationId": "mig-...", "status": "PENDING"} and mints a migration record.
Why: a client gets a migration id for a migration that can never run and must poll a PENDING migration that will never make progress, instead of being told at submit time which input is wrong.
Fix: StartMigration's handler — application existence lookup and migrationOptions.source validation before minting the migration.
Traces: opensearch.start-migration-nonexistent-application-rejected, opensearch.overly-permissive-migration-invalid-source-endpoint-rejected, opensearch.start-migration-invalid-source-version-rejected, opensearch.concurrent-start-migration-independent-domains.

Cause 7 of 10: AttachDataSource accepts a cross-account ARN and an ARN of an unsupported resource type.
AWS: Smithy: AttachDataSource(id, dataSourceArn @length 20..2048) models ValidationException (400), AccessDeniedException (403), ResourceNotFoundException (409) and ConflictException (409). A dataSourceArn whose account id is not the caller's is refused (the caller cannot attach another account's domain), and an ARN that names a resource type that is not a supported data source (e.g. an arn:aws:ec2:...:instance/i-...) is refused ValidationException naming the unsupported ARN.
Emulator: both are accepted with HTTP 200 and an attachment record in status PENDING (attachmentId minted, dataSourceArn echoed verbatim).
Why: an application ends up with attachments to resources that cannot be read, so the attachment list no longer describes anything real and cross-account isolation is not enforced.
Fix: AttachDataSource's handler — ARN parsing/owner check and supported-resource-type check before creating the attachment.
Traces: opensearch.overly-permissive-attach-data-source-cross-account-rejected, opensearch.overly-permissive-attach-data-source-unsupported-type-rejected.

Cause 8 of 10: CreatePackage accepts a PackageSource that does not exist.
AWS: Smithy: CreatePackage(PackageName, PackageType, PackageSource required) models ValidationException (400), AccessDeniedException (403) and BaseException (400). A PackageSource naming an S3 bucket/key the service cannot read is refused at submit time.
Emulator: HTTP 200 with PackageDetails {PackageStatus: 'COPYING'} for a source that does not exist.
Why: the package is reported as COPYING forever; the client has no refusal to act on and the package never becomes available.
Fix: CreatePackage's handler — resolve and validate PackageSource before accepting.
Traces: opensearch.overly-permissive-package-missing-and-bad-source.

Cause 9 of 10: GetPackageVersionHistory serves a package the caller does not own.
AWS: Smithy: GetPackageVersionHistory(PackageID @pattern "^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$") models AccessDeniedException (403) and ResourceNotFoundException (409). A PackageID owned by another account is refused for the calling principal.
Emulator: HTTP 200 with PackageID and PackageVersionHistoryList for a cross-account package (and in one variant returns a successful response body of None rather than any refusal).
Why: package version history leaks across account boundaries — an authorization rule is simply absent.
Fix: GetPackageVersionHistory's handler must check the package's PackageOwner against the calling account before serving the history.
Traces: opensearch.get-package-version-history-cross-account-access-denied, opensearch.get-package-version-history-cross-account-denied.

Cause 10 of 10: writes that AWS gates on current resource state are accepted unconditionally — RollbackServiceSoftwareUpdate, CreateIndex settings and the managed default-application setting.
AWS: Smithy: RollbackServiceSoftwareUpdate models BaseException (400), ValidationException (400) and ResourceNotFoundException (409); it is only legal while a service-software update is in progress and a previous version exists, otherwise it is refused. CreateIndex models ValidationException (400) and refuses index settings naming an analyzer the engine cannot build. The default-application setting is service-managed and the corresponding write is refused.
Emulator: RollbackServiceSoftwareUpdate answers HTTP 200 with RollbackServiceSoftwareOptions {RollbackAvailable: false, Description: 'No previous software version available for rollback...'} — reporting the refusal reason inside a SUCCESS; CreateIndex returns 200 Status='CREATED' for the unicode-analyzer settings AWS refuses; and the managed default-application write returns 200 with an applicationArn.
Why: a caller cannot distinguish "rolled back" from "nothing to roll back" because both are 200, and state-machine preconditions AWS enforces on write are only discoverable by reading the response body — so client code that checks for an exception proceeds down the wrong branch.
Fix: the precondition checks in the RollbackServiceSoftwareUpdate, CreateIndex and default-application-setting handlers must refuse with the modelled error instead of returning 200.
Traces: opensearch.rollback-service-software-update-no-update-in-progress-error, opensearch.index-settings-unicode-analyzer-rejected, opensearch.default-application-setting-managed.

## Evidence
Cause 1 — expected ValidationException; actual: `zone awareness with a single data node must be rejected, got None` (no exception raised).
Cause 2 — expected ValidationException; actual: `EngineVersion 'OpenSearch_99.99' must be rejected, got None`.
Cause 3 — expected ValidationException; actual: `FGAC without EncryptionAtRest: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-aad25c4aed`.
Cause 4 — expected ValidationException; actual: `CustomEndpointEnabled without CustomEndpoint: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-17462db298`.
Cause 5 — expected ValidationException; actual: `a non-JSON AccessPolicies document must be rejected, got None`.
Cause 6 — expected ResourceNotFoundException/ValidationException; actual HTTP 200 `{'migrationId': 'mig-gy8c079qmaaa', 'status': 'PENDING'}` for a nonexistent application, for an invalid source endpoint and for an invalid source version.
Cause 7 — expected a rejection; actual HTTP 200 `{'attachmentId': 'dsa-4kubnyc2aaaa', 'id': 'sfi9sv1baaaa', 'dataSourceArn': 'arn:aws:es:us-west-2:555555555555:domain/...', 'status': 'PENDING'}` (cross-account) and `{'attachmentId': 'dsa-3ibp6ed2aaaa', ..., 'dataSourceArn': 'arn:aws:ec2:us-west-2:000000000000:instance/i-0123456789abcdef0', 'status': 'PENDING'}` (unsupported type).
Cause 8 — expected a rejection; actual HTTP 200 `{'PackageDetails': {'PackageID': 'pkg-408a...', 'PackageStatus': 'COPYING', ...}}`.
Cause 9 — expected a ClientError (AccessDeniedException); actual HTTP 200 `{'PackageID': 'pkg-73ede...', 'PackageVersionHistoryList': [{'PackageVersion': '1', ...}]}`, and in the second variant `expected a ClientError, but the call succeeded: None`.
Cause 10 — expected a rejection; actual HTTP 200 `{'RollbackServiceSoftwareOptions': {'CurrentVersion': 'R20250101', 'RollbackAvailable': False, 'Description': 'No previous software version available for rollback. Please contact AWS Support for assistance.'}}`; CreateIndex `{'Status': 'CREATED'}`; default-application write `{'applicationArn': 'arn:aws:es:us-west-2:000000000000:application/app-00000...'}`.

## Steps to Reproduce
1. Cause 1: CreateDomain(ClusterConfig={InstanceType:'t3.small.search', InstanceCount:1, ZoneAwarenessEnabled:true}).
2. Cause 2: CreateDomain(EngineVersion='OpenSearch_99.99').
3. Cause 3: CreateDomain(AdvancedSecurityOptions={Enabled:true, InternalUserDatabaseEnabled:true, MasterUserOptions:{...}}) with EncryptionAtRestOptions omitted.
4. Cause 4: CreateDomain(DomainEndpointOptions={EnforceHTTPS:true, CustomEndpointEnabled:true}) with no CustomEndpoint.
5. Cause 5: CreateDomain(AccessPolicies='not json at all').
6. Cause 6: StartMigration(applicationId=<nonexistent id>, migrationOptions={...}); then with a migrationOptions.source endpoint/ARN that is not a legal source; then with an unmigratable source version.
7. Cause 7: CreateApplication, then AttachDataSource(id=<app>, dataSourceArn='arn:aws:es:us-west-2:555555555555:domain/<other-account domain>') and AttachDataSource(..., dataSourceArn='arn:aws:ec2:us-west-2:<acct>:instance/i-0123456789abcdef0').
8. Cause 8: CreatePackage(PackageName=<n>, PackageType='TXT-DICTIONARY', PackageSource={S3BucketName:<nonexistent>, S3Key:<nonexistent>}).
9. Cause 9: create a package under one account, then GetPackageVersionHistory(PackageID=<that id>) as a different account.
10. Cause 10: on a domain with no update in progress call RollbackServiceSoftwareUpdate(DomainName=<d>); CreateIndex with settings naming a unicode analyzer the engine cannot build; issue the managed default-application-setting write.
