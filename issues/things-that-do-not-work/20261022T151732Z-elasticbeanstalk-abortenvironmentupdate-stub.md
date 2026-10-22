Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.abort-environment-update-unknown-env-fails
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.abort-environment-update-unknown-env-fails|run#4|AssertionError|assert code == "InvalidParameterValue", (
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Implement elasticbeanstalk AbortEnvironmentUpdate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AbortEnvironmentUpdate

## Description
`AbortEnvironmentUpdate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/abort_environment_update.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AbortEnvironmentUpdate` call, so each one unblocks only when `AbortEnvironmentUpdate` works: `elasticbeanstalk.abort-environment-update-unknown-env-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/abort_environment_update.rs`. Replaying the corpus, the emulator refused the `AbortEnvironmentUpdate` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_elasticbeanstalk_abort_environment_update_unknown_env_fails.py", line 38, in run
    assert code == "InvalidParameterValue", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: AbortEnvironmentUpdate on a nonexistent environment must fail with InvalidParameterValue, got 'NotImplemented' ('operation AbortEnvironmentUpdate is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the elasticbeanstalk emulator
Call AbortEnvironmentUpdate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
