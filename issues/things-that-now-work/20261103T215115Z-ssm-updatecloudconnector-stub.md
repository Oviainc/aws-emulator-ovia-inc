Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.cloud-connector-concurrent-update-unknown-id-not-found ssm.cloud-connector-delete-during-update-conflict-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.cloud-connector-concurrent-update-unknown-id-not-found|run#6|AssertionError|assert codes == [expected] * 5, (
Issue-Key: ssm.cloud-connector-delete-during-update-conflict-rejected|run#6|AssertionError|assert update_err["Code"] == "ResourceNotFoundException", update_err
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm UpdateCloudConnector: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCloudConnector

## Description
`UpdateCloudConnector` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_cloud_connector.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateCloudConnector` call, so each one unblocks only when `UpdateCloudConnector` works: `ssm.cloud-connector-concurrent-update-unknown-id-not-found`, `ssm.cloud-connector-delete-during-update-conflict-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_cloud_connector.rs`. Replaying the corpus, the emulator refused the `UpdateCloudConnector` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ssm_cloud_connector_concurrent_update_unknown_id_not_found.py", line 32, in run
    assert codes == [expected] * 5, (
           ^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected all 5 concurrent racers to get ('ResourceNotFoundException', 'CloudConnector not found for account.'), got [('NotImplemented', 'operation UpdateCloudConnector is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation UpdateCloudConnector is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation UpdateCloudConnector is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation UpdateCloudConnector is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation UpdateCloudConnector is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the ssm emulator
Call UpdateCloudConnector and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
