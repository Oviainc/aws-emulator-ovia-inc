Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sagemaker.deploy-sagemaker-domain-auth-mode-invalid-value-rejected cloudformation__sagemaker.deploy-sagemaker-domain-userprofile-space-round-trip-then-delete
Issue-Key: cloudformation__sagemaker.deploy-sagemaker-domain-auth-mode-invalid-value-rejected|run#10|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"rejected stack ended {bad_status!r}"
Issue-Key: cloudformation__ec2__iam__sagemaker|CreateStack|ValidationError
Issue-Commit: e901ea078181297e70b2ac3a5c8be54768ac76d3

# cloudformation (SageMaker Domain/Space stacks): 2 net-new causes — AWS::SageMaker::Space is an unrecognized resource type, and a Domain with an invalid AuthMode reaches CREATE_COMPLETE instead of rolling back

> _Note: this issue was identified against the code base at commit `e901ea078181297e70b2ac3a5c8be54768ac76d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new behavioral root causes under this service heading. Neither failing check is marked ALREADY FILED or OWNED by the replay.

Cause 1 of 2: AWS::SageMaker::Space is absent from the emulator's CloudFormation resource-type registry.
AWS: recorded behavior only — resource-type support is a CloudFormation registry fact. AWS::SageMaker::Space fronts the SageMaker control plane's CreateSpace/DescribeSpace/DeleteSpace (Smithy: services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json). In AWS, a template declaring a Domain, a UserProfile and a Space returns 200 with a StackId from CreateStack, the stack reaches CREATE_COMPLETE, each logical resource carries the SageMaker identifier as its PhysicalResourceId, and DeleteStack removes all three.
Emulator: CreateStack is refused with HTTP 400 ValidationError and message "Template format error: Unrecognized resource types: [AWS::SageMaker::Space]" — the fixed text is "Template format error: Unrecognized resource types: [" ... "]" and the bracketed list is taken from the submitted template. No stack is created, so the Domain/UserProfile/Space round trip and the teardown are never exercised.
Why: a client that provisions Studio spaces by CloudFormation (the normal path) is told its own template is malformed and has no parameter change that can succeed.
Fix: the CloudFormation resource-type registry and an AWS::SageMaker::Space provisioner (create -> CreateSpace, read -> DescribeSpace, delete -> DeleteSpace) wired into the stack resource lifecycle.
Traces: cloudformation__sagemaker.deploy-sagemaker-domain-userprofile-space-round-trip-then-delete.

Cause 2 of 2: a stack whose AWS::SageMaker::Domain declares an AuthMode outside the modeled enum is accepted and reported CREATE_COMPLETE.
AWS: Smithy: CreateDomain input member AuthMode targets the AuthMode enum, whose only values are "SSO" and "IAM"; a value outside it is refused by the control plane. Through CloudFormation that refusal surfaces as the Domain resource going CREATE_FAILED with SageMaker's validation message as its ResourceStatusReason, the stack then rolling back and DescribeStacks reporting StackStatus "ROLLBACK_COMPLETE", and no domain of that name existing in SageMaker afterwards (the check's positive control is the same template with AuthMode=IAM, which does deploy).
Emulator: the stack is created and DescribeStacks reports StackStatus "CREATE_COMPLETE" for the invalid-AuthMode template — the out-of-enum AuthMode is neither rejected by the Domain provisioner nor by CreateDomain behind it, so an invalid domain is left provisioned and no rollback occurs.
Why: a client validating its templates against the emulator gets a green CREATE_COMPLETE for a configuration real SageMaker refuses, so the defect is discovered only in production; and a client that relies on CloudFormation's all-or-nothing rollback is left with a half-valid domain plus whatever the stack created around it, with no CREATE_FAILED event or ResourceStatusReason to explain it.
Fix: enum validation of AuthMode in the CreateDomain handler's input validation (so any caller, not just CloudFormation, is refused), and the CloudFormation stack resource lifecycle that must turn that refusal into a CREATE_FAILED resource event carrying the reason and drive the stack to ROLLBACK_COMPLETE.
Traces: cloudformation__sagemaker.deploy-sagemaker-domain-auth-mode-invalid-value-rejected.

## Evidence
Cause 1 — cloudformation__sagemaker.deploy-sagemaker-domain-userprofile-space-round-trip-then-delete [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SageMaker::Space]
  Expected: CreateStack 200 with a StackId and DescribeStacks StackStatus CREATE_COMPLETE, then DeleteStack to DELETE_COMPLETE. Actual: HTTP 400 ValidationError, no stack.

Cause 2 — cloudformation__sagemaker.deploy-sagemaker-domain-auth-mode-invalid-value-rejected [mismatch]:
  assert bad_status == "ROLLBACK_COMPLETE", f"rejected stack ended {bad_status!r}"
  AssertionError: rejected stack ended 'CREATE_COMPLETE'
  i.e. DescribeStacks(StackName=<the invalid-AuthMode stack>)["Stacks"][0]["StackStatus"] is "CREATE_COMPLETE" where AWS reports "ROLLBACK_COMPLETE". The stack name, VPC/subnet ids and role ARN in the recording are per-run values; the requirement is that an AuthMode outside {SSO, IAM} is refused and the stack rolls back.

## Steps to Reproduce
1. Build and boot the emulator, run the behavioral suite and re-run the two checks in trace_ids.
2. Cause 1: CreateStack with a template declaring AWS::SageMaker::Domain + AWS::SageMaker::UserProfile + AWS::SageMaker::Space (Space referencing the domain id). AWS returns a StackId and the stack reaches CREATE_COMPLETE; the emulator returns 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SageMaker::Space]".
3. Cause 2: CreateStack with a template whose AWS::SageMaker::Domain Properties set AuthMode to a value outside {SSO, IAM} (plus DomainName, VpcId, SubnetIds and a DefaultUserSettings.ExecutionRole). Poll DescribeStacks to a terminal status: AWS reports ROLLBACK_COMPLETE with the Domain resource CREATE_FAILED and SageMaker's AuthMode validation message as its ResourceStatusReason, and ListDomains/DescribeDomain show no such domain; the emulator reports CREATE_COMPLETE. Then call CreateDomain directly with the same out-of-enum AuthMode: AWS refuses it, the emulator accepts it.
4. Positive control for cause 2: the same template with AuthMode=IAM must still deploy to CREATE_COMPLETE.
