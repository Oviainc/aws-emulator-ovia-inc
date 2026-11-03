Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.access-request-start-then-get-token-lifecycle ssm.start-access-request-denied-without-permission ssm.start-access-request-nonexistent-resource-rejected ssm.delete-association-nonexistent-rejected ssm.delete-inventory-schema-delete-builtin-type-rejected ssm.deregister-managed-instance-rejects-unknown-id ssm.describe-instance-patches-invalid-filter-rejected ssm.ops-metadata-get-nonexistent-rejected ssm.ops-metadata-delete-nonexistent-rejected ssm.put-inventory-unauthorized-principal-denied ssm.update-managed-instance-role-invalid-instance-rejected ssm.delete-cloud-connector-unknown-id-rejected ssm.list-nodes-summary-aggregates-by-filter ssm.put-resource-policy-malformed-document-rejected ssm.start-execution-preview-malformed-input-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.access-request-start-then-get-token-lifecycle|run#4|AssertionError|assert start_code == "ValidationException", (
Issue-Key: ssm.start-access-request-denied-without-permission|run#7|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: ssm.start-access-request-nonexistent-resource-rejected|run#17|AssertionError|assert code == "AccessDeniedException", code
Issue-Key: ssm.delete-association-nonexistent-rejected|run#4|AssertionError|assert code == "AssociationDoesNotExist", code
Issue-Key: ssm.delete-inventory-schema-delete-builtin-type-rejected|run#3|AssertionError|assert code == "InvalidTypeNameException", f"expected InvalidTypeNameException, got {code!r}"
Issue-Key: ssm.deregister-managed-instance-rejects-unknown-id|run#3|AssertionError|assert code == "InvalidInstanceId", f"expected InvalidInstanceId, got {code!r}"
Issue-Key: ssm.describe-instance-patches-invalid-filter-rejected|run#5|AssertionError|assert bad_filter_code == "ValidationException", (
Issue-Key: ssm.ops-metadata-get-nonexistent-rejected|run#6|AssertionError|assert code == "OpsMetadataNotFoundException", f"got {code!r}"
Issue-Key: ssm.ops-metadata-delete-nonexistent-rejected|run#6|AssertionError|assert pre_code == "OpsMetadataNotFoundException", f"got {pre_code!r}"
Issue-Key: ssm.put-inventory-unauthorized-principal-denied|run#16|AssertionError|assert admin_code == "InvalidInstanceId", (
Issue-Key: ssm.update-managed-instance-role-invalid-instance-rejected|run#4|AssertionError|assert code == "InvalidInstanceId", (
Issue-Key: ssm.delete-cloud-connector-unknown-id-rejected|run#8|AssertionError|assert create_code == "ValidationException", f"unexpected code: {create_code}"
Issue-Key: ssm.list-nodes-summary-aggregates-by-filter|run#4|AssertionError|assert list_nodes_code == "UnsupportedOperationException", (
Issue-Key: ssm.put-resource-policy-malformed-document-rejected|run#9|AssertionError|assert code == "MalformedResourcePolicyDocumentException", code
Issue-Key: ssm.start-execution-preview-malformed-input-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# SSM control-plane operations answer NotImplemented where AWS returns a modelled refusal (10 causes)

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAccessRequest

## Description
Ten SSM operations reached by behavioral checks are still scaffold stubs: every call comes back as error code 'NotImplemented' with the message 'operation <Op> is scaffolded but not implemented in this emulator', where AWS answers a modelled 4xx error (or succeeds). None of these checks was marked ALREADY FILED or OWNED by the suite, so each needs its behavioral replay key attached; where an open stub issue already names the operation I name it in the cause. 10 CAUSES.

In all ten, the emulator's response is the same shape: Error.Code 'NotImplemented', Error.Message 'operation <Op> is scaffolded but not implemented in this emulator'. That code is not in any of these operations' modelled error sets, so no client error handler can classify it.

Cause 1 of 10 — StartAccessRequest (and the GetAccessToken pair on the same account state).
AWS: on an account where Just-In-Time Node Access was never enabled, StartAccessRequest is refused 400 ValidationException (Smithy: StartAccessRequest errors ValidationException, AccessDeniedException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException) with the fixed sentence 'Just-In-Time Node Access service is not enabled in this account.' — refused before any target lookup. A no-permissions caller gets 400 AccessDeniedException instead.
Emulator: NotImplemented for every call.
Why: the whole JIT-access surface is unobservable, and a caller cannot distinguish 'feature off', 'denied' and 'no such resource'.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/start_access_request.rs (and the GetAccessToken handler, whose stub issue is open as 20261102T091623Z-ssm-createactivation-stub.md's sibling listing for GetAccessToken).
Traces: ssm.access-request-start-then-get-token-lifecycle, ssm.start-access-request-denied-without-permission, ssm.start-access-request-nonexistent-resource-rejected.

Cause 2 of 10 — DeleteAssociation.
AWS: deleting an association that does not exist is refused 400 AssociationDoesNotExist (Smithy: DeleteAssociation errors AssociationDoesNotExist, InvalidDocument, InvalidInstanceId, TooManyUpdates).
Emulator: NotImplemented.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/delete_association.rs.
Traces: ssm.delete-association-nonexistent-rejected.

Cause 3 of 10 — DeleteInventory.
AWS: DeleteInventory for a built-in 'AWS:' type name is refused 400 InvalidTypeNameException (Smithy: DeleteInventory errors InvalidTypeNameException, InvalidOptionException, InvalidDeleteInventoryParametersException, InvalidInventoryRequestException; TypeName @pattern ^(AWS|Custom):.*$).
Emulator: NotImplemented.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/delete_inventory.rs.
Traces: ssm.delete-inventory-schema-delete-builtin-type-rejected.

Cause 4 of 10 — DeregisterManagedInstance.
AWS: a well-formed but unknown managed-instance id is refused 400 InvalidInstanceId (Smithy: DeregisterManagedInstance errors InvalidInstanceId; InstanceId @length 20..124, @pattern ^(^mi-[0-9a-f]{17}$)|(^eks_c:...)$).
Emulator: NotImplemented.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/deregister_managed_instance.rs.
Traces: ssm.deregister-managed-instance-rejects-unknown-id.

Cause 5 of 10 — DescribeInstancePatches.
AWS: an unsupported Filters key is refused 400 ValidationException, and a malformed InstanceId 400 ValidationException (Smithy: DescribeInstancePatches errors InvalidFilter, InvalidInstanceId, InvalidNextToken; Filters @length 0..5).
Emulator: NotImplemented.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_patches.rs.
Traces: ssm.describe-instance-patches-invalid-filter-rejected.

Cause 6 of 10 — GetOpsMetadata (and DeleteOpsMetadata behind it).
AWS: GetOpsMetadata on a well-formed but unregistered OpsMetadataArn in the caller's own account is refused 400 OpsMetadataNotFoundException (Smithy: GetOpsMetadata errors OpsMetadataNotFoundException, OpsMetadataInvalidArgumentException) with 'OpsMetadata object not found. Verify the Amazon Resource Name (ARN) and try again.'; an ARN naming another account is refused 400 OpsMetadataInvalidArgumentException with 'Invalid OpsMetadataArn. Requester is not the owner of the resource. Verify the Amazon Resource Name (ARN) and try again.' DeleteOpsMetadata must classify the same two cases identically.
Emulator: NotImplemented on the first GetOpsMetadata, so neither classification nor message is observable.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/get_ops_metadata.rs and delete_ops_metadata.rs. (The open issue for the ListOpsMetadata stub does not cover these two.)
Traces: ssm.ops-metadata-get-nonexistent-rejected, ssm.ops-metadata-delete-nonexistent-rejected.

Cause 7 of 10 — PutInventory.
AWS: an authorized caller's PutInventory against a fabricated InstanceId fails InstanceId validation — 400 InvalidInstanceId (Smithy: PutInventory errors InvalidInstanceId, InvalidTypeNameException, InvalidItemContentException, ItemContentMismatchException, ...; InstanceId @pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$) — i.e. validation precedes authorization, and an unauthorized principal gets AccessDeniedException.
Emulator: NotImplemented, so neither ordering nor class can be observed.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/put_inventory.rs.
Traces: ssm.put-inventory-unauthorized-principal-denied.

Cause 8 of 10 — UpdateManagedInstanceRole.
AWS: a well-formed but nonexistent managed-instance id is refused 400 InvalidInstanceId (Smithy: UpdateManagedInstanceRole errors InvalidInstanceId; InstanceId @length 20..124 with the mi-/eks_c: pattern, IamRole required).
Emulator: NotImplemented.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/update_managed_instance_role.rs.
Traces: ssm.update-managed-instance-role-invalid-instance-rejected.

Cause 9 of 10 — the CloudConnector surface (CreateCloudConnector / DeleteCloudConnector).
AWS: CreateCloudConnector with a malformed request is refused 400 ValidationException, and DeleteCloudConnector on an unknown id 400 ResourceNotFoundException; this check never reaches the delete because the create stub answers first.
Emulator: NotImplemented on CreateCloudConnector.
Fix: services/service-com-amazonaws-ssm-amazonssm/src/ops/create_cloud_connector.rs and delete_cloud_connector.rs. A CreateCloudConnector stub issue is already filed from another check; this cause attaches this check's replay key and the DeleteCloudConnector half.
Traces: ssm.delete-cloud-connector-unknown-id-rejected.

Cause 10 of 10 — three more checks blocked by stubs whose own issues are already open: ListNodes, PutResourcePolicy and StartExecutionPreview.
AWS: ListNodes on an account with no Systems Manager integrated experience enabled is refused 400 UnsupportedOperationException; PutResourcePolicy with a malformed policy document is refused 400 MalformedResourcePolicyDocumentException; StartExecutionPreview with malformed input is refused 400 ValidationException.
Emulator: NotImplemented for all three.
Why this is listed: the stub issues name the operations but do not carry these three behavioral check ids, so a fixer closing them would not re-replay these checks.
Fix: the three existing handler stubs (list_nodes.rs, put_resource_policy.rs, start_execution_preview.rs); close together with their already-open stub issues.
Traces: ssm.list-nodes-summary-aggregates-by-filter, ssm.put-resource-policy-malformed-document-rejected, ssm.start-execution-preview-malformed-input-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1; all 15 checks are listed under '### ssm' in the suite report and none carries an ALREADY FILED or OWNED marker.

Cause 1 — ssm.access-request-start-then-get-token-lifecycle: "expected ValidationException for StartAccessRequest on an account with just-in-time node access never enabled, got NotImplemented" (expected message 'Just-In-Time Node Access service is not enabled in this account.'); ssm.start-access-request-denied-without-permission: "expected AccessDeniedException for a no-permissions caller, got 'NotImplemented'"; ssm.start-access-request-nonexistent-resource-rejected: assert code == 'AccessDeniedException' → AssertionError: NotImplemented.
Cause 2 — ssm.delete-association-nonexistent-rejected: assert code == 'AssociationDoesNotExist' → AssertionError: NotImplemented.
Cause 3 — ssm.delete-inventory-schema-delete-builtin-type-rejected: "expected InvalidTypeNameException, got 'NotImplemented'".
Cause 4 — ssm.deregister-managed-instance-rejects-unknown-id: "expected InvalidInstanceId, got 'NotImplemented'".
Cause 5 — ssm.describe-instance-patches-invalid-filter-rejected: "expected ValidationException, got NotImplemented".
Cause 6 — ssm.ops-metadata-get-nonexistent-rejected and ssm.ops-metadata-delete-nonexistent-rejected: both assert code == 'OpsMetadataNotFoundException' and report "got 'NotImplemented'" on the GetOpsMetadata call.
Cause 7 — ssm.put-inventory-unauthorized-principal-denied: "an authorized caller's PutInventory against a fabricated InstanceId must fail InstanceId validation, not authorization; got 'NotImplemented'".
Cause 8 — ssm.update-managed-instance-role-invalid-instance-rejected: "expected UpdateManagedInstanceRole for a well-formed but nonexistent managed instance id to be rejected with InvalidInstanceId, got 'NotImplemented'".
Cause 9 — ssm.delete-cloud-connector-unknown-id-rejected: "unexpected code: NotImplemented" on the CreateCloudConnector precondition (expected ValidationException).
Cause 10 — ssm.list-nodes-summary-aggregates-by-filter: "ListNodes: expected UnsupportedOperationException, got 'NotImplemented'"; ssm.put-resource-policy-malformed-document-rejected: assert code == 'MalformedResourcePolicyDocumentException' → AssertionError: NotImplemented; ssm.start-execution-preview-malformed-input-rejected: assert code == 'ValidationException' → AssertionError: NotImplemented.

Every actual response body is 'NotImplemented' / 'operation <Op> is scaffolded but not implemented in this emulator'. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json for each operation's modelled error set and input constraints.

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids. 3. Per cause, call directly: StartAccessRequest(Reason=..., Targets=[{Key:'InstanceIds',Values:['i-00000000000000000']}]) and GetAccessToken(AccessRequestId='oi-<12 hex>') (cause 1); DeleteAssociation(AssociationId=<unused uuid>) (cause 2); DeleteInventory(TypeName='AWS:InstanceInformation', SchemaDeleteOption='DeleteSchema') (cause 3); DeregisterManagedInstance(InstanceId='mi-<17 hex>') (cause 4); DescribeInstancePatches with an unsupported Filters key (cause 5); GetOpsMetadata / DeleteOpsMetadata with an unregistered opsmetadata ARN in the caller's account, then with one naming another account (cause 6); PutInventory(InstanceId='i-00000000000000000', Items=[...]) as an authorized principal (cause 7); UpdateManagedInstanceRole(InstanceId='mi-<17 hex>', IamRole=<role>) (cause 8); CreateCloudConnector with a malformed request, then DeleteCloudConnector with an unknown id (cause 9); ListNodes, PutResourcePolicy with a malformed document, StartExecutionPreview with malformed input (cause 10). 4. Each must return its modelled 4xx class and message instead of NotImplemented.
