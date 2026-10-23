Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway.put-hypervisor-property-mappings-fails-deterministically-without-gateway
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway.put-hypervisor-property-mappings-fails-deterministically-without-gateway|run#7|AssertionError|assert first_put == ("ValidationException", GATEWAY_OUT_OF_DATE), \
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement backup-gateway PutHypervisorPropertyMappings: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutHypervisorPropertyMappings

## Description
`PutHypervisorPropertyMappings` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/put_hypervisor_property_mappings.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutHypervisorPropertyMappings` call, so each one unblocks only when `PutHypervisorPropertyMappings` works: `backup-gateway.put-hypervisor-property-mappings-fails-deterministically-without-gateway`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/put_hypervisor_property_mappings.rs`. Replaying the corpus, the emulator refused the `PutHypervisorPropertyMappings` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_backup_gateway_put_hypervisor_property_mappings_fails_deterministically_without_gateway.py", line 42, in run
    assert first_put == ("ValidationException", GATEWAY_OUT_OF_DATE), \
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException 'The specified gateway is out of date.' on first Put, got ('NotImplemented', 'operation PutHypervisorPropertyMappings is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the backup-gateway emulator
Call PutHypervisorPropertyMappings and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
