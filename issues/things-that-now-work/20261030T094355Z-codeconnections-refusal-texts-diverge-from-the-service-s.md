Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.host-delete-removes-and-not-found codeconnections.connection-invalid-provider-type-rejected codeconnections.create-repository-link-duplicate-rejected codeconnections.get-repository-link-not-found codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected codeconnections.tag-resource-limit-exceeded-rejected codeconnections.update-sync-blocker-not-found
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections.host-delete-removes-and-not-found|run#15|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Key: codeconnections.connection-invalid-provider-type-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.create-repository-link-duplicate-rejected|run#13|AssertionError|assert message == expected_message, (
Issue-Key: codeconnections.get-repository-link-not-found|run#20|AssertionError|assert deleted_code_message == expected_deleted_code_message, (
Issue-Key: codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected|run#23|AssertionError|assert final_code_message == expected_final_code_message, (
Issue-Key: codeconnections|TagResource|LimitExceededException
Issue-Key: codeconnections.update-sync-blocker-not-found|run#5|AssertionError|assert message == "Sync blocker with id blocker-1 does not exist ", f"message: {message!r}"
Issue-Commit: 4d1cac4581690df404b0414e482f7f8268561e90

# codeconnections refusal texts diverge from the service's fixed wording (enum order, legacy ARN namespace, uuid label, tag-cap sentence, trailing space) and UpdateHost is still a NotImplemented stub

> _Note: this issue was identified against the code base at commit `4d1cac4581690df404b0414e482f7f8268561e90`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHost

## Description
SHARED SERVICE, 6 SEPARATE ROOT CAUSES. The codeconnections emulator now answers almost the whole control plane correctly (38/45 behavioral checks pass): the error CODES and the resource effects are right in every case below. What still diverges is one unimplemented mutation (UpdateHost) and five refusal MESSAGE templates that the emulator authors itself instead of emitting the sentence the service emits for that error class. Per-run values inside these messages (Request ID, generated ARNs/uuids, generated names) are NOT part of any requirement — only the fixed text and the substitutions taken from the request or from emulator state are.

There are SIX separate causes in this issue; each must be closed before submitting.

Related open issues (neither carries these behavioral check ids, so neither closes them): '20261029T175842Z-codeconnections-refusal-messages-are-emulator-authored-instead-of.md' describes the same message faults as causes 2, 3 and 6 from the WIRE corpus, and '20261029T175842Z-codeconnections-client-error-responses-use-the-modeled-httperror.md' covers the separate HTTP-status mapping question (the behavioral checks here assert only code+message, so status is not part of this issue). Cause 1's operation is covered by no open issue; the fixed issue '20261022T201008Z-codeconnections-core-control-plane-answers-notimplemented-8-operation.md' named this check id under its CreateHost cause but listed only 8 operations, and UpdateHost was not one of them.

=== Cause 1 of 6: UpdateHost is still an unimplemented scaffold stub ===
AWS: Smithy: UpdateHost(input UpdateHostInput{HostArn required @length 0..256 @pattern ^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$, ProviderEndpoint @length 1..512, VpcConfiguration}) -> UpdateHostOutput{}; modeled errors ConflictException (409), ResourceNotFoundException (404), ResourceUnavailableException (400), UnsupportedOperationException (400). A mutation addressed at a host ARN that no longer exists is refused with ResourceNotFoundException and the message 'No host exists with the ARN: <hostArn>' — fixed text 'No host exists with the ARN: ' plus the ARN taken verbatim from the request (recorded behavior for the message text; GetHost on the same deleted ARN already answers exactly this in the emulator).
Emulator: UpdateHost answers the scaffold error code 'NotImplemented' ('operation ... UpdateHost is not implemented in this emulator'), for a deleted host and therefore for every host.
Why: hosts can never be updated (ProviderEndpoint / VpcConfiguration changes are impossible), and because 'NotImplemented' is in no modeled error set for this operation, a client that catches ResourceNotFoundException to detect a vanished host, or ConflictException to retry, matches nothing and surfaces the refusal as an unclassified failure.
Fix: the UpdateHost handler — host lookup by ARN against host state, the ResourceNotFoundException path reusing the same not-found message constant GetHost uses, and the empty 200 output on success (services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/update_host.rs plus the service's host state store).
Traces: codeconnections.host-delete-removes-and-not-found.

=== Cause 2 of 6: the CreateConnection ValidationException for a bad ProviderType lists the enum members in the wrong order ===
AWS: Smithy: CreateConnection.ProviderType targets enum ProviderType; the framework refuses an off-enum value with ValidationException (400) and the standard message '1 validation error detected: Value '<rejected value>' at 'providerType' failed to satisfy constraint: Member must be one of: [<members>]'. The fixed text is that whole sentence; the substitutions are the rejected value and the member name from the request. The member LIST is itself fixed text: the service renders it as '[GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]' (recorded behavior only — note this is NOT the model's declaration order, which is Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps, so the order must be pinned to the recorded sequence rather than derived from the enum definition).
Emulator: same code and same sentence, but the member list is rendered in the model's declaration order: '... Member must be one of: [Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps]'.
Why: callers and golden-file tests that compare the validation message byte-for-byte (the standard AWS validation sentence is widely asserted on) do not match, and a client that parses the bracketed list to offer the user the allowed values presents them in an order the service never uses.
Fix: the ProviderType validation-message rendering in the CreateConnection input validation — the allowed-value sequence must be a fixed list beside the message template, not an iteration over the enum shape (src/ops/create_connection.rs / the service's ValidationException message builder).
Traces: codeconnections.connection-invalid-provider-type-rejected.

=== Cause 3 of 6: the CreateRepositoryLink duplicate refusal renders the existing link ARN in the codeconnections namespace instead of the legacy codestar-connections one ===
AWS: Smithy: CreateRepositoryLink errors include ResourceAlreadyExistsException (@httpError 409). A second link for an already-linked (ConnectionArn, OwnerId, RepositoryName) triple is refused with that code and the message 'RepoConfiguration already exists. Arn: <existingRepositoryLinkArn> (Service: CodeConnections; Status Code: 409; Error Code: ResourceAlreadyExistsException; Request ID: <id>; Proxy: null)' — fixed text 'RepoConfiguration already exists. Arn: ' plus the framework tail, with the ARN of the EXISTING repository link built in the legacy 'codestar-connections' ARN service namespace (arn:aws:codestar-connections:<region>:<account>:repository-link/<existingRepositoryLinkId>). Recorded behavior for the template; the region, account and link id are state-derived substitutions and the Request ID is per-run.
Emulator: identical code and sentence, but the ARN is built in the 'codeconnections' namespace ('arn:aws:codeconnections:<region>:<account>:repository-link/<id>').
Why: a client that parses the ARN out of this message to address or record the pre-existing link gets an ARN whose service namespace the service does not use for repository-link resources, so the follow-up call targets a resource that does not exist.
Fix: the repository-link ARN construction used when rendering the already-exists refusal in the CreateRepositoryLink handler (src/ops/create_repository_link.rs / the ARN builder in src/state.rs) — the duplicate-refusal message must use the legacy namespace.
Traces: codeconnections.create-repository-link-duplicate-rejected.

=== Cause 4 of 6: the repository-link not-found message capitalizes the 'uuid' label ===
AWS: Smithy: GetRepositoryLink errors include ResourceNotFoundException (@httpError 404). Reading a deleted or unknown RepositoryLinkId is refused with that code and the message 'Repository not found for accountId: <accountId> and uuid: <repositoryLinkId> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)' — the fixed text spells the label lowercase 'uuid:'; accountId comes from the caller and the link id from the request (recorded behavior for the text).
Emulator: same code, same sentence, but the label is uppercase: '... and UUID: <id> ...'.
Why: the message is part of the wire contract clients assert on; a re-cased label breaks exact-match expectations and log/alarm patterns while telling the user nothing new.
Fix: the repository-link not-found message constant used by the GetRepositoryLink path (src/state.rs message constants / src/ops/get_repository_link.rs) — one shared template so every repository-link lookup renders the same label.
Traces: codeconnections.get-repository-link-not-found, codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected (the second check reaches the same message on its post-delete read-back, after its SyncConfigurationStillExistsException leg passes).

=== Cause 5 of 6: the TagResource tag-cap refusal uses an emulator-authored sentence ===
AWS: Smithy: TagResource(input{ResourceArn required, Tags required @length 0..200}); modeled errors LimitExceededException (429), ResourceNotFoundException (404). The per-resource cap actually enforced is the standard 50 user tags, and growing a resource past it is refused with LimitExceededException and the message 'The resultant tag set must not have more than 50 user tags.' — entirely fixed text, with the cap being the service's tag limit (recorded behavior only: the model's @length 0..200 on TagList is a per-call bound, not this cap).
Emulator: the right code (LimitExceededException) and the right effect (the existing 50 tags are untouched, freeing a slot lifts the block), but the message 'Exceeded the maximum limit for tags (50).' — an invented sentence.
Why: a client that distinguishes the tag-set cap from the other LimitExceededException conditions on this operation by matching the service's sentence cannot, and operators grepping for the service's wording see nothing.
Fix: the tag-cap refusal message constant in the TagResource handler's tag-count validation (src/ops/tag_resource.rs / the tagging state's cap check).
Traces: codeconnections.tag-resource-limit-exceeded-rejected.

=== Cause 6 of 6: the UpdateSyncBlocker blocker-not-found message is missing its trailing space ===
AWS: Smithy: UpdateSyncBlocker errors include SyncBlockerDoesNotExistException (@httpError 404). A blocker Id that does not exist is refused with that code — the Id is evaluated before the ResourceName/SyncType pair, so an unconfigured ResourceName still surfaces this error — and the message 'Sync blocker with id <id> does not exist ' , whose fixed text ends with a trailing space after 'does not exist'; the id is substituted from the request (recorded behavior for the text).
Emulator: same code and ordering, same sentence, but without the trailing space: 'Sync blocker with id <id> does not exist'.
Why: byte-exact message assertions and golden-file comparisons fail on the missing trailing character even though the text looks identical to a human reader.
Fix: the blocker-not-found message constant in the UpdateSyncBlocker handler (src/ops/update_sync_blocker.rs) — the template must retain the trailing space.
Traces: codeconnections.update-sync-blocker-not-found.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout: codeconnections 38/45 passed (84%), 7 mismatched, 0 errored, 0 skipped. All 7 failures are in this service.

Cause 1 — codeconnections.host-delete-removes-and-not-found:
  CreateHost/GetHost/DeleteHost/ListHosts and the GetHost not-found message all passed; then
  AssertionError: expected UpdateHost on a deleted host to raise ResourceNotFoundException, got 'NotImplemented'
  (expected code ResourceNotFoundException with message 'No host exists with the ARN: <arn>', the ARN being the one from the request).

Cause 2 — codeconnections.connection-invalid-provider-type-rejected (code ValidationException matched):
  expected message "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]"
  got      message "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps]"
  ('NotARealProvider' and 'providerType' are request-derived substitutions; the bracketed member sequence is fixed text.)

Cause 3 — codeconnections.create-repository-link-duplicate-rejected (code ResourceAlreadyExistsException and the one-link-only effect both matched):
  expected 'RepoConfiguration already exists. Arn: arn:aws:codestar-connections:<region>:<account>:repository-link/<existing-link-id> (Service: CodeConnections; Status Code: 409; Error Code: ResourceAlreadyExistsException; Request ID: <id>; Proxy: null)'
  got      the same sentence with 'arn:aws:codeconnections:<region>:<account>:repository-link/<existing-link-id>'
  (region, account and link id are state-derived; the Request ID is per-run and masked by the check.)

Cause 4 — codeconnections.get-repository-link-not-found and codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected:
  expected 'Repository not found for accountId: <account> and uuid: <link-id> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'
  got      'Repository not found for accountId: <account> and UUID: <link-id> (... same tail ...)'
  The second check first exercised DeleteRepositoryLink being blocked by an active sync configuration (SyncConfigurationStillExistsException) and the unblock-then-delete leg — both passed — and failed only on this read-back message.

Cause 5 — codeconnections.tag-resource-limit-exceeded-rejected (code LimitExceededException matched, tag set unchanged, overwrite and restore legs passed):
  expected 'The resultant tag set must not have more than 50 user tags.'
  got      'Exceeded the maximum limit for tags (50).'

Cause 6 — codeconnections.update-sync-blocker-not-found (code SyncBlockerDoesNotExistException matched):
  expected 'Sync blocker with id blocker-1 does not exist ' (note the trailing space)
  got      'Sync blocker with id blocker-1 does not exist'
  ('blocker-1' is the Id from the request.)

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore/AWS SDK codeconnections client at it, then:

1. Cause 1: cc.create_host(Name=<n>, ProviderType='GitHubEnterpriseServer', ProviderEndpoint='https://ghe.example.com') -> HostArn; cc.delete_host(HostArn=arn); then cc.update_host(HostArn=arn, ProviderEndpoint='https://ghe-updated.example.com'). Expect ClientError ResourceNotFoundException with message 'No host exists with the ARN: <arn>' (the same message GetHost already returns); observe error code 'NotImplemented'.
2. Cause 2: cc.create_connection(ConnectionName=<n>, ProviderType='NotARealProvider'). Expect ValidationException whose message ends 'Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]'; observe the members in model-declaration order.
3. Cause 3: create a GitHub connection, cc.create_repository_link(ConnectionArn=..., OwnerId=<o>, RepositoryName=<r>), then repeat the identical CreateRepositoryLink. Expect ResourceAlreadyExistsException whose message renders the existing link ARN with the 'codestar-connections' namespace; observe 'codeconnections'.
4. Cause 4: cc.get_repository_link(RepositoryLinkId=<a deleted or never-created UUID>). Expect the message label 'and uuid:'; observe 'and UUID:'. The same message is reached by deleting a repository link (after removing its sync configuration) and reading it back.
5. Cause 5: create a connection, cc.tag_resource(ResourceArn=conn, Tags=[50 distinct keys]), then cc.tag_resource(ResourceArn=conn, Tags=[{'Key':'<a 51st new key>','Value':'v'}]). Expect LimitExceededException with message 'The resultant tag set must not have more than 50 user tags.'; observe 'Exceeded the maximum limit for tags (50).'
6. Cause 6: cc.update_sync_blocker(Id='blocker-1', SyncType='CFN_STACK_SYNC', ResourceName=<name with no sync configuration>, ResolvedReason='x'). Expect SyncBlockerDoesNotExistException with message 'Sync blocker with id blocker-1 does not exist ' including the trailing space; observe the message without it.

Or replay the 7 check ids listed in trace_ids.
