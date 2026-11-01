Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-2-deployment-mode-express cloudformation.deep-cloudformation-2-retain-except-on-create cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2 cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2 cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deep-cloudformation-2-deployment-mode-express|run#5|AssertionError|assert stack.get("StackStatusReason") == (
Issue-Key: cloudformation.deep-cloudformation-2-retain-except-on-create|_create_and_roll_back#3|AssertionError|assert stack.get("StackStatusReason") is None, (
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2|run#13|AssertionError|assert op["Status"] == "SUCCEEDED", (
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2|run#22|AssertionError|assert conflicts == [(
Issue-Key: cloudformation|DeleteStack|NotImplemented
Issue-Commit: b86628da10cf577bf631674c2f60a96534f73f23

# cloudformation: 5 net-new behavioral root causes — EXPRESS-mode CREATE_FAILED reason, a reason leaked onto ROLLBACK_COMPLETE, FailureToleranceCount ignored by stack-set operation status, no VALIDATION_ERROR operation events, and DeleteStack(RetainResources) NotImplemented

> _Note: this issue was identified against the code base at commit `b86628da10cf577bf631674c2f60a96534f73f23`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
FIVE independent net-new behavioral root causes of service `cloudformation`, grounded in the five failing behavioral checks listed in trace_ids. Each cause is independent and must be closed separately before this issue is submitted as fixed.

=== Cause 1 of 5: an EXPRESS-mode failed create reports the STANDARD-mode StackStatusReason ===
AWS: Smithy models CreateStack.DeploymentConfig -> DeploymentConfig{Mode: DeploymentConfigMode, DisableRollback: boolean}; Mode=EXPRESS is documented as "Complete the stack operation when resource configuration is applied, without waiting for resources to become ready to serve traffic. Resources continue becoming ready in the background." Recorded behavior for the failure path: a stack created with DeploymentConfig={"Mode":"EXPRESS"} whose resource create fails stops at StackStatus CREATE_FAILED (EXPRESS resolves DisableRollback to its mode default, true) and DescribeStacks reports an EXPRESS-specific StackStatusReason. Its FIXED text is `The following resource(s) failed to create via Express Mode: [` ... `]. ` (note the trailing space); its VARIABLE part is the comma-separated list of logical resource ids of the resources that failed, taken from the template and the operation's state. The same check further requires, for the EXPRESS + DisableRollback=false arm that does roll back, the fixed EXPRESS reason `Stack operation completed using Express Mode. Resources may continue becoming available in the background.`, and requires DescribeStacks to echo DeploymentConfig back with Mode=EXPRESS and the resolved DisableRollback=true.
Emulator: the create does stop at CREATE_FAILED, but StackStatusReason is the mode-independent sentence `The following resource(s) failed to create: [B]. ` — the `via Express Mode` segment is absent, so an EXPRESS stack is indistinguishable from a STANDARD one.
Why: the reason string is the only place the operation tells a client which deployment mode produced the terminal state; a client (or an operator diffing against AWS) reading the STANDARD sentence concludes the EXPRESS request was silently downgraded, and tooling that keys remediation off the mode-specific wording never fires.
Fix: the terminal-status reason composition for a failed create in the cloudformation service's CreateStack/stack-lifecycle handler — it must branch on the operation's resolved DeploymentConfig.Mode and render the EXPRESS texts, and DescribeStacks must project the resolved DeploymentConfig onto the stack.
Traces: cloudformation.deep-cloudformation-2-deployment-mode-express.

=== Cause 2 of 5: a ROLLBACK_COMPLETE stack carries the create-failure reason as its StackStatusReason ===
AWS: recorded behavior only — after a failed create rolls back, DescribeStacks reports StackStatus ROLLBACK_COMPLETE with NO StackStatusReason member at all. The `The following resource(s) failed to create: [...]. ` sentence belongs to the failing resource's event and to the CREATE_FAILED / ROLLBACK_IN_PROGRESS transitions, not to the terminal rolled-back stack.
Emulator: the terminal ROLLBACK_COMPLETE stack carries StackStatusReason `The following resource(s) failed to create: [B]. ` (fixed text `The following resource(s) failed to create: [` ... `]. ` with the failed logical ids interpolated).
Why: clients and CLI output treat a present StackStatusReason on a terminal state as the reason for THAT state; here it mis-attributes a create failure to a completed rollback, and any client that gates on "rolled back cleanly == no status reason" (the standard way to distinguish a clean rollback from ROLLBACK_FAILED) takes the failure branch on a successful rollback.
Fix: the rollback-completion transition in the cloudformation service's stack-lifecycle/state code — clear the stack-level status reason when entering ROLLBACK_COMPLETE instead of carrying the create-failure reason forward; DescribeStacks must then omit the member.
Traces: cloudformation.deep-cloudformation-2-retain-except-on-create.

=== Cause 3 of 5: a stack-set operation's terminal status ignores OperationPreferences.FailureToleranceCount ===
AWS: Smithy models StackSetOperation.Status as the StackSetOperationStatus enum and documents the rule in the model itself — `SUCCEEDED`: "The operation completed creating or updating all the specified stacks without exceeding the failure tolerance for the operation"; `FAILED`: "The operation exceeded the specified failure tolerance. ... If the number of failed stacks within a Region exceeds the failure tolerance, the status of the operation in the Region is set to FAILED. This in turn sets the status of the operation as a whole to FAILED, and CloudFormation cancels the operation in any remaining Regions." StackSetOperationPreferences.FailureToleranceCount is modelled (integer, range min 0) and documented as "The number of accounts per Region this operation can fail in before CloudFormation stops the operation in that Region." So a SEQUENTIAL two-region CreateStackInstances with FailureToleranceCount=1 and exactly ONE failed instance has NOT exceeded tolerance: DescribeStackSetOperation must report Status=SUCCEEDED, and the remaining region must still be deployed.
Emulator: the operation ends Status=FAILED with StatusDetails={"FailedStackInstancesCount": 1} and StatusReason `The following resource(s) failed to create: [Param]. ` — i.e. any non-zero failure count is treated as operation failure and the tolerance is never consulted. (The zero-tolerance arm does end FAILED, so the defect is specifically that the tolerance threshold is not compared against the failure count.)
Why: FailureToleranceCount is the entire mechanism by which an operator fans a stack set out across accounts/regions and absorbs expected per-target failures; with it ignored, every multi-target rollout that tolerates a single failure is reported as a failed operation, and any client that stops on FAILED abandons regions AWS would have deployed.
Fix: the terminal-status computation for a stack-set operation in the cloudformation service's stack-set operation engine — compare the per-Region failed-instance count against the resolved FailureToleranceCount/FailureTolerancePercentage before setting Status, and only cancel remaining regions once the tolerance is exceeded.
Traces: cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2.

=== Cause 4 of 5: DescribeEvents never emits a VALIDATION_ERROR operation event, so a failed operation has no root cause ===
AWS: Smithy models DescribeEvents with output OperationEvents (list of OperationEvent) and input Filters -> EventFilter{FailedEvents: boolean}, documented as "When set to true, only returns failed events within the operation. This helps quickly identify root causes for a failed operation." OperationEvent.EventType is the EventType enum whose values are [STACK_EVENT, PROGRESS_EVENT, VALIDATION_ERROR, PROVISIONING_ERROR, HOOK_INVOCATION_ERROR], and OperationEvent carries ValidationName, ValidationStatus, ValidationStatusReason and ValidationPath for exactly this purpose. Recorded behavior: when a resource create fails because a resource with that explicit name already exists, DescribeEvents(StackName=<stack id>, Filters={"FailedEvents": True}) contains an OperationEvent with EventType=VALIDATION_ERROR, ValidationName=`NAME_CONFLICT_VALIDATION` and ValidationStatusReason whose fixed text is `Resource of type '` ... `' with identifier '` ... `' already exists.` — the two variable parts are the CloudFormation resource type of the colliding resource and the colliding physical identifier, both taken from the template/request and the existing state.
Emulator: DescribeEvents with FailedEvents=true returns OperationEvents containing no VALIDATION_ERROR entry at all — the collected (ValidationName, ValidationStatusReason) pairs are `[]` — even though the operation and the stack instance are both reported FAILED.
Why: the stack-instance StatusReason AWS returns is only a generic "Validation failed with N error(s)" pointer; the VALIDATION_ERROR operation event is the only place the actual cause is named. With it missing, a client that drills from a FAILED stack-set operation into DescribeEvents can never learn WHY a target failed, and the whole FailedEvents filter is vacuous.
Fix: operation-event recording in the cloudformation service's DescribeEvents/operation-event store — pre-flight and runtime resource validation failures must be recorded as OperationEvents with EventType=VALIDATION_ERROR and the ValidationName/ValidationStatusReason/ValidationPath members populated, and the EventFilter.FailedEvents filter must select them.
Traces: cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2.

=== Cause 5 of 5: DeleteStack with RetainResources answers NotImplemented on the happy path ===
AWS: Smithy models DeleteStack.RetainResources as a first-class input member (list of LogicalResourceId) alongside StackName/RoleARN/ClientRequestToken/DeletionMode/DeploymentConfig, with only TokenAlreadyExistsException declared. DeleteStack(StackName=<a DELETE_FAILED stack>, RetainResources=[<logical id>]) must succeed: the stack reaches DELETE_COMPLETE while the named resource and its contents are left physically intact in the owning service.
Emulator: the call is refused with HTTP error code `NotImplemented` and message `operation DeleteStack with RetainResources is scaffolded but not implemented in this emulator` — a server-class (Receiver) refusal for a modelled input member, so the feature is entirely unreachable.
Why: RetainResources is the only way an operator deletes a stack while keeping specific resources (the standard escape hatch for a DELETE_FAILED stack holding a non-empty bucket); against the emulator that recovery path does not exist, and the misclassification as a service fault makes SDK retry layers retry a permanent condition.
Note: the OPEN issue 20261029T214321Z-cloudformation-resource-scans-find-nothing-hook-handler-progress.md already describes this same unimplemented branch as its Cause 7, grounded in the REFUSAL check cloudformation.delete-stack-retain-resources-invalid-list-rejected. This check is the happy-path half of the same branch and the replay report marks it neither ALREADY FILED nor OWNED, so it is recorded here; a fixer closing that issue's Cause 7 should re-replay this check id too.
Fix: the RetainResources branch of the DeleteStack handler in the cloudformation service's stack ops — validate the named logical ids against their resource states, skip deletion of those resources (DELETE_SKIPPED), and take the stack to DELETE_COMPLETE.
Traces: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact.

## Evidence
Cause 1 — cloudformation.deep-cloudformation-2-deployment-mode-express [mismatch]:
  AssertionError at check_cloudformation_deep_cloudformation_2_deployment_mode_express.py:137
    assert stack.get("StackStatusReason") == ("The following resource(s) failed to create via Express Mode: [B]. ")
  actual StackStatusReason: 'The following resource(s) failed to create: [B]. '
  (stack reached CREATE_FAILED as required; only the reason text diverges)
  Smithy: CreateStack.DeploymentConfig -> DeploymentConfig{Mode: DeploymentConfigMode (STANDARD|EXPRESS), DisableRollback}; Mode EXPRESS documented as completing the operation when resource configuration is applied.

Cause 2 — cloudformation.deep-cloudformation-2-retain-except-on-create [mismatch]:
  AssertionError at check_cloudformation_deep_cloudformation_2_retain_except_on_create.py:136
    assert stack.get("StackStatusReason") is None
  actual: [retain] ROLLBACK_COMPLETE carries reason 'The following resource(s) failed to create: [B]. '
  (StackStatus was ROLLBACK_COMPLETE as required; the leaked reason is the divergence)

Cause 3 — cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2 [mismatch]:
  AssertionError: operation <op-id> ended 'FAILED', expected 'SUCCEEDED' (1 failure within tolerance 1); op={'Action': 'CREATE', 'Status': 'FAILED', 'OperationPreferences': {'RegionConcurrencyType': 'SEQUENTIAL', 'RegionOrder': ['us-west-2', 'us-east-1'], 'FailureToleranceCount': 1, 'MaxConcurrentCount': 1}, 'StatusReason': 'The following resource(s) failed to create: [Param]. ', 'StatusDetails': {'FailedStackInstancesCount': 1}}
  Smithy: StackSetOperation.Status (StackSetOperationStatus) documents SUCCEEDED = completed "without exceeding the failure tolerance for the operation", FAILED = "exceeded the specified failure tolerance"; StackSetOperationPreferences.FailureToleranceCount (integer, range min 0, default 0).

Cause 4 — cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2 [mismatch]:
  AssertionError at line 85: region-1 stack validation errors were [], expected exactly the planted name conflict
    expected [( 'NAME_CONFLICT_VALIDATION', "Resource of type 'AWS::SSM::Parameter' with identifier '<param-name>' already exists." )]
  collected from DescribeEvents(StackName=<region-1 stack id>, Filters={'FailedEvents': True}) over OperationEvents where EventType == 'VALIDATION_ERROR'
  Smithy: DescribeEvents output OperationEvents -> OperationEvent{EventType, ValidationName, ValidationStatus, ValidationStatusReason, ValidationPath, ...}; EventType enum = [STACK_EVENT, PROGRESS_EVENT, VALIDATION_ERROR, PROVISIONING_ERROR, HOOK_INVOCATION_ERROR]; EventFilter.FailedEvents documented to "quickly identify root causes for a failed operation".

Cause 5 — cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteStack operation: operation DeleteStack with RetainResources is scaffolded but not implemented in this emulator
  Smithy: DeleteStack input {StackName required, RetainResources (list of LogicalResourceId), RoleARN, ClientRequestToken, DeletionMode, DeploymentConfig}, errors = [TokenAlreadyExistsException (client, 400)].

## Steps to Reproduce
Cause 1: CreateStack with DeploymentConfig={"Mode":"EXPRESS"} and a template whose resource B fails only at runtime (two AWS::SSM::Parameter resources where B's Name is {"Ref": "A"}, so B collides with A after A is CREATE_COMPLETE). Poll DescribeStacks to the terminal state: status is CREATE_FAILED (correct) but StackStatusReason omits "via Express Mode". Repeat with DeploymentConfig={"Mode":"EXPRESS","DisableRollback":false} and check the ROLLBACK_COMPLETE reason text and the echoed DeploymentConfig.

Cause 2: CreateStack with the same runtime-colliding template and no DeploymentConfig; poll DescribeStacks to ROLLBACK_COMPLETE and read StackStatusReason — it carries the create-failure sentence instead of being absent.

Cause 3: CreateStackSet (SELF_MANAGED, admin/exec role pair) over a template that writes an AWS::SSM::Parameter named by a parameter; plant that parameter out of band in region 1 only; CreateStackInstances for [region1, region2] with OperationPreferences={RegionConcurrencyType: SEQUENTIAL, RegionOrder: [region1, region2], MaxConcurrentCount: 1, FailureToleranceCount: 1}; DescribeStackSetOperation — Status is FAILED instead of SUCCEEDED, and region 2 is not deployed.

Cause 4: same setup as Cause 3 but with FailureToleranceCount=0; after the operation ends FAILED, take the region-1 stack instance's StackId and call DescribeEvents(StackName=<that stack id>, Filters={"FailedEvents": True}) in region 1 — no OperationEvent has EventType=VALIDATION_ERROR, so no ValidationName/ValidationStatusReason naming the name conflict is returned.

Cause 5: create a stack owning an S3 bucket, make its delete fail (non-empty bucket) so the stack is DELETE_FAILED, then DeleteStack(StackName=<stack>, RetainResources=[<bucket logical id>]) — observe NotImplemented instead of the stack reaching DELETE_COMPLETE with the bucket and its objects intact.

Re-replay all five check ids listed in trace_ids.
