Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.add-list-tags rds.add-source-identifier-to-subscription rds.concurrent-tag-writes-converge rds.event-subscription-source-management rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged rds.remove-tags
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBParameterGroup|NotImplemented
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Implement rds CreateDBParameterGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBParameterGroup

## Description
`CreateDBParameterGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_parameter_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateDBParameterGroup` call, so each one unblocks only when `CreateDBParameterGroup` works: `rds.add-list-tags`, `rds.add-source-identifier-to-subscription`, `rds.concurrent-tag-writes-converge`, `rds.event-subscription-source-management`, `rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged`, `rds.remove-tags`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_parameter_group.rs`. Replaying the corpus, the emulator refused the `CreateDBParameterGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBParameterGroup operation: operation CreateDBParameterGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rds emulator
Call CreateDBParameterGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
