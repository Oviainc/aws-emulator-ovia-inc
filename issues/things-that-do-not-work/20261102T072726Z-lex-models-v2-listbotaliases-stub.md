Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models__lexv2-runtime.recognize-text-elicits-then-resolves-slot-synonym lexv2-models__lexv2-runtime.recognize-text-fills-slot-from-utterance lexv2-models__lexv2-runtime.recognize-text-rejects-out-of-vocabulary-slot-value lexv2-models__lexv2-runtime.unbuilt-bot-locale-recognize-text-is-rejected
Issue-Key: lex-models-v2|ListBotAliases|NotImplemented
Issue-Commit: 88dedb85e694d97545027b37e695bddb9d3861f2

# Implement lex-models-v2 ListBotAliases: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `88dedb85e694d97545027b37e695bddb9d3861f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBotAliases

## Description
`ListBotAliases` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/list_bot_aliases.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `ListBotAliases` call, so each one unblocks only when `ListBotAliases` works: `lex-models-v2__lex-runtime-v2.recognize-text-elicits-then-resolves-slot-synonym`, `lex-models-v2__lex-runtime-v2.recognize-text-fills-slot-from-utterance`, `lex-models-v2__lex-runtime-v2.recognize-text-rejects-out-of-vocabulary-slot-value`, `lex-models-v2__lex-runtime-v2.unbuilt-bot-locale-recognize-text-is-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/list_bot_aliases.rs`. Replaying the corpus, the emulator refused the `ListBotAliases` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListBotAliases operation: operation ListBotAliases is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lex-models-v2 emulator
Call ListBotAliases and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
