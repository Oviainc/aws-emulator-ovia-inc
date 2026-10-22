Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.deploy-application-behaves-as-deployment-target codedeploy.deploy-codedeploy-application-behaves codedeploy.deploy-codedeploy-application-cfn-behaves codedeploy.deploy-deploymentgroup-accepts-deployment
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|GetApplication|ApplicationDoesNotExistException
Issue-Key: codedeploy|CreateDeployment|ApplicationDoesNotExistException
Issue-Key: codedeploy|GetDeploymentGroup|DeploymentGroupDoesNotExistException
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CloudFormation stacks declaring AWS::CodeDeploy::Application / ::DeploymentGroup reach CREATE_COMPLETE without creating anything in CodeDeploy

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
Cause 1 of 1: a CloudFormation stack whose template declares AWS::CodeDeploy::Application and/or AWS::CodeDeploy::DeploymentGroup is driven to CREATE_COMPLETE (the stack_create_complete waiter returns, and stack Outputs even echo the expected logical values), but no corresponding resource exists in the CodeDeploy service state. Every subsequent CodeDeploy data-plane call against the declared name is refused as nonexistent.

AWS: Smithy GetApplication/GetDeploymentGroup/CreateDeployment model ApplicationDoesNotExistException and DeploymentGroupDoesNotExistException (HTTP 400) for names that do not exist — and a CFN-provisioned CodeDeploy application or deployment group DOES exist: recorded behavior only for the cross-service part, measured by the four checks, which assert that a CFN-created application is a fully live data-plane resource (GetApplication and ListApplications reflect it, a deployment group can be created underneath it, and a CreateDeployment on a CFN-created group runs to a terminal GetDeployment status).
Emulator: after CREATE_COMPLETE, GetApplication answers 400 ApplicationDoesNotExistException, GetDeploymentGroup answers 400 DeploymentGroupDoesNotExistException, and CreateDeployment answers 400 ApplicationDoesNotExistException for the stack's own ApplicationName.
Why: CREATE_COMPLETE is a lie — a client that provisions CodeDeploy through CloudFormation (the normal path) believes its application and deployment group exist and every following call fails as not-found. Infrastructure-as-code flows cannot be emulated at all, and the failure appears to come from CodeDeploy rather than from the provisioning layer that never created the resource.
Fix: the CloudFormation resource-provisioning layer for the AWS::CodeDeploy::* resource types — the stack must materialise the resources in the CodeDeploy service state (and tear them down on stack delete) rather than only recording the logical resource. The CodeDeploy handlers themselves are answering correctly for state that was never written.
Traces: codedeploy.deploy-application-behaves-as-deployment-target, codedeploy.deploy-codedeploy-application-behaves, codedeploy.deploy-codedeploy-application-cfn-behaves, codedeploy.deploy-deploymentgroup-accepts-deployment. Related already-filed issues describe the same class of gap for other resource types (e.g. "CloudFormation stack Outputs return synthesized logical names instead of the real physical resource ids of the EC2/EMR resources the stack declares" and "ECR Public: CFN PublicRepository creates nothing..."); none of them covers AWS::CodeDeploy::Application or AWS::CodeDeploy::DeploymentGroup.

## Evidence
Replayed behavioral suite, codedeploy service. All four checks create a stack, wait for stack_create_complete (which succeeds), then fail on the first CodeDeploy read of the declared resource:

- codedeploy.deploy-application-behaves-as-deployment-target: botocore.errorfactory.ApplicationDoesNotExistException: An error occurred (ApplicationDoesNotExistException) when calling the GetApplication operation: No application found for name sim2real-cfnapp-12a210f0eb49 — template declares `App: {Type: AWS::CodeDeploy::Application, Properties: {ApplicationName: !Ref AppName, ComputePlatform: Server}}`.
- codedeploy.deploy-codedeploy-application-behaves: ApplicationDoesNotExistException when calling CreateDeployment: No application found for name sim2real-cdapp-7939a56266ca — template declares AWS::IAM::Role + AWS::CodeDeploy::Application + AWS::CodeDeploy::DeploymentGroup; the stack's Outputs for ApplicationName/DeploymentGroupName were accepted before the call.
- codedeploy.deploy-codedeploy-application-cfn-behaves: ApplicationDoesNotExistException when calling GetApplication: No application found for name sim2real-cd-appstack-...-CDApplication-9B39E9D30B0B.
- codedeploy.deploy-deploymentgroup-accepts-deployment: DeploymentGroupDoesNotExistException when calling GetDeploymentGroup: No deployment group found for name sim2real-cddg-febbccaed56b in application sim2real-cdapp-f2a5b80bf330.

Expected: the stack's CodeDeploy resources exist — GetApplication returns applicationName/computePlatform, ListApplications contains the name, GetDeploymentGroup returns the group, and CreateDeployment is accepted and reaches a terminal status.
Actual: HTTP 400 ApplicationDoesNotExistException / DeploymentGroupDoesNotExistException. Stack and resource names are environment-specific examples.

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation.CreateStack with a template declaring:
   Resources: {App: {Type: AWS::CodeDeploy::Application, Properties: {ApplicationName: <name>, ComputePlatform: Server}}}
3. Wait on the stack_create_complete waiter — it succeeds.
4. codedeploy.GetApplication(applicationName=<name>) -> expect the application; observe 400 ApplicationDoesNotExistException.
5. Repeat with a template that also declares AWS::CodeDeploy::DeploymentGroup (ApplicationName: {Ref: App}, ServiceRoleArn from an AWS::IAM::Role) and call codedeploy.GetDeploymentGroup and codedeploy.CreateDeployment on the declared names -> expect both to work; observe DeploymentGroupDoesNotExistException / ApplicationDoesNotExistException.
