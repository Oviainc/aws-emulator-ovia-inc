Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-error-paths-experiment-definitions
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-error-paths-experiment-definitions appconfig-experiment-run-conflict-and-notfound-errors
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateExperimentDefinition|4xx->4xx|$.Message
Issue-Key: appconfig|CreateExperimentDefinition|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig CreateExperimentDefinition refuses an invalid FlagKey as an unmodeled ValidationException with no message

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperimentDefinition

## Description
=== Cause 1 of 1: constraint violations are refused with an error class this service does not model, and the body carries no Message ===
AWS: a FlagKey that violates its pattern is refused with BadRequestException, HTTP 400, x-amzn-errortype 'BadRequestException:' and a request-derived validation message naming the member and the constraint — recorded: "1 validation error detected: Value '1invalidFlagKey' at 'flagKey' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z][a-zA-Z0-9_-]{1,64}" (Smithy: CreateExperimentDefinition.FlagKey required, target FlagKey @pattern ^[a-z][a-zA-Z0-9_-]{1,64}$; the operation's modeled errors are BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402 — the appconfig model declares no ValidationException at all. BadRequestException members: Message, Reason, Details).
Emulator: the status is right (400) but x-amzn-errortype is 'ValidationException', an error class this service does not model, and $.Message is absent from the body entirely — so the refusal carries neither the modeled code nor the rule-determined text.
Why: a client that catches or switches on the modeled BadRequestException does not catch ValidationException, so an input AWS refuses with a classifiable 400 becomes an unrecognized error class; with no Message the caller also cannot tell which member failed, even though the message here is rule-determined (member name + the pattern from the model).
Fix: map pre-handler constraint failures in the appconfig service to the modeled BadRequestException with AWS's "N validation error detected: Value <v> at '<member>' failed to satisfy constraint: ..." text — in the service's validation/error-mapping layer, not in the CreateExperimentDefinition handler alone, and make sure the serialized body includes Message.
Traces: appconfig-error-paths-experiment-definitions#task#6 (cluster also covers appconfig-experiment-run-conflict-and-notfound-errors#task#8).

Related: the open behavioral issue 20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md records the same mapping fault from the behavioral corpus (its cause 4) and the open stub issue 20261023T032202Z-appconfig-createexperimentdefinition-stub.md concerns the operation's implementation; neither carries this fixture step's replay key, so it is bound here.

## Evidence
Wire replay (simparity-wire-aws-v1): appconfig-error-paths-experiment-definitions#task#6 CreateExperimentDefinition — expected 400 / actual 400; $headers.x-amzn-errortype value_mismatch (expected 'BadRequestException:', got 'ValidationException'); $.Message missing (expected "1 validation error detected: Value '1invalidFlagKey' at 'flagKey' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z][a-zA-Z0-9_-]{1,64}", got None). Same cluster: appconfig-experiment-run-conflict-and-notfound-errors#task#8.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_application -> create_environment -> create_configuration_profile(LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags') -> create_hosted_configuration_version.
2. create_experiment_definition(ApplicationIdentifier=<app>, Name=..., ConfigurationProfileIdentifier=<cp>, EnvironmentIdentifier=<env>, FlagKey='1invalidFlagKey', Treatments=[...], Control=..., AudienceRule='true').
The refusal must be BadRequestException (400) with x-amzn-errortype 'BadRequestException:' and a Message naming 'flagKey' and its pattern; the emulator returns 400 with errortype 'ValidationException' and no Message.
Re-replay appconfig-error-paths-experiment-definitions#task#6.
