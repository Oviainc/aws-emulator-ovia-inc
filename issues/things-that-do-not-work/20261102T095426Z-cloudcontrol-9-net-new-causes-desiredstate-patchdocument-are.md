Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.create-resource-malformed-desired-state-invalid-request cloudcontrol.create-resource-missing-required-property-rejected cloudcontrol.patch-remove-required-property-rejected cloudcontrol.create-resource-unknown-extra-property-rejected cloudcontrol.desired-state-unknown-property-rejected cloudcontrol.create-resource-schema-type-mismatch-rejected cloudcontrol.desired-state-wrong-json-type-rejected cloudcontrol.invalid-schema-property-rejected cloudcontrol.numeric-property-type-mismatch-rejected cloudcontrol.update-resource-type-mismatch-value-rejected cloudcontrol.desired-state-enum-violation-rejected cloudcontrol.desired-state-pattern-violation-rejected cloudcontrol.create-resource-out-of-range-property-rejected cloudcontrol.create-resource-out-of-range-value-rejected cloudcontrol.update-create-only-property-rejected cloudcontrol.update-immutable-property-rejected cloudcontrol.e2e-immutable-property-update-rejected cloudcontrol.update-resource-conflicting-name-rejected cloudcontrol.failed-request-progress-event-carries-error-code-and-status-message cloudcontrol.cloudcontrol-downstream-handler-rejected-create-leaves-no-native-table cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed cloudcontrol.long-running-request-timeout-yields-failed-status cloudcontrol.create-resource-access-denied-blocked cloudcontrol.create-resource-missing-permission-denied-rejected cloudcontrol.get-resource-request-status-unauthorized-denied cloudcontrol.delete-nonexistent-resource-failed-request-ordering-preserved cloudcontrol.update-resource-nonexistent-fails-async-not-sync cloudcontrol.update-nonexistent-resource-must-fail cloudcontrol.list-resource-requests-ordering-reflects-submission-order
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.create-resource-malformed-desired-state-invalid-request|_failure#12|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.create-resource-missing-required-property-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudcontrol.patch-remove-required-property-rejected|_refusal#7|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.create-resource-unknown-extra-property-rejected|run#6|AssertionError|assert final["OperationStatus"] == "FAILED", f"unexpected final status: {final}"
Issue-Key: cloudcontrol.desired-state-unknown-property-rejected|_refusal#5|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.create-resource-schema-type-mismatch-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudcontrol.desired-state-wrong-json-type-rejected|_refusal#5|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.invalid-schema-property-rejected|_create_and_capture_failure#5|AssertionError|raise AssertionError(f"expected the malformed request to fail, but it succeeded: {event}")
Issue-Key: cloudcontrol.numeric-property-type-mismatch-rejected|_error#5|AssertionError|raise AssertionError(f"the call was expected to fail but returned {result!r}")
Issue-Key: cloudcontrol.update-resource-type-mismatch-value-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudcontrol.desired-state-enum-violation-rejected|_refusal#5|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.desired-state-pattern-violation-rejected|_refusal#5|AssertionError|assert status == "FAILED", (
Issue-Key: cloudcontrol.create-resource-out-of-range-property-rejected|run#9|AssertionError|assert final["OperationStatus"] == "FAILED", f"unexpected status: {final}"
Issue-Key: cloudcontrol.create-resource-out-of-range-value-rejected|run#7|AssertionError|assert bad_final["OperationStatus"] == "FAILED", f"expected FAILED, got {bad_final}"
Issue-Key: cloudcontrol.update-create-only-property-rejected|_mutate_error#9|AssertionError|raise AssertionError(f"expected this call to be refused, but it reached {event}")
Issue-Key: cloudcontrol.update-immutable-property-rejected|_error#11|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudcontrol.e2e-immutable-property-update-rejected|_expect_rejected#10|AssertionError|raise AssertionError(f"expected UpdateResource to be rejected, but it SUCCEEDED: {event}")
Issue-Key: cloudcontrol.update-resource-conflicting-name-rejected|run#18|AssertionError|assert rename_final["OperationStatus"] == "FAILED", f"unexpected final status: {rename_final}"
Issue-Key: cloudcontrol.failed-request-progress-event-carries-error-code-and-status-message|run#11|AssertionError|assert terminal["OperationStatus"] == "FAILED", (
Issue-Key: cloudcontrol.cloudcontrol-downstream-handler-rejected-create-leaves-no-native-table|run#7|AssertionError|assert terminal["OperationStatus"] == "FAILED", (
Issue-Key: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed|run#10|AssertionError|assert final["OperationStatus"] == "FAILED", (
Issue-Key: cloudcontrol.long-running-request-timeout-yields-failed-status|run#12|AssertionError|assert pe["OperationStatus"] == "FAILED", (
Issue-Key: cloudcontrol.create-resource-access-denied-blocked|run#14|AssertionError|assert final["OperationStatus"] == "FAILED", f"unexpected status: {final}"
Issue-Key: cloudcontrol.create-resource-missing-permission-denied-rejected|run#18|AssertionError|assert final["OperationStatus"] == "FAILED", f"unexpected final status: {final}"
Issue-Key: cloudcontrol.get-resource-request-status-unauthorized-denied|_poll_for_error#19|AssertionError|raise AssertionError(f"expected a denial error, but the call kept succeeding: {result!r}")
Issue-Key: cloudcontrol|DeleteResource|ResourceNotFoundException
Issue-Key: cloudcontrol|UpdateResource|ResourceNotFoundException
Issue-Key: cloudcontrol|UpdateResource|InvalidRequestException
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudcontrol: 9 net-new causes — DesiredState/PatchDocument are never validated against the resource type schema and refusals that AWS makes are reported as SUCCESS

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
NINE net-new root causes, all in the cloudcontrol (Cloud Control API) service. Every one of them is a case where AWS refuses (synchronously, or by driving the request to a terminal FAILED ProgressEvent) and the emulator instead reports SUCCESS / IN_PROGRESS, or refuses in the wrong plane. A fixer must close all nine. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json; ProgressEvent carries OperationStatus (PENDING|IN_PROGRESS|SUCCESS|FAILED|CANCEL_IN_PROGRESS|CANCEL_COMPLETE), ErrorCode (HandlerErrorCode: NotUpdatable, InvalidRequest, AccessDenied, AlreadyExists, NotFound, ...) and StatusMessage.

Cause 1 of 9: a required property missing from DesiredState (or removed by a patch) is not detected.
AWS: DesiredState must satisfy the type's registry schema. A create missing a required property is refused — either at the front door with InvalidRequestException (HTTP 400, "The resource handler has returned that invalid input from the user has generated a generic exception") or by the handler, which drives the request to OperationStatus=FAILED with ErrorCode=InvalidRequest and a StatusMessage naming the missing property. The same holds for an UpdateResource whose patch removes a required property (Smithy: NotUpdatableException/InvalidRequestException are both modeled on UpdateResource).
Emulator: the create/update is accepted and reaches terminal SUCCESS (e.g. AWS::SSM::Parameter with no Value; AWS::IAM::Role with no AssumeRolePolicyDocument answers 200/IN_PROGRESS; a patch removing /Value reaches UPDATE SUCCESS).
Why: a client gets SUCCESS for a resource that cannot exist, so a broken template is reported as a completed deployment.
Fix: schema validation in front of the CreateResource/UpdateResource handlers must check required properties and refuse (InvalidRequestException, or FAILED with ErrorCode=InvalidRequest).
Failing checks: cloudcontrol.create-resource-malformed-desired-state-invalid-request, cloudcontrol.create-resource-missing-required-property-rejected, cloudcontrol.patch-remove-required-property-rejected

Cause 2 of 9: a property that is not in the type's schema is accepted.
AWS: an unknown/extra property in DesiredState is refused (InvalidRequestException at the front door, or FAILED with ErrorCode=InvalidRequest naming the unrecognised property).
Emulator: reaches SUCCESS with the unknown key echoed back in ProgressEvent.ResourceModel (observed with "ThisPropertyDoesNotExistInTheSchema" on AWS::SQS::Queue and "SimParityNotAProperty" on AWS::SSM::Parameter).
Why: a misspelled property is silently ignored rather than reported, so the resource is created without the setting the caller asked for and the caller is told it succeeded.
Fix: the same DesiredState validation step must reject properties absent from the type schema.
Failing checks: cloudcontrol.create-resource-unknown-extra-property-rejected, cloudcontrol.desired-state-unknown-property-rejected

Cause 3 of 9: a property value of the wrong JSON type is accepted.
AWS: DesiredState values must match the schema's declared JSON types; a string where a number is declared (AWS::CloudWatch::Alarm Threshold="not-a-number"), a number where a string is declared (AWS::SSM::Parameter Value=12345), or a type-mismatched value in an UpdateResource patch is refused (InvalidRequestException / FAILED with ErrorCode=InvalidRequest).
Emulator: accepted; the request reaches SUCCESS (or 200/IN_PROGRESS) with the mistyped value stored verbatim in ResourceModel.
Why: the resource is created with a value the owning service could never store, and every later read of it returns a value of the wrong type to clients that trust the schema.
Fix: type-check each DesiredState/patched value against the schema in the create/update validation path.
Failing checks: cloudcontrol.create-resource-schema-type-mismatch-rejected, cloudcontrol.desired-state-wrong-json-type-rejected, cloudcontrol.invalid-schema-property-rejected, cloudcontrol.numeric-property-type-mismatch-rejected, cloudcontrol.update-resource-type-mismatch-value-rejected

Cause 4 of 9: enum, pattern and numeric-range constraints are not enforced.
AWS: a value outside the schema's enum (AWS::SSM::Parameter Tier="sim2real-not-a-tier"), violating its pattern (AWS::Logs::LogGroup LogGroupName containing ':') or outside its range (AWS::SQS::Queue DelaySeconds=901, max 900) is refused — front-door InvalidRequestException or a terminal FAILED event with ErrorCode=InvalidRequest.
Emulator: all reach SUCCESS; the illegal value is stored and echoed in ResourceModel.
Why: Cloud Control is the generic front door for resource creation, so these constraint refusals are the only validation many clients get; accepting them produces resources the owning service would reject and makes FAILED-path error handling dead code.
Fix: enforce enum/pattern/range constraints from the type schema in the create/update validation path.
Failing checks: cloudcontrol.desired-state-enum-violation-rejected, cloudcontrol.desired-state-pattern-violation-rejected, cloudcontrol.create-resource-out-of-range-property-rejected, cloudcontrol.create-resource-out-of-range-value-rejected

Cause 5 of 9: UpdateResource mutates create-only/immutable properties instead of refusing.
AWS: Smithy models NotUpdatableException (HTTP 400, "One or more properties included in this resource operation are defined as create-only, and therefore can't be updated"). A patch touching a create-only property (AWS::SQS::Queue FifoQueue or QueueName, AWS::CloudFront::CachePolicy's name rename) is refused; the resource is left untouched.
Emulator: the UpdateResource is accepted and reaches UPDATE SUCCESS with the create-only property changed in ResourceModel (observed FifoQueue flipped to true on a standard queue).
Why: an immutable property appears to change while the real resource cannot have changed, so a client's model of the resource diverges permanently and a replacement that AWS would have forced never happens.
Note: this is the acceptance of the mutation; the WORDING of the refusal once it is implemented is a separate cause filed in the companion issue "cloudcontrol: 10 net-new causes — Cloud Control refusal messages..." (Cause 5 there).
Fix: the UpdateResource validation path must compare the patched model against the schema's createOnlyProperties and refuse with NotUpdatableException before any effect is recorded.
Failing checks: cloudcontrol.update-create-only-property-rejected, cloudcontrol.update-immutable-property-rejected, cloudcontrol.e2e-immutable-property-update-rejected, cloudcontrol.update-resource-conflicting-name-rejected

Cause 6 of 9: a downstream handler refusal never drives the request to FAILED.
AWS: when the resource type's handler rejects the desired state (an SSM Parameter whose Value violates its own AllowedPattern, a DynamoDB table whose KeySchema names an attribute absent from AttributeDefinitions, an SQS queue whose name ends .fifo while FifoQueue=false), the request terminates OperationStatus=FAILED with ProgressEvent.ErrorCode set from HandlerErrorCode and a StatusMessage carrying the handler's diagnostic, and no resource is left behind. A request that never satisfies its handler's stabilization (an ACM certificate whose DNS validation never completes) likewise ends FAILED rather than SUCCESS.
Emulator: every one of these reaches terminal SUCCESS with no ErrorCode or StatusMessage.
Why: FAILED + ErrorCode + StatusMessage is the only channel through which a Cloud Control client learns a provisioning request was rejected; reporting SUCCESS means callers proceed against resources that do not exist and never see the diagnosis.
Fix: the request materialisation path (the code that moves a request record to a terminal state) must invoke the owning type's handler semantics and terminate FAILED with ErrorCode/StatusMessage when it refuses.
Failing checks: cloudcontrol.failed-request-progress-event-carries-error-code-and-status-message, cloudcontrol.cloudcontrol-downstream-handler-rejected-create-leaves-no-native-table, cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed, cloudcontrol.long-running-request-timeout-yields-failed-status

Cause 7 of 9: RoleArn / caller authorization is ignored.
AWS: CreateResource accepts an optional RoleArn (pattern ^arn:.+:iam::[0-9]{12}:role/.+$) and performs the resource operation with THAT role's permissions. A role with no policies cannot call the downstream service, so the request terminates OperationStatus=FAILED with ProgressEvent.ErrorCode="AccessDenied" (a modeled HandlerErrorCode value) and nothing is created; there is no fallback to the caller's credentials. Likewise a principal denied cloudformation/cloudcontrol access is refused (AccessDeniedException) on GetResourceRequestStatus.
Emulator: the create with a permission-less RoleArn reaches SUCCESS and the resource is created; a denied principal's GetResourceRequestStatus keeps succeeding.
Why: this silently grants callers rights they do not have — the opposite of the security contract — and tests or tooling that verify least-privilege behavior (and expect AccessDenied) see success.
Fix: honour RoleArn in the CreateResource/UpdateResource/DeleteResource handlers by evaluating the named role's permissions for the downstream operation, and authorize the calling principal on GetResourceRequestStatus; refuse with FAILED/ErrorCode=AccessDenied (resp. AccessDeniedException) and leave no effect.
Failing checks: cloudcontrol.create-resource-access-denied-blocked, cloudcontrol.create-resource-missing-permission-denied-rejected, cloudcontrol.get-resource-request-status-unauthorized-denied

Cause 8 of 9: DeleteResource/UpdateResource against an absent identifier are refused synchronously instead of asynchronously.
AWS: these operations are asynchronous — they answer HTTP 200 with a ProgressEvent carrying a RequestToken, and the not-found verdict comes from the handler, so the REQUEST terminates OperationStatus=FAILED with ErrorCode=NotFound and the handler-wrapped StatusMessage. ResourceNotFoundException is modeled on them, but it is not how an absent target of a delete/update is reported: the request record must exist and be pollable (and must keep its place in the request history).
Emulator: the call itself raises ResourceNotFoundException (HTTP 404) with message "Resource of type '<TypeName>' with identifier '<Identifier>' was not found.", so no request token is minted and nothing is pollable.
Why: a client that submits a delete/update and polls GetResourceRequestStatus (the documented flow) instead gets a synchronous exception; the failure is also absent from the request history, so ordering/audit consumers never see it.
Fix: the absent-identifier path of the DeleteResource/UpdateResource handlers must record a request and terminate it FAILED with ErrorCode=NotFound rather than refusing at the front door.
Failing checks: cloudcontrol.delete-nonexistent-resource-failed-request-ordering-preserved, cloudcontrol.update-resource-nonexistent-fails-async-not-sync, cloudcontrol.update-nonexistent-resource-must-fail

Cause 9 of 9: a well-formed patch is refused because the patch target is the stored DesiredState rather than the full resource model.
AWS: UpdateResource applies the JSON Patch to the resource's CURRENT model as the handler reads it — which includes every property the owning service defaults (for AWS::SQS::Queue, DelaySeconds=0). So [{"op":"replace","path":"/DelaySeconds","value":3}] against a queue created without DelaySeconds succeeds and the request terminates SUCCESS.
Emulator: the call is refused with InvalidRequestException "Invalid request provided: the patch document could not be applied to the resource model", because the stored model holds only the keys the create's DesiredState mentioned, so replace finds no such path.
Why: the normal way to change one property of an existing resource — replace on a property the create left defaulted — is impossible; a client must guess which properties happen to be present and fall back to add, which AWS does not require.
Fix: the UpdateResource handler must materialise the complete current resource model (including service defaults and read-only properties) before applying the patch.
Failing check: cloudcontrol.list-resource-requests-ordering-reflects-submission-order

Traces: failing check ids per cause above, collected in Issue-Trace-Ids. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs, update_resource.rs, delete_resource.rs and the request/effect materialisation in src/state.rs.

Related open issues that do not cover these causes: 20261102T041105Z-cloudcontrol-dynamodb-... and 20261102T044041Z-cloudcontrol-sqs-... describe the missing provisioning/read-through into owning services, not the absence of validation or the refusal plane.

## Evidence
Quotes from the replayed checks (suite simparity-behavioral-aws-v1); generated names/tokens/timestamps are per-run fill-ins, not requirements.

Cause 1 — "operation was expected to be refused but reached SUCCESS: {... 'TypeName': 'AWS::SSM::Parameter', 'OperationStatus': 'SUCCESS', 'ResourceModel': '{\"Name\":\"...\",\"Type\":\"String\"}'}" (no Value); "expected a ClientError, but the call SUCCEEDED and returned {'ProgressEvent': {'TypeName': 'AWS::IAM::Role', ... 'OperationStatus': 'IN_PROGRESS'}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}"; and an UPDATE removing the required /Value reaching 'OperationStatus': 'SUCCESS'.

Cause 2 — "unexpected final status: {... 'ResourceModel': '{\"QueueName\":\"...\",\"ThisPropertyDoesNotExistInTheSchema\":\"surprise\"}'}" with OperationStatus SUCCESS; same with "SimParityNotAProperty":"x" on AWS::SSM::Parameter.

Cause 3 — "expected the malformed request to fail, but it succeeded: {... 'TypeName': 'AWS::CloudWatch::Alarm', ... \"Threshold\":\"not-a-number\"}"; "operation was expected to be refused but reached SUCCESS: ... '{\"Name\":\"...\",\"Type\":\"String\",\"Value\":12345}'"; "the call was expected to fail but returned {'ProgressEvent': {... 'OperationStatus': 'IN_PROGRESS'}, ... 'HTTPStatusCode': 200}".

Cause 4 — SSM Tier "sim2real-not-a-tier" → SUCCESS; LogGroupName "<name>:colon-not-allowed" → SUCCESS; SQS DelaySeconds 901 → "expected FAILED, got {... 'OperationStatus': 'SUCCESS', 'ResourceModel': '{\"QueueName\":\"...\",\"DelaySeconds\":901}'}".

Cause 5 — "expected UpdateResource to be rejected, but it SUCCEEDED: {... 'Operation': 'UPDATE', 'OperationStatus': 'SUCCESS', 'ResourceModel': '{\"QueueName\":\"...\",\"VisibilityTimeout\":30,\"FifoQueue\":true}'}"; also 200/IN_PROGRESS accepted for a FifoQueue mutation, and a CachePolicy rename reaching UPDATE SUCCESS.

Cause 6 — "a handler refusal must drive the request to FAILED, got {... 'OperationStatus': 'SUCCESS', 'ResourceModel': '{... \"Value\":\"not-a-number\",\"AllowedPattern\":\"^[0-9]+$\" ...}'}"; "a create whose KeySchema references an undefined attribute reached {... 'OperationStatus': 'SUCCESS'}"; "expected the FIFO name/flag mismatch to fail the create, got: {... 'Identifier': '<name>.fifo', 'ResourceModel': '{\"QueueName\":\"<name>.fifo\",\"FifoQueue\":false}'}"; "expected the never-validated certificate create to terminate FAILED, got {... 'OperationStatus': 'SUCCESS'}".

Cause 7 — "unexpected status: {'TypeName': 'AWS::SQS::Queue', ... 'OperationStatus': 'SUCCESS'}" where the check requires OperationStatus=FAILED and ErrorCode="AccessDenied" for a create submitted with RoleArn of a role holding no policies; "expected a denial error, but the call kept succeeding: {'ProgressEvent': {... 'OperationStatus': 'SUCCESS'}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}" for GetResourceRequestStatus by a denied principal.

Cause 8 — "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteResource operation: Resource of type 'AWS::SQS::Queue' with identifier '<identifier>' was not found." and the same shape from UpdateResource; AWS answers 200 and the request then terminates FAILED/NotFound.

Cause 9 — "botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the UpdateResource operation: Invalid request provided: the patch document could not be applied to the resource model" for PatchDocument [{"op":"replace","path":"/DelaySeconds","value":3}] on a queue created without DelaySeconds.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol").

Cause 1: cc.create_resource(TypeName="AWS::SSM::Parameter", DesiredState=json({"Name": n, "Type": "String"})) — must be refused (InvalidRequestException) or terminate FAILED/InvalidRequest; observed SUCCESS. Same for AWS::IAM::Role without AssumeRolePolicyDocument, and for update_resource PatchDocument [{"op":"remove","path":"/Value"}].
Cause 2: cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState=json({"QueueName": n, "ThisPropertyDoesNotExistInTheSchema": "surprise"})) — must be refused.
Cause 3: DesiredState with AWS::CloudWatch::Alarm Threshold="not-a-number", or AWS::SSM::Parameter Value=12345 — must be refused.
Cause 4: AWS::SSM::Parameter Tier="not-a-tier"; AWS::Logs::LogGroup LogGroupName="<n>:colon"; AWS::SQS::Queue DelaySeconds=901 — each must be refused or terminate FAILED.
Cause 5: create an AWS::SQS::Queue, then cc.update_resource(PatchDocument=[{"op":"add","path":"/FifoQueue","value":true}]) — must be NotUpdatableException; observed UPDATE SUCCESS.
Cause 6: AWS::SSM::Parameter with Value="not-a-number" and AllowedPattern="^[0-9]+$"; AWS::DynamoDB::Table whose KeySchema names an attribute not in AttributeDefinitions; AWS::SQS::Queue QueueName ending ".fifo" with FifoQueue=false — each must terminate OperationStatus=FAILED with ErrorCode and StatusMessage set.
Cause 7: iam.create_role(trust: cloudformation.amazonaws.com) with no policies, then cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState=json({"QueueName": n}), RoleArn=<that role>) and poll — must reach FAILED with ErrorCode "AccessDenied" and create no queue.
Cause 8: cc.delete_resource(TypeName="AWS::SQS::Queue", Identifier="<identifier nothing created>") and cc.update_resource likewise — must return 200 with a RequestToken whose request then terminates FAILED/NotFound, not raise ResourceNotFoundException.
Cause 9: create an AWS::SQS::Queue without DelaySeconds, then cc.update_resource(PatchDocument=[{"op":"replace","path":"/DelaySeconds","value":3}]) — must be accepted and terminate SUCCESS.
