Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models__lexv2-runtime.recognize-text-elicits-then-resolves-slot-synonym lexv2-models__lexv2-runtime.recognize-text-fills-slot-from-utterance lexv2-models__lexv2-runtime.recognize-text-rejects-out-of-vocabulary-slot-value lexv2-models__lexv2-runtime.unbuilt-bot-locale-recognize-text-is-rejected lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected lexv2-runtime.put-session-rejects-an-intent-the-locale-does-not-define lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn lexv2-runtime.recognize-text-elicits-missing-required-slot lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent lexv2-runtime.recognize-text-rejects-an-unbuilt-locale lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text lexv2-runtime.session-state-persists-across-turns lexv2-runtime.unmatched-utterance-returns-fallback-intent
Issue-Key: lex-models-v2|CreateSlot|NotImplemented
Issue-Commit: a4dc6e21f65e74528fa71c599283ec5aa084cf98

# Implement lex-models-v2 CreateSlot: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a4dc6e21f65e74528fa71c599283ec5aa084cf98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlot

## Description
`CreateSlot` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateSlot` call, so each one unblocks only when `CreateSlot` works: `lex-models-v2__lex-runtime-v2.recognize-text-elicits-then-resolves-slot-synonym`, `lex-models-v2__lex-runtime-v2.recognize-text-fills-slot-from-utterance`, `lex-models-v2__lex-runtime-v2.recognize-text-rejects-out-of-vocabulary-slot-value`, `lex-models-v2__lex-runtime-v2.unbuilt-bot-locale-recognize-text-is-rejected`, `lex-runtime-v2.deleted-session-state-is-gone-and-get-session-rejected`, `lex-runtime-v2.put-session-rejects-an-intent-the-locale-does-not-define`, `lex-runtime-v2.put-session-seeds-dialog-state-for-the-next-turn`, `lex-runtime-v2.recognize-text-elicits-missing-required-slot`, `lex-runtime-v2.recognize-text-fills-slot-and-fulfills-intent`, `lex-runtime-v2.recognize-text-rejects-an-unbuilt-locale`, `lex-runtime-v2.recognize-text-rejects-an-unknown-bot-alias`, `lex-runtime-v2.recognize-utterance-text-stream-agrees-with-recognize-text`, `lex-runtime-v2.session-state-persists-across-turns`, `lex-runtime-v2.unmatched-utterance-returns-fallback-intent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot.rs`. Replaying the corpus, the emulator refused the `CreateSlot` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSlot operation: operation CreateSlot is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lex-models-v2 emulator
Call CreateSlot and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
