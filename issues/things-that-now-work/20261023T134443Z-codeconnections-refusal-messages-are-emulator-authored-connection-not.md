Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.get-connection-not-found codeconnections.connection-delete-removes-and-not-found codeconnections.connection-arn-never-reused-after-delete codeconnections.concurrent-delete-connection-exactly-one-succeeds codeconnections.list-tags-for-resource-not-found codeconnections.get-repository-link-not-found codeconnections.update-repository-link-not-found codeconnections.create-repository-link-duplicate-rejected codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected codeconnections.get-sync-configuration-not-found codeconnections.delete-sync-configuration-nonexistent-is-idempotent
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections.get-connection-not-found|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.connection-delete-removes-and-not-found|run#10|AssertionError|assert get_message == expected, f"expected message {expected!r}, got {get_message!r}"
Issue-Key: codeconnections.connection-arn-never-reused-after-delete|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.concurrent-delete-connection-exactly-one-succeeds|run#16|AssertionError|assert all(msg == expected for _, (_code, msg) in errors), (
Issue-Key: codeconnections.list-tags-for-resource-not-found|run#10|AssertionError|assert message == "No connection exists with the ARN", f"message: {message!r}"
Issue-Key: codeconnections.get-repository-link-not-found|run#15|AssertionError|assert fake_code_message == expected_fake_code_message, (
Issue-Key: codeconnections.update-repository-link-not-found|run#6|AssertionError|assert message == expected_message, (
Issue-Key: codeconnections.create-repository-link-duplicate-rejected|run#13|AssertionError|assert message == expected_message, (
Issue-Key: codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected|run#15|AssertionError|assert message == expected_message, (
Issue-Key: codeconnections.get-sync-configuration-not-found|run#16|AssertionError|assert message == expected_message, (
Issue-Key: codeconnections.delete-sync-configuration-nonexistent-is-idempotent|run#6|AssertionError|assert message == expected_message, (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# codeconnections refusal messages are emulator-authored: connection not-found leaks the ARN instead of the service's fixed text, and every repository-link / sync-configuration refusal drops the service-framework tail (5 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
Five net-new root causes, all in the message body the codeconnections emulator attaches to refusals whose error CODE is already correct. These messages have a known rule: CodeConnections refusals use a fixed per-case template interpolated from the request (account id, resource id, owner/repo), and the service framework appends a fixed tail " (Service: CodeConnections; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)" to the message — the request id inside it is the only per-run part and the checks mask it. Recorded message texts below are examples of each template.

Cause 1 of 5: connection-scoped not-found refusals use emulator prose that appends the ARN, instead of the service's fixed text.
AWS: recorded behavior only — GetConnection, TagResource and ListTagsForResource against a connection ARN that does not resolve (never created, or deleted), and every losing racer of a concurrent DeleteConnection, are refused with code ResourceNotFoundException and the fixed message "No connection exists with the ARN" — no ARN appended. (Smithy: ResourceNotFoundException httpError 404, modeled on GetConnection/ListTagsForResource/DeleteConnection; its own doc prose is "Resource not found. Verify the connection resource ARN and try again.")
Emulator: returns ResourceNotFoundException (correct) with "Resource not found. Verify the connection resource ARN and try again: <full connection arn>", and for ListTagsForResource "Resource not found. Verify the resource ARN and try again: <arn>" — i.e. the shape's doc sentence plus the request's ARN.
Why: clients that match the documented refusal text (and recorded fixtures of it) do not match, and the emulator echoes the full ARN where AWS returns a constant.
Fix: the not-found refusal construction shared by the connection read/tag/delete handlers in the codeconnections ops layer.
Traces: codeconnections.get-connection-not-found, codeconnections.connection-delete-removes-and-not-found, codeconnections.connection-arn-never-reused-after-delete, codeconnections.concurrent-delete-connection-exactly-one-succeeds, codeconnections.list-tags-for-resource-not-found.

Cause 2 of 5: repository-link refusals omit the service-framework tail entirely.
AWS: recorded behavior only — GetRepositoryLink and UpdateRepositoryLink on a nonexistent RepositoryLinkId are refused with ResourceNotFoundException and "Repository not found for accountId: <account> and UUID: <id>" followed by the framework tail " (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)". (Smithy: GetRepositoryLink/UpdateRepositoryLink model ResourceNotFoundException httpError 404.)
Emulator: emits the template body correctly but with no tail at all.
Why: the tail is part of every CodeConnections refusal message a client sees and logs; its absence changes every message of this family, and clients/tests that parse the status or code out of the message text get nothing.
Fix: the codeconnections error serializer should append the framework tail (service name, http status, error code, request id) to refusal messages.
Traces: codeconnections.get-repository-link-not-found, codeconnections.update-repository-link-not-found.

Cause 3 of 5: CreateRepositoryLink duplicate refusal uses emulator prose instead of the service template.
AWS: recorded behavior only — a second CreateRepositoryLink for the same (ConnectionArn, OwnerId, RepositoryName) triple is refused with ResourceAlreadyExistsException and "RepoConfiguration already exists. Arn: arn:aws:codestar-connections:<region>:<account>:repository-link/<existing link id>" + the 409 framework tail. (Smithy: CreateRepositoryLink errors include ResourceAlreadyExistsException httpError 409.)
Emulator: code is correct, message is "Unable to create resource. Repository link <id> already links <owner>/<repo> through that connection." — a different template, no ARN of the existing link, no tail.
Why: the refusal no longer carries the existing link's ARN in the form AWS emits, so a client that extracts it to adopt the existing link cannot.
Fix: CreateRepositoryLink's duplicate-detection refusal message.
Traces: codeconnections.create-repository-link-duplicate-rejected.

Cause 4 of 5: DeleteRepositoryLink's "blocked by sync configuration" refusal uses emulator prose.
AWS: recorded behavior only — DeleteRepositoryLink while a CFN_STACK_SYNC configuration references the link is refused with code SyncConfigurationStillExistsException and "Cfn stack sync configurations still exist for repository <ownerId>/<repositoryName>. Please delete them before retrying." + the 409 framework tail (whose Error Code field reads ResourceStillExistsException). (Smithy: DeleteRepositoryLink errors include SyncConfigurationStillExistsException httpError 409.)
Emulator: code is correct, message is "Unable to continue. The CFN_STACK_SYNC sync configuration for resource <resourceName> still uses repository link <linkId>." — different template, no tail.
Why: the message no longer identifies the blocked repository as owner/repo the way AWS does, and carries no framework tail.
Fix: DeleteRepositoryLink's still-referenced refusal message.
Traces: codeconnections.delete-repository-link-blocked-by-active-sync-configuration-rejected.

Cause 5 of 5: GetSyncConfiguration not-found uses a resourceName-based template instead of AWS's configTarget template.
AWS: recorded behavior only — GetSyncConfiguration for a ResourceName/SyncType with no sync configuration is refused with ResourceNotFoundException and "CloudFormation Stack Sync configuration not found for accountId: <account> and configTarget: cfn-stack-sync/arn:aws:cloudformation:<region>:<account>:stack/<resourceName>" + the 404 framework tail. (Smithy: GetSyncConfiguration errors include ResourceNotFoundException httpError 404.)
Emulator: code is correct, message is "CFN_STACK_SYNC configuration not found for accountId: <account> and resourceName: <resourceName>" — the sync type is spelled as the enum token instead of "CloudFormation Stack Sync", the configTarget (a cfn-stack-sync/<stack ARN> locator derived from the request) is replaced by a bare resourceName, and there is no tail.
Why: a client reading the configTarget locator out of the refusal (to report which stack has no sync configuration) gets nothing; the DeleteSyncConfiguration idempotency flow that reads this message pre/post delete sees a different text.
Fix: GetSyncConfiguration's missing-configuration refusal message.
Traces: codeconnections.get-sync-configuration-not-found, codeconnections.delete-sync-configuration-nonexistent-is-idempotent.

## Evidence
Cause 1 — get-connection-not-found / connection-delete-removes-and-not-found / connection-arn-never-reused-after-delete: expected message 'No connection exists with the ARN', got 'Resource not found. Verify the connection resource ARN and try again: arn:aws:codeconnections:us-west-2:000000000000:connection/<uuid>'. concurrent-delete-connection-exactly-one-succeeds: all 7 losing racers returned the same emulator message (codes were correctly ResourceNotFoundException, and the race partition itself passed). list-tags-for-resource-not-found: got 'Resource not found. Verify the resource ARN and try again: <arn>'.
Cause 2 — get-repository-link-not-found: got 'Repository not found for accountId: 000000000000 and UUID: <id>', expected the same text + ' (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'. update-repository-link-not-found: identical shortfall.
Cause 3 — create-repository-link-duplicate-rejected: got 'Unable to create resource. Repository link <id> already links <owner>/<repo> through that connection.', expected 'RepoConfiguration already exists. Arn: arn:aws:codestar-connections:us-west-2:000000000000:repository-link/<id> (Service: CodeConnections; Status Code: 409; Error Code: ResourceAlreadyExistsException; Request ID: <id>; Proxy: null)'.
Cause 4 — delete-repository-link-blocked-by-active-sync-configuration-rejected: got 'Unable to continue. The CFN_STACK_SYNC sync configuration for resource <name> still uses repository link <id>.', expected 'Cfn stack sync configurations still exist for repository <owner>/<repo>. Please delete them before retrying. (Service: CodeConnections; Status Code: 409; Error Code: ResourceStillExistsException; Request ID: <id>; Proxy: null)'.
Cause 5 — get-sync-configuration-not-found / delete-sync-configuration-nonexistent-is-idempotent: got 'CFN_STACK_SYNC configuration not found for accountId: 000000000000 and resourceName: <name>', expected 'CloudFormation Stack Sync configuration not found for accountId: 000000000000 and configTarget: cfn-stack-sync/arn:aws:cloudformation:us-west-2:000000000000:stack/<name> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'.

## Steps to Reproduce
1. Boot the emulator and point a botocore codeconnections client at it.
2. Cause 1: create a connection, delete it, then cc.get_connection / cc.tag_resource / cc.list_tags_for_resource on the dead ARN (and on a fabricated connection ARN) — inspect Error.Message.
3. Cause 2: cc.get_repository_link(RepositoryLinkId=<random uuid>) and cc.update_repository_link(RepositoryLinkId=<random uuid>) — note the absent ' (Service: CodeConnections; ...)' tail.
4. Cause 3: create a connection and a repository link, then repeat CreateRepositoryLink with the identical (ConnectionArn, OwnerId, RepositoryName) — inspect Error.Message.
5. Cause 4: create a repository link plus a CFN_STACK_SYNC sync configuration for it, then cc.delete_repository_link on that link — inspect Error.Message.
6. Cause 5: cc.get_sync_configuration(SyncType='CFN_STACK_SYNC', ResourceName='<never-configured>') — inspect Error.Message.
