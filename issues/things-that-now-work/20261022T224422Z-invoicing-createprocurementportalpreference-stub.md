Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: invoicing.put-procurement-portal-preference-updates-version
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|CreateProcurementPortalPreference|NotImplemented
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# Implement invoicing CreateProcurementPortalPreference: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProcurementPortalPreference

## Description
`CreateProcurementPortalPreference` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateProcurementPortalPreference` call, so each one unblocks only when `CreateProcurementPortalPreference` works: `invoicing.put-procurement-portal-preference-updates-version`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs`. Replaying the corpus, the emulator refused the `CreateProcurementPortalPreference` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateProcurementPortalPreference operation: operation CreateProcurementPortalPreference is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the invoicing emulator
Call CreateProcurementPortalPreference and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
