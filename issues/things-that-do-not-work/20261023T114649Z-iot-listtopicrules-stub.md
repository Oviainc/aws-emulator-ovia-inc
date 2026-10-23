Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cli-iot-topic-rule-list-and-describe-roundtrip
Issue-Key: iot__lambda.cli-iot-topic-rule-list-and-describe-roundtrip|_run_shell#13|AssertionError|raise AssertionError(
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement iot ListTopicRules: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTopicRules

## Description
`ListTopicRules` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_topic_rules.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListTopicRules` call, so each one unblocks only when `ListTopicRules` works: `iot__lambda.cli-iot-topic-rule-list-and-describe-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_topic_rules.rs`. Replaying the corpus, the emulator refused the `ListTopicRules` call: File "check_iot__lambda_cli_iot_topic_rule_list_and_describe_roundtrip.py", line 109, in _run_aws
    proc = _run_shell(
        ctx, ["aws", "--output", "json", *args], env_extra={"AWS_PAGER": ""}, timeout_s=timeout_s, check=check
    )
  File "check_iot__lambda_cli_iot_topic_rule_list_and_describe_roundtrip.py", line 89, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'iot', 'list-topic-rules']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the ListTopicRules operation: operation ListTopicRules is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call ListTopicRules and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
