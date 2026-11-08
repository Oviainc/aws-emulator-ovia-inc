Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.resource-not-found-error-family
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.resource-not-found-error-family|_assert_error#12|AssertionError|assert err["Code"] == code, f"{op}: expected {code}, got {err!r}"
Issue-Commit: 64e3d31da0f219acb3f0523749fc49dddb7bfb57

# Implement cloudwatch GetAlarmMuteRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `64e3d31da0f219acb3f0523749fc49dddb7bfb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAlarmMuteRule

## Description
`GetAlarmMuteRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_alarm_mute_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetAlarmMuteRule` call, so each one unblocks only when `GetAlarmMuteRule` works: `cloudwatch.resource-not-found-error-family`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_alarm_mute_rule.rs`. Replaying the corpus, the emulator refused the `GetAlarmMuteRule` call: File "check_cloudwatch_resource_not_found_error_family.py", line 79, in run
    _assert_error(err, "GetAlarmMuteRule", "ResourceNotFoundException", "")
    ~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_cloudwatch_resource_not_found_error_family.py", line 27, in _assert_error
    assert err["Code"] == code, f"{op}: expected {code}, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: GetAlarmMuteRule: expected ResourceNotFoundException, got {'Message': 'operation GetAlarmMuteRule is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the cloudwatch emulator
Call GetAlarmMuteRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
