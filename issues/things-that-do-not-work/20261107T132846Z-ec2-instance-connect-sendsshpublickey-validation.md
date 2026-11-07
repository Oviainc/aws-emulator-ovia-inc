Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: e4e3e5befb4453536c7145f8d5553fe1cd0770b8

# The InstanceOSUser pattern refusal is documented as happening before any instance is looked up, but the EC2 lookup hop runs first

_Filed: 2026-11-07T13:28:46Z_
_Operation: SendSSHPublicKey | Class: validation_

> _Note: this issue was identified against the code base at commit `e4e3e5befb4453536c7145f8d5553fe1cd0770b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's documented decision order makes the `InstanceOSUser` `@pattern` check step 1, and the comment directly above the check states that it is "Enforced BEFORE any state is read — a request is not a resource lookup until its own values are legal". The code does the opposite: the cross-service lookup of the instance (`crate::state::ec2::instance(..).await`, which itself issues `DescribeInstances` and, for the instance type, `DescribeInstanceTypes`) is awaited at line 211, BEFORE the pattern check at line 220. A request whose `InstanceOSUser` the model's pattern forbids therefore drives the full instance resolution into EC2 and is only then refused.

The same handler also documents the authorization gate as "taken BEFORE the instance is resolved, so a caller holding no grant learns nothing about what the account holds", while an inline comment eleven lines above the gate admits the opposite ordering ("Resolved before the decision below, because the instance's tags are part of the request's condition context"). Both statements cannot be true of this code; the stated decision order and the code's actual order have drifted apart, and the sibling serial-console handler carries the same mismatch (its comment "checked before the instance is looked up" sits below an instance lookup that already ran).

## Evidence

The stated order — `src/ops/send_ssh_public_key.rs:15-21`:
```
//! 1. `InstanceOSUser` satisfies the `@pattern` the model declares for it — the one
//!    constraint the framework cannot evaluate — else `InvalidArgsException`;
//! 2. the caller's IAM identity-based policies allow
//!    `ec2-instance-connect:SendSSHPublicKey` on the instance's ARN, else
//!    `AccessDeniedException` (HTTP 403) — the decision is taken BEFORE the instance is
//!    resolved, so a caller holding no grant learns nothing about what the account holds
```
and the comment on the check itself, `src/ops/send_ssh_public_key.rs:215-219`:
```
        // The one modelled constraint the framework's boundary validator cannot enforce:
        // `InstanceOSUser`'s `@pattern` ... Enforced BEFORE any state is read
        // — a request is not a resource lookup until its own values are legal — and refused
        // as the modelled `InvalidArgsException`, naming the member and the constraint it
        // broke.
```

The code that breaks it, `src/ops/send_ssh_public_key.rs:210-220`:
```
        let region = crate::state::request_region(ctx.parts());
        let owner_says = crate::state::ec2::instance(ctx, &region, &input.instance_id).await;
        ...
        if let Some(violation) = crate::state::instance_os_user_violation(&input.instance_os_user)
```
where that hop is the instance lookup itself (`src/state/ec2.rs:79-89` `pub async fn instance(..)` → `client.describe_instances().instance_ids(instance_id).send().await`, plus `src/state/ec2.rs:142` `instance_type_is_nitro(ctx, region, &instance_type).await`).

The same ordering claim is restated by the test suite, `tests/all/error_status_and_input_constraints.rs:11-14`:
```
//! 2. a value that breaks a constraint the model declares is refused as
//!    `InvalidArgsException`, naming the member and the constraint, before any instance is
//!    looked up — including `InstanceOSUser`'s pattern, which the framework's boundary
//!    validator cannot evaluate;
```
yet its rows only assert the response code (`:371-380`), never that no lookup happened, so nothing pins the real order.

The self-contradiction about the authorization gate, same file: `src/ops/send_ssh_public_key.rs:19-21` ("the decision is taken BEFORE the instance is resolved") versus `src/ops/send_ssh_public_key.rs:234-235`:
```
        // instance's tags are part of the request's condition context. Resolved before the
        // decision below, because the instance's tags are part of the request's condition context.
```
and the gate at `:253-259`. Sibling with the same mismatch: `src/ops/send_serial_console_ssh_public_key.rs:270-272` ("checked before the instance is looked up") above a lookup already performed at `:230`.

## Steps to Reproduce

1. Open `src/ops/send_ssh_public_key.rs` and read lines 210-230 in order: `crate::state::ec2::instance(..).await` (line 211) precedes `crate::state::instance_os_user_violation(..)` (line 220), under a comment claiming the check is "Enforced BEFORE any state is read".
2. Mount this service together with EC2 on one engine as `tests/all/ec2_owned_instances.rs:68-76` does, with tracing at debug level.
3. POST `/` with `X-Amz-Target: AWSEC2InstanceConnectService.SendSSHPublicKey` and body `{"InstanceId":"i-0abcdef012345678","InstanceOSUser":"ec2 user","SSHPublicKey":"<80+ char key>"}` — an OS user the model's pattern forbids (`tests/all/error_status_and_input_constraints.rs:439` pins it as refused).
4. Observed: the request is refused `InvalidArgsException`, but only after the handler has already issued `DescribeInstances` (and `DescribeInstanceTypes`) for the id, i.e. after the instance was looked up.
5. Expected per the handler's own decision order and the comment above the check: the illegal value is refused before any instance lookup is made. Either the order or the three statements asserting it have to change.
