Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.describe-app-instance-bot-not-found-rejected
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.describe-app-instance-bot-not-found-rejected|run#6|AssertionError|assert error["Code"] == "ForbiddenException", f"expected ForbiddenException, got {error!r}"
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# Implement chime-sdk-identity DescribeAppInstanceBot: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppInstanceBot

## Description
`DescribeAppInstanceBot` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance_bot.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeAppInstanceBot` call, so each one unblocks only when `DescribeAppInstanceBot` works: `chime-sdk-identity.describe-app-instance-bot-not-found-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance_bot.rs`. Replaying the corpus, the emulator refused the `DescribeAppInstanceBot` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_chime_sdk_identity_describe_app_instance_bot_not_found_rejected.py", line 36, in run
    assert error["Code"] == "ForbiddenException", f"expected ForbiddenException, got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ForbiddenException, got {'Message': 'operation DescribeAppInstanceBot is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the chime-sdk-identity emulator
Call DescribeAppInstanceBot and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
