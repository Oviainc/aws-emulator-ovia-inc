Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected elasticbeanstalk.swap-environment-cnames-nonexistent-source-fails
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected|run#6|AssertionError|assert code == "InvalidParameterValue", f"swap_environment_cnames: {code}: {message}"
Issue-Key: elasticbeanstalk.swap-environment-cnames-nonexistent-source-fails|run#6|AssertionError|assert code == "InvalidParameterValue", (
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Implement elasticbeanstalk SwapEnvironmentCNAMEs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SwapEnvironmentCNAMEs

## Description
`SwapEnvironmentCNAMEs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/swap_environment_cnam_es.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `SwapEnvironmentCNAMEs` call, so each one unblocks only when `SwapEnvironmentCNAMEs` works: `elasticbeanstalk.swap-environment-cnames-and-related-ops-against-nonexistent-environments-rejected`, `elasticbeanstalk.swap-environment-cnames-nonexistent-source-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/swap_environment_cnam_es.rs`. Replaying the corpus, the emulator refused the `SwapEnvironmentCNAMEs` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_elasticbeanstalk_swap_environment_cnames_and_related_ops_against_nonexistent_environments_rejected.py", line 55, in run
    assert code == "InvalidParameterValue", f"swap_environment_cnames: {code}: {message}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: swap_environment_cnames: NotImplemented: operation SwapEnvironmentCNAMEs is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticbeanstalk emulator
Call SwapEnvironmentCNAMEs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
