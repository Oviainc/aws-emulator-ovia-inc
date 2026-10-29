Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.provisioning-profile-concurrent-create-idempotent-client-token iot-managed-integrations.provisioning-profile-lifecycle-status
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.provisioning-profile-concurrent-create-idempotent-client-token|run#11|AssertionError|assert winners and len(set(winners)) == 1, f"expected one winning Id, got {results}"
Issue-Key: iot-managed-integrations|CreateProvisioningProfile|NotImplemented
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# Implement iot-managed-integrations CreateProvisioningProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProvisioningProfile

## Description
`CreateProvisioningProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_provisioning_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateProvisioningProfile` call, so each one unblocks only when `CreateProvisioningProfile` works: `iot-managed-integrations.provisioning-profile-concurrent-create-idempotent-client-token`, `iot-managed-integrations.provisioning-profile-lifecycle-status`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_provisioning_profile.rs`. Replaying the corpus, the emulator refused the `CreateProvisioningProfile` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_iot_managed_integrations_provisioning_profile_concurrent_create_idempotent_client_token.py", line 44, in run
    assert winners and len(set(winners)) == 1, f"expected one winning Id, got {results}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected one winning Id, got [('err', ('NotImplemented', 'operation CreateProvisioningProfile is scaffolded but not implemented in this emulator')), ('err', ('NotImplemented', 'operation CreateProvisioningProfile is scaffolded but not implemented in this emulator')), ('err', ('NotImplemented', 'operation CreateProvisioningProfile is scaffolded but not implemented in this emulator')), ('err', ('NotImplemented', 'operation CreateProvisioningProfile is scaffolded but not implemented in this emulator')), ('err', ('NotImplemented', 'operation CreateProvisioningProfile is scaffolded but not implemented in this emulator'))]

## Steps to Reproduce
Boot the iot-managed-integrations emulator
Call CreateProvisioningProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
