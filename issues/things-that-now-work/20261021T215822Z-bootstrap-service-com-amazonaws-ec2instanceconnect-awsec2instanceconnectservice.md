Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the EC2 Instance Connect emulator

_Filed: 2026-10-21T21:58:22Z_
_Operation: service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Nothing in this service works today: `src/ops/send_ssh_public_key.rs` and `src/ops/send_serial_console_ssh_public_key.rs` both still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState` has no fields (`// TODO: add service-specific state fields`).

This service models exactly TWO operations (see OPERATIONS.md), so the bootstrap slice is the whole surface — but it must be STATEFUL, not two hollow `{"Success": true}` replies.

### Happy path to implement (in order)

1. **Seed/registry of EC2 instances** (prerequisite, not an operation): the emulator must be able to decide whether an instance exists, what AZ it is in, and whether it is running. Share or mirror the EC2 service's instance store so an instance created there is visible here; absent that, keep this service's own instance registry keyed by instance id.
2. **SendSSHPublicKey** — input `InstanceId` (required, len 10..32, pattern `^i-[a-f0-9]+$`), `InstanceOSUser` (required, len 1..32, pattern as modeled), `SSHPublicKey` (required, len 80..4096), `AvailabilityZone` (optional, len 6..32, pattern `^(\w+-){2,3}\d+\w+$`). Output `Success` (required, default false) and `RequestId`. On success: STORE the key against (InstanceId, InstanceOSUser) with a 60-second expiry — the model's own doc says "The key remains for 60 seconds" — generate a fresh unique `RequestId` per call, and return `Success: true`.
3. **SendSerialConsoleSSHPublicKey** — input `InstanceId` (required, same pattern), `SerialPort` (required, range 0..0, default 0), `SSHPublicKey` (required, len 80..4096). Output `Success` (required) and `RequestId`. On success: store the serial-console key against (InstanceId, SerialPort) with its own expiry and return `Success: true` with a fresh `RequestId`.
4. **Read-back / verification path**: a pushed key must be observable in state. A second push for the same (instance, user) replaces the prior key and refreshes the expiry; a key older than its TTL is gone. This is what makes the implementation stateful rather than canned — without it the two operations are indistinguishable from hardcoded `Success: true`.
5. **Modeled error paths** that fall out of the same state, not from input shape alone:
   - `EC2InstanceNotFoundException` (HTTP 404) when `InstanceId` is not in the instance registry.
   - `EC2InstanceStateInvalidException` (HTTP 400) when the instance exists but is stopped/terminated (the doc: "Connecting to a stopped or terminated instance is not supported").
   - `InvalidArgsException` (HTTP 400) for a well-formed-but-wrong argument the modeled constraints do not already refuse — notably an `AvailabilityZone` that does not match the AZ recorded for that instance.
   - `SerialConsoleAccessDisabledException` (HTTP 403) from `SendSerialConsoleSSHPublicKey` when serial-console access is not enabled for the account in state.

Every one of these responses must be computed from persisted state. Hardcoded `Success: true`, a constant `RequestId`, or a random id that is never stored are all failures of this issue — a push followed by a read-back of state must show the key that was pushed, and a push to an unknown instance must 404.

## State Model

Add to `ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState`:

- `instances: HashMap<String, Instance>` — keyed by instance id (`i-...`).
  - `Instance { instance_id: String, availability_zone: String, instance_type: String, state: InstanceState /* Pending | Running | Stopping | Stopped | Terminated */, serial_console_supported: bool }`
- `pushed_keys: HashMap<(String /*instance_id*/, String /*os_user*/), PushedKey>`
  - `PushedKey { public_key: String, os_user: String, pushed_at: Instant/SystemTime, expires_at: /* pushed_at + 60s */, request_id: String }`
- `serial_console_keys: HashMap<(String /*instance_id*/, i32 /*serial_port*/), PushedKey>` — same record, keyed by port (only port 0 is modeled valid).
- `serial_console_access_enabled: bool` — account-level toggle backing `SerialConsoleAccessDisabledException`; default it to `true` so the happy path works, and let the error branch read it.
- `requests: Vec<RequestRecord { request_id, operation, instance_id, at }>` (optional but cheap) — lets a test assert `RequestId` values are unique per call and that a call was actually recorded.

Expired entries must be treated as absent on read (lazy expiry at lookup time is fine — that materialises a change the clock already made due).

Wire format for both: `awsJson1_1`, `POST /` with `X-Amz-Target: AWSEC2InstanceConnectService.<OperationName>`, sigv4 signing name `ec2-instance-connect`.

Done when a client can push a key for a running instance and see `Success: true` with a unique `RequestId`, push to an unknown instance and get a 404 `EC2InstanceNotFoundException`, push to a stopped instance and get a 400 `EC2InstanceStateInvalidException`, and see the stored key expire after 60 seconds — all driven by state, with no 501 from either operation.

## Evidence

OPERATIONS.md (crate root) lists exactly 2 bound operations with their full contracts: `SendSSHPublicKey` — Input `InstanceId` (string, len 10..32, pattern ^i-[a-f0-9]+$, required), `InstanceOSUser` (required), `SSHPublicKey` (len 80..4096, required), `AvailabilityZone` (optional, pattern ^(\w+-){2,3}\d+\w+$); Output `RequestId` (string), `Success` (boolean, default false, required); Errors `AuthException (403)`, `EC2InstanceNotFoundException (404)`, `EC2InstanceStateInvalidException (400)`, `EC2InstanceUnavailableException (503)`, `InvalidArgsException (400)`, `ServiceException (500)`, `ThrottlingException (429)`. `SendSerialConsoleSSHPublicKey` — Input `InstanceId` (required), `SerialPort` (integer, range 0..0, default 0, required), `SSHPublicKey` (required); same Output; additionally declares `SerialConsoleAccessDisabledException (403)`, `SerialConsoleSessionLimitExceededException (400)`, `SerialConsoleSessionUnavailableException (500)`, `SerialConsoleSessionUnsupportedException (400)`, `EC2InstanceTypeInvalidException (400)`. Wire protocol from OPERATIONS.md: awsJson1_1, `X-Amz-Target: AWSEC2InstanceConnectService.<Op>`, sigv4 name `ec2-instance-connect`.

The 60-second key lifetime is the model's own operation doc, quoted in `src/ops/send_ssh_public_key.rs` lines 52-54: "Pushes an SSH public key to the specified EC2 instance for use by the specified user. The key remains for 60 seconds."

Stub proof: `src/ops/send_ssh_public_key.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 182 `Err(crate::scaffolded::Scaffolded::not_implemented("SendSSHPublicKey"))`; same shape in `src/ops/send_serial_console_ssh_public_key.rs`. `src/state.rs` line 8 is `// TODO: add service-specific state fields`, so no state exists to compute any response from. No tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Register (or seed) a running EC2 instance `i-0123456789abcdef` in availability zone `us-east-1a`.
2. POST / with `X-Amz-Target: AWSEC2InstanceConnectService.SendSSHPublicKey` and body {"InstanceId":"i-0123456789abcdef","InstanceOSUser":"ec2-user","SSHPublicKey":"<an 80+ char ssh-rsa key>","AvailabilityZone":"us-east-1a"} — expect HTTP 200 with {"Success":true,"RequestId":"<unique id>"}; today: HTTP 501 NotImplemented.
3. Inspect service state: the key must be stored against (i-0123456789abcdef, ec2-user) with a 60-second expiry; today nothing is stored because the state struct has no fields.
4. Repeat the call with a different key and confirm the stored key is REPLACED and the expiry refreshed, and that the returned `RequestId` differs from the first call's.
5. POST SendSSHPublicKey for `i-00000000deadbeef` (never registered) — expect HTTP 404 `EC2InstanceNotFoundException`; today: HTTP 501.
6. Stop the instance, then POST SendSSHPublicKey again — expect HTTP 400 `EC2InstanceStateInvalidException`; today: HTTP 501.
7. POST SendSSHPublicKey with `AvailabilityZone: "us-west-2b"` for the us-east-1a instance — expect HTTP 400 `InvalidArgsException`; today: HTTP 501.
8. POST / with `X-Amz-Target: AWSEC2InstanceConnectService.SendSerialConsoleSSHPublicKey` and body {"InstanceId":"i-0123456789abcdef","SerialPort":0,"SSHPublicKey":"<an 80+ char key>"} on a running instance — expect HTTP 200 {"Success":true,"RequestId":...} and the key stored under (instance, port 0); today: HTTP 501.
9. Set `serial_console_access_enabled = false` and repeat the serial-console call — expect HTTP 403 `SerialConsoleAccessDisabledException`; today: HTTP 501.
10. Advance time past 60 seconds and re-inspect state — the pushed key must be gone (treated as absent on lookup).
