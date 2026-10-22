Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-deployment-group-lookup-and-validation-errors
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-deployment-group-lookup-and-validation-errors codedeploy-application-revision-registration-and-lookup codedeploy-deployment-inspection-not-found-paths codedeploy-deployment-control-actions-rejected-paths
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|CreateDeploymentGroup|4xx->2xx|$.__type
Issue-Key: codedeploy|CreateDeploymentGroup|4xx->2xx|$.message
Issue-Key: codedeploy|CreateDeploymentGroup|4xx->2xx|status
Issue-Key: codedeploy|CreateDeploymentGroup|4xx->4xx|$.__type
Issue-Key: codedeploy|CreateDeploymentGroup|4xx->4xx|$.message
Issue-Key: codedeploy|RegisterApplicationRevision|4xx->2xx|$
Issue-Key: codedeploy|RegisterApplicationRevision|4xx->2xx|status
Issue-Key: codedeploy|ListDeployments|4xx->2xx|$.__type
Issue-Key: codedeploy|ListDeployments|4xx->2xx|$.message
Issue-Key: codedeploy|ListDeployments|4xx->2xx|status
Issue-Key: codedeploy|StopDeployment|4xx->4xx|$.__type
Issue-Key: codedeploy|StopDeployment|4xx->4xx|$.message
Issue-Key: codedeploy|GetDeployment|4xx->4xx|$.__type
Issue-Key: codedeploy|GetDeployment|4xx->4xx|$.message
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# CodeDeploy accepts invalid input and misclassifies rejections

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
Six wire divergences on request-validation paths: three requests AWS refuses with a modeled 400 are accepted with HTTP 200, and three are refused with the WRONG error code (an unmodeled ValidationException, or an existence error where AWS reports a format error). Six independent validation rules, six causes.

Cause 1 of 6: CreateDeploymentGroup accepts a syntactically invalid serviceRoleArn.
AWS: Smithy CreateDeploymentGroup declares `serviceRoleArn` (Role) required and models InvalidRoleException (HTTP 400, doc "The service role ARN was specified in an invalid format..."). A value that is not an ARN at all is refused at input validation and no deployment group is created (recorded message example: "ARNs must start with 'arn:': not-a-role-arn").
Emulator: HTTP 200 with no `__type` / `message`, and the group is created.
Why: a client that depends on create-time ARN syntax validation stores a deployment group that can never deploy, and the failure surfaces far from the offending call.
Fix: CreateDeploymentGroup's input validation (serviceRoleArn syntax gate before persistence), services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/.
Traces: codedeploy-deployment-group-lookup-and-validation-errors#task#9 (same cluster also at #task#11).

Cause 2 of 6: CreateDeploymentGroup refuses an untrusted service role with an unmodeled ValidationException instead of InvalidRoleException.
AWS: Smithy CreateDeploymentGroup models InvalidRoleException (HTTP 400); ValidationException is NOT in its error list. For a well-formed ARN naming a role CodeDeploy cannot assume, AWS answers HTTP 400 / InvalidRoleException (recorded message example: 'AWS CodeDeploy does not have the permissions required to assume the role arn:aws:iam::000000000000:role/...'). In the recorded request the ec2TagFilters member also carried an out-of-enum type, and AWS still reported InvalidRoleException — recorded behavior only: the role check precedes tag-filter enum validation.
Emulator: HTTP 400 with `__type` 'ValidationException' (message example: "1 validation error detected: Value 'BOGUS_TYPE' at 'ec2TagFilters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [KEY_ONLY, VALUE_ONLY, KEY_AND_VALUE]").
Why: the code is a requirement and ValidationException is unmodeled for this operation, so a client catching the modeled InvalidRoleException (exposed on the botocore client's exception factory) does not catch the emulator's error at all; the documented retry-on-InvalidRoleException loop for freshly created roles can never run.
Fix: CreateDeploymentGroup's validation — refuse through the modeled error and order the role check ahead of member enum validation.
Traces: codedeploy-deployment-group-lookup-and-validation-errors#task#10.

Cause 3 of 6: RegisterApplicationRevision accepts an over-long revision description.
AWS: Smithy RegisterApplicationRevision declares `description` (Description) and models DescriptionTooLongException (HTTP 400, doc "The description is too long"). A description beyond the 512-character limit is refused with HTTP 400 / DescriptionTooLongException (recorded message example: 'Revision description has a maximum length of 512').
Emulator: HTTP 200 with an empty body `{}` — neither the modeled error nor any error member.
Why: the revision is registered with a description AWS would never store, so later GetApplicationRevision reads diverge and the client's own length guard (keyed on the modeled error) never fires.
Fix: RegisterApplicationRevision's input validation (description length gate). Note the open issue "Implement codedeploy RegisterApplicationRevision: unimplemented scaffold stub"; the replay shows this operation answering 200 rather than being blocked, and that issue carries no replay key for this step.
Traces: codedeploy-application-revision-registration-and-lookup#task#8.

Cause 4 of 6: ListDeployments accepts applicationName without deploymentGroupName.
AWS: Smithy ListDeployments declares `applicationName` and `deploymentGroupName` (both optional members) and models DeploymentGroupNameRequiredException (HTTP 400, doc "The deployment group name was not specified"). Supplying applicationName while omitting deploymentGroupName is refused with HTTP 400 / DeploymentGroupNameRequiredException (recorded message example: 'Deployment Group name is missing') — the pair is conditionally required.
Emulator: HTTP 200 with a deployments list and no `__type` / `message`.
Why: a client that omits the group name gets a silently different (unscoped) result set instead of the refusal, so paging logic built on the AWS contract reads deployments AWS would not have returned.
Fix: ListDeployments' input validation (conditional requirement between applicationName and deploymentGroupName).
Traces: codedeploy-deployment-inspection-not-found-paths#task#3.

Cause 5 of 6: StopDeployment has no deploymentId format gate.
AWS: Smithy StopDeployment models InvalidDeploymentIdException (HTTP 400, doc "At least one of the deployment IDs was specified in an invalid format") alongside DeploymentDoesNotExistException and DeploymentIdRequiredException. A malformed id is refused at input validation with InvalidDeploymentIdException (recorded message example: 'Specified DeploymentId is not in the valid format: !!not-a-valid-id!!'); only a well-formed-but-unknown id is discriminated on existence.
Emulator: HTTP 400 with `__type` 'DeploymentDoesNotExistException', collapsing the two error classes.
Why: a client that branches on "bad id" (fix the caller) versus "unknown id" (treat as absent/retryable) takes the wrong branch, and malformed ids are reported as a transient-looking absence.
Fix: StopDeployment's validation — format gate before the existence lookup.
Traces: codedeploy-deployment-control-actions-rejected-paths#task#2.

Cause 6 of 6: GetDeployment has the same missing deploymentId format gate.
AWS: Smithy GetDeployment models InvalidDeploymentIdException, DeploymentDoesNotExistException and DeploymentIdRequiredException (all HTTP 400). A malformed id yields HTTP 400 / InvalidDeploymentIdException (recorded message example: 'Specified DeploymentId is not in the valid format: not-a-deployment-id').
Emulator: HTTP 400 with `__type` 'DeploymentDoesNotExistException'.
Why: same client impact as Cause 5, on the most frequently polled read of the service.
Fix: GetDeployment's validation — format gate before the existence lookup. The open behavioral issue "CodeDeploy deployment lifecycle, response normalization and error classification diverge from AWS (6 causes)" describes this same gate as its Cause 4 but carries no replay key for this fixture step, so the wire cluster is filed here and both must be closed together.
Traces: codedeploy-deployment-inspection-not-found-paths#task#7.

## Evidence
Replayed wire corpus simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0). Expected = recorded AWS response, Actual = emulator.

1. codedeploy-deployment-group-lookup-and-validation-errors#task#9 CreateDeploymentGroup — expected 400 with $.__type 'InvalidRoleException'; got 200, $.__type and $.message absent. Same cluster at #task#11.
2. ...#task#10 CreateDeploymentGroup — expected $.__type 'InvalidRoleException'; got 'ValidationException' (both HTTP 400).
3. codedeploy-application-revision-registration-and-lookup#task#8 RegisterApplicationRevision — expected 400 with body {'__type': 'DescriptionTooLongException', 'message': 'Revision description has a maximum length of 512'}; got 200 with empty body {}.
4. codedeploy-deployment-inspection-not-found-paths#task#3 ListDeployments — expected 400 with $.__type 'DeploymentGroupNameRequiredException'; got 200.
5. codedeploy-deployment-control-actions-rejected-paths#task#2 StopDeployment — expected $.__type 'InvalidDeploymentIdException'; got 'DeploymentDoesNotExistException' (both 400).
6. codedeploy-deployment-inspection-not-found-paths#task#7 GetDeployment — expected $.__type 'InvalidDeploymentIdException'; got 'DeploymentDoesNotExistException' (both 400).

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): CreateDeploymentGroup declares serviceRoleArn required and models InvalidRoleException (400) and does NOT model ValidationException; RegisterApplicationRevision models DescriptionTooLongException (400); ListDeployments models DeploymentGroupNameRequiredException (400); StopDeployment and GetDeployment both model InvalidDeploymentIdException and DeploymentDoesNotExistException (400). The quoted messages are recorded examples — the model documents no message text for these exceptions (member doc is only "The message that corresponds to the exception thrown by CodeDeploy"); the requirements here are the HTTP status and the error code.

Related open issues that do NOT cover these steps: "CodeDeploy CreateDeploymentGroup and CreateDeployment accept input real AWS refuses, returning 200 and creating the resource (5 causes)" (behavioral; its role cause is a well-formed non-existent ARN, and it carries no wire replay key) and "CodeDeploy deployment lifecycle, response normalization and error classification diverge from AWS (6 causes)".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus) — all six clusters reproduce.
2. Cause 1: CreateApplication(app); CreateDeploymentGroup(applicationName=app, deploymentGroupName=dg, serviceRoleArn='not-a-role-arn') -> expect 400 InvalidRoleException and no group created; observe 200.
3. Cause 2: create an IAM role NOT trusting codedeploy.amazonaws.com; CreateDeploymentGroup(..., serviceRoleArn=<that role>, ec2TagFilters=[{Key:'k',Value:'v',Type:'KEY_AND_VALUE'},{Key:'k2',Value:'v2',Type:'BOGUS_TYPE'}]) -> expect 400 InvalidRoleException; observe 400 ValidationException.
4. Cause 3: RegisterApplicationRevision(applicationName=app, revision=<S3 revision>, description=<513+ chars>) -> expect 400 DescriptionTooLongException; observe 200 with {}.
5. Cause 4: ListDeployments(applicationName=app) with deploymentGroupName omitted -> expect 400 DeploymentGroupNameRequiredException; observe 200.
6. Cause 5: StopDeployment(deploymentId='!!not-a-valid-id!!') -> expect 400 InvalidDeploymentIdException; StopDeployment(deploymentId='d-YYYYYYYYY') -> expect 400 DeploymentDoesNotExistException.
7. Cause 6: GetDeployment(deploymentId='not-a-deployment-id') -> expect 400 InvalidDeploymentIdException.
