Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: aa53e03837fde54d44dc5b021f911ec63548de2f

# Bootstrap the snow-device-management emulator

_Filed: 2026-10-21T22:22:58Z_
_Operation: service-com-amazonaws-snowdevicemanagement-snowdevicemanagement | Class: stub_

> _Note: this issue was identified against the code base at commit `aa53e03837fde54d44dc5b021f911ec63548de2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding no state. This issue defines the FIRST buildable slice: the device-inventory + task lifecycle loop, implemented statefully over the restJson1 wire protocol (sigv4 signing name `snow-device-management`).

Scope — exactly these 6 operations, in this order:

1. **ListDevices** — `GET /managed-devices`. Input: `jobId` (@httpQuery), `maxResults` (1..100, @httpQuery), `nextToken` (1..1024, pattern `[a-zA-Z0-9+/=]*`, @httpQuery). Output: `devices` (list<DeviceSummary>), `nextToken`. Must enumerate the managed devices held in state (the emulator seeds a small device inventory at startup, since the API models no device-creation operation), filter by `jobId` when given, honor `maxResults` and return/accept a real opaque `nextToken` that resumes where the previous page stopped.
2. **DescribeDevice** — `POST /managed-device/{managedDeviceId}/describe`. Input: `managedDeviceId` (@httpLabel, len 1..64, required). Output: `managedDeviceId`, `managedDeviceArn`, `deviceType`, `associatedWithJob`, `deviceState` (LOCKED|UNLOCKED|UNLOCKING), `physicalNetworkInterfaces`, `deviceCapacities`, `software`, `tags`, `lastReachedOutAt`, `lastUpdatedAt`. Every field must come from the stored device record — the ARN and ID must be the same ones ListDevices returned. An unknown id must return `ResourceNotFoundException` (HTTP 404), not an empty 200.
3. **CreateTask** — `POST /task`. Input: `targets` (list<string>, len 1..10, required), `command` (Command union: `unlock` or `reboot`, required), `description` (len 1..128), `tags`, `clientToken` (@idempotencyToken). Output: `taskId`, `taskArn`. Must mint a fresh task id, persist a Task record (state `IN_PROGRESS`, `createdAt`, targets, description, tags) AND one Execution record per target device, return `ResourceNotFoundException` when a target id is not a known device, and treat a repeated `clientToken` as idempotent (same `taskId`/`taskArn` returned, no second task created).
4. **DescribeTask** — `POST /task/{taskId}`. Input: `taskId` (@httpLabel, required). Output: `taskId`, `taskArn`, `targets`, `state` (CANCELED|COMPLETED|IN_PROGRESS), `createdAt`, `lastUpdatedAt`, `completedAt`, `description`, `tags`. Must echo back exactly what CreateTask persisted (targets and description included), and 404 for an unknown `taskId`.
5. **ListExecutions** — `GET /executions`. Input: `taskId` (@httpQuery, required), `state` (@httpQuery), `maxResults`, `nextToken`. Output: `executions` (list<ExecutionSummary> with `taskId`, `executionId`, `managedDeviceId`, `state`), `nextToken`. Must list the execution records CreateTask created for that task — one per target — filtered by `state` when supplied, paginated like ListDevices.
6. **CancelTask** — `POST /task/{taskId}/cancel`. Input: `taskId` (@httpLabel, required). Output: `taskId`. Must transition the stored task to `CANCELED`, bump `lastUpdatedAt`, and transition its still-pending executions to `CANCELED`, so a following DescribeTask/ListExecutions observes the change. Unknown `taskId` → 404.

No canned or echoed responses: each handler must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. A `CreateTask` must be visible to a later `DescribeTask`, `ListTasks` and `ListExecutions` in the same process; a `CancelTask` must be visible as `CANCELED` afterwards. Hardcoded output, input echo, or minting an id without storing the record are all failures of this issue.

Out of scope for this first slice (stay 501, filed separately later): `ListTasks` beyond the loop above, `DescribeDeviceEc2Instances`, `ListDeviceResources`, `DescribeExecution`, `TagResource`, `UntagResource`, `ListTagsForResource`.

## State Model

A single service-state struct behind the engine lock, with:

- `devices: BTreeMap<String /*managedDeviceId*/, Device>` where `Device { managed_device_id: String, managed_device_arn: String, device_type: String, associated_with_job: String, device_state: DeviceState /*LOCKED|UNLOCKED|UNLOCKING*/, physical_network_interfaces: Vec<PhysicalNetworkInterface>, device_capacities: Vec<Capacity>, software: SoftwareInformation, tags: BTreeMap<String,String>, last_reached_out_at: DateTime, last_updated_at: DateTime }` — seeded with 2-3 devices at startup (ids and ARNs stable for the process).
- `tasks: BTreeMap<String /*taskId*/, Task>` where `Task { task_id: String, task_arn: String, targets: Vec<String>, command: Command, description: Option<String>, state: TaskState /*IN_PROGRESS|COMPLETED|CANCELED*/, tags: BTreeMap<String,String>, created_at: DateTime, last_updated_at: DateTime, completed_at: Option<DateTime>, client_token: Option<String> }`.
- `executions: Vec<Execution>` (or `BTreeMap<(taskId, managedDeviceId), Execution>`) where `Execution { task_id: String, execution_id: String, managed_device_id: String, state: ExecutionState /*QUEUED|IN_PROGRESS|SUCCEEDED|FAILED|CANCELED|REJECTED|TIMED_OUT*/, started_at: Option<DateTime>, last_updated_at: DateTime }`.
- `idempotency: BTreeMap<String /*clientToken*/, String /*taskId*/>` for CreateTask replay.
- `task_id_seq: u64` for deterministic id minting; ARNs formatted as `arn:aws:snow-device-management:<region>:<account>:task/<taskId>` and `.../managed-device/<managedDeviceId>`.

Done when a client script can run ListDevices → DescribeDevice → CreateTask → DescribeTask → ListExecutions → CancelTask → DescribeTask end-to-end with correct, stateful bodies (not merely 2xx).

## Evidence

Scaffold census: all 13 handler files under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and end in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))` — confirmed by reading `src/ops/create_task.rs` lines 4 and 173. Operation routes, required members, output members and error shapes are taken from the crate's generated `OPERATIONS.md` (ListDevices lines 56-62, DescribeDevice 48-54, CreateTask 80-85, DescribeTask 87-93, CancelTask 103-108, ListExecutions 118-124), which denormalizes `smithy-model/com-amazonaws-snowdevicemanagement-snowdevicemanagement.json` (restJson1, sdkId `Snow Device Management`). The 404-on-unknown-id and idempotency-token behaviors are grounded in the modeled `ResourceNotFoundException` on each of these operations and the `@idempotencyToken` trait on `CreateTask.clientToken`; pagination is grounded in the `@paginated(inputToken=nextToken, outputToken=nextToken, items=...)` traits on ListDevices and ListExecutions. No tests exist for this crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK snow-device-management client at it.
2. Call ListDevices — expect a non-empty `devices` list of seeded managed devices with stable ids/ARNs (today: HTTP 501 NotImplemented).
3. Call DescribeDevice with a `managedDeviceId` from step 2 — expect the same id/ARN plus deviceType, deviceState and software from stored state; call it with `managedDeviceId="does-not-exist"` and expect ResourceNotFoundException (404).
4. Call CreateTask with targets=[the device id from step 2], command={unlock:{}}, description="bootstrap test", clientToken="tok-1" — expect a taskId and taskArn.
5. Call CreateTask again with the identical clientToken "tok-1" — expect the SAME taskId returned and no second task created.
6. Call DescribeTask with that taskId — expect state=IN_PROGRESS, targets and description exactly as submitted, and a createdAt timestamp.
7. Call ListExecutions with that taskId — expect one execution per target, each with the taskId, an executionId and the target managedDeviceId.
8. Call CancelTask with that taskId, then DescribeTask again — expect state=CANCELED with an advanced lastUpdatedAt, and ListExecutions to show the executions CANCELED.
9. Call DescribeTask with an unknown taskId — expect ResourceNotFoundException (404).
