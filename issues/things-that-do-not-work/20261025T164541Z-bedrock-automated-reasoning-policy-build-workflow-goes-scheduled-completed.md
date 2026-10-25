Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-automated-reasoning-policy-in-use-rejected bedrock.start-test-workflow-while-build-in-progress-blocked
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-automated-reasoning-policy-in-use-rejected|run#10|AssertionError|raise AssertionError(
Issue-Key: bedrock.start-test-workflow-while-build-in-progress-blocked|run#13|AssertionError|raise AssertionError(
Issue-Commit: 32539e30dc69a7e797fcddb8b2bba51f1e444ebd

# Automated Reasoning policy build workflow goes SCHEDULED -> COMPLETED with no in-progress state, so PREPROCESSING/BUILDING/TESTING and the two in-progress rejections are unobservable

> _Note: this issue was identified against the code base at commit `32539e30dc69a7e797fcddb8b2bba51f1e444ebd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomatedReasoningPolicyBuildWorkflow

## Description
ONE CAUSE on the Automated Reasoning policy build-workflow state machine. (Smithy model read: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json.)

Cause 1 of 1 - a started build workflow never occupies a PROCESSING state: it is reported SCHEDULED and then COMPLETED, so no caller can ever observe the workflow while it is running, and neither in-progress rejection that depends on that window can be reached.

AWS: Smithy StartAutomatedReasoningPolicyBuildWorkflow (POST /automated-reasoning-policies/{policyArn}/build-workflows/{buildWorkflowType}/start, 200) is documented as "Starts a new build workflow ... This initiates the process of analyzing source documents and generating policy rules, variables, and types" - an asynchronous build. GetAutomatedReasoningPolicyBuildWorkflow returns a @required `status` of AutomatedReasoningPolicyBuildWorkflowStatus, whose values are [SCHEDULED, CANCEL_REQUESTED, PREPROCESSING, BUILDING, TESTING, COMPLETED, FAILED, CANCELLED]; a real INGEST_CONTENT build passes through the processing values (PREPROCESSING -> BUILDING -> TESTING) before it terminates, which is why those values exist on the enum at all and why `updatedAt` is @required alongside the status. Two modeled rejections are defined only over that non-terminal window: StartAutomatedReasoningPolicyTestWorkflow against a build workflow that is not COMPLETED is refused with ValidationException (Smithy: errorKind client, httpError 400), message "Build workflow: <buildWorkflowId> is not in COMPLETED status" - fixed text "Build workflow: " + " is not in COMPLETED status", with the buildWorkflowId taken from the request (message text is recorded behavior only; the status class and error code are modeled); and DeleteAutomatedReasoningPolicy on a policy that still has an undeleted build workflow is refused with ValidationException (400), message "There are build workflow that have not been deleted." - entirely fixed text (recorded behavior only).

Emulator: the start handler mints the record with status SCHEDULED and a single `completed_due_at = now + BUILD_WORKFLOW_COMPLETE_AFTER`, and the settle step moves it straight to COMPLETED; there is no PREPROCESSING, BUILDING or TESTING step in between. Both checks poll GetAutomatedReasoningPolicyBuildWorkflow until the status is one of {PREPROCESSING, BUILDING, TESTING, COMPLETED, FAILED}, and the first status that ever satisfies that predicate is COMPLETED - i.e. the workflow is terminal the moment it stops being SCHEDULED. Consequently neither check could issue its probe against a running build, and whether the emulator implements the two ValidationException rejections above is not even observable.

Why: a client that polls a build workflow for its processing states (the normal way to drive this API, and the only way to show progress) sees them never; and a client whose negative paths depend on the in-progress conflicts - "you cannot start a test workflow until the build is COMPLETED" and "you cannot delete a policy whose build workflows still exist" - cannot exercise them against the emulator at all, so code validated here will meet 400 ValidationException in production unexercised.

Fix: the build-workflow state machine in the service state (`settle_automated_reasoning_policy_build_workflows` / `BUILD_WORKFLOW_COMPLETE_AFTER` in the control-plane crate's state) together with the record the start handler mints, so a started workflow advances through the modeled processing statuses with a non-zero observable window before COMPLETED; and the in-progress rejection paths in the StartAutomatedReasoningPolicyTestWorkflow and DeleteAutomatedReasoningPolicy handlers, which this cause makes unreachable.

Traces: bedrock.delete-automated-reasoning-policy-in-use-rejected, bedrock.start-test-workflow-while-build-in-progress-blocked.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock 32/84 passed. The emulator answered these calls (CreateAutomatedReasoningPolicy, StartAutomatedReasoningPolicyBuildWorkflow and GetAutomatedReasoningPolicyBuildWorkflow all returned 200s), so the service is up and answering wrongly.

bedrock.delete-automated-reasoning-policy-in-use-rejected [mismatch]:
  AssertionError: the build workflow reached a terminal state before we could probe the delete-in-use conflict; timing is not guaranteed on real AWS
  (raised after the check polled GetAutomatedReasoningPolicyBuildWorkflow for up to 120 s at 5 s intervals and the first status matching {PREPROCESSING, BUILDING, TESTING, COMPLETED, FAILED} was a terminal one)

bedrock.start-test-workflow-while-build-in-progress-blocked [mismatch]:
  AssertionError: the build workflow reached a terminal state before we could probe it while still in progress; timing is not guaranteed on real AWS
  (same poll; the check never got to issue StartAutomatedReasoningPolicyTestWorkflow against a non-COMPLETED build, which is where it asserts code == 'ValidationException' and message == f"Build workflow: {workflow_id} is not in COMPLETED status")

Code citations for the state machine: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/start_automated_reasoning_policy_build_workflow.rs lines 12-14 ("A workflow starts `SCHEDULED` and the clock owes it `COMPLETED`"), 195 (settle_automated_reasoning_policy_build_workflows) and 267-268 (status = BUILD_WORKFLOW_STATUS_SCHEDULED, completed_due_at = now + BUILD_WORKFLOW_COMPLETE_AFTER); services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs lines 71-75 (the stored workflow holds only "the instant its non-terminal `status` is owed its move to `COMPLETED`").

Model evidence: smithy_query operation StartAutomatedReasoningPolicyBuildWorkflow / GetAutomatedReasoningPolicyBuildWorkflow / StartAutomatedReasoningPolicyTestWorkflow / DeleteAutomatedReasoningPolicy and shape AutomatedReasoningPolicyBuildWorkflowStatus on services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json - status enum values [SCHEDULED, CANCEL_REQUESTED, PREPROCESSING, BUILDING, TESTING, COMPLETED, FAILED, CANCELLED]; both Start*TestWorkflow and DeleteAutomatedReasoningPolicy model ValidationException (400).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the check ids in trace_ids.
3. CreateAutomatedReasoningPolicy(name=<unique>, policyDefinition={}) and take its policyArn.
4. StartAutomatedReasoningPolicyBuildWorkflow(policyArn=<arn>, buildWorkflowType='INGEST_CONTENT', sourceContent={'workflowContent': {'documents': [{'document': <txt bytes>, 'documentContentType': 'txt', 'documentName': 'safety-policy.txt'}]}}) and take its buildWorkflowId.
5. Poll GetAutomatedReasoningPolicyBuildWorkflow(policyArn, buildWorkflowId) every 5 s and record each status. Expected: at least one reading in {PREPROCESSING, BUILDING, TESTING} before COMPLETED. Observed: SCHEDULED readings followed directly by COMPLETED - no processing status is ever reported.
6. While the build is non-terminal, StartAutomatedReasoningPolicyTestWorkflow(policyArn, buildWorkflowId). Expected 400 ValidationException with message 'Build workflow: <buildWorkflowId> is not in COMPLETED status'. Not reachable on the emulator, because step 5 leaves no non-terminal window.
7. While a build workflow of the policy still exists, DeleteAutomatedReasoningPolicy(policyArn=<arn>). Expected 400 ValidationException with message 'There are build workflow that have not been deleted.' Not reachable for the same reason.
