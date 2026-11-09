Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam.cdk-three-region-ec2-estate-deploys-serves-and-destroys cloudformation__ec2__iam.deploy-ec2-spot-fleet-wrong-trust-policy-rejected-and-valid-role-succeeds
Issue-Key: cloudformation__ec2__iam.cdk-three-region-ec2-estate-deploys-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Key: cloudformation__ec2__iam.deploy-ec2-spot-fleet-wrong-trust-policy-rejected-and-valid-role-succeeds|run#14|AssertionError|assert _spot_fleet_ids_with_role(ec2, good_role_arn) == [fleet_id], "control fleet listing"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# cloudformation__ec2__iam: 2 net-new causes — AWS::EC2::Instance refuses its LaunchTemplate property, and a requested spot fleet is not listed with its IamFleetRole

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes, both reached through CloudFormation-driven EC2 provisioning.

Cause 1 of 2: the AWS::EC2::Instance resource handler rejects the LaunchTemplate property outright.
AWS: AWS::EC2::Instance accepts a LaunchTemplate property ({LaunchTemplateId|LaunchTemplateName, Version}) and launches the instance from that template, merging template settings with the resource's own properties; the resource reaches CREATE_COMPLETE. Smithy: ec2#RunInstances carries LaunchTemplateSpecification (LaunchTemplateId/LaunchTemplateName + Version) as a first-class input member, so the launch path exists on the wire — the template-to-API mapping is a CloudFormation fact, recorded behavior only, and the recording shows the stack deploying.
Emulator: the resource fails with HandlerErrorCode InvalidRequest and the handler message `this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance`, so the stack fails to deploy (`cdk deploy --all` exits 1 with DeploymentError on AWS::EC2::Instance). That message is emulator-authored text that no AWS response ever contains.
Why: any stack or CDK app that standardises instance configuration in a launch template cannot deploy, and the client is told the emulator's limitation instead of being given an AWS error code it can handle.
Fix: the AWS::EC2::Instance provisioner must map LaunchTemplate to RunInstances' LaunchTemplate specification instead of refusing it.
Traces: cloudformation__ec2__iam.cdk-three-region-ec2-estate-deploys-serves-and-destroys.

Cause 2 of 2: a successfully requested spot fleet is not reported against its IAM fleet role.
AWS: after RequestSpotFleet succeeds with a valid IamFleetRole, DescribeSpotFleetRequests returns that fleet with SpotFleetRequestConfig.IamFleetRole equal to the role ARN passed in, so a caller can enumerate the fleets a given fleet role owns. Smithy: ec2#SpotFleetRequestConfigData.IamFleetRole is a required member of the request and is echoed in DescribeSpotFleetRequests' SpotFleetRequestConfigs[].SpotFleetRequestConfig — recorded behavior only for the enumeration itself.
Emulator: the control fleet created with the valid trust policy is not among the fleets DescribeSpotFleetRequests reports for that role, so the positive leg of the check finds no fleet.
Why: a client that reconciles fleets by fleet role (cleanup, cost attribution, drift detection) sees none of its fleets and may re-create or orphan them.
Fix: the spot-fleet state and the DescribeSpotFleetRequests serializer must persist and echo IamFleetRole for every accepted RequestSpotFleet.
Traces: cloudformation__ec2__iam.deploy-ec2-spot-fleet-wrong-trust-policy-rejected-and-valid-role-succeeds.

## Evidence
Cause 1 — Expected: stack with an AWS::EC2::Instance carrying a LaunchTemplate property deploys (CREATE_COMPLETE).
Actual: `cdk deploy --all` exited 1; Resource handler returned message: "this emulator does not apply the LaunchTemplate property of AWS::EC2::Instance" (AWS::EC2::Instance, HandlerErrorCode: InvalidRequest).

Cause 2 — Expected: DescribeSpotFleetRequests for the valid IamFleetRole lists exactly the created fleet id.
Actual: AssertionError: control fleet listing (the role-scoped listing was empty).

## Steps to Reproduce
Cause 1: CreateStack (or `cdk deploy`) with an AWS::EC2::Instance whose Properties include LaunchTemplate: {LaunchTemplateId, Version}; observe the resource CREATE_FAILED with the emulator's LaunchTemplate message.
Cause 2: create a role with the spot-fleet trust policy, call RequestSpotFleet with it, then DescribeSpotFleetRequests and filter on SpotFleetRequestConfig.IamFleetRole; observe the fleet is absent.
