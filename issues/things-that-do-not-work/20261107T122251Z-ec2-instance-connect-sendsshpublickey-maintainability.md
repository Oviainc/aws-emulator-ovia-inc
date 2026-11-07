Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# The instance-resolution fallback rule is implemented twice, once per handler

_Filed: 2026-11-07T12:22:51Z_
_Operation: SendSSHPublicKey | Class: maintainability_

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides WHICH record a push is served against — EC2's answer wins, an EC2 that answered "absent" means not found, and this service's own registry is read only when EC2 could not be asked at all — is written out verbatim in both handlers. `crate::state::ec2` owns the `InstanceLookup` enum but no helper that applies the fallback, so each handler re-derives it (including the `store.lock()` read of the fallback registry). The two copies can drift: a change to the fallback rule (for example which lookup outcome may consult the local registry, or whether the registry read should be guarded) has to be made twice, and one handler silently keeping the old rule would answer a different error for the same instance id. Folding the five lines into one helper on `crate::state::ec2` leaves every response and every existing test unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/src/ops/send_ssh_public_key.rs:236-242`:
```
        let found = match owner_says {
            crate::state::ec2::InstanceLookup::Owned(instance) => Some(instance),
            crate::state::ec2::InstanceLookup::Absent => None,
            crate::state::ec2::InstanceLookup::Unanswered => {
                store.lock().instance(&input.instance_id).cloned()
            }
        };
```
Copy 2 — `src/ops/send_serial_console_ssh_public_key.rs:236-242`, character for character the same:
```
        let found = match owner_says {
            crate::state::ec2::InstanceLookup::Owned(instance) => Some(instance),
            crate::state::ec2::InstanceLookup::Absent => None,
            crate::state::ec2::InstanceLookup::Unanswered => {
                store.lock().instance(&input.instance_id).cloned()
            }
        };
```
The crate already centralises its other shared rule rather than copying it — `src/state.rs:279-305`, `ConnectBlocked` / `InstanceState::blocks_connect`: "Both operations model both outcomes, and each maps this to its own error variant, so the state-to-outcome rule lives in one place." The fallback rule is the same kind of rule and has no such home: `src/state/ec2.rs:24-33` defines `InstanceLookup` only.

Existing tests that pin both copies' behaviour, and would stay green: `tests/all/ec2_owned_instances.rs:263` (`a_push_is_decided_by_what_ec2_reports_about_the_instance_it_launched`), `:353` (`an_instance_ec2_does_not_hold_is_refused_as_not_found`, which asserts the seeded registry has no say when EC2 answers), `:390` (the serial-console row), and the EC2-less path in `tests/all/instance_connect_key_push.rs:785` (`the_seeded_registry_serves_a_push_with_no_test_setup`).

## Steps to Reproduce

1. Open `src/ops/send_ssh_public_key.rs` at lines 236-242 and `src/ops/send_serial_console_ssh_public_key.rs` at lines 236-242 side by side: the same `match owner_says` fallback rule, including the local-registry read, appears in both.
2. Grep the crate for a helper that applies the rule once (`rg "InstanceLookup::Unanswered" services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/src`): the only matches are the two handler copies and the enum definition in `src/state/ec2.rs`.
3. Change the rule in one handler only (e.g. stop consulting the local registry) and observe that the other operation keeps the old behaviour, with no test or type forcing the two to agree.
