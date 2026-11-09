Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__greengrassv2__iot.deploy-greengrassv2-component-and-deployment-round-trip-and-delete cloudformation__greengrassv2__iot.deploy-greengrassv2-deployment-nonexistent-target-rejected-and-rolled-back
Issue-Key: cloudformation__greengrassv2__iot|CreateStack|ValidationError
Issue-Commit: ec57509239c646de823e7950d2544f6a600b68b2

# cloudformation__greengrassv2__iot: CreateStack refuses AWS::GreengrassV2::ComponentVersion and ::Deployment as unrecognized resource types, so no Greengrass V2 estate can be deployed by template

> _Note: this issue was identified against the code base at commit `ec57509239c646de823e7950d2544f6a600b68b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::GreengrassV2::* resource types are not registered with CloudFormation.
AWS: CloudFormation registers AWS::GreengrassV2::ComponentVersion and AWS::GreengrassV2::Deployment; a template declaring them is accepted, the stack creates the component version and the deployment, and a deployment naming a target that does not exist fails the resource and rolls the stack back. Recorded behavior only for the registration itself (AWS's aws-resource-greengrassv2-* template reference is the authority); the underlying greengrassv2 operations (CreateComponentVersion, CreateDeployment) are already modeled in this emulator.
Emulator: CreateStack is refused with HTTP 400, error code ValidationError and message 'Template format error: Unrecognized resource types: [<declared type list>]' — fixed text 'Template format error: Unrecognized resource types: [' + ']' around the bracketed list, the list being the types read from the request template. No stack is created, so neither the happy-path round trip nor the nonexistent-target rollback can be exercised.
Why: a client cannot provision Greengrass V2 components or deployments as infrastructure-as-code at all; both the positive deploy/read-back/delete path and the template-driven rejection path are unreachable, and CDK/CFN estates that include a core-device deployment abort at stack creation.
Fix: the CloudFormation resource-type registry plus AWS::GreengrassV2::ComponentVersion and AWS::GreengrassV2::Deployment resource handlers (create/read/delete delegating to the greengrassv2 control plane, including the target-not-found resource failure).
Traces: cloudformation__greengrassv2__iot.deploy-greengrassv2-component-and-deployment-round-trip-and-delete, cloudformation__greengrassv2__iot.deploy-greengrassv2-deployment-nonexistent-target-rejected-and-rolled-back.

## Evidence
Expected: CreateStack accepts the template; the stack reaches CREATE_COMPLETE (component version + deployment readable through greengrassv2), and the variant whose Deployment names a nonexistent target ends ROLLBACK_COMPLETE.
Actual (both checks, at CreateStack): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::GreengrassV2::ComponentVersion, AWS::GreengrassV2::Deployment] — and, for the negative variant, ... Unrecognized resource types: [AWS::GreengrassV2::Deployment].

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Call cloudformation CreateStack with a template declaring an AWS::GreengrassV2::ComponentVersion and an AWS::GreengrassV2::Deployment.
3. Observe the 400 ValidationError 'Template format error: Unrecognized resource types: [...]' instead of an accepted stack.
