Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__securityagent.deploy-securityagent-agent-space-and-artifact-lifecycle cloudformation__securityagent.deploy-securityagent-artifact-invalid-type-rejected cloudformation__securityagent.deploy-securityagent-application-lifecycle cloudformation__securityagent.deploy-securityagent-private-connection-lifecycle cloudformation__securityagent.deploy-securityagent-security-requirement-pack-lifecycle cloudformation__securityagent.deploy-securityagent-security-requirement-pack-invalid-status-rejected
Issue-Key: cloudformation__securityagent|CreateStack|ValidationError
Issue-Key: cloudformation__securityagent|UpdateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__securityagent: CloudFormation does not register any AWS::SecurityAgent::* resource type

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::SecurityAgent::AgentSpace, Artifact, Application, PrivateConnection and SecurityRequirementPack are not recognized resource types.
AWS: all five are public CloudFormation resource types, so CreateStack and UpdateStack accept templates declaring them and provision them; an Artifact with an invalid type and a SecurityRequirementPack with an invalid status are then refused by the service and roll the stack back.
Emulator: CreateStack (and UpdateStack) answer HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared AWS::SecurityAgent::* types>]`. The fixed text is "Template format error: Unrecognized resource types: "; the list comes from the request.
Why: the whole Security Agent surface is unreachable from CloudFormation, so neither the lifecycles nor the two modelled refusals can be exercised.
Fix: register the AWS::SecurityAgent::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the six failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SecurityAgent::AgentSpace, AWS::SecurityAgent::Artifact]; ... [AWS::SecurityAgent::Application]; ... [AWS::SecurityAgent::SecurityRequirementPack]; and when calling UpdateStack: ... [AWS::SecurityAgent::PrivateConnection]. Expected the call accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring any AWS::SecurityAgent::* type.
3. Observe the ValidationError.
