Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.concurrent-create-connections-produce-unique-arns codeconnections.concurrent-delete-connection-exactly-one-succeeds codeconnections.concurrent-tag-resource-no-lost-writes codeconnections.connection-arn-never-reused-after-delete codeconnections.connection-created-in-pending-status codeconnections.connection-delete-removes-and-not-found codeconnections.connection-invalid-provider-type-rejected codeconnections.connection-malformed-host-arn-rejected codeconnections.connection-name-exceeds-max-length-rejected codeconnections.connection-reflects-provider-type-without-host codeconnections.create-connection-with-tags-reflected codeconnections.create-repository-link-duplicate-rejected codeconnections.create-repository-link-inaccessible-connection-rejected codeconnections.create-repository-link-with-tags-reflected codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected codeconnections.get-connection-not-found codeconnections.get-repository-link-not-found codeconnections.get-sync-configuration-not-found codeconnections.list-connections-reflects-created-and-provider-filter codeconnections.list-connections-reflects-lifecycle codeconnections.list-repository-sync-definitions-bad-link-not-found codeconnections.list-sync-configurations-bad-link-not-found codeconnections.list-tags-for-resource-not-found codeconnections.tag-resource-limit-exceeded-rejected codeconnections.tag-resource-overwrite-updates-value codeconnections.tag-resource-round-trip codeconnections.untag-resource-removes-tag codeconnections.update-sync-configuration-publish-deployment-status codeconnections.create-connection-with-pending-host-succeeds codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected codeconnections.get-host-not-found codeconnections.host-created-in-pending-status codeconnections.host-delete-removes-and-not-found codeconnections.host-reflects-fields-without-vpc codeconnections.list-hosts-reflects-lifecycle codeconnections.create-connection-with-nonexistent-host-rejected codeconnections.list-connections-bad-host-arn-filter-not-found codeconnections.delete-sync-configuration-nonexistent-is-idempotent codeconnections.get-repository-sync-status-not-found codeconnections.update-repository-link-not-found codeconnections.update-sync-blocker-not-found codeconnections.update-sync-configuration-not-found
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections|CreateConnection|NotImplemented
Issue-Key: codeconnections.connection-invalid-provider-type-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: codeconnections.connection-malformed-host-arn-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: codeconnections.connection-name-exceeds-max-length-rejected|run#6|AssertionError|assert code == "ValidationException", (
Issue-Key: codeconnections|CreateHost|NotImplemented
Issue-Key: codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected|run#6|AssertionError|assert code == "ValidationException", (
Issue-Key: codeconnections|ListConnections|NotImplemented
Issue-Key: codeconnections.list-connections-bad-host-arn-filter-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codeconnections.delete-sync-configuration-nonexistent-is-idempotent|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException before delete, got {code!r}"
Issue-Key: codeconnections.get-repository-sync-status-not-found|run#4|AssertionError|assert code == "InvalidInputException", "actual_code=" + str(code)
Issue-Key: codeconnections.update-repository-link-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codeconnections.update-sync-blocker-not-found|run#4|AssertionError|assert code == "SyncBlockerDoesNotExistException", "actual_code=" + str(code)
Issue-Key: codeconnections.update-sync-configuration-not-found|run#4|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# CodeConnections core control plane answers NotImplemented: 8 operation families return the scaffold stub instead of their modeled responses and errors (8 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
The codeconnections emulator is bootstrapped and routes requests (requests are signed, dispatched and answered), but 8 distinct operation handlers are still scaffold stubs that answer with an unmodeled `NotImplemented` error (`operation amazonaws.codeconnections.CodeConnections_20231201.<Op> is not implemented in this emulator`). Because CreateConnection and CreateHost are the entry points for almost every resource in the service, this blocks the entire connection/host/repository-link/sync surface: all 45 behavioral checks in this suite fail, 42 of them for the causes below (the other 3 — DeleteRepositoryLink, GetResourceSyncStatus, GetSyncBlockerSummary — are already filed separately as stubs and are NOT part of this issue).

No existing open issue covers any of these 8 operations.

There are EIGHT separate causes in this issue; each must be closed before submitting.

=== Cause 1 of 8: CreateConnection is a scaffold stub ===
AWS: Smithy: CreateConnection(input CreateConnectionInput{ConnectionName required @length 1..32 ; ProviderType enum ; HostArn @length 0..256 @pattern ^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$ ; Tags @length 0..200}) -> CreateConnectionOutput{ConnectionArn required, Tags}; modeled errors LimitExceededException (429), ResourceNotFoundException (404), ResourceUnavailableException (400). Real AWS creates the connection, returns 200 with a unique ConnectionArn, makes it readable via GetConnection/ListConnections in PENDING status, and refuses malformed input with ValidationException (400) — recorded behavior for the three rejection checks below, which observed code `ValidationException` with messages of the form "1 validation error detected: Value ... at '<member>' failed to satisfy constraint: ..." (the member/constraint wording is built from the request, so naming the offending member and its constraint is the requirement; the exact recorded text is an example).
Emulator: every CreateConnection call returns the `NotImplemented` error, so no connection can ever be created and no downstream read, tag, delete, concurrency or validation behavior can be exercised.
Why: a client cannot create a connection at all; SDK calls raise ClientError(NotImplemented), an error code that is not in the operation's modeled error set, so client error handling (retry on LimitExceeded/Throttling, catch ValidationException) never matches.
Fix: the CreateConnection handler plus its input validation and connection state (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/create_connection.rs and the service's connection state store).
Traces: codeconnections.concurrent-create-connections-produce-unique-arns, codeconnections.concurrent-delete-connection-exactly-one-succeeds, codeconnections.concurrent-tag-resource-no-lost-writes, codeconnections.connection-arn-never-reused-after-delete, codeconnections.connection-created-in-pending-status, codeconnections.connection-delete-removes-and-not-found, codeconnections.connection-invalid-provider-type-rejected, codeconnections.connection-malformed-host-arn-rejected, codeconnections.connection-name-exceeds-max-length-rejected, codeconnections.connection-reflects-provider-type-without-host, codeconnections.create-connection-with-tags-reflected, codeconnections.create-repository-link-duplicate-rejected, codeconnections.create-repository-link-inaccessible-connection-rejected, codeconnections.create-repository-link-with-tags-reflected, codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected, codeconnections.get-connection-not-found, codeconnections.get-repository-link-not-found, codeconnections.get-sync-configuration-not-found, codeconnections.list-connections-reflects-created-and-provider-filter, codeconnections.list-connections-reflects-lifecycle, codeconnections.list-repository-sync-definitions-bad-link-not-found, codeconnections.list-sync-configurations-bad-link-not-found, codeconnections.list-tags-for-resource-not-found, codeconnections.tag-resource-limit-exceeded-rejected, codeconnections.tag-resource-overwrite-updates-value, codeconnections.tag-resource-round-trip, codeconnections.untag-resource-removes-tag, codeconnections.update-sync-configuration-publish-deployment-status

=== Cause 2 of 8: CreateHost is a scaffold stub ===
AWS: Smithy: CreateHost(input CreateHostInput{Name required @length 1..64 ; ProviderType required enum ; ProviderEndpoint (Url) required @length 1..512 ; VpcConfiguration ; Tags @length 0..200}) -> CreateHostOutput{HostArn, Tags}; modeled error LimitExceededException (429). Real AWS creates the host, returns 200 with a HostArn readable via GetHost/ListHosts, and refuses an over-length ProviderEndpoint with ValidationException (400) naming 'providerEndpoint' and its 512-character limit (recorded behavior; the message is built from the request so the member+constraint is the requirement).
Emulator: every CreateHost call returns `NotImplemented`, so no host exists and host reads, deletes, listing and the host-backed CreateConnection path cannot run.
Why: host-backed providers (GitHubEnterpriseServer, GitLabSelfManaged) are entirely unusable, and an over-length endpoint is neither accepted nor refused with a modeled error.
Fix: the CreateHost handler plus its input validation and host state (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/create_host.rs).
Traces: codeconnections.create-connection-with-pending-host-succeeds, codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected, codeconnections.get-host-not-found, codeconnections.host-created-in-pending-status, codeconnections.host-delete-removes-and-not-found, codeconnections.host-reflects-fields-without-vpc, codeconnections.list-hosts-reflects-lifecycle

=== Cause 3 of 8: ListConnections is a scaffold stub ===
AWS: Smithy: ListConnections(input ListConnectionsInput{ProviderTypeFilter enum, HostArnFilter @pattern ^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$, MaxResults @range 0..100, NextToken @length 1..1024}) -> ListConnectionsOutput{Connections, NextToken}, @paginated; modeled error ResourceNotFoundException (404). Recorded behavior: an unfiltered ListConnections returns 200 with a Connections list, and a HostArnFilter naming a host that does not exist is refused with ResourceNotFoundException (message example: "No host exists with the ARN: <arn>" — derived from the request's ARN).
Emulator: ListConnections returns `NotImplemented` for both the filtered and the unfiltered form.
Why: clients cannot enumerate connections or discover a bad host filter; the paginated listing contract (Connections key always present, NextToken) is unobservable.
Fix: the ListConnections handler, its HostArnFilter/ProviderTypeFilter resolution against host+connection state, and its pagination serializer (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/list_connections.rs).
Traces: codeconnections.create-connection-with-nonexistent-host-rejected, codeconnections.list-connections-bad-host-arn-filter-not-found

=== Cause 4 of 8: GetSyncConfiguration is a scaffold stub ===
AWS: Smithy: GetSyncConfiguration(input{SyncType required enum, ResourceName required @length 1..100}) -> {SyncConfiguration required}; modeled errors AccessDeniedException (403), InternalServerException (503), InvalidInputException (400), ResourceNotFoundException (404), ThrottlingException (429). Recorded behavior: for a ResourceName that has no sync configuration the call is refused with ResourceNotFoundException at HTTP 404 (message example: "CloudFormation Stack Sync configuration not found for accountId: <acct> and configTarget: cfn-stack-sync/arn:aws:cloudformation:<region>:<acct>:stack/<resourceName> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: ...; Proxy: null)").
Emulator: GetSyncConfiguration returns `NotImplemented` instead of ResourceNotFoundException/404, which also makes the DeleteSyncConfiguration idempotency contract unverifiable (the check reads back through GetSyncConfiguration).
Why: a client that distinguishes "no sync configuration" (404 ResourceNotFoundException) from a real failure cannot do so; nothing matches the modeled error set.
Fix: the GetSyncConfiguration handler and its sync-configuration state lookup (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/get_sync_configuration.rs).
Traces: codeconnections.delete-sync-configuration-nonexistent-is-idempotent

=== Cause 5 of 8: GetRepositorySyncStatus is a scaffold stub ===
AWS: Smithy: GetRepositorySyncStatus(input{Branch required @length 1..255, RepositoryLinkId required @pattern UUID, SyncType required enum}) -> {LatestSync required}; modeled errors AccessDeniedException (403), InternalServerException (503), InvalidInputException (400), ResourceNotFoundException (404), ThrottlingException (429). Recorded behavior: a well-formed but nonexistent RepositoryLinkId is refused with InvalidInputException (400), not ResourceNotFoundException (message example: "Repository Link with id <uuid> was not found", built from the request's id).
Emulator: returns `NotImplemented`.
Why: clients branching on InvalidInputException for an unknown repository link see an unmodeled code and cannot classify the refusal.
Fix: the GetRepositorySyncStatus handler and its repository-link lookup/validation (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/get_repository_sync_status.rs).
Traces: codeconnections.get-repository-sync-status-not-found

=== Cause 6 of 8: UpdateRepositoryLink is a scaffold stub ===
AWS: Smithy: UpdateRepositoryLink(input{RepositoryLinkId required @pattern UUID, ConnectionArn, EncryptionKeyArn}) -> {RepositoryLinkInfo required}; modeled errors AccessDeniedException (403), ConditionalCheckFailedException (409), InternalServerException (503), InvalidInputException (400), ResourceNotFoundException (404), ThrottlingException (429), UpdateOutOfSyncException (409). Recorded behavior: a nonexistent RepositoryLinkId is refused with ResourceNotFoundException at HTTP 404 (message example: "Repository not found for accountId: <acct> and UUID: <uuid> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; ...)").
Emulator: returns `NotImplemented`.
Why: a client cannot update a repository link and cannot detect a missing one via the modeled 404.
Fix: the UpdateRepositoryLink handler and its repository-link state (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/update_repository_link.rs).
Traces: codeconnections.update-repository-link-not-found

=== Cause 7 of 8: UpdateSyncBlocker is a scaffold stub ===
AWS: Smithy: UpdateSyncBlocker(input{Id required @length 1..50, SyncType required enum, ResourceName required @length 1..100, ResolvedReason required @length 1..250}) -> {ResourceName required, ParentResourceName, SyncBlocker required}; modeled errors include SyncBlockerDoesNotExistException (404), ResourceNotFoundException (404), InvalidInputException (400), RetryLatestCommitFailedException (503). Recorded behavior: the blocker Id is evaluated before the ResourceName/SyncType pair, so a ResourceName with no sync configuration surfaces SyncBlockerDoesNotExistException (404), not ResourceNotFoundException (message example: "Sync blocker with id blocker-1 does not exist ").
Emulator: returns `NotImplemented`, so neither the ordering nor the error code can be observed.
Why: a client resolving sync blockers cannot tell an unknown blocker id from an unknown resource, and sees an unmodeled error code.
Fix: the UpdateSyncBlocker handler and its blocker-id-before-resource validation ordering (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/update_sync_blocker.rs).
Traces: codeconnections.update-sync-blocker-not-found

=== Cause 8 of 8: UpdateSyncConfiguration is a scaffold stub ===
AWS: Smithy: UpdateSyncConfiguration(input{ResourceName required @length 1..100, SyncType required enum, Branch, ConfigFile, RepositoryLinkId, RoleArn, PublishDeploymentStatus, TriggerResourceUpdateOn, PullRequestComment}) -> {SyncConfiguration required}; modeled errors AccessDeniedException (403), ConcurrentModificationException (409), InternalServerException (503), InvalidInputException (400), ResourceNotFoundException (404), ThrottlingException (429), UpdateOutOfSyncException (409). Recorded behavior: a ResourceName with no sync configuration is refused with InvalidInputException (400) (message example: "Sync Configuration for Resource <name> was not found", built from the request).
Emulator: returns `NotImplemented`.
Why: the sync-configuration update path is unusable and the refusal for an unconfigured resource carries an unmodeled code.
Fix: the UpdateSyncConfiguration handler and its sync-configuration state lookup (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/update_sync_configuration.rs).
Traces: codeconnections.update-sync-configuration-not-found

## Evidence
Suite result: codeconnections 0/45 passed (0%) — 10 mismatched, 35 errored, 0 skipped.

Every failure is the same emulator response shape. Examples:

1) Expected: CreateConnection returns 200 with a unique ConnectionArn.
   Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConnection operation: operation amazonaws.codeconnections.CodeConnections_20231201.CreateConnection is not implemented in this emulator
   (25 checks error out on this very first call; 3 more report it as a mismatch.)

2) Expected: ValidationException for ProviderType='NotARealProvider'.
   Actual: AssertionError: expected ValidationException, got 'NotImplemented'  (codeconnections.connection-invalid-provider-type-rejected)

3) Expected: ValidationException for a 66-char ConnectionName (modeled @length max 32).
   Actual: AssertionError: expected ValidationException creating a 66-char ConnectionName (documented limit is 32), got 'NotImplemented'

4) Expected: ValidationException for a malformed HostArn 'not-an-arn' (modeled @pattern on HostArn).
   Actual: AssertionError: expected ValidationException creating a connection with malformed HostArn 'not-an-arn', got 'NotImplemented'

5) Expected: CreateHost returns 200 with a HostArn; over-length ProviderEndpoint (>512) refused with ValidationException.
   Actual: NotImplemented for CreateHost; AssertionError: a ProviderEndpoint over 512 characters should be rejected with ValidationException, got 'NotImplemented'

6) Expected: ListConnections(HostArnFilter=<nonexistent host arn>) -> ResourceNotFoundException (404).
   Actual: AssertionError: expected ResourceNotFoundException for a HostArnFilter naming a host that was never created, got 'NotImplemented'

7) Expected: GetSyncConfiguration on an unconfigured ResourceName -> ResourceNotFoundException (HTTP 404).
   Actual: AssertionError: expected ResourceNotFoundException before delete, got 'NotImplemented'

8) Expected: GetRepositorySyncStatus on a nonexistent RepositoryLinkId -> InvalidInputException (400).
   Actual: AssertionError: actual_code=NotImplemented

9) Expected: UpdateRepositoryLink on a nonexistent RepositoryLinkId -> ResourceNotFoundException (404).
   Actual: AssertionError: expected ResourceNotFoundException for a nonexistent RepositoryLinkId, got 'NotImplemented'

10) Expected: UpdateSyncBlocker with an unknown blocker Id -> SyncBlockerDoesNotExistException (404).
    Actual: AssertionError: actual_code=NotImplemented

11) Expected: UpdateSyncConfiguration on an unconfigured ResourceName -> InvalidInputException (400).
    Actual: AssertionError: expected InvalidInputException, got 'NotImplemented'

In every case the emulator answers the error code `NotImplemented`, which appears in no operation's modeled error set in com-amazonaws-codeconnections-codeconnections-20231201.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK codeconnections client at it, then:

1. CreateConnection: cc.create_connection(ConnectionName='demo-conn', ProviderType='GitHub')
   -> expect 200 + ConnectionArn; observe ClientError(NotImplemented).
2. CreateConnection validation: cc.create_connection(ConnectionName='demo-conn', ProviderType='NotARealProvider'); also ConnectionName of 66 chars; also HostArn='not-an-arn'
   -> expect ValidationException (400) naming the offending member and its constraint; observe NotImplemented.
3. CreateHost: cc.create_host(Name='demo-host', ProviderType='GitHubEnterpriseServer', ProviderEndpoint='https://ghe.example.com')
   -> expect 200 + HostArn; observe NotImplemented. Repeat with a ProviderEndpoint longer than 512 chars -> expect ValidationException.
4. ListConnections: cc.list_connections() -> expect 200 with a Connections key; and cc.list_connections(HostArnFilter='arn:aws:codeconnections:<region>:<acct>:host/<random-uuid>') -> expect ResourceNotFoundException (404). Observe NotImplemented for both.
5. GetSyncConfiguration: cc.get_sync_configuration(SyncType='CFN_STACK_SYNC', ResourceName='nosuchresource') -> expect ResourceNotFoundException with HTTP 404; observe NotImplemented.
6. GetRepositorySyncStatus: cc.get_repository_sync_status(Branch='main', RepositoryLinkId=str(uuid4()), SyncType='CFN_STACK_SYNC') -> expect InvalidInputException (400); observe NotImplemented.
7. UpdateRepositoryLink: cc.update_repository_link(RepositoryLinkId=str(uuid4())) -> expect ResourceNotFoundException (404); observe NotImplemented.
8. UpdateSyncBlocker: cc.update_sync_blocker(Id='blocker-1', SyncType='CFN_STACK_SYNC', ResourceName='nosuchresource', ResolvedReason='x') -> expect SyncBlockerDoesNotExistException (404); observe NotImplemented.
9. UpdateSyncConfiguration: cc.update_sync_configuration(ResourceName='nosuchresource', SyncType='CFN_STACK_SYNC', Branch='main') -> expect InvalidInputException (400); observe NotImplemented.

Or replay the 42 check ids listed in trace_ids.
