Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-connections.create-sync-configuration-duplicate-resource-rejected codestar-connections.create-sync-configuration-success-fields codestar-connections.create-sync-configuration-with-publish-deployment-status-persisted codestar-connections.delete-repository-link-blocked-by-sync-configuration-rejected codestar-connections.delete-sync-configuration-removes-it codestar-connections.get-resource-sync-status-succeeds-without-sync-activity codestar-connections.list-sync-configurations-reflects-created-config codestar-connections.update-sync-blocker-nonexistent-id-rejected codestar-connections.update-sync-configuration-branch-persisted
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|CreateSyncConfiguration|NotImplemented
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# Implement codestar-connections CreateSyncConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSyncConfiguration

## Description
`CreateSyncConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/create_sync_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateSyncConfiguration` call, so each one unblocks only when `CreateSyncConfiguration` works: `codestar-connections.create-sync-configuration-duplicate-resource-rejected`, `codestar-connections.create-sync-configuration-success-fields`, `codestar-connections.create-sync-configuration-with-publish-deployment-status-persisted`, `codestar-connections.delete-repository-link-blocked-by-sync-configuration-rejected`, `codestar-connections.delete-sync-configuration-removes-it`, `codestar-connections.get-resource-sync-status-succeeds-without-sync-activity`, `codestar-connections.list-sync-configurations-reflects-created-config`, `codestar-connections.update-sync-blocker-nonexistent-id-rejected`, `codestar-connections.update-sync-configuration-branch-persisted`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/create_sync_configuration.rs`. Replaying the corpus, the emulator refused the `CreateSyncConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSyncConfiguration operation: operation CreateSyncConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codestar-connections emulator
Call CreateSyncConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
