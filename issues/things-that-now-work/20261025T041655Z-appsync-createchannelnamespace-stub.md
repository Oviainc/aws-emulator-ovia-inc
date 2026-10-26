Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.access-denied-channel-namespace appsync.channel-namespace-lifecycle appsync.channel-namespace-list-ordering appsync.event-api-ops-reject-graphql-api-id appsync.op-contract-list-channel-namespaces appsync.update-channel-namespace-auth-modes-persists
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateChannelNamespace|NotImplemented
Issue-Commit: 8b3fb3b9641f9f0b47110ea9811423e23ffd107b

# Implement appsync CreateChannelNamespace: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8b3fb3b9641f9f0b47110ea9811423e23ffd107b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannelNamespace

## Description
`CreateChannelNamespace` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_channel_namespace.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateChannelNamespace` call, so each one unblocks only when `CreateChannelNamespace` works: `appsync.access-denied-channel-namespace`, `appsync.channel-namespace-lifecycle`, `appsync.channel-namespace-list-ordering`, `appsync.event-api-ops-reject-graphql-api-id`, `appsync.op-contract-list-channel-namespaces`, `appsync.update-channel-namespace-auth-modes-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_channel_namespace.rs`. Replaying the corpus, the emulator refused the `CreateChannelNamespace` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateChannelNamespace operation: operation CreateChannelNamespace is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appsync emulator
Call CreateChannelNamespace and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
