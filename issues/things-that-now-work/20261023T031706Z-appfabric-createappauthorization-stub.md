Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.app-authorization-initial-status-pending-connect appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed appfabric.delete-app-authorization-removes-resource appfabric.delete-app-bundle-with-contents-rejected appfabric.get-app-authorization-nonexistent-not-found appfabric.start-ingestion-without-connected-authorization-rejected appfabric.start-user-access-tasks-reaches-failed-terminal-state appfabric.update-app-authorization-tenant-field-persists-pending-connect
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|CreateAppAuthorization|NotImplemented
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# Implement appfabric CreateAppAuthorization: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization

## Description
`CreateAppAuthorization` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `CreateAppAuthorization` call, so each one unblocks only when `CreateAppAuthorization` works: `appfabric.app-authorization-initial-status-pending-connect`, `appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed`, `appfabric.delete-app-authorization-removes-resource`, `appfabric.delete-app-bundle-with-contents-rejected`, `appfabric.get-app-authorization-nonexistent-not-found`, `appfabric.start-ingestion-without-connected-authorization-rejected`, `appfabric.start-user-access-tasks-reaches-failed-terminal-state`, `appfabric.update-app-authorization-tenant-field-persists-pending-connect`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs`. Replaying the corpus, the emulator refused the `CreateAppAuthorization` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAppAuthorization operation: operation CreateAppAuthorization is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appfabric emulator
Call CreateAppAuthorization and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
