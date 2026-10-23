Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.e2e-request-and-retrieve-environment-info-flow elasticbeanstalk.environment-info-tail-log-content-round-trip elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elastic-beanstalk|RequestEnvironmentInfo|NotImplemented
Issue-Key: elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected|run#15|AssertionError|assert code == "InvalidParameterValue", f"request_environment_info: {code}: {message}"
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Implement elasticbeanstalk RequestEnvironmentInfo: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestEnvironmentInfo

## Description
`RequestEnvironmentInfo` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/request_environment_info.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `RequestEnvironmentInfo` call, so each one unblocks only when `RequestEnvironmentInfo` works: `elasticbeanstalk.e2e-request-and-retrieve-environment-info-flow`, `elasticbeanstalk.environment-info-tail-log-content-round-trip`, `elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/request_environment_info.rs`. Replaying the corpus, the emulator refused the `RequestEnvironmentInfo` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RequestEnvironmentInfo operation: operation RequestEnvironmentInfo is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticbeanstalk emulator
Call RequestEnvironmentInfo and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
