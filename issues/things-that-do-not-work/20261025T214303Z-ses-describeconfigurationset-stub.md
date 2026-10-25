Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.configuration-set-lifecycle ses.deploy-ses-configuration-set-cfn-usable
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.configuration-set-lifecycle|run#4|AssertionError|assert err == ("ConfigurationSetDoesNotExist", f"Configuration set <{name}> does not exist."), f"expected not-found error, got {err!r}"
Issue-Key: ses|DescribeConfigurationSet|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement ses DescribeConfigurationSet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationSet

## Description
`DescribeConfigurationSet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/describe_configuration_set.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DescribeConfigurationSet` call, so each one unblocks only when `DescribeConfigurationSet` works: `ses.configuration-set-lifecycle`, `ses.deploy-ses-configuration-set-cfn-usable`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/describe_configuration_set.rs`. Replaying the corpus, the emulator refused the `DescribeConfigurationSet` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_configuration_set_lifecycle.py", line 31, in run
    assert err == ("ConfigurationSetDoesNotExist", f"Configuration set <{name}> does not exist."), f"expected not-found error, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected not-found error, got ('NotImplemented', 'operation DescribeConfigurationSet is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ses emulator
Call DescribeConfigurationSet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
