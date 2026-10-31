Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager.checkout-of-undeclared-entitlement-name-rejected license-manager.concurrent-checkouts-never-exceed-entitlement-max-count license-manager.entitlement-consumed-count-tracks-checkout-and-checkin
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager|GetLicenseUsage|NotImplemented
Issue-Commit: 5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954

# Implement license-manager GetLicenseUsage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLicenseUsage

## Description
`GetLicenseUsage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-licensemanager-awslicensemanager/src/ops/get_license_usage.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `GetLicenseUsage` call, so each one unblocks only when `GetLicenseUsage` works: `license-manager.checkout-of-undeclared-entitlement-name-rejected`, `license-manager.concurrent-checkouts-never-exceed-entitlement-max-count`, `license-manager.entitlement-consumed-count-tracks-checkout-and-checkin`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-licensemanager-awslicensemanager/src/ops/get_license_usage.rs`. Replaying the corpus, the emulator refused the `GetLicenseUsage` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetLicenseUsage operation: operation GetLicenseUsage is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the license-manager emulator
Call GetLicenseUsage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
