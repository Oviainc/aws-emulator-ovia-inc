Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lightsail.deploy-lightsail-keypair-round-trip-and-delete cloudformation__lightsail.deploy-lightsail-keypair-invalid-name-rejected-and-rolled-back cloudformation__lightsail.deploy-lightsail-static-ip-round-trip-and-delete
Issue-Key: cloudformation__lightsail|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__lightsail: CloudFormation does not register AWS::Lightsail::KeyPair or AWS::Lightsail::StaticIp

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Lightsail::KeyPair and AWS::Lightsail::StaticIp are not recognized resource types.
AWS: both are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the key pair / static IP; a key-pair name that breaks the name constraint then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::Lightsail::KeyPair]` / `[AWS::Lightsail::StaticIp]` — fixed text plus the bracketed type list from the request.
Why: no Lightsail key pair or static IP can be deployed through CloudFormation, and the name-constraint refusal can never be observed.
Fix: register AWS::Lightsail::KeyPair and AWS::Lightsail::StaticIp in the CloudFormation resource-type registry and add their resource handlers.
Traces: the three failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Lightsail::KeyPair] (twice) and [AWS::Lightsail::StaticIp]. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::Lightsail::KeyPair, then one declaring AWS::Lightsail::StaticIp.
3. Observe the ValidationError.
