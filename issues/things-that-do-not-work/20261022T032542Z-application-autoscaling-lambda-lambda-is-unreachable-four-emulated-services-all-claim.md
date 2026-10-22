Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up
Issue-Key: application-autoscaling__lambda|GetAccountSettings|AmbiguousService
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Lambda is unreachable: four emulated services all claim the 'lambda' signing name, so every Lambda request is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSettings

## Description
Cause 1 of 1: the emulator registers four services for the sigv4 signing name 'lambda', so it refuses every Lambda request instead of serving it.
AWS: Lambda (signing name 'lambda') answers GetAccountSettings and PutProvisionedConcurrencyConfig; `function:<name>:<alias>` with scalable dimension lambda:function:ProvisionedConcurrency is an Application Auto Scaling scalable resource, and registering a MinCapacity pulls the alias's provisioned concurrency up to that floor (recorded behavior).
Emulator: the very first Lambda call is refused after 3 retries with error code `AmbiguousService`: '4 emulated services all claim requests for \'lambda\': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one'. This is the emulator's own routing refusal — it announced four services for one signing name.
Why: no client can call Lambda through the emulator at all, so neither Lambda itself nor the Lambda namespace of Application Auto Scaling (provisioned-concurrency scaling) is usable; the failure surfaces as a non-AWS error code that no SDK retry policy understands.
Fix: the service-registration/routing table that maps a sigv4 signing name to exactly one emulated service — collapse the four 'lambda' claimants to the single Lambda service (the duplicate registrations appear to come from the several lambda-* crates in the workspace).
Traces: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up.

## Evidence
`botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the GetAccountSettings operation (reached max retries: 3): 4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one`
Expected: a normal Lambda GetAccountSettings response (HTTP 200), after which the check creates a function/alias and asserts that RegisterScalableTarget's MinCapacity raises the alias's provisioned concurrency. The same ambiguity pattern also blocks Keyspaces ('cassandra', 2 claimants), filed separately under application-auto-scaling.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Issue any Lambda API call, e.g. GetAccountSettings, against the emulator endpoint — observe the AmbiguousService refusal naming four 'lambda' services instead of a Lambda response.
3. Re-replay application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up once routing resolves to a single Lambda service.
