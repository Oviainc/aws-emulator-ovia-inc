Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.create-configuration-template-and-verify-options elasticbeanstalk.create-configuration-template-duplicate-name-must-fail elasticbeanstalk.create-environment-invalid-solution-stack-must-fail elasticbeanstalk.delete-configuration-template-removes-it elasticbeanstalk.delete-environment-configuration-removes-draft-configuration elasticbeanstalk.e2e-compose-environments-from-templates-produces-linked-envs elasticbeanstalk.update-configuration-template-option-settings elasticbeanstalk.validate-configuration-settings-asg-min-above-max-rejected elasticbeanstalk.validate-configuration-settings-enforces-published-range-rejected elasticbeanstalk.validate-configuration-settings-flags-bad-option elasticbeanstalk.validate-configuration-settings-out-of-range-rejected elasticbeanstalk.validate-configuration-settings-unknown-namespace-rejected elasticbeanstalk.validate-configuration-settings-unknown-option-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elastic-beanstalk|CreateConfigurationTemplate|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Implement elasticbeanstalk CreateConfigurationTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationTemplate

## Description
`CreateConfigurationTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_configuration_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateConfigurationTemplate` call, so each one unblocks only when `CreateConfigurationTemplate` works: `elasticbeanstalk.create-configuration-template-and-verify-options`, `elasticbeanstalk.create-configuration-template-duplicate-name-must-fail`, `elasticbeanstalk.create-environment-invalid-solution-stack-must-fail`, `elasticbeanstalk.delete-configuration-template-removes-it`, `elasticbeanstalk.delete-environment-configuration-removes-draft-configuration`, `elasticbeanstalk.e2e-compose-environments-from-templates-produces-linked-envs`, `elasticbeanstalk.update-configuration-template-option-settings`, `elasticbeanstalk.validate-configuration-settings-asg-min-above-max-rejected`, `elasticbeanstalk.validate-configuration-settings-enforces-published-range-rejected`, `elasticbeanstalk.validate-configuration-settings-flags-bad-option`, `elasticbeanstalk.validate-configuration-settings-out-of-range-rejected`, `elasticbeanstalk.validate-configuration-settings-unknown-namespace-rejected`, `elasticbeanstalk.validate-configuration-settings-unknown-option-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_configuration_template.rs`. Replaying the corpus, the emulator refused the `CreateConfigurationTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationTemplate operation: operation CreateConfigurationTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticbeanstalk emulator
Call CreateConfigurationTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
