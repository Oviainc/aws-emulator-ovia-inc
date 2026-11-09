Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ivschat.deploy-ivschat-logging-configuration-and-room-round-trip-and-delete cloudformation__ivschat.cfn-ivschat-room-maximummessagelength-above-schema-max-rejected
Issue-Key: cloudformation__ivschat|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__ivschat: CloudFormation does not register AWS::IVSChat::Room or AWS::IVSChat::LoggingConfiguration

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::IVSChat::Room and AWS::IVSChat::LoggingConfiguration are not recognized resource types.
AWS: both are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the room and its logging configuration; a MaximumMessageLength above the schema maximum then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared types from the request>]`.
Why: no IVS Chat estate can be deployed through CloudFormation, and the property-maximum refusal can never be observed.
Fix: register AWS::IVSChat::Room and AWS::IVSChat::LoggingConfiguration in the CloudFormation resource-type registry and add their resource handlers.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IVSChat::LoggingConfiguration, AWS::IVSChat::Room]; and ... [AWS::IVSChat::Room] for the MaximumMessageLength check. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::IVSChat::Room (and AWS::IVSChat::LoggingConfiguration).
3. Observe the ValidationError.
