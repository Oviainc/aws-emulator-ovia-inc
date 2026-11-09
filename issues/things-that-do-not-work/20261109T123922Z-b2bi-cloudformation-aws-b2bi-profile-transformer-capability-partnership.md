Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi__cloudformation.deploy-b2bi-capability-nonexistent-transformer-rejected-and-rolled-back b2bi__cloudformation.deploy-b2bi-profile-transformer-capability-partnership-round-trip-and-delete
Issue-Key: b2bi__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# b2bi__cloudformation: AWS::B2BI::{Profile,Transformer,Capability,Partnership} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::B2BI::Profile, ::Transformer, ::Capability and ::Partnership are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts all four types, provisions the B2B Data Interchange profile, transformer, capability and partnership, and reaches CREATE_COMPLETE; a capability naming a transformer that does not exist instead rolls the stack back with B2BI's own not-found refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::B2BI::Transformer, AWS::B2BI::Capability]` / `[AWS::B2BI::Profile, AWS::B2BI::Transformer, AWS::B2BI::Capability, AWS::B2BI::Partnership]`, so nothing is created and the unknown-transformer refusal is never reached.
Why: no B2BI estate can be deployed from a template; the user is told these AWS resource types do not exist.
Fix: register the four types in the CloudFormation template resource-type registry and add a resource handler per type over the b2bi crate, including the transformer-reference validation on capability create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing B2BI's refusal for the nonexistent-transformer leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::B2BI::Transformer, AWS::B2BI::Capability] / [AWS::B2BI::Profile, AWS::B2BI::Transformer, AWS::B2BI::Capability, AWS::B2BI::Partnership]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::B2BI::Profile, ::Transformer, ::Capability or ::Partnership.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 2 checks in trace_ids.
