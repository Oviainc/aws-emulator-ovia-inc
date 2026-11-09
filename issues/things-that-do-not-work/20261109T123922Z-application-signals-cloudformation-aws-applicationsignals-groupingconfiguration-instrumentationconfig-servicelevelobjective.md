Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-signals__cloudformation.deploy-application-signals-grouping-configuration-singleton-round-trip-and-delete application-signals__cloudformation.deploy-application-signals-instrumentation-config-and-slo-round-trip-and-delete application-signals__cloudformation.deploy-application-signals-instrumentation-config-missing-code-unit-rejected
Issue-Key: application-signals__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# application-signals__cloudformation: AWS::ApplicationSignals::{GroupingConfiguration,InstrumentationConfig,ServiceLevelObjective} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ApplicationSignals::GroupingConfiguration, ::InstrumentationConfig and ::ServiceLevelObjective are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts all three types, provisions them through Application Signals, and reaches CREATE_COMPLETE (GroupingConfiguration is a singleton, so the stack round-trip also covers its replace-on-update behaviour); an InstrumentationConfig missing its code-unit property instead rolls the stack back with Application Signals' own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::ApplicationSignals::GroupingConfiguration]` / `[AWS::ApplicationSignals::InstrumentationConfig, AWS::ApplicationSignals::ServiceLevelObjective]` / `[AWS::ApplicationSignals::InstrumentationConfig]`, so nothing is created and the missing-code-unit refusal is never reached.
Why: no Application Signals grouping configuration, instrumentation config or SLO can be deployed from a template; the user is told these AWS resource types do not exist.
Fix: register the three types in the CloudFormation template resource-type registry and add a resource handler per type over the application-signals crate, including the singleton semantics of GroupingConfiguration and the required-property validation on create.
Traces: the 3 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the refusal for the missing-code-unit leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ApplicationSignals::GroupingConfiguration] / [AWS::ApplicationSignals::InstrumentationConfig, AWS::ApplicationSignals::ServiceLevelObjective] / [AWS::ApplicationSignals::InstrumentationConfig]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ApplicationSignals::GroupingConfiguration, ::InstrumentationConfig or ::ServiceLevelObjective.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 3 checks in trace_ids.
