Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.deploy-iot-topic-rule-cfn-routes-messages iot__lambda.cli-iot-create-topic-rule-lambda-invoke iot__lambda.cli-iot-delete-topic-rule-then-publish-no-invoke-negative iot__lambda.cli-lambda-add-permission-wrong-principal-negative iot__lambda.iot-rule-action-role-trusts-iot-service-positive iot__lambda.iot-rule-action-role-wrong-trust-negative iot__lambda.iot-rule-disabled-no-invocation-negative iot__lambda.iot-rule-disabled-rule-no-invocation-negative iot__lambda.iot-rule-error-action-lambda-positive iot__lambda.iot-rule-lambda-action-positive iot__lambda.iot-rule-lambda-missing-invoke-permission-negative iot__lambda.iot-rule-lambda-payload-data-format iot__lambda.iot-rule-lambda-wrong-arn-negative iot__lambda.iot-rule-lambda-wrong-source-arn-negative iot__lambda.iot-rule-republish-chain-lambda-positive iot__lambda.iot-rule-sql-filter-lambda-positive iot__lambda.iot-topicrule-create-with-untrusted-role-negative iot__lambda.lambda-iot-data-publish-missing-permission-negative iot__lambda.lambda-iot-data-publish-positive iot__lambda.lambda-sdk-iot-publish-missing-permission-negative iot__lambda.lambda-sdk-iot-publish-positive iot__s3.rule-s3-action-substitution-template-computes-the-object-key
Issue-Key: iot|CreateTopicRule|NotImplemented
Issue-Key: iot__lambda.cli-iot-create-topic-rule-lambda-invoke|_run_shell#6|AssertionError|raise AssertionError(
Issue-Key: iot__lambda.cli-iot-delete-topic-rule-then-publish-no-invoke-negative|_run_shell#6|AssertionError|raise AssertionError(
Issue-Key: iot__lambda.cli-lambda-add-permission-wrong-principal-negative|_run_shell#6|AssertionError|raise AssertionError(
Issue-Key: iot__s3.rule-s3-action-substitution-template-computes-the-object-key|_retry_client_error#26|AssertionError|raise AssertionError(f"{what}: {last!r}")
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Implement iot CreateTopicRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopicRule

## Description
`CreateTopicRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_topic_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `CreateTopicRule` call, so each one unblocks only when `CreateTopicRule` works: `iot.deploy-iot-topic-rule-cfn-routes-messages`, `iot__lambda.cli-iot-create-topic-rule-lambda-invoke`, `iot__lambda.cli-iot-delete-topic-rule-then-publish-no-invoke-negative`, `iot__lambda.cli-lambda-add-permission-wrong-principal-negative`, `iot__lambda.iot-rule-action-role-trusts-iot-service-positive`, `iot__lambda.iot-rule-action-role-wrong-trust-negative`, `iot__lambda.iot-rule-disabled-no-invocation-negative`, `iot__lambda.iot-rule-disabled-rule-no-invocation-negative`, `iot__lambda.iot-rule-error-action-lambda-positive`, `iot__lambda.iot-rule-lambda-action-positive`, `iot__lambda.iot-rule-lambda-missing-invoke-permission-negative`, `iot__lambda.iot-rule-lambda-payload-data-format`, `iot__lambda.iot-rule-lambda-wrong-arn-negative`, `iot__lambda.iot-rule-lambda-wrong-source-arn-negative`, `iot__lambda.iot-rule-republish-chain-lambda-positive`, `iot__lambda.iot-rule-sql-filter-lambda-positive`, `iot__lambda.iot-topicrule-create-with-untrusted-role-negative`, `iot__lambda.lambda-iot-data-publish-missing-permission-negative`, `iot__lambda.lambda-iot-data-publish-positive`, `iot__lambda.lambda-sdk-iot-publish-missing-permission-negative` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_topic_rule.rs`. Replaying the corpus, the emulator refused the `CreateTopicRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTopicRule operation: operation CreateTopicRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call CreateTopicRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
