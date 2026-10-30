Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-endpoint-create-active-delete-404 acm.acme-endpoint-delete-then-describe-404 acm.acme-endpoint-lifecycle-transitions acm.acme-endpoint-terminal-removal acm.delete-acme-endpoint-removes-resource acm.delete-acme-endpoint-without-permission-denied acm.tag-resource-nonexistent-acme-endpoint-not-found acm.untag-resource-on-deleted-acme-resource-rejected acm.update-acme-endpoint-missing-returns-conflict acm.update-acme-endpoint-nonexistent-conflict
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DeleteAcmeEndpoint|NotImplemented
Issue-Key: acm.delete-acme-endpoint-without-permission-denied|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code} ({message!r})"
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement acm DeleteAcmeEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAcmeEndpoint

## Description
`DeleteAcmeEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/delete_acme_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `DeleteAcmeEndpoint` call, so each one unblocks only when `DeleteAcmeEndpoint` works: `acm.acme-endpoint-create-active-delete-404`, `acm.acme-endpoint-delete-then-describe-404`, `acm.acme-endpoint-lifecycle-transitions`, `acm.acme-endpoint-terminal-removal`, `acm.delete-acme-endpoint-removes-resource`, `acm.delete-acme-endpoint-without-permission-denied`, `acm.tag-resource-nonexistent-acme-endpoint-not-found`, `acm.untag-resource-on-deleted-acme-resource-rejected`, `acm.update-acme-endpoint-missing-returns-conflict`, `acm.update-acme-endpoint-nonexistent-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/delete_acme_endpoint.rs`. Replaying the corpus, the emulator refused the `DeleteAcmeEndpoint` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteAcmeEndpoint operation: operation DeleteAcmeEndpoint is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm emulator
Call DeleteAcmeEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
