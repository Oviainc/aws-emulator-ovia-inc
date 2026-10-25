Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Implement lightsail CreateCloudFormationStack: unimplemented scaffold stub

_Filed: 2026-10-24T01:05:05Z_
_Operation: CreateCloudFormationStack | Class: stub_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCloudFormationStack` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_cloud_formation_stack.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_cloud_formation_stack.rs`.

## Steps to Reproduce

1. Boot the lightsail emulator
2. Call CreateCloudFormationStack and observe HTTP 501 with no state persisted

## Resolution

Implemented in `src/ops/create_cloud_formation_stack.rs` over the store's own
`cloud_formation_stack_records`, together with its only reader,
`GetCloudFormationStackRecords` (its own stub issue). The call creates the AWS
CloudFormation stack for the export snapshot record the single instance entry
names and appends the one record that tracks it — `Started`, settled to
`Succeeded` by the clock — reporting the stack's ARN as `destinationInfo`.

EMULATOR-CHOSEN behaviour, recorded here so the deviation is discoverable: a
second `CreateCloudFormationStack` naming an export snapshot record whose
earlier stack is still `Started` is refused with the modelled
`InvalidInputException`. AWS states this only as guidance to the caller ("Wait
until after your new Amazon EC2 instance is created before running the create
cloud formation stack operation again with the same export snapshot record"),
not as a documented service-enforced rejection, so a request real Lightsail may
accept is refused here. Everything else the handler refuses comes from a
documented rule (one instance entry per call, an export snapshot record of this
account whose export has succeeded, a zone of that export's Region, an Amazon
EC2 instance type name, Amazon EC2's 16 KB user-data cap).
