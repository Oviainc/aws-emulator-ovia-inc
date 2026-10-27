Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.put-configuration-set-archiving-options-unknown-cs-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.put-configuration-set-archiving-options-unknown-cs-rejected|run#5|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Commit: 6e8bba18a5a906dda1323a55e22368d1363d6826

# Implement sesv2 PutConfigurationSetArchivingOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e8bba18a5a906dda1323a55e22368d1363d6826`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigurationSetArchivingOptions

## Description
`PutConfigurationSetArchivingOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_configuration_set_archiving_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutConfigurationSetArchivingOptions` call, so each one unblocks only when `PutConfigurationSetArchivingOptions` works: `sesv2.put-configuration-set-archiving-options-unknown-cs-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_configuration_set_archiving_options.rs`. Replaying the corpus, the emulator refused the `PutConfigurationSetArchivingOptions` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_put_configuration_set_archiving_options_unknown_cs_rejected.py", line 24, in run
    assert err == expected, f"expected {expected!r}, got {err!r}"
           ^^^^^^^^^^^^^^^
AssertionError: expected ('NotFoundException', 'Configuration set <sim2real-nocs-88e9ceacf114> does not exist.'), got ('NotImplemented', 'operation PutConfigurationSetArchivingOptions is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sesv2 emulator
Call PutConfigurationSetArchivingOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
