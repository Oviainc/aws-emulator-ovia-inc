Issue-Source: trace-sweep-divergence
Issue-Trace: evs-create-environment-input-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-create-environment-input-validation
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|CreateEnvironment|4xx->2xx|$.__type
Issue-Key: evs|CreateEnvironment|4xx->2xx|$.fieldList
Issue-Key: evs|CreateEnvironment|4xx->2xx|$.message
Issue-Key: evs|CreateEnvironment|4xx->2xx|status
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# CreateEnvironment accepts an invalid request with HTTP 200

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEnvironment

## Description
CreateEnvironment performs no request-level precondition validation: a request that AWS rejects wholesale is accepted and an environment is created.

Cause 1 of 1: CreateEnvironment does not reject a request that fails its creation preconditions.

AWS: CreateEnvironment models exactly one error, ValidationException (Smithy: CreateEnvironment errors = [ValidationException]; ValidationException @error(client), members message (required), reason (required), fieldList -> ValidationExceptionFieldList). The recorded response for a request carrying termsAccepted=false, an account without a Premium Support subscription and a non-existent vpcId is HTTP 400 with a ValidationException body that AGGREGATES every failed precondition into fieldList (recorded example entries: 'You must be subscribed to AWS Premium Support', 'Terms and Conditions must be accepted before creating an environment', 'The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.') and a top-level message that joins those same entries with ' | ' (recorded example: 'Request validation failure: <entry> | <entry> | <entry>'). The recorded __type is the service's internal namespace form of the error (recorded example value 'com.amazonaws.promenad.service#ValidationException'); the requirement is the error SHAPE ValidationException, not that exact namespace string. Semantics of the checks themselves: termsAccepted must be true and the referenced vpcId must exist for the call to succeed. Hypothesis: the Premium Support entry is an account-level subscription precondition the recording's account did not satisfy.

Emulator: returns HTTP 200 with a success payload. No __type, no message and no fieldList are emitted at all, so the request is treated as fully valid and an environment is created.

Why: a client that submits an unaccepted-terms or bad-VPC CreateEnvironment gets a success and a live environment handle instead of the 400 ValidationException it must handle. Error-path code (surfacing fieldList to the user, prompting for terms acceptance, correcting the VPC id) is never exercised, and the emulator's state now contains an environment AWS would never have created, so every later step in a negative-path test diverges too.

Fix: CreateEnvironment's handler/validation in the evs service crate — add the request-level precondition checks (termsAccepted, vpcId existence, account subscription) that run after shape validation, accumulate ALL failures, and raise ValidationException (HTTP 400) with one fieldList entry per failure and a top-level message joining those entries, rather than falling through to environment creation.

Traces: evs-create-environment-input-validation#task#1 (expected 400 ValidationException with fieldList, got 200).

## Evidence
Fixture evs-create-environment-input-validation, step task#1.
Expected: status 400; $.__type = ValidationException (recorded 'com.amazonaws.promenad.service#ValidationException'); $.fieldList = [{message: 'You must be subscribed to AWS Premium Support'}, {message: 'Terms and Conditions must be accepted before creating an environment'}, {message: 'The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.'}]; $.message = 'Request validation failure: You must be subscribed to AWS Premium Support | Terms and Conditions must be accepted before creating an environment | The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.'
Actual: status 200; $.__type, $.fieldList and $.message all absent.

## Steps to Reproduce
1. Boot the emulator and replay fixture evs-create-environment-input-validation.
2. Step task#1 POSTs an awsJson1_0 CreateEnvironment (X-Amz-Target AmazonElasticVMwareService.CreateEnvironment) whose body sets termsAccepted=false and vpcId=vpc-01234567 (a VPC that does not exist).
3. Observe HTTP 200 and a success body; AWS returns HTTP 400 with a ValidationException whose fieldList names each failed precondition.
