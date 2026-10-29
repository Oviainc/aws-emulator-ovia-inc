Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.generated-template-and-scan-not-found-errors cloudformation.resource-scan-lifecycle cloudformation.resource-scan-result-ttl-expiry cloudformation.generated-template-retention-window-expiry cloudformation.type-registry-not-found-errors cloudformation.type-version-publish-and-test cloudformation.export-value-update-refused-while-imported
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.generated-template-and-scan-not-found-errors|_expect_code#8|AssertionError|assert actual == message, f"{code}: expected message {message!r}, got {actual!r}"
Issue-Key: cloudformation.resource-scan-lifecycle|run#11|AssertionError|assert any(s["ResourceScanId"] == scan_id and s["Status"] == "COMPLETE" for s in summaries), summaries
Issue-Key: cloudformation.resource-scan-result-ttl-expiry|run#8|AssertionError|assert gate_code == "ResourceScanInProgress", gate_code
Issue-Key: cloudformation.generated-template-retention-window-expiry|run#22|AssertionError|assert update_code == "GeneratedTemplateNotFound", (
Issue-Key: cloudformation|ActivateType|ValidationError
Issue-Key: cloudformation.type-version-publish-and-test|run#16|AssertionError|assert describe_error == expected_describe, (
Issue-Key: cloudformation.export-value-update-refused-while-imported|run#24|AssertionError|assert control["outcome"] == "UPDATE_COMPLETE", (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation: 7 net-new root causes in the resource-scan, generated-template and type-registry surfaces plus the missing in-place AWS::SNS::Topic update path

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceScans

## Description
Seven distinct net-new root causes of service `cloudformation`, each grounded in its own failing behavioral check. A fixer must close all seven before submitting.

Cause 1 of 7 — the ResourceScanNotFound message is emulator-authored.
AWS: Smithy models ResourceScanNotFoundException on DescribeResourceScan and ListResourceScanResources: errorKind client, httpError 400, awsQueryError code `ResourceScanNotFound`, Message @length 1..255. Recorded behavior: the Message is the fixed sentence `There is no ResourceScan found for the ResourceScanId.` — entirely fixed text, with no part taken from the request (the offending scan id is NOT echoed).
Emulator: 400 ResourceScanNotFound with `Resource scan <resource-scan-arn> cannot be found` — a reworded sentence that interpolates the requested ARN.
Why: an error-mapping layer matching AWS's fixed sentence never matches, and the echoed ARN makes the message unstable per call.
Fix: the not-found refusal builder of the resource-scan handlers (DescribeResourceScan / ListResourceScanResources) in the cloudformation crate.
Traces: cloudformation.generated-template-and-scan-not-found-errors

Cause 2 of 7 — a scan that StartResourceScan created and DescribeResourceScan reports COMPLETE is absent from ListResourceScans.
AWS: Smithy: ListResourceScans returns ResourceScanSummaries (paginated, items ResourceScanSummaries) and has no modelled errors; recorded behavior: a scan started in this account/Region appears in ListResourceScans with its ResourceScanId and its current Status, so the same scan is visible from both the item read and the listing.
Emulator: StartResourceScan mints an id and DescribeResourceScan drives it to COMPLETE, but ListResourceScans answers 200 with an EMPTY ResourceScanSummaries list — the account's scan index is never populated.
Why: the documented discovery flow (list scans, adopt/await the in-progress one, then read its resources) is impossible; a client cannot see that a scan exists or ran, and an "only one scan at a time" caller cannot find the scan it must wait on.
Fix: the StartResourceScan handler must register the scan in the account/Region scan state that ListResourceScans reads (shared resource-scan state in the cloudformation crate). Related issues claiming these operations were implemented are already marked fixed (20261025T015306Z-cloudformation-startresourcescan-stub.md, 20261025T015306Z-cloudformation-listresourcescans-stub.md); neither covers this: both calls now answer 200, they simply do not share state.
Traces: cloudformation.resource-scan-lifecycle

Cause 3 of 7 — the ResourceScanInProgress gate is unreachable: ListResourceScanResources answers NotImplemented.
AWS: Smithy: ListResourceScanResources models ResourceScanInProgressException (client, httpError 400, awsQueryError `ResourceScanInProgress`) and ResourceScanNotFoundException. Recorded behavior: while the scan is IN_PROGRESS the call is refused 400 ResourceScanInProgress with the fixed Message `The ResourceScan is in progress. Please wait for it to finish before performing any further operations on it. Describe ResourceScans to see the status of the in-progress ResourceScan.`
Emulator: the call is refused with code `NotImplemented` (scaffold stub), so neither the in-progress gate nor the resource listing exists.
Why: a caller cannot tell "not ready yet, poll" from "unsupported", and never gets the scanned resources.
Fix: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_resource_scan_resources.rs. A scaffold-stub issue naming that operation and file is already filed by the suite; this check was not attributed to it, so its replay key is recorded here and the two must be closed together.
Traces: cloudformation.resource-scan-result-ttl-expiry

Cause 4 of 7 — UpdateGeneratedTemplate answers NotImplemented, so a deleted generated template is never refused with GeneratedTemplateNotFound.
AWS: Smithy: UpdateGeneratedTemplate models GeneratedTemplateNotFoundException (client, httpError 404, awsQueryError `GeneratedTemplateNotFound`), AlreadyExistsException and LimitExceededException; recorded behavior: naming a generated template that was deleted is refused 404 GeneratedTemplateNotFound.
Emulator: the call is refused with code `NotImplemented`.
Why: the generated-template lifecycle has no mutation path and a client cannot distinguish "this template is gone" from "this API is absent".
Fix: .../src/ops/update_generated_template.rs. A scaffold-stub issue naming that operation is already filed; this check was not attributed to it and its replay key is recorded here.
Traces: cloudformation.generated-template-retention-window-expiry

Cause 5 of 7 — ActivateType refuses a public type ARN with a Smithy pattern violation instead of the registry's refusal.
AWS: Smithy: ActivateType.PublicTypeArn carries pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$` and the operation models CFNRegistryException (client, httpError 400) and TypeNotFoundException (404). Recorded behavior: AWS does NOT answer a constraint violation for this ARN shape — it answers 400 CFNRegistryException with Message `TypeName is invalid` (fixed text).
Emulator: answers ValidationError with `1 validation error detected: Value '<arn>' at 'publicTypeArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:...` — the wrong error class, a non-AWS code for this path, and a message that echoes the caller's value.
Why: callers that catch CFNRegistryException to detect an unusable public extension ARN instead see a generic validation error, and the taxonomy (registry error vs request-shape error) is inverted relative to AWS.
Fix: the ActivateType handler's ARN resolution/validation ordering in the cloudformation crate — the registry lookup must own this refusal.
Traces: cloudformation.type-registry-not-found-errors

Cause 6 of 7 — TypeNotFoundException names the ARN instead of the type name.
AWS: Smithy: TypeNotFoundException is client, httpError 404, awsQueryError `TypeNotFoundException`, Message @length 1..255, doc "The specified extension doesn't exist in the CloudFormation registry." Recorded behavior: DescribeType against a type version ARN that was never registered answers 404 TypeNotFoundException with Message `The type '<Namespace::Service::Resource>' cannot be found.` — fixed words "The type '" ... "' cannot be found."; the variable part is the extension's three-part TYPE NAME, resolved from the ARN, not the ARN itself.
Emulator: Message `The type 'arn:aws:cloudformation:<region>:<account>:type/resource/<Namespace-Service-Resource>' cannot be found.` — it interpolates the raw ARN (with the type name's `::` separators flattened to `-`).
Why: tools that parse the type name out of this message to report which extension is missing read an ARN; the reported identity differs from the one AWS reports for the same request.
Fix: the type-registry not-found refusal builder (DescribeType and the sibling registry reads) must render the resolved type name.
Traces: cloudformation.type-version-publish-and-test

Cause 7 of 7 — there is no in-place update path for AWS::SNS::Topic, so any stack update touching a topic's properties ends UPDATE_FAILED.
AWS: recorded behavior only — an UpdateStack that changes only a parameter feeding an existing AWS::SNS::Topic's properties reaches UPDATE_COMPLETE, the topic being modified in place (no replacement).
Emulator: the stack lands UPDATE_FAILED with ResourceStatusReason `The following resource(s) failed to update: [<logical-id>]. NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Topic in place; the resource was left as it is.`
Why: every stack carrying an SNS topic becomes un-updatable; worse, the failure is reported as the stack's verdict, so a caller cannot tell an unsupported provisioner from a genuine resource failure, and the positive control of any export/import scenario over a topic cannot run.
Fix: the AWS::SNS::Topic resource provisioner's update path in the cloudformation crate's resource-provisioner layer. The same family is already recorded for other types (fixed: 20261023T135552Z-... AWS::SQS::Queue in-place update, 20261025T121312Z-... AWS::IAM::Role AssumeRolePolicyDocument update); neither covers AWS::SNS::Topic.
Traces: cloudformation.export-value-update-refused-while-imported

## Evidence
Cause 1 — cloudformation.generated-template-and-scan-not-found-errors:
  AssertionError: ResourceScanNotFound: expected message 'There is no ResourceScan found for the ResourceScanId.', got 'Resource scan arn:aws:cloudformation:us-west-2:000000000000:resourceScan/<uuid> cannot be found'

Cause 2 — cloudformation.resource-scan-lifecycle (StartResourceScan returned a ScanId and DescribeResourceScan reached COMPLETE before this line):
  assert any(s["ResourceScanId"] == scan_id and s["Status"] == "COMPLETE" for s in summaries) -> AssertionError: []

Cause 3 — cloudformation.resource-scan-result-ttl-expiry:
  assert gate_code == "ResourceScanInProgress" -> AssertionError: NotImplemented   (gate call: cf.list_resource_scan_resources(ResourceScanId=scan_id) while DescribeResourceScan reported IN_PROGRESS)

Cause 4 — cloudformation.generated-template-retention-window-expiry:
  AssertionError: expected UpdateGeneratedTemplate on deleted '<name>' to fail with GeneratedTemplateNotFound, got 'NotImplemented'

Cause 5 — cloudformation.type-registry-not-found-errors:
  AssertionError: expected ('CFNRegistryException', 'TypeName is invalid'), got ('ValidationError', "1 validation error detected: Value 'arn:aws:cloudformation:us-west-2::type/resource/<name>/00000001' at 'publicTypeArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$")

Cause 6 — cloudformation.type-version-publish-and-test:
  AssertionError: expected ('TypeNotFoundException', "The type 'Sim2Real::Probe::Nonexistent' cannot be found.") from DescribeType on the same nonexistent type version ARN, got ('TypeNotFoundException', "The type 'arn:aws:cloudformation:us-west-2:000000000000:type/resource/Sim2Real-Probe-Nonexistent' cannot be found.")

Cause 7 — cloudformation.export-value-update-refused-while-imported:
  AssertionError: positive control: an update changing only the NON-exported parameter must reach UPDATE_COMPLETE, got 'UPDATE_FAILED' / 'The following resource(s) failed to update: [Marker]. NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Topic in place; the resource was left as it is.'

Smithy consulted: ResourceScanNotFoundException (client/400/ResourceScanNotFound), ResourceScanInProgressException (client/400/ResourceScanInProgress), GeneratedTemplateNotFoundException (client/404/GeneratedTemplateNotFound), TypeNotFoundException (client/404), CFNRegistryException (client/400); ListResourceScans (no modelled errors, paginated on ResourceScanSummaries); ActivateType.PublicTypeArn pattern; UpdateGeneratedTemplate error set.

## Steps to Reproduce
1. Boot the emulator and replay the checks named in trace_ids.
2. Cause 1: DescribeResourceScan(ResourceScanId=<never-created resourceScan ARN>) -> expect 400 ResourceScanNotFound, Message exactly 'There is no ResourceScan found for the ResourceScanId.'
3. Cause 2: StartResourceScan(ScanFilters=[{'Types':['AWS::SNS::Topic']}]), poll DescribeResourceScan to COMPLETE, then ListResourceScans() -> the scan must appear in ResourceScanSummaries with Status COMPLETE; the emulator returns [].
4. Cause 3: immediately after StartResourceScan (while DescribeResourceScan says IN_PROGRESS) call ListResourceScanResources(ResourceScanId=<id>) -> expect 400 ResourceScanInProgress with the documented in-progress sentence; the emulator answers NotImplemented.
5. Cause 4: CreateGeneratedTemplate, DeleteGeneratedTemplate, then UpdateGeneratedTemplate on the deleted name -> expect 404 GeneratedTemplateNotFound; the emulator answers NotImplemented.
6. Cause 5: ActivateType(PublicTypeArn='arn:aws:cloudformation:us-west-2::type/resource/<Pub-Id>/00000001') -> expect 400 CFNRegistryException 'TypeName is invalid'.
7. Cause 6: DescribeType(Arn=<type version ARN of an unregistered Namespace::Service::Resource>) -> expect 404 TypeNotFoundException "The type '<Namespace::Service::Resource>' cannot be found."
8. Cause 7: CreateStack with an AWS::SNS::Topic whose properties come from a parameter, then UpdateStack changing only that parameter -> expect UPDATE_COMPLETE; the emulator lands UPDATE_FAILED with the NotImplemented in-place reason.
