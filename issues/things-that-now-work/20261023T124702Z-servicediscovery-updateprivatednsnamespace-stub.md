Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.namespace-description-unicode-round-trip servicediscovery.update-namespace-not-found-error servicediscovery.update-private-dns-namespace-properties
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|UpdatePrivateDnsNamespace|NotImplemented
Issue-Key: servicediscovery.update-namespace-not-found-error|run#14|AssertionError|assert error.get("Code") == "NamespaceNotFound", f"expected NamespaceNotFound, got {error!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement servicediscovery UpdatePrivateDnsNamespace: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePrivateDnsNamespace

## Description
`UpdatePrivateDnsNamespace` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_private_dns_namespace.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdatePrivateDnsNamespace` call, so each one unblocks only when `UpdatePrivateDnsNamespace` works: `servicediscovery.namespace-description-unicode-round-trip`, `servicediscovery.update-namespace-not-found-error`, `servicediscovery.update-private-dns-namespace-properties`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_private_dns_namespace.rs`. Replaying the corpus, the emulator refused the `UpdatePrivateDnsNamespace` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdatePrivateDnsNamespace operation: operation UpdatePrivateDnsNamespace is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the servicediscovery emulator
Call UpdatePrivateDnsNamespace and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
