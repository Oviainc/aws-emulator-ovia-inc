Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-mute-rule-lifecycle-create-get-list-delete cloudwatch.alarm-mute-rule-lifecycle-put-get-list-delete cloudwatch.alarm-mute-rule-put-get-list-delete-lifecycle cloudwatch.alarm-mute-rule-status-computed-from-dates cloudwatch.delete-alarm-mute-rule-nonexistent-idempotent cloudwatch.double-delete-returns-not-found-not-success cloudwatch.get-alarm-mute-rule-after-delete-rejected cloudwatch.put-alarm-mute-rule-crud-visible-immediately-in-list-alarm-mute-rules cloudwatch.put-alarm-mute-rule-limit-exceeded-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|DeleteAlarmMuteRule|NotImplemented
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# Implement cloudwatch DeleteAlarmMuteRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAlarmMuteRule

## Description
`DeleteAlarmMuteRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_alarm_mute_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `DeleteAlarmMuteRule` call, so each one unblocks only when `DeleteAlarmMuteRule` works: `cloudwatch.alarm-mute-rule-lifecycle-create-get-list-delete`, `cloudwatch.alarm-mute-rule-lifecycle-put-get-list-delete`, `cloudwatch.alarm-mute-rule-put-get-list-delete-lifecycle`, `cloudwatch.alarm-mute-rule-status-computed-from-dates`, `cloudwatch.delete-alarm-mute-rule-nonexistent-idempotent`, `cloudwatch.double-delete-returns-not-found-not-success`, `cloudwatch.get-alarm-mute-rule-after-delete-rejected`, `cloudwatch.put-alarm-mute-rule-crud-visible-immediately-in-list-alarm-mute-rules`, `cloudwatch.put-alarm-mute-rule-limit-exceeded-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_alarm_mute_rule.rs`. Replaying the corpus, the emulator refused the `DeleteAlarmMuteRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteAlarmMuteRule operation: operation DeleteAlarmMuteRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatch emulator
Call DeleteAlarmMuteRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
