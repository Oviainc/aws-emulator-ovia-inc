Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.get-host-not-found codeconnections.create-connection-with-nonexistent-host-rejected codeconnections.get-resource-sync-status-not-found
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections.get-host-not-found|run#11|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codeconnections.create-connection-with-nonexistent-host-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: codeconnections|GetResourceSyncStatus|InvalidInputException
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# codeconnections: host lookups answer ResourceUnavailableException (400) instead of ResourceNotFoundException (404), and GetResourceSyncStatus rejects an unconfigured resource instead of answering 200 (2 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHost

## Description
Two net-new root causes in the codeconnections emulator, both of which change the error CODE / HTTP STATUS a client sees (not just message text).

Cause 1 of 2: a host ARN that does not resolve is reported as ResourceUnavailableException (HTTP 400) instead of ResourceNotFoundException (HTTP 404).
AWS: GetHost and CreateConnection both model BOTH errors — Smithy: GetHost errors = [ResourceNotFoundException (httpError 404), ResourceUnavailableException (httpError 400)]; CreateConnection errors = [LimitExceededException 429, ResourceNotFoundException 404, ResourceUnavailableException 400]. The recorded behavior shows the service uses ResourceNotFoundException for the "this host ARN names nothing" case: GetHost on a well-formed, never-created host ARN is refused with code ResourceNotFoundException, and CreateConnection with HostArn pointing at a nonexistent host is likewise refused with ResourceNotFoundException and creates no connection. Recorded message (example, built from the request): "No host exists with the ARN: <hostArn>".
Emulator: both calls are refused with code ResourceUnavailableException (the model gives that shape httpError 400), so the client sees a 400/ResourceUnavailableException where AWS sends 404/ResourceNotFoundException.
Why: ResourceUnavailableException is the "resource exists but is not usable" signal; clients and SDK waiters branch on ResourceNotFoundException to decide a host is absent (create-if-missing, cleanup-tolerating-404, retry-vs-fail). With the wrong code they treat an absent host as a transient/unusable-resource condition, and a 400 is not retried or swallowed the way a 404 is. CreateConnection additionally loses the "your HostArn does not exist" diagnosis.
Fix: the not-found classification in the GetHost handler and in CreateConnection's HostArn-existence check (the host lookup in the codeconnections ops layer) must map an unresolvable host ARN to ResourceNotFoundException/404; keep ResourceUnavailableException for an existing-but-unusable host.
Traces: codeconnections.get-host-not-found, codeconnections.create-connection-with-nonexistent-host-rejected.

Cause 2 of 2: GetResourceSyncStatus raises InvalidInputException (400) for a resource that has no sync configuration, where AWS answers 200 with no sync-state fields.
AWS: recorded behavior only — GetResourceSyncStatus for a ResourceName/SyncType pair that has never had a sync configuration SUCCEEDS and returns a payload with none of the semantic members populated (no DesiredState, no LatestSync, no LatestSuccessfulSync). Note the Smithy model marks GetResourceSyncStatusOutput.LatestSync as required, so the model does not predict this; the recording does. (Smithy: GetResourceSyncStatus errors = [AccessDeniedException 403, InternalServerException 503, InvalidInputException 400, ResourceNotFoundException 404, ThrottlingException 429].)
Emulator: the call is refused with code InvalidInputException, message example "Sync Configuration for Resource <name> was not found" — a 400 where AWS returns 200.
Why: a client polling sync status for a resource it has not yet configured gets an exception instead of an empty status, so "never synced" is indistinguishable from "bad request"; polling loops abort instead of reporting no sync state.
Fix: the GetResourceSyncStatus handler (its missing-configuration branch) must return a success response with no sync-state members instead of raising.
Traces: codeconnections.get-resource-sync-status-not-found.

## Evidence
Cause 1 — codeconnections.get-host-not-found: expected code ResourceNotFoundException, got 'ResourceUnavailableException' (AssertionError at check line 45). codeconnections.create-connection-with-nonexistent-host-rejected: expected ResourceNotFoundException, got 'ResourceUnavailableException' (check line 59). Recorded AWS message example for both: "No host exists with the ARN: arn:aws:codeconnections:<region>:<account>:host/<uuid>".
Cause 2 — codeconnections.get-resource-sync-status-not-found errored with botocore.errorfactory.InvalidInputException: An error occurred (InvalidInputException) when calling the GetResourceSyncStatus operation: Sync Configuration for Resource s2rsync17b73f1f7633 was not found; the check expects the call to succeed and the response (minus ResponseMetadata) to be empty.

## Steps to Reproduce
1. Boot the emulator and point a botocore codeconnections client at it.
2. Cause 1a: cc.get_host(HostArn='arn:aws:codeconnections:<region>:<account>:host/<random-uuid>') -> observe ResourceUnavailableException instead of ResourceNotFoundException.
3. Cause 1b: cc.create_connection(ConnectionName='x', HostArn='arn:aws:codeconnections:<region>:<account>:host/<random-uuid>') -> observe ResourceUnavailableException instead of ResourceNotFoundException.
4. Cause 2: cc.get_resource_sync_status(ResourceName='<never-configured-name>', SyncType='CFN_STACK_SYNC') -> observe InvalidInputException instead of a 200 response with no sync-state fields.
