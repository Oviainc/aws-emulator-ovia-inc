Issue-Source: trace-sweep-divergence
Issue-Trace: codeconnections-host-delete-blocked-by-connection
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeconnections-host-delete-blocked-by-connection codeconnections-repository-link-errors codeconnections-repository-link-lifecycle codeconnections-sync-status-and-blockers
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections|DeleteHost|4xx->4xx|$.Message
Issue-Key: codeconnections|CreateRepositoryLink|4xx->4xx|$.Message
Issue-Key: codeconnections|CreateRepositoryLink|4xx->4xx|status
Issue-Key: codeconnections|GetRepositoryLink|4xx->4xx|$.Message
Issue-Key: codeconnections|GetRepositorySyncStatus|4xx->4xx|$.Message
Issue-Key: codeconnections|GetRepositorySyncStatus|4xx->4xx|status
Issue-Key: codeconnections|UpdateSyncBlocker|4xx->4xx|$.Message
Issue-Key: codeconnections|UpdateSyncBlocker|4xx->4xx|status
Issue-Related: 20261029T175842Z-codeconnections-client-error-responses-use-the-modeled-httperror.md
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# codeconnections refusal messages are emulator-authored instead of the service's fixed text

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteHost

## Description
SHARED FAULT (5 causes): five refusal paths return a `Message` the emulator wrote itself instead of the fixed text CodeConnections emits for that error class. The error codes are right; the sentences are not. Per-run values inside these messages (Request ID, and in cause 1 the generated host ARN) are NOT part of the requirement — only the fixed text and the request/state-derived substitutions are. Causes 2-5 ALSO carry the wrong HTTP status described by the separate status-mapping issue ("codeconnections client-error responses use the modeled httpError status instead of HTTP 400"); fixing the message alone will not make those steps pass, and fixing the status alone will not make them pass either.

Cause 1 of 5 — DeleteHost refused because connections are still associated with the host (fixture codeconnections-host-delete-blocked-by-connection, step task#4).
AWS: HTTP 400 (status matched here) with the fixed sentence "There exist connections for host <hostArn>, host cannot be deleted" — fixed text "There exist connections for host " + ", host cannot be deleted", with the host ARN taken from the request. (Smithy: DeleteHost errors = ResourceNotFoundException, ResourceUnavailableException; the message text is recorded behavior only.)
Emulator: HTTP 400 with "The host cannot be deleted while connections are associated with it. Delete the 1 connection(s) associated with the host and try again: <hostArn>" — a different sentence that additionally leaks a count AWS does not report.
Why: operators and tests that match on the service's refusal sentence (and tooling that parses it to extract the blocking host ARN) do not recognize the emulator's prose.
Fix: the DeleteHost handler's blocked-delete refusal text, stated as one template beside the service's other message constants (src/ops/delete_host.rs / src/state.rs message constants).
Traces: codeconnections-host-delete-blocked-by-connection#task#4.

Cause 2 of 5 — CreateRepositoryLink for a (connection, owner, repository) triple that already has a link (fixture codeconnections-repository-link-errors, step task#3).
AWS: HTTP 400 on the wire, body __type ResourceAlreadyExistsException, message "RepoConfiguration already exists. Arn: <repositoryLinkArn> (Service: CodeConnections; Status Code: 409; Error Code: ResourceAlreadyExistsException; Request ID: <id>; Proxy: null)" where the ARN uses the legacy `codestar-connections` ARN service namespace for the existing repository-link resource. (Smithy: CreateRepositoryLink errors include ResourceAlreadyExistsException, @httpError 409; the message template is recorded behavior only.)
Emulator: HTTP 409, and the same sentence with the ARN rendered in the `codeconnections` namespace ("arn:aws:codeconnections:...:repository-link/...").
Why: a client that stores or re-submits the ARN parsed out of this message gets a namespace the service does not use for repository-link ARNs, so the follow-up call addresses a resource that does not exist.
Fix: the repository-link ARN construction used when rendering the already-exists refusal (and the 400 status from the status-mapping issue) in the CreateRepositoryLink handler/state.
Note: the embedded Request ID differs on every run and is not part of the requirement.
Traces: codeconnections-repository-link-errors#task#3.

Cause 3 of 5 — GetRepositoryLink for a deleted/unknown repository-link id (fixture codeconnections-repository-link-lifecycle, step task#11).
AWS: HTTP 400 on the wire, body __type ResourceNotFoundException, message "Repository not found for accountId: <accountId> and uuid: <repositoryLinkId> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)" — the fixed text spells the label lowercase "uuid:"; accountId and the link id come from the request/caller.
Emulator: HTTP 404, and "... and UUID: ..." (uppercase label).
Why: the message is part of the wire contract clients assert on; a re-worded label breaks exact-match expectations while telling the user nothing new.
Fix: the GetRepositoryLink not-found message template (src/state.rs message constants), plus the status from the status-mapping issue.
Note: the Request ID differs per run and is not part of the requirement.
Traces: codeconnections-repository-link-lifecycle#task#11.

Cause 4 of 5 — GetRepositorySyncStatus for a repository link that has no sync attempt of the requested SyncType (fixture codeconnections-sync-status-and-blockers, step task#3).
AWS: HTTP 400 on the wire, body __type ResourceNotFoundException, message "Could not find a sync status for repository link: <repositoryLinkId>" — fixed text plus the repository-link id from the request; the branch and sync type are NOT named. (Smithy: GetRepositorySyncStatus errors include ResourceNotFoundException; message is recorded behavior only.)
Emulator: HTTP 404 with "No CFN_STACK_SYNC sync attempt was found for branch main of repository link <id>" — an invented sentence that interpolates the requested SyncType and Branch.
Why: clients matching the service's sentence do not match, and the emulator's variant implies the refusal is branch/type-specific when the service reports it per repository link.
Fix: the GetRepositorySyncStatus handler's not-found message template, plus the status from the status-mapping issue.
Traces: codeconnections-sync-status-and-blockers#task#3.

Cause 5 of 5 — UpdateSyncBlocker naming a blocker id that does not exist (fixture codeconnections-sync-status-and-blockers, step task#7).
AWS: HTTP 400 on the wire, body __type SyncBlockerDoesNotExistException, message "Sync blocker with id <id> does not exist " — the fixed text ends with a trailing space after "does not exist"; the id comes from the request. (Smithy: UpdateSyncBlocker errors include SyncBlockerDoesNotExistException, @httpError 404; the message text is recorded behavior only.)
Emulator: HTTP 404 and the same sentence without the trailing space.
Why: byte-exact message assertions (and golden-file comparisons) fail on the missing trailing character even though the text looks identical.
Fix: the UpdateSyncBlocker handler's blocker-not-found message constant, plus the status from the status-mapping issue.
Traces: codeconnections-sync-status-and-blockers#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator:
- DeleteHost (codeconnections-host-delete-blocked-by-connection#task#4), status 400 both: $.Message expected 'There exist connections for host arn:aws:codeconnections:...:host/<id>, host cannot be deleted', got 'The host cannot be deleted while connections are associated with it. Delete the 1 connection(s) associated with the host and try again: arn:aws:codeconnections:...:host/<id>'.
- CreateRepositoryLink (codeconnections-repository-link-errors#task#3): status expected 400, got 409; $.Message expected 'RepoConfiguration already exists. Arn: arn:aws:codestar-connections:us-west-2:000000000000:repository-link/<uuid> (Service: CodeConnections; Status Code: 409; Error Code: ResourceAlreadyExistsException; Request ID: <per-run>; Proxy: null)', got the same sentence with 'arn:aws:codeconnections:...:repository-link/<uuid>'.
- GetRepositoryLink (codeconnections-repository-link-lifecycle#task#11): status expected 400, got 404; $.Message expected '... and uuid: <uuid> ...', got '... and UUID: <uuid> ...'.
- GetRepositorySyncStatus (codeconnections-sync-status-and-blockers#task#3): status expected 400, got 404; $.Message expected 'Could not find a sync status for repository link: <uuid>', got 'No CFN_STACK_SYNC sync attempt was found for branch main of repository link <uuid>'.
- UpdateSyncBlocker (codeconnections-sync-status-and-blockers#task#7): status expected 400, got 404; $.Message expected 'Sync blocker with id nonexistent-blocker does not exist ' (trailing space), got 'Sync blocker with id nonexistent-blocker does not exist'.
Request IDs embedded in the recorded messages are per-run values and are excluded from the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay codeconnections-host-delete-blocked-by-connection step task#4: create a GitHub Enterprise Server host, create a connection on it, then DeleteHost the host — compare the refusal $.Message with the recorded sentence.
3. Replay codeconnections-repository-link-errors step task#3 (CreateRepositoryLink twice with the same connection/owner/repository), codeconnections-repository-link-lifecycle step task#11 (GetRepositoryLink after DeleteRepositoryLink), codeconnections-sync-status-and-blockers step task#3 (GetRepositorySyncStatus on a link with no sync attempt) and step task#7 (UpdateSyncBlocker with id 'nonexistent-blocker'), comparing $.Message and the status line in each.
