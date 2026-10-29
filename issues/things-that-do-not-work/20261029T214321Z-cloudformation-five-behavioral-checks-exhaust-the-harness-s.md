Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.changeset-execute-invalid-status-rejected cloudformation.missing-capability-iam-error cloudformation.resource-scan-lifecycle cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.changeset-execute-invalid-status-rejected|_wait_change_set_status#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.missing-capability-iam-error|run#19|Timeout|cf.get_waiter("stack_create_complete").wait(StackName=stack_name)
Issue-Key: cloudformation.resource-scan-lifecycle|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_http_until#13|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write|_http_until#12|Timeout|time.sleep(3.0)
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: five behavioral checks exhaust the harness's 60s deadline — change-set execution, capability refusal, resource-scan lifecycle and CDK REST-API deployments never reach a terminal state (1 cause)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
One net-new behavioral root cause of service `cloudformation`, grounded in five failing behavioral checks. A fixer must close it before submitting.

Cause 1 of 1: operations these five checks poll never reach a terminal state inside the suite's per-check 60s budget, so each check dies on the harness deadline with no response to compare. The failure is the emulator's: the checks poll the emulator's own status surfaces (DescribeChangeSet ExecutionStatus, DescribeStacks StackStatus, DescribeResourceScans Status) and those never settle, or settle too late to be observed.

AWS: recorded behavior only — Smithy models the status enums (ChangeSetStatus/ExecutionStatus, StackStatus, ResourceScanStatus) but no timing. On AWS each of these sequences converges well inside the budget: ExecuteChangeSet drives the change set from AVAILABLE through EXECUTE_IN_PROGRESS to EXECUTE_COMPLETE (and the stack to a terminal *_COMPLETE/*_FAILED); a CreateStack missing CAPABILITY_IAM is refused synchronously; StartResourceScan reaches COMPLETE; and a CDK-deployed AWS::ApiGateway::RestApi stack reaches CREATE_COMPLETE and serves its stage.

Emulator: each check is cut off by `Error: Check timed out after 60.0s` with no terminal status observed. Because the deadline fires first, nothing can be said about the response SHAPE of these five flows — only that they do not converge.

Why: convergence is the contract every poll-based client depends on. A client waiting on a waiter (`stack_create_complete`, `change_set_create_complete`) hangs rather than being told what happened, so no deployment driven by the emulator can be scripted the way it is on AWS.

Fix: the status/terminal-state progression in the cloudformation service's stack and change-set engines and in the resource-scan engine — each polled surface must reach a terminal value promptly, and the synchronous refusal path (missing capability) must refuse instead of leaving the caller polling. The previously-resolved issue 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md addressed this class for a different set of 23 checks; these five are not among its replay keys and still do not converge, so this is reported as the remaining set rather than a re-open of that work item.

Traces: cloudformation.changeset-execute-invalid-status-rejected, cloudformation.missing-capability-iam-error, cloudformation.resource-scan-lifecycle, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write.

## Evidence
All five report the same terminal diagnosis with no AWS response to diff:
  cloudformation.changeset-execute-invalid-status-rejected [mismatch] — Error: Check timed out after 60.0s
  cloudformation.missing-capability-iam-error [mismatch] — Error: Check timed out after 60.0s
  cloudformation.resource-scan-lifecycle [mismatch] — Error: Check timed out after 60.0s
  cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves [mismatch] — Error: Check timed out after 60.0s
  cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write [mismatch] — Error: Check timed out after 60.0s

The 60.0s figure is the suite's per-check budget, not a requirement; the requirement is that the polled status reaches a terminal value (and that the missing-capability CreateStack is refused synchronously rather than polled at all). Smithy consulted: ChangeSetStatus/ExecutionStatus on DescribeChangeSetOutput, StackStatus on DescribeStacks, and the InsufficientCapabilitiesException (client, httpError 400) modelled on CreateStack/CreateChangeSet/ExecuteChangeSet — the model carries the terminal values but no timing, hence recorded behavior only.

## Steps to Reproduce
1. CreateStack with a trivial template, CreateChangeSet(ChangeSetType='UPDATE'), ExecuteChangeSet, then poll DescribeChangeSet for ExecutionStatus and DescribeStacks for StackStatus. Observe no terminal value within 60s.
2. CreateStack with a template declaring an AWS::IAM::Role and NO Capabilities. Observe the call does not refuse synchronously with 400 InsufficientCapabilitiesException, leaving the check polling.
3. StartResourceScan and poll DescribeResourceScan(s) for Status. Observe no COMPLETE within 60s.
4. Deploy the CDK RestApi stacks the two cdk-restapi checks synthesise and poll DescribeStacks to CREATE_COMPLETE, then invoke the stage URL. Observe no terminal status within 60s.
5. Re-replay: cloudformation.changeset-execute-invalid-status-rejected, cloudformation.missing-capability-iam-error, cloudformation.resource-scan-lifecycle, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write.
