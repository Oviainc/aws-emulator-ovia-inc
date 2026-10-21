Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Global Accelerator emulator

_Filed: 2026-10-21T21:09:10Z_
_Operation: service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 55 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL, stateful slice covering the lifecycle of the primary resource (Accelerator) and its child (Listener). Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: GlobalAccelerator_V20180706.<OperationName>` (OPERATIONS.md "Wire protocol").

### Happy path to implement (in order)

1. **CreateAccelerator** — input `Name` (required), `IdempotencyToken` (required, @idempotencyToken), `IpAddressType` (DUAL_STACK|IPV4), `IpAddresses` (0..2), `Enabled`, `Tags`. Output `Accelerator`. Must MINT and PERSIST a new accelerator: generate `AcceleratorArn` (`arn:aws:globalaccelerator::<account>:accelerator/<uuid>`), a `DnsName` of the form `a<16-hex>.awsglobalaccelerator.com` (plus `DualStackDnsName` `a<16-hex>.dualstack.awsglobalaccelerator.com` when `IpAddressType=DUAL_STACK`), two allocated static IPs in `IpSets[0]` (`IpAddressFamily=IPv4`, `IpAddresses` len 2), `Status` (`IN_PROGRESS` then settling to `DEPLOYED` is acceptable; a stable `DEPLOYED` is fine), `Enabled` defaulting to true when omitted, and `CreatedTime`/`LastModifiedTime`. Repeating a call with the same `IdempotencyToken` must return the SAME accelerator, not a second one.
2. **DescribeAccelerator** — input `AcceleratorArn`. Must look the accelerator up in the store and return exactly what CreateAccelerator persisted (same ARN, Name, DnsName, IpSets, Enabled, timestamps). Unknown ARN → `AcceleratorNotFoundException` (HTTP 404).
3. **ListAccelerators** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Accelerators)`, input `MaxResults` (1..100), `NextToken`. Must return every persisted accelerator, honour `MaxResults` by truncating and emitting a `NextToken` that resumes exactly where the page ended, and omit `NextToken` on the last page. An unparseable/stale token → `InvalidNextTokenException` (HTTP 400).
4. **CreateListener** — input `AcceleratorArn` (required), `PortRanges` (1..10, required), `Protocol` (TCP|UDP, required), `IdempotencyToken` (required), `ClientAffinity` (default `NONE`). Output `Listener` with a minted `ListenerArn` derived from the parent accelerator ARN (`.../accelerator/<id>/listener/<id>`) and the stored `PortRanges`/`Protocol`/`ClientAffinity`. Unknown `AcceleratorArn` → `AcceleratorNotFoundException` (404); a `PortRange` with `FromPort > ToPort` or outside 1..65535 → `InvalidPortRangeException` (400).
5. **ListListeners** — input `AcceleratorArn` (required), `MaxResults`, `NextToken`. Must return the listeners actually attached to that accelerator (and only those), paginated as above. Unknown accelerator → `AcceleratorNotFoundException` (404).
6. **DeleteListener** then **UpdateAccelerator**(`Enabled=false`) then **DeleteAccelerator** — teardown. `DeleteListener` removes the listener from its parent's child list (unknown ARN → `ListenerNotFoundException`, 404). `UpdateAccelerator` mutates the stored `Name`/`IpAddressType`/`Enabled` in place and bumps `LastModifiedTime`, returning the UPDATED record. `DeleteAccelerator` must enforce the two real preconditions from the model: `AcceleratorNotDisabledException` (400) while `Enabled=true`, and `AssociatedListenerFoundException` (400) while any listener still exists; on success it removes the accelerator so a subsequent `DescribeAccelerator` returns `AcceleratorNotFoundException` (404) and `ListAccelerators` no longer lists it.

Every one of these must compute its response FROM PERSISTED STATE reached through `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back, no minting an ARN that is never stored — a create followed by a describe/list must return what was created, and a delete must be observable by the next caller.

## State Model

In `src/state.rs`, on the service state struct:

- `accelerators: BTreeMap<String /* AcceleratorArn */, Accelerator>` where
  `Accelerator { arn: String, name: String, ip_address_type: IpAddressType, ip_sets: Vec<IpSet { ip_address_family, ip_addresses: Vec<String> }>, dns_name: String, dual_stack_dns_name: Option<String>, enabled: bool, status: AcceleratorStatus, created_time: SystemTime, last_modified_time: SystemTime, tags: Vec<Tag>, listener_arns: Vec<String> }`
- `listeners: BTreeMap<String /* ListenerArn */, Listener>` where
  `Listener { arn: String, accelerator_arn: String, port_ranges: Vec<PortRange { from_port, to_port }>, protocol: Protocol, client_affinity: ClientAffinity }`
- `accelerator_attributes: BTreeMap<String /* AcceleratorArn */, AcceleratorAttributes { flow_logs_enabled: bool, flow_logs_s3_bucket: Option<String>, flow_logs_s3_prefix: Option<String> }>` (created with defaults alongside the accelerator; lets Describe/UpdateAcceleratorAttributes land next without a schema change)
- `idempotency: HashMap<String /* IdempotencyToken */, String /* minted ARN */>` so repeated creates are de-duplicated
- `next_ip: u32` (or similar counter) so allocated static IPs are deterministic and unique per accelerator

Insertion order must be preserved for list pagination (use the BTreeMap key order or a parallel `Vec<String>` of ARNs) so `NextToken` is stable across calls.

## Out of scope for this slice

All custom-routing operations, BYOIP (`ProvisionByoipCidr`/`AdvertiseByoipCidr`/...), cross-account attachments, endpoint groups/endpoints (`CreateEndpointGroup`, `AddEndpoints`, ...), and tagging stay at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) documents 56 operations for this model; `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 55 handler files (e.g. `src/ops/create_accelerator.rs`, `src/ops/describe_accelerator.rs`, `src/ops/list_accelerators.rs`, `src/ops/create_listener.rs`, `src/ops/list_listeners.rs`, `src/ops/delete_accelerator.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so the service holds zero state. Field names/requiredness/constraints above are taken from OPERATIONS.md (CreateAccelerator L51-55, CreateListener L87-91, DeleteAccelerator L93-97, DescribeAccelerator L147-151, ListAccelerators L201-206, ListListeners L277-282, UpdateAccelerator L320-324) and from the Smithy model shapes `Accelerator`, `Listener`, `IpSet` (smithy_query). The DNS-name conventions and the "accelerator must be disabled and have no listeners before deletion" preconditions are documented in the model's own shape/operation docs (`Accelerator$DnsName`, `Accelerator$DualStackDnsName`) and surfaced as the modeled `AcceleratorNotDisabledException` / `AssociatedListenerFoundException` errors on DeleteAccelerator. No tests exercise any of this: `tests/` contains only the generated `state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `globalaccelerator` at it.
2. CreateAccelerator with Name="bootstrap-acc", IdempotencyToken="tok-1", Enabled=true → expect 200 with an Accelerator carrying a minted AcceleratorArn, DnsName, two static IPs in IpSets[0], Status and CreatedTime (today: HTTP 501 NotImplemented).
3. Repeat the identical CreateAccelerator call with IdempotencyToken="tok-1" → expect the SAME AcceleratorArn back, not a second accelerator.
4. DescribeAccelerator with that AcceleratorArn → expect the identical Name/DnsName/IpSets/Enabled that create returned.
5. DescribeAccelerator with a bogus ARN → expect AcceleratorNotFoundException, HTTP 404.
6. ListAccelerators with no MaxResults → expect the accelerator present; then ListAccelerators with MaxResults=1 twice using the returned NextToken → expect the pages to partition the set with no repeats and no NextToken on the final page.
7. CreateListener with that AcceleratorArn, PortRanges=[{FromPort:80,ToPort:80}], Protocol=TCP, IdempotencyToken="tok-2" → expect 200 with a minted ListenerArn prefixed by the accelerator ARN and ClientAffinity defaulted to NONE.
8. CreateListener with PortRanges=[{FromPort:443,ToPort:80}] → expect InvalidPortRangeException, HTTP 400.
9. ListListeners for that AcceleratorArn → expect exactly the one listener just created, with its stored PortRanges and Protocol.
10. DeleteAccelerator while Enabled=true and the listener still exists → expect AcceleratorNotDisabledException / AssociatedListenerFoundException, HTTP 400 (not a success).
11. DeleteListener, then UpdateAccelerator with Enabled=false → expect the returned Accelerator to show Enabled=false and a bumped LastModifiedTime; DescribeAccelerator confirms the change persisted.
12. DeleteAccelerator → expect 200 with an empty body; then DescribeAccelerator returns AcceleratorNotFoundException (404) and ListAccelerators no longer includes it.
