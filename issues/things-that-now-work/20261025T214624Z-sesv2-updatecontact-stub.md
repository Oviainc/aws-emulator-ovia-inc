Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.contact-attributes-data-round-trips-and-is-replaced-while-topic-preferences-merge sesv2.contact-update-undefined-topic-rejected sesv2.unsubscribe-all-overrides-an-explicit-topic-optin-without-erasing-it
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.contact-attributes-data-round-trips-and-is-replaced-while-topic-preferences-merge|run#11|AssertionError|assert update_err == _MALFORMED_REFUSAL, (
Issue-Key: sesv2|UpdateContact|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement sesv2 UpdateContact: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateContact

## Description
`UpdateContact` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/update_contact.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdateContact` call, so each one unblocks only when `UpdateContact` works: `sesv2.contact-attributes-data-round-trips-and-is-replaced-while-topic-preferences-merge`, `sesv2.contact-update-undefined-topic-rejected`, `sesv2.unsubscribe-all-overrides-an-explicit-topic-optin-without-erasing-it`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/update_contact.rs`. Replaying the corpus, the emulator refused the `UpdateContact` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_contact_attributes_data_round_trips_and_is_replaced_while_topic_preferences_merge.py", line 111, in run
    assert update_err == _MALFORMED_REFUSAL, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: UpdateContact with AttributesData='{"tier": "gold"', which is not valid JSON, returned ('NotImplemented', 'operation UpdateContact is scaffolded but not implemented in this emulator'), expected ('BadRequestException', 'Attributes must be valid JSON and it can contain up to 1000 characters.')

## Steps to Reproduce
Boot the sesv2 emulator
Call UpdateContact and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
