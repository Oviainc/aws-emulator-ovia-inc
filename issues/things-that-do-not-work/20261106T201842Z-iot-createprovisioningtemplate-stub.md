Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.concurrent-provisioning-template-version-creation-conflict iot.delete-provisioning-template-in-use-rejected iot.delete-provisioning-template-version-conflict-rejected iot.deploy-iot-provisioning-template-cfn-enrolls-device iot.e2e-fleet-provisioning-claim-to-thing iot.provisioning-claim-unknown-template-rejected iot.provisioning-template-json-body-round-trip iot.provisioning-template-version-concurrent-update-conflict iot.provisioning-template-version-delete-default-rejected iot.provisioning-template-version-lifecycle iot.provisioning-template-version-lifecycle-and-default iot.representation-provisioning-template-json-round-trip
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateProvisioningTemplate|NotImplemented
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# Implement iot CreateProvisioningTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProvisioningTemplate

## Description
`CreateProvisioningTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_provisioning_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateProvisioningTemplate` call, so each one unblocks only when `CreateProvisioningTemplate` works: `iot.concurrent-provisioning-template-version-creation-conflict`, `iot.delete-provisioning-template-in-use-rejected`, `iot.delete-provisioning-template-version-conflict-rejected`, `iot.deploy-iot-provisioning-template-cfn-enrolls-device`, `iot.e2e-fleet-provisioning-claim-to-thing`, `iot.provisioning-claim-unknown-template-rejected`, `iot.provisioning-template-json-body-round-trip`, `iot.provisioning-template-version-concurrent-update-conflict`, `iot.provisioning-template-version-delete-default-rejected`, `iot.provisioning-template-version-lifecycle`, `iot.provisioning-template-version-lifecycle-and-default`, `iot.representation-provisioning-template-json-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_provisioning_template.rs`. Replaying the corpus, the emulator refused the `CreateProvisioningTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateProvisioningTemplate operation: operation CreateProvisioningTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call CreateProvisioningTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
