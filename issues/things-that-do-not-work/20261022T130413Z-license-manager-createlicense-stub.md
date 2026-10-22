Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager.checked-in-consumption-token-is-not-reusable license-manager.checkout-of-undeclared-entitlement-name-rejected license-manager.checkout-past-entitlement-max-count-rejected license-manager.checkout-with-mismatched-entitlement-unit-rejected license-manager.concurrent-checkouts-never-exceed-entitlement-max-count license-manager.entitlement-consumed-count-tracks-checkout-and-checkin license-manager.overage-entitlement-allows-consumption-past-max-count
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager|CreateLicense|NotImplemented
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Implement license-manager CreateLicense: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLicense

## Description
`CreateLicense` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-licensemanager-awslicensemanager/src/ops/create_license.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateLicense` call, so each one unblocks only when `CreateLicense` works: `license-manager.checked-in-consumption-token-is-not-reusable`, `license-manager.checkout-of-undeclared-entitlement-name-rejected`, `license-manager.checkout-past-entitlement-max-count-rejected`, `license-manager.checkout-with-mismatched-entitlement-unit-rejected`, `license-manager.concurrent-checkouts-never-exceed-entitlement-max-count`, `license-manager.entitlement-consumed-count-tracks-checkout-and-checkin`, `license-manager.overage-entitlement-allows-consumption-past-max-count`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-licensemanager-awslicensemanager/src/ops/create_license.rs`. Replaying the corpus, the emulator refused the `CreateLicense` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateLicense operation: operation CreateLicense is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the license-manager emulator
Call CreateLicense and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
