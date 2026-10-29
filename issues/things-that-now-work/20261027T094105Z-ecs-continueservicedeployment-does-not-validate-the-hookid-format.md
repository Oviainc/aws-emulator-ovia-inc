Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.continue-service-deployment-rejects-non-waiting-state
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|ContinueServiceDeployment|InvalidParameterException
Issue-Commit: 22d21de87b4de3adf3790608eb9eeee3e17c55ae

# ECS ContinueServiceDeployment does not validate the hookId format: a malformed hookId is refused with emulator-internal wording instead of AWS's 'Invalid hookId: ...' message

> _Note: this issue was identified against the code base at commit `22d21de87b4de3adf3790608eb9eeee3e17c55ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueServiceDeployment

## Description
THIS BUNDLE CONTAINS 1 CAUSE.

Cause 1 of 1 — ContinueServiceDeployment accepts a syntactically invalid hookId and refuses it as a 'not the waiting hook' condition, emitting a message AWS never produces.

AWS: Smithy (com.amazonaws.ecs#ContinueServiceDeployment): input members serviceDeploymentArn (String, required), hookId (String, required), action (DeploymentLifecycleHookAction: CONTINUE | ROLLBACK, default CONTINUE); modeled errors AccessDeniedException (403), ClientException (400, httpError defaulted), InvalidParameterException (400, httpError defaulted), ServerException (500), ServiceDeploymentNotFoundException (400, httpError defaulted), UnsupportedFeatureException (400). The model documents hookId as 'The ID of the paused lifecycle hook to act on. You can find the hookId by calling DescribeServiceDeployments and inspecting the lifecycleHookDetails field', and InvalidParameterException as 'The specified parameter isn't valid.' Recorded behavior: AWS validates the SHAPE of hookId before it looks the deployment or its hooks up — a hookId that is not of the form ecs-{hookType}-{id} is refused with code InvalidParameterException, HTTP 400, and message 'Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}'. That whole message is FIXED text for this error class: there is no variable part — the literal token sequence 'ecs-{hookType}-{id}' is the format template AWS prints, not a substitution of the request's value, and the offending hookId is NOT echoed. (The same recorded path also shows that a serviceDeploymentArn whose deployment-id suffix is not a real deployment-id token is refused InvalidParameterException with the fixed message 'The ServiceDeployment ARN is invalid.' — ARN-shape validation, not a resource lookup, so NOT ServiceDeploymentNotFoundException.)

Emulator: the call is refused with the right code (InvalidParameterException) and status, but with the message 'The specified hookId not-a-paused-hook isn't valid for service deployment arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<deployment-id>. Review the available parameters for the API request.' — i.e. the hookId is echoed and the deployment ARN is interpolated, and the text describes a 'not the waiting hook' condition. There is no hookId-format check at all: ContinueServiceDeployment's handler trims hookId and hands it straight to the state machine (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/continue_service_deployment.rs, handle(), the match on state::act_on_lifecycle_hook), so EVERY unrecognized hookId — malformed or merely not-currently-waiting — collapses onto the single LifecycleHookRefusal::NotAwaitingAction arm and its message builder state::lifecycle_hook_not_awaiting_message(hook_id, deployment_arn). AWS distinguishes the two: a malformed hookId never reaches the hook lookup.

Why: a client that distinguishes 'you passed a hookId that is not even well-formed' (a caller bug — fix the string, e.g. read lifecycleHookDetails) from 'that hook is not the one currently awaiting an action' (a timing/state condition — re-describe and retry) by matching the refusal message gets no match, and an operator is shown text pointing at the deployment's state when the real cause is the request's syntax. Deployment-automation that retries on the state condition and fails fast on the syntax condition takes the wrong branch. The message also leaks the full deployment ARN into a refusal where AWS leaks nothing, so message-based assertions and log scrubbers both diverge.

Fix: the ContinueServiceDeployment handler's input validation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/continue_service_deployment.rs) must validate hookId's format BEFORE consulting state and, when it does not match ecs-{hookType}-{id}, return InvalidParameterException with AWS's fixed text 'Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}'; the existing NotAwaitingAction message must remain only for a well-formed hookId that is not the waiting one. The serviceDeploymentArn shape check belongs in the same validation step, returning InvalidParameterException / 'The ServiceDeployment ARN is invalid.' rather than the ServiceDeploymentNotFoundException path the handler currently takes for an unknown ARN; the check aborts on the first mismatch, so that second assertion was never reached — re-replay the check after fixing the hookId message to grade it.

This is net-new: the nearest open issue, 20261025T112916Z-ecs-refusal-messages-use-emulator-internal-wording-createservice.md, bundles the CreateService all-INACTIVE-family and CreateCluster clusterName messages and names neither ContinueServiceDeployment nor its handler.

Traces: ecs.continue-service-deployment-rejects-non-waiting-state.

## Evidence
Check ecs.continue-service-deployment-rejects-non-waiting-state (verdict: mismatch). The check creates a cluster, registers a task definition, creates an EC2/REPLICA service with desiredCount=1 on a cluster with no capacity, waits for a service deployment to appear, confirms it is PENDING/IN_PROGRESS with no lifecycleHookDetails, then calls ContinueServiceDeployment{serviceDeploymentArn=<that deployment>, hookId='not-a-paused-hook', action='CONTINUE'}.

The error CODE assertion passed (InvalidParameterException, as AWS returns), so only the message diverges. Failure:

  assert _message(e) == "Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}"
  AssertionError: unexpected message: "The specified hookId not-a-paused-hook isn't valid for service deployment arn:aws:ecs:us-west-2:000000000000:service-deployment/sim2real-ecs-contdep-fcac316ac4dd/sim2real-ecs-contdep-svc-6307561527d9/0000000000000000001. Review the available parameters for the API request."

(The cluster/service/deployment-id tokens and the region/account in the emulator's message are per-run values, not part of the requirement; the requirement is that AWS's message for this class is the fixed sentence 'Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}' with no interpolation.)

The later assertions in the same check — that the rejected call leaves the deployment status at PENDING/IN_PROGRESS, and that a bogus deployment ARN is refused InvalidParameterException / 'The ServiceDeployment ARN is invalid.' — were never evaluated because the check aborts on the first failed assertion.

Source: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/continue_service_deployment.rs lines 178-229 — hookId is only trimmed, and the single LifecycleHookRefusal::NotAwaitingAction arm produces the message via crate::state::lifecycle_hook_not_awaiting_message(&hook_id, &deployment_arn); no format validation exists, and an unknown ARN maps to ServiceDeploymentNotFoundException.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the failing check is ecs.continue-service-deployment-rejects-non-waiting-state.
2. By hand: CreateCluster{clusterName=C}; RegisterTaskDefinition{family=F, containerDefinitions=[{name:'c', image:'public.ecr.aws/docker/library/busybox:latest', memory:64, essential:true}]}; CreateService{cluster=C, serviceName=S, taskDefinition=<F's revision ARN>, desiredCount=1, launchType='EC2', schedulingStrategy='REPLICA'} (the cluster has no registered capacity, so the deployment stays PENDING/IN_PROGRESS).
3. ListServiceDeployments{cluster=C, service=S} to get serviceDeploymentArn; DescribeServiceDeployments confirms status PENDING/IN_PROGRESS and no lifecycleHookDetails.
4. ContinueServiceDeployment{serviceDeploymentArn=<that ARN>, hookId='not-a-paused-hook', action='CONTINUE'}. Observe InvalidParameterException with message "The specified hookId not-a-paused-hook isn't valid for service deployment <arn>. Review the available parameters for the API request." where AWS returns InvalidParameterException with the fixed message "Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}".
5. Then ContinueServiceDeployment{serviceDeploymentArn=<same ARN with its deployment-id suffix replaced by '0000000000000000000000000000000000000000'>, hookId='not-a-paused-hook', action='CONTINUE'}: AWS returns InvalidParameterException / 'The ServiceDeployment ARN is invalid.'; the emulator takes its ServiceDeploymentNotFoundException path.
