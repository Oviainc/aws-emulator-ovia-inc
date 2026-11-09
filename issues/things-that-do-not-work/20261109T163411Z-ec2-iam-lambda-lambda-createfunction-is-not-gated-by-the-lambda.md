Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__lambda.create-function-requires-matching-security-group-condition
Issue-Key: ec2__iam__lambda.create-function-requires-matching-security-group-condition|_probe_without_match#45|AssertionError|raise AssertionError(
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# Lambda CreateFunction is not gated by the lambda:VpcIds/SecurityGroupIds request conditions: a function naming a non-allowed security group is created

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction's authorization does not publish the VPC-configuration request context keys, so a policy that allows the call only for a named security group does not constrain it.

AWS: CreateFunction with a VpcConfig is authorized with the request's VPC context keys in scope, so an identity policy whose Allow is conditioned on the permitted security group (and denies otherwise) refuses a CreateFunction whose VpcConfig.SecurityGroupIds names a different group: the call fails with `AccessDeniedException` and AWS's standard no-matching-condition denial — `User: <principal arn> is not authorized to perform: lambda:CreateFunction on resource: <function arn> because no identity-based policy allows the lambda:CreateFunction action` wording, with principal, action and resource taken from the request. Smithy: lambda#CreateFunction.VpcConfig carries SubnetIds and SecurityGroupIds and the operation declares AccessDeniedException-class refusals through the service's common errors; the condition-key set is a service fact — recorded behavior only, and the recording shows the non-matching call refused and the matching one allowed.

Emulator: the CreateFunction naming the non-allowed security group SUCCEEDS.

Why: a client that confines functions to approved security groups (a standard network-isolation control) has no control at all: the guardrail it tested as effective is inert, and functions can be attached to any group. Because the allowed leg also succeeds, the client cannot tell an enforced policy from an unenforced one.

Fix: the CreateFunction authorization step must publish the request's VpcConfig-derived condition keys into the authorization context so conditioned Allow/Deny statements are evaluated.

Traces: ec2__iam__lambda.create-function-requires-matching-security-group-condition.

## Evidence
Expected: CreateFunction whose VpcConfig names the non-allowed security group is refused (AccessDeniedException).
Actual: AssertionError: CreateFunction naming the non-allowed security group was expected to be refused but it SUCCEEDED

## Steps to Reproduce
1. Create a VPC with two security groups and a role whose policy allows lambda:CreateFunction only when the request's security group is the approved one.
2. As that principal, call CreateFunction with VpcConfig.SecurityGroupIds = [the other group].
3. Observe the function is created.
