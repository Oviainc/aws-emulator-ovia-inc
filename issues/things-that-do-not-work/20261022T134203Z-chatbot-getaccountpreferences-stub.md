Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.account-preferences-update-get-roundtrip chatbot.account-preferences-update-roundtrip
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|GetAccountPreferences|NotImplemented
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Implement chatbot GetAccountPreferences: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountPreferences

## Description
`GetAccountPreferences` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/get_account_preferences.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetAccountPreferences` call, so each one unblocks only when `GetAccountPreferences` works: `chatbot.account-preferences-update-get-roundtrip`, `chatbot.account-preferences-update-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/get_account_preferences.rs`. Replaying the corpus, the emulator refused the `GetAccountPreferences` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccountPreferences operation: operation GetAccountPreferences is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the chatbot emulator
Call GetAccountPreferences and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
