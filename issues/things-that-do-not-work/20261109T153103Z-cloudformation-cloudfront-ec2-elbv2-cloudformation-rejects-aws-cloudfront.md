Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete
Issue-Key: cloudformation__cloudfront__ec2__elbv2|CreateStack|ValidationError
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudformation__cloudfront__ec2__elbv2: CloudFormation rejects AWS::CloudFront::VpcOrigin as an unrecognized resource type

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (Cause 1 of 1). No open issue carries this replay key; 20261104T051153Z-cloudfront-elbv2-ec2-describemanagedprefixlists-reports-no-aws-managed.md is the nearest filing and concerns EC2 prefix lists, not the CloudFormation resource type.

Cause 1 of 1: CreateStack refuses a template containing AWS::CloudFront::VpcOrigin with ValidationError "Template format error: Unrecognized resource types", so the resource type is not registered with the emulator's CloudFormation.
AWS: Smithy (cloudformation) CreateStack declares no error for a supported resource type; AWS reserves ValidationError "Template format error: Unrecognized resource types: [<Type>]" (HTTP 400) for types CloudFormation does not know. AWS::CloudFront::VpcOrigin is a supported CloudFormation resource type, so AWS accepts the template, returns a StackId, and creates/deletes a CloudFront VPC origin bound to the referenced ALB ARN over the stack's lifecycle. The fixed text of the message is AWS's; the bracketed type list is taken from the request.
Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::CloudFront::VpcOrigin]" and nothing is provisioned.
Why: a client cannot model a CloudFront VPC origin in a stack at all, and the refusal it gets is the one AWS uses for a malformed template — so a valid template is reported as the client's mistake, with no indication that the type is simply unregistered.
Fix: the CloudFormation resource-type registry and its CloudFront resource providers must register AWS::CloudFront::VpcOrigin and create/update/delete it against the CloudFront service's state (including the round-trip read of its ARN and the delete path).
Traces: cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudformation__cloudfront__ec2__elbv2 0/1 (0%), 1 failed, not OWNED.

[error] cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudFront::VpcOrigin]

Expected: CreateStack returns a StackId, the stack reaches CREATE_COMPLETE with a CloudFront VPC origin bound to the ALB, and DELETE removes it. Actual: HTTP 400 ValidationError for an unrecognized resource type.

Smithy consulted: cloudformation CreateStack (input TemplateBody, output StackId).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the id in trace_ids reproduces.
2. Create a VPC with subnets and an internal ALB.
3. cloudformation.create_stack with a template declaring AWS::CloudFront::VpcOrigin whose VpcOriginEndpointConfig points at that ALB's ARN. Expect a StackId and CREATE_COMPLETE; the emulator answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::CloudFront::VpcOrigin]".
4. Re-replay the id in trace_ids to confirm the fix.
