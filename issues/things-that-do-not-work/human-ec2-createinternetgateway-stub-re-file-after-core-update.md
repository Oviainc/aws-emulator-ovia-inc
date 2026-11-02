# ec2 CreateInternetGateway stub re-file after core update

# Implement ec2 CreateInternetGateway: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInternetGateway

## Description
`CreateInternetGateway` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateInternetGateway` call, so each one unblocks only when `CreateInternetGateway` works: `autoscaling__ecs__elbv2.ecs-service-deregisters-tasks-on-scale-down`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs`. Replaying the corpus, the emulator refused the `CreateInternetGateway` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateInternetGateway operation: operation CreateInternetGateway is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateInternetGateway and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal


## Re-file note
This re-files `issues/things-that-do-not-work/20261022T202249Z-ec2-createinternetgateway-stub.md`, which was closed as wont_fix (vendor_blocker) on 2026-11-02 only because the merged tree did not compile after the 13:14 UTC core-components update. The previous attempt's change itself worked: `CreateInternetGateway` stored the gateway, `DescribeInternetGateways` read it back, `cargo test -p service-com-amazonaws-ec2-amazonec2` passed, and replaying this issue's unit showed `ec2|CreateInternetGateway|NotImplemented` fixed (the unit then fails on `AttachInternetGateway`, which has its own issue).

Keep the change to the ec2 crate. Do not repair other crates for the core update: a separate issue (`vendor-breaking-change-573d73e630d5`) owns that. If the merged tree still does not compile because of the core update, wait for that fix to land on mainline rather than patching other services.
