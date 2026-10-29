Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# UpdateInstanceMetadataOptions rewrites the metadata configuration of an instance that is neither running nor stopped, the precondition its own modelled documentation states and two siblings enforce

_Filed: 2026-10-29T02:42:37Z_
_Operation: UpdateInstanceMetadataOptions | Class: validation_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own modelled documentation scopes it to two instance states: "Modifies the Amazon Lightsail instance metadata parameters on a running or stopped instance." The handler never looks at the instance's state. It resolves the record and writes the new selection straight away, so an instance in `pending` (still coming up) or `stopping` (on its way down) is reconfigured and answers 200.

This crate already owns that exact rule in two other places, both derived from the same sentence in their own modelled docs, and both refusing before the first write:

* `AttachDisk` refuses when the instance is neither RUNNING nor STOPPED, quoting its own doc ("it 'attaches a block storage disk to a running or stopped Lightsail instance'");
* `SetResourceAccessForBucket` refuses on the same pair, quoting its member's doc ("The instance must be in a running or stopped state.").

Beyond the missing refusal, accepting the call in a transient state also corrupts what the record then reports. `StoredInstance::apply_metadata_options` derives the pending window from `self.state == RUNNING` alone, so for a `pending` instance `applied_at_ms` is `None` and `StoredMetadataOptions::view` immediately reports `state: "applied"` — the metadata service of an instance that is not yet up is reported as already serving the new configuration, and the operation-log entry for the call is answered terminal (`applied_at_ms` is what `start_operation` is given as `settles_at_ms`). The documented `pending` → `applied` span that the operation's doc describes is skipped entirely.

## Evidence

The modelled sentence that states the precondition — smithy_query(query="doc", shape="UpdateInstanceMetadataOptions"):
```
Modifies the Amazon Lightsail instance metadata parameters on a running or stopped
instance. When you modify the parameters on a running instance, the GetInstance
or GetInstances API operation initially responds with a state of pending. After
the parameter modifications are successfully applied, the state changes to applied
```
(the same sentence is carried in the handler's own header — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_instance_metadata_options.rs:49-50: `//! Modifies the Amazon Lightsail instance metadata parameters on a running or stopped` / `//! instance.`)

The handler, which checks the hop limit and the IPv6 Region but never the state — src/ops/update_instance_metadata_options.rs:217-237:
```
        crate::state::with_state(&*self.engine, |store| {
            let instance = store
                .instance_mut(&instance_name)
                .ok_or_else(|| not_found(&instance_name))?;
            ...
            if selection.http_protocol_ipv6.is_some() {
                if let Some(message) =
                    crate::state::ipv6_metadata_region_refusal(&instance.location.region_name)
                {
                    return Err(invalid(message));
                }
            }
            // The record carries the new selection from here on; the instant the
            // clock owes the instance the applied options is what leaves the
            // options — and the log entry for them — non-terminal until then.
            let applied_at_ms = instance.apply_metadata_options(selection, now);
```
Its own comment at src/ops/update_instance_metadata_options.rs:188-194 enumerates what is left to check and does not include the instance's state: "What is left are the two rules the model cannot express: `httpPutResponseHopLimit` ... and `httpProtocolIpv6` configures an endpoint only the instances of one Region have".

Sibling 1, which DOES enforce it — src/ops/attach_disk.rs:253-264:
```
            // From this operation's own documentation: it "attaches a block
            // storage disk to a running or stopped Lightsail instance". An
            // instance still coming up or going down is neither, so there is
            // nothing to expose the disk to yet.
            if instance_state != crate::state::RUNNING && instance_state != crate::state::STOPPED
            {
                return Err(operation_failure(format!(
                    "The instance {instance_name} is {}: a disk can be attached only to a \
                     running or stopped instance.",
                    instance_state.name
                )));
            }
```
Sibling 2 — src/ops/set_resource_access_for_bucket.rs:226-235:
```
            // From the member's own documentation: "The instance must be in a
            // running or stopped state." An instance still coming up or going
            // down is neither.
            if instance_state != crate::state::RUNNING && instance_state != crate::state::STOPPED {
                return Err(invalid(format!(
                    "The instance {} is {}: bucket access can be set only for an instance \
                     in a running or stopped state.",
                    input.resource_name, instance_state.name
                )));
```

The reporting consequence — src/state.rs:5257-5260:
```
        let applied_at_ms = (self.state == RUNNING)
            .then(|| now_ms.saturating_add(STATE_TRANSITION_MILLIS));
        self.metadata_options.applied_at_ms = applied_at_ms;
        applied_at_ms
```
read with src/state.rs:5007-5016:
```
            state: Some(
                if self.applied_at_ms.is_some() {
                    INSTANCE_METADATA_STATE_PENDING
                } else {
                    INSTANCE_METADATA_STATE_APPLIED
                }
```
so a `pending` instance reports `applied` at once.

Test coverage: tests/all/instance_metadata_options.rs only ever drives a running instance (`running_instance`, lines 192-210, which `wait_for_state(emulator, name, "running")`) and a stopped one (`a_stopped_instance_has_nothing_left_to_put_in_place`, line 423-464). No case calls the operation against an instance in `pending` or `stopping`, so nothing pins the current acceptance.

## Steps to Reproduce

1. Call CreateInstances for "meta-pending" — the instance is stored `pending`, with the clock owing it `running` (src/ops/create_instances.rs).
2. WITHOUT waiting for it to settle, call UpdateInstanceMetadataOptions with instanceName="meta-pending" and httpTokens="required".
3. Observe HTTP 200: the selection was written although the instance is neither running nor stopped, which the operation's own modelled documentation scopes it to.
4. Call GetInstance for "meta-pending": metadataOptions.state is already "applied" (src/state.rs:5257-5260 with src/state.rs:5007-5016), and the operation-log entry for the update is terminal — the documented pending → applied span never happens.
5. For contrast, call AttachDisk or SetResourceAccessForBucket naming the same `pending` instance: both are refused, quoting the identical "running or stopped" rule (src/ops/attach_disk.rs:253-264, src/ops/set_resource_access_for_bucket.rs:226-235).
