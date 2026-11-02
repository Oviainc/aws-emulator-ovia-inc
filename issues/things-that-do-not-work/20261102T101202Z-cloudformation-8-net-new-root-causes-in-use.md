Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.export-value-update-refused-while-imported cloudformation.stack-export-removal-while-imported-rolls-back-producer cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard cloudformation.export-deletion-blocked-while-imported-rejected cloudformation.export-in-use-blocks-stack-deletion cloudformation.e2e-resource-scan-generated-template-to-managed-stack cloudformation.concurrent-generated-template-limit cloudformation.generated-template-concurrency-limit-rejected cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.export-value-update-refused-while-imported|run#29|AssertionError|assert refused == expected_refusal, (
Issue-Key: cloudformation.stack-export-removal-while-imported-rolls-back-producer|run#16|AssertionError|assert _settle(cfn, producer, timeout_s=_cap(ctx, _DEADLINE_S)) == "UPDATE_ROLLBACK_COMPLETE", f"dropping the used export: {_stack(cfn, producer)!r}"
Issue-Key: cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard|run#38|AssertionError|assert rc != 0, (
Issue-Key: cloudformation.export-deletion-blocked-while-imported-rejected|run#22|AssertionError|assert got == expected, (
Issue-Key: cloudformation.export-in-use-blocks-stack-deletion|run#23|AssertionError|assert got == expected, (
Issue-Key: cloudformation.e2e-resource-scan-generated-template-to-managed-stack|run#10|AssertionError|assert found is not None, "resource scan did not surface the manually-created SNS topic"
Issue-Key: cloudformation.concurrent-generated-template-limit|run#17|AssertionError|assert message.startswith(
Issue-Key: cloudformation.generated-template-concurrency-limit-rejected|run#13|AssertionError|assert message.startswith(
Issue-Key: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact|run#22|AssertionError|assert re.fullmatch(_BUCKET_NOT_EMPTY_REASON, bucket_reason), (
Issue-Key: cloudformation|DescribeStacks|ValidationError
Issue-Key: cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one|run#16|AssertionError|assert (bad["ResourceStatus"], reasons) == ("DELETE_COMPLETE", [
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_http_until#30|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_http_until#37|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write|_http_until#38|Timeout|time.sleep(3.0)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation: 8 net-new root causes — in-use export guard missing on update, delete-canceled message, resource scan finds nothing, nested child stacks not addressable, collision reason wording, unsettled CDK deploys

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Eight distinct net-new root causes across exports, resource scans, nested stacks and failure reasons. Open issue 20261029T214321Z-cloudformation-five-behavioral-checks-exhaust-the-harness-s.md covers other budget-exhausting checks; 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md covers neighbouring nested-stack symptoms. Neither carries the replay keys below.

Cause 1 of 8: an update that changes or removes an export another stack imports is accepted.
AWS: recorded behavior only — the update is refused/rolled back and the producer settles UPDATE_ROLLBACK_COMPLETE with reason 'Update canceled. Cannot update export <export-name> as it is in use by <importing-stack-name>.' (fixed text, with the export name and importer taken from state).
Emulator: the producer settles UPDATE_COMPLETE with no reason, and the export value changes under the importer.
Why: cross-stack contracts break silently — importers keep a value the producer no longer publishes.
Fix: export in-use guard in the UpdateStack path (export registry).
Traces: cloudformation.export-value-update-refused-while-imported, cloudformation.stack-export-removal-while-imported-rolls-back-producer, cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard.

Cause 2 of 8: the delete-canceled reason for an in-use export is missing its trailing period.
AWS: the reason is 'Delete canceled. Cannot delete export <export-name> as it is in use by <importing-stack-name>.' — the sentence ends with a period; only the names are variable.
Emulator: emits the same sentence without the final period.
Why: clients that match the message exactly do not recognise the cancellation.
Fix: the delete-path message formatter in the stack delete handler.
Traces: cloudformation.export-deletion-blocked-while-imported-rejected, cloudformation.export-in-use-blocks-stack-deletion.

Cause 3 of 8: resource scans find no out-of-band resources.
AWS: recorded behavior only — StartResourceScan/ListResourceScanResources surface resources created outside CloudFormation, e.g. a manually created AWS::SNS::Topic appears with its ARN, and the generated template can then be imported into a managed stack.
Emulator: the scan returns [] for AWS::SNS::Topic even though the topic exists.
Why: the whole scan -> generate-template -> import workflow is unusable.
Fix: the resource-scan handler's inventory source.
Traces: cloudformation.e2e-resource-scan-generated-template-to-managed-stack.

Cause 4 of 8: the generated-template concurrency-limit refusal message does not match AWS.
AWS: recorded behavior only — exceeding the in-flight generated-template limit is refused with AWS's LimitExceeded wording for that quota.
Emulator: 'No more than 5 generated templates can be in an InProgress or Pending status at one time'.
Why: clients that branch on the limit message mis-handle the refusal.
Fix: the CreateGeneratedTemplate limit message.
Traces: cloudformation.concurrent-generated-template-limit, cloudformation.generated-template-concurrency-limit-rejected.

Cause 5 of 8: a retained-resource delete failure carries the wrong reason and HandlerErrorCode.
AWS: the failure reason is 'Resource handler returned message: "The bucket you tried to delete is not empty (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)' — the service/status/attempt wrapper and GeneralServiceException are fixed; the ids are per-call.
Emulator: 'Resource handler returned message: "The bucket you tried to delete is not empty" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)' — no service wrapper and the wrong HandlerErrorCode.
Why: callers that classify by HandlerErrorCode treat a transient service error as a client error and never retry.
Fix: resource-handler error wrapping in the delete path.
Traces: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact.

Cause 6 of 8: a nested child stack is not addressable as a stack of its own.
AWS: recorded behavior only — a nested stack's PhysicalResourceId is the child stack's ARN and DescribeStacks on that id returns the child; deleting the parent cascades to it.
Emulator: DescribeStacks on the child id raises ValidationError 'Stack with id <parent>-<Logical><suffix> does not exist' — only a generated name exists, not a stack.
Why: every second-hop navigation into nested stacks (CDK nested stacks, outputs, events) fails.
Fix: nested-stack provisioning must register a real child stack and return its ARN.
Traces: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.

Cause 7 of 8: a name-collision CREATE_FAILED reason is the downstream handler text instead of CloudFormation's.
AWS: recorded behavior only — when a resource's declared physical name is already owned, the resource event's reason is '<physical-name> already exists in stack <owning-stack-ARN>' and the stack-level reason is 'The following resource(s) failed to create: [<LogicalId>]. '.
Emulator: the resource reason is 'Resource handler returned message: "Parameter <name> already exists; set Overwrite to update it" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)'.
Why: clients cannot tell which stack owns the colliding name, which is the actionable part of AWS's message.
Fix: collision detection/reason formatting in the resource provisioner.
Traces: cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one.

Cause 8 of 8: CDK L2 deploys that must then serve or read back never settle inside the check budget.
AWS: recorded behavior only — each of these deploys completes and the deployed endpoint/resource answers well inside 60s.
Emulator: the check exhausts its 60s budget ('Check timed out after 60.0s') — the deploy/serve path never completes. Hypothesis: the stack operation or the served endpoint never reaches a terminal state.
Why: a client gets a hang instead of a deployed, serving stack.
Fix: whichever stage of the CreateStack/UpdateStack pipeline fails to settle for these resource sets.
Traces: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write.

## Evidence
C1: "changing export '<name>' while '<importer>' imports it must be rolled back as {'outcome': 'UPDATE_ROLLBACK_COMPLETE', 'evidence': 'Update canceled. Cannot update export <name> as it is in use by <importer>.'}, got {'outcome': 'UPDATE_COMPLETE', 'evidence': None}"; and `cdk deploy <producer>` exited 0 while changing an exported value still imported by the consumer.
C2: expected '... as it is in use by <importer>.', got '... as it is in use by <importer>' (no trailing period).
C3: 'expected arn:aws:sns:<region>:<account>:<topic> among scanned AWS::SNS::Topic resources, got []'.
C4: "unexpected limit message 'No more than 5 generated templates can be in an InProgress or Pending status at one time'".
C5: expected reason matching 'Resource handler returned message: "The bucket you tried to delete is not empty (Service: S3, Status Code: 409, Request ID: ..., Extended Request ID: ...) (SDK Attempt Count: N)" (RequestToken: ..., HandlerErrorCode: GeneralServiceException)', got 'Resource handler returned message: "The bucket you tried to delete is not empty" (RequestToken: ..., HandlerErrorCode: InvalidRequest)'.
C6: ClientError (ValidationError) on DescribeStacks: 'Stack with id <parent>-ChildNestedStackChildNestedStackResource<suffix> does not exist'.
C7: CREATE_FAILED event ResourceStatusReason 'Resource handler returned message: "Parameter /<name> already exists; set Overwrite to update it" (RequestToken: ..., HandlerErrorCode: AlreadyExists)'.
C8: 'Error: Check timed out after 60.0s' for all three checks.
Ids, names, uuids and timestamps are per-run fillers.

## Steps to Reproduce
Replay the checks in trace_ids: 1) create a producer stack with an Export, import it from a second stack, then UpdateStack the producer to change/remove that export; 2) delete the producer while the import is live and read the stack's reason; 3) create a resource out of band, run StartResourceScan and list the scan's resources; 4) exceed the in-flight generated-template limit; 5) DeleteStack with RetainResources naming a non-empty bucket and read the resource's failure reason; 6) deploy a CDK app with a nested stack and DescribeStacks the child's PhysicalResourceId; 7) create a stack whose resource name is already owned by another stack and read the CREATE_FAILED event; 8) run the three CDK deploy checks listed and observe the 60s budget being exhausted.
