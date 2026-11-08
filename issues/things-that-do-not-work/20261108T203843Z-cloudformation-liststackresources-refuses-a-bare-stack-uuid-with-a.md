Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically|run#26|AssertionError|assert message == f"Stack with id {bare_uuid} does not exist", (
Issue-Commit: 903d830d252bdd68b0796b6265be694baf73f62a

# ListStackResources refuses a bare stack uuid with a pattern-constraint envelope instead of "Stack with id <id> does not exist"

> _Note: this issue was identified against the code base at commit `903d830d252bdd68b0796b6265be694baf73f62a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
One net-new root cause in the CloudFormation stack-identifier pre-flight.

Cause 1 of 1: a stack identifier that is well formed as an identifier but names no stack — specifically the bare uuid suffix of a stack id — is refused by a service-local regular-expression pre-check rather than resolved and answered as a missing stack.

AWS: Smithy: ListStackResources.StackName targets `com.amazonaws.cloudformation#StackName`, a plain string with NO `@pattern` and NO `@length` (`smithy_query(query="shape", shape="StackName")` returns `{"type": "string"}` and nothing else), and ListStackResources models no operation errors, so the refusal is the service-common `ValidationError` at HTTP 400. Recorded behavior: a stack is addressable by its name or by its full stack-id ARN; passing only the uuid tail of the stack id (`<stack-id>.rsplit("/")[-1]`) is accepted as an identifier, resolves to nothing, and is refused `ValidationError` with the message `Stack with id <identifier> does not exist`. Fixed text: `Stack with id ` … ` does not exist`; the only variable part is the identifier copied verbatim from the request's StackName.

Emulator: the handler runs `crate::state::refuse_malformed_identifier(STACK_NAME_OR_ID_PATH, ...)` before any lookup, and the uuid fails the service-local expression `[a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*` because a uuid may begin with a digit. The call is refused with the right wire code (`ValidationError`, 400) but the constraint envelope instead of the not-found message: `1 validation error detected: Value '<uuid>' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*`.

Why: a client cannot tell "this stack is gone" from "you sent me garbage". Code that matches on the not-found wording — the stack-existence probe every waiter and cleanup path in the suite uses, e.g. `"does not exist" in str(exc)` — treats the stack as still present, and any caller that holds a uuid (from a log line, an event, a split stack id) is told its parameter is malformed rather than that the stack is not there. The same overbroad pre-check applies to every identifier that is digit-leading or contains a character outside the name expression, so it is not specific to uuids.

Fix: the stack-identifier pre-flight — `malformed_identifier` / `refuse_malformed_identifier` in `services/service-com-amazonaws-cloudformation-cloudformation/src/state/stack_identifier.rs`, as called at the top of the ListStackResources handler. Only an identifier CloudFormation could never resolve should be refused there; one it resolves-and-misses must fall through to the existing `crate::state::stack_does_not_exist` refusal the handler already builds.

Traces: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically (the earlier assertions in that check — StackId ARN shape, name-vs-ARN equality across ListStackResources, DescribeStackResources and DescribeStackResource — all passed; only the bare-uuid refusal diverged).

## Evidence
From the behavioral replay, check cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically:

  AssertionError: ListStackResources on the bare stack uuid answered "1 validation error detected: Value '0e81d159-090a-7b61-9856-04d0999cb465' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*"

  assert message == f"Stack with id {bare_uuid} does not exist"

Expected (AWS): HTTP 400, Error.Code `ValidationError`, Error.Message `Stack with id <identifier> does not exist` where `<identifier>` is the StackName as sent.
Actual (emulator): HTTP 400, Error.Code `ValidationError` (correct), Error.Message the constraint envelope quoted above.

Source: `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_resources.rs:192` calls `refuse_malformed_identifier(STACK_NAME_OR_ID_PATH, Some(&input.stack_name))` ahead of `store.resolve(...)`, and `STACK_IDENTIFIER_PATTERN` is defined at `services/service-com-amazonaws-cloudformation-cloudformation/src/state/stack_identifier.rs:39`. The correct message template already exists as `stack_does_not_exist` in `src/state.rs`.

## Steps to Reproduce
1. CreateStack with any template (the check uses one AWS::SSM::Parameter plus one AWS::SQS::Queue) and wait for CREATE_COMPLETE.
2. Take the StackId the create returned and keep only the segment after the last '/' (the bare uuid).
3. Call ListStackResources(StackName=<bare uuid>).
4. Observe Error.Code `ValidationError` with the pattern-constraint envelope; AWS answers `ValidationError` with `Stack with id <bare uuid> does not exist`.

Re-replay: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically.
