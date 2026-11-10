Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.delete-instance-onboarding-job-no-job-not-found
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.delete-instance-onboarding-job-no-job-not-found|run#4|AssertionError|assert control_code == "InvalidStateException", (
Issue-Commit: 70ed2d2e7473e9995477fa4e995826b18e438b3d

# Implement connectcampaigns DeleteInstanceOnboardingJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `70ed2d2e7473e9995477fa4e995826b18e438b3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInstanceOnboardingJob

## Description
`DeleteInstanceOnboardingJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/delete_instance_onboarding_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteInstanceOnboardingJob` call, so each one unblocks only when `DeleteInstanceOnboardingJob` works: `connectcampaigns.delete-instance-onboarding-job-no-job-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/delete_instance_onboarding_job.rs`. Replaying the corpus, the emulator refused the `DeleteInstanceOnboardingJob` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_connectcampaigns_delete_instance_onboarding_job_no_job_not_found.py", line 24, in run
    assert control_code == "InvalidStateException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: positive control: delete on a real, onboarded instance must be refused with InvalidStateException, got NotImplemented: operation DeleteInstanceOnboardingJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the connectcampaigns emulator
Call DeleteInstanceOnboardingJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
