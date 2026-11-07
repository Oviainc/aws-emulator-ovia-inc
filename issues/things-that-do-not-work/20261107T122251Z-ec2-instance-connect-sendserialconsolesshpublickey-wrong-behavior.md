Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# An instance type EC2 reports no hypervisor for is refused as not serial-console capable instead of leaving the capability unestablished

_Filed: 2026-11-07T12:22:51Z_
_Operation: SendSerialConsoleSSHPublicKey | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::ec2::instance` resolves an instance's serial-console capability with `instance_type_is_nitro(..).await.unwrap_or(false)`. That helper is documented to answer `None` when EC2 did not establish the capability, but `unwrap_or(false)` converts "not established" into the positive fact "this instance type does not support the serial console", and the handler then refuses the push with the modelled `EC2InstanceTypeInvalidException` ("Only Nitro instance types are currently supported").

This contradicts how the very same function treats every OTHER fact it cannot establish, and how the handler treats the account-level grant it cannot establish:

* an unreadable lifecycle state yields `InstanceLookup::Unanswered` with the explicit comment that a value it cannot read "establishes nothing rather than standing in for a state EC2 did not name";
* an unanswered serial-console account status falls back to this service's own stored toggle (`owner_access.unwrap_or(state.serial_console_access_enabled)`), not to a literal refusal;
* the module doc states the fallback rule as reading this service's own registry "rather than inventing an answer here".

The `None` branch is reachable for a RUNNING instance Amazon EC2 holds: EC2's instance-type catalog in this repo deliberately reports NO `hypervisor` for a bare-metal type, while recording that same type as Nitro. So a serial-console push at a running `m5.metal` instance EC2 launched is answered 400 `EC2InstanceTypeInvalidException` even though the type is Nitro by EC2's own record, and no key is stored.

## Evidence

The documented rule, `services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/src/state/ec2.rs:173-175`:
```
/// Whether `instance_type` runs on the AWS Nitro System, as EC2's instance-type catalog
/// reports it (`InstanceTypeInfo$Hypervisor`); `None` when EC2 did not answer, so the
/// capability is not established.
```
and the module doc, `src/state/ec2.rs:15-20`: "Only when EC2 cannot be asked AT ALL ... is nothing known ([`InstanceLookup::Unanswered`]), and the caller falls back to this service's own registry ([`super::seed_instances`]) rather than inventing an answer here."

The code that invents it, `src/state/ec2.rs:142-144`:
```
    let serial_console_supported = instance_type_is_nitro(ctx, region, &instance_type)
        .await
        .unwrap_or(false);
```
versus the sibling treatment of an unreadable fact ten lines above, `src/state/ec2.rs:122-136`:
```
    // The lifecycle state decides every other answer, so a value this service cannot read
    // establishes nothing rather than standing in for a state EC2 did not name.
    let Some(state) = reported ... else { ... return InstanceLookup::Unanswered; };
```
and the sibling treatment in the handler itself, `src/ops/send_serial_console_ssh_public_key.rs:278`:
```
        if !owner_access.unwrap_or(state.serial_console_access_enabled) {
```

What the invented `false` causes, `src/ops/send_serial_console_ssh_public_key.rs:337-347`:
```
        if !instance.serial_console_supported {
            return Err(
                SendSerialConsoleSSHPublicKeyErrorModeled::EC2InstanceTypeInvalidException(
                    EC2InstanceTypeInvalidException { message: Some(format!(
                        "Instance type {} of instance {} is not supported for connecting \
                         via the serial console. Only Nitro instance types are currently \
                         supported.", ...
```
The model states that error's trigger (smithy_query, `SendSerialConsoleSSHPublicKey` errors): "The instance type is not supported for connecting via the serial console. Only Nitro instance types are currently supported."

Proof the `None` is reachable for a running instance EC2 holds — `services/service-com-amazonaws-ec2-amazonec2/src/state/instance_ids_and_instance_types.rs:2017-2019`:
```
        // hypervisor under it, so it reports none at all.
        hypervisor: (!record.bare_metal)
            .then(|| if nitro { "nitro" } else { "xen" }.to_string()),
```
with `:1732-1735` recording both facts for the same type (`bare_metal: name.bare_metal(), ... nitro: name.nitro(),`) and `:970-972` (`pub fn bare_metal(&self) -> bool { self.size == "metal" || self.size.starts_with("metal-") }`); `nitro()` at `:1012-1024` is true for `m5.metal` via `FIRST_NITRO_GENERATION`. `instance_type_is_nitro` requires the hypervisor to decide: `src/state/ec2.rs:197` `.and_then(|info| info.hypervisor())?`.

Test coverage: `tests/all/ec2_owned_instances.rs:390` (`a_serial_console_push_reads_the_account_status_and_the_instance_type_from_ec2`) exercises only `NITRO_INSTANCE_TYPE = "m5.large"` and `PRE_NITRO_INSTANCE_TYPE = "m3.medium"` (`:46-49`); no row launches a bare-metal type, and no comment anywhere in the crate justifies refusing a capability EC2 did not report.

## Steps to Reproduce

1. Mount both `service-com-amazonaws-ec2-amazonec2` and this service on one engine, as `tests/all/ec2_owned_instances.rs:68-76` does.
2. Resolve an AMI and call EC2 `RunInstances` with `InstanceType=m5.metal`, MinCount/MaxCount 1; note the instance id.
3. Sleep past `ec2::state::TRANSITION_DELAY` so EC2 reports the instance `running`.
4. Call EC2 `EnableSerialConsoleAccess` so the account grant is in place.
5. POST `/` with `X-Amz-Target: AWSEC2InstanceConnectService.SendSerialConsoleSSHPublicKey` and body `{"InstanceId":"<id>","SerialPort":0,"SSHPublicKey":"<80+ char key>"}`.
6. Observed: HTTP 400 `EC2InstanceTypeInvalidException` and no key in `serial_console_keys`, because EC2 reports no `hypervisor` for a bare-metal type and `unwrap_or(false)` reads that silence as "not supported" — while EC2's own record for that same type says `nitro: true`.
