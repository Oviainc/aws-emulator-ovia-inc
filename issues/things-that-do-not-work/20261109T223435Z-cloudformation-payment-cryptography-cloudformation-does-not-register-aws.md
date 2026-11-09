Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__payment-cryptography.deploy-payment-cryptography-alias-round-trip-and-delete cloudformation__payment-cryptography.deploy-payment-cryptography-alias-nonexistent-keyarn-rejected-and-rolled-back
Issue-Key: cloudformation__payment-cryptography|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__payment-cryptography: CloudFormation does not register AWS::PaymentCryptography::Key or AWS::PaymentCryptography::Alias

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::PaymentCryptography::Key and AWS::PaymentCryptography::Alias are not recognized resource types.
AWS: both are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the key and its alias; an alias pointing at a KeyArn that does not exist then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared types from the request>]`.
Why: Payment Cryptography keys and aliases cannot be deployed through CloudFormation at all, and the dangling-KeyArn refusal can never be observed.
Fix: register AWS::PaymentCryptography::Key and AWS::PaymentCryptography::Alias in the CloudFormation resource-type registry and add their resource handlers.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::PaymentCryptography::Key, AWS::PaymentCryptography::Alias]; and ... [AWS::PaymentCryptography::Alias]. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::PaymentCryptography::Key and AWS::PaymentCryptography::Alias.
3. Observe the ValidationError.
