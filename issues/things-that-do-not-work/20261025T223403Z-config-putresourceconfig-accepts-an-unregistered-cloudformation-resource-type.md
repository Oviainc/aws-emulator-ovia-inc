Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.put-resource-config-and-list-discovered
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.put-resource-config-and-list-discovered|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: e42d9f0f95dc249c28ad9205e8f81eb720b8f287

# Config PutResourceConfig accepts an unregistered CloudFormation resource type (200 empty body) instead of 400 ValidationException (1 cause)

> _Note: this issue was identified against the code base at commit `e42d9f0f95dc249c28ad9205e8f81eb720b8f287`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourceConfig

## Description
ONE cause must be closed.

Cause 1 of 1: with a running configuration recorder, PutResourceConfig accepts a ResourceType that is not a registered CloudFormation resource type.

AWS: PutResourceConfig takes required ResourceType (ResourceTypeString, @length 1..196), SchemaVersionId (@length 1..128, @pattern ^[A-Za-z0-9-]+$), ResourceId (@length 1..768) and Configuration, returns an empty output shape (Unit), and models InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException and ValidationException — all errorKind client, httpError 400 (Smithy: PutResourceConfig, model services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json). The model does not spell out the registration precondition, so for that part this is recorded behavior only: the recording shows that once a recorder IS running, a ResourceType naming a type that is not registered in the CloudFormation registry is refused with HTTP 400, error code ValidationException, and the message template "Cannot find registered Resource Type with name <ResourceType>. Please register the Resource Type with Cloudformation and try again." The fixed text of that message is "Cannot find registered Resource Type with name " + " . Please register the Resource Type with Cloudformation and try again."; its single variable part is the ResourceType value echoed verbatim from the request (the recording's "MyFidelityNamespace::Testing::Widget" is one filling of that slot, not part of the requirement). Nothing is recorded as a side effect: the rejected resource id never appears in ListDiscoveredResources and BatchGetResourceConfig returns it in neither baseConfigurationItems nor unprocessedResourceKeys.

Emulator: the same call answers HTTP 200 with an empty JSON body ({}, content-length 2, content-type application/x-amz-json-1.1) and records the item — no ValidationException is raised at all. The check's earlier no-recorder precondition now passes (NoRunningConfigurationRecorderException and its exact message are produced correctly), so the divergence is purely the missing resource-type registration validation on the accept path.

Why: a client gets a write path that is strictly more permissive than AWS. Configuration items can be injected for resource types that do not exist, polluting the recorded inventory with types AWS would refuse; every client code path that handles ValidationException for an unregistered type is dead code against the emulator, and a caller that relies on the refusal to detect a typo'd or unregistered third-party type (the normal CloudFormation-registry contract) silently gets a 200 and a phantom resource.

Fix: the resource-type validation in the PutResourceConfig handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_resource_config.rs) — after the running-recorder precondition, reject a ResourceType absent from the registered resource-type set with 400 ValidationException carrying the message template above, and do not mutate recorded state on that path.

Related already-open issue: 20261023T140058Z-config-write-paths-unvalidated-or-wrongly-refused-putresourceconfig.md describes the same unregistered-type acceptance as its Cause 1, but it carries this check only for its Cause 6 (the NoRunningConfigurationRecorderException message, which now passes) and keys Cause 1 on other check ids. This issue is filed because this check's failure is no longer covered by any replay key in that issue; if Cause 1 there is fixed, re-replay config.put-resource-config-and-list-discovered to confirm this one closes with it.

Traces: config.put-resource-config-and-list-discovered (failing assertion is the second _error(...) call, after the recorder is started).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; 64/117 passed, config 61/103).

[mismatch] config.put-resource-config-and-list-discovered
  File "check_config_put_resource_config_and_list_discovered.py", line 394, in run
    code, code_message = _error(
      lambda: cfg.put_resource_config(ResourceType=resource_type, SchemaVersionId="1-0", ResourceId=resource_id, Configuration=configuration))
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Expected (recorded AWS behavior asserted by the check):
  HTTP 400, Error.Code == "ValidationException"
  Error.Message == "Cannot find registered Resource Type with name <ResourceType>. Please register the Resource Type with Cloudformation and try again."
  with <ResourceType> taken from the request (the run used "MyFidelityNamespace::Testing::Widget").

Actual: HTTP 200, empty body {}, no Error member; the resource is then recorded.

The preceding no-recorder assertion in the same check passed, i.e. the emulator already returns 400 NoRunningConfigurationRecorderException with "You must create and start a configuration recorder before you call the PutResourceConfig API." — so the failure is isolated to the post-start resource-type validation.

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — PutResourceConfig input requires ResourceType/SchemaVersionId/ResourceId/Configuration, output is Unit, and ValidationException is client/400.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Create an S3 delivery bucket, a Config recorder role, PutConfigurationRecorder + PutDeliveryChannel, then StartConfigurationRecorder so a recorder is RUNNING.
3. Call: aws config put-resource-config --resource-type 'MyFidelityNamespace::Testing::Widget' --schema-version-id 1-0 --resource-id <any-id> --configuration '{"Id":"<any-id>","Name":"fidelity widget","Enabled":true}'
4. Observe HTTP 200 with an empty body. Expected: HTTP 400 ValidationException with message "Cannot find registered Resource Type with name MyFidelityNamespace::Testing::Widget. Please register the Resource Type with Cloudformation and try again."
5. Re-replay check config.put-resource-config-and-list-discovered to confirm a fix.
