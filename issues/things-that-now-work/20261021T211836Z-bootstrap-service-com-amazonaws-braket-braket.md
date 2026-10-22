Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Braket emulator

_Filed: 2026-10-21T21:18:36Z_
_Operation: service-com-amazonaws-braket-braket | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-braket-braket/src/ops/*.rs` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds zero state. This issue defines the FIRST buildable slice: the quantum-task happy path, the core loop of Amazon Braket (restJson1, sigv4 name `braket`).

### Happy path to implement (in order)

1. **SearchDevices** — `POST /devices`. Input: `filters` (list<SearchDevicesFilter>, required), `maxResults`, `nextToken`. Output: `devices` (list<DeviceSummary>, required), `nextToken`. Must return the emulator's seeded device inventory from state, honoring `filters` (e.g. `deviceStatus`, `deviceArn`, `providerName`) and `maxResults`/`nextToken` paging — not a fixed canned list independent of the filter.
2. **GetDevice** — `GET /device/{deviceArn}`. Output: `deviceArn`, `deviceName`, `providerName`, `deviceType` (QPU|SIMULATOR), `deviceStatus` (OFFLINE|ONLINE|RETIRED), `deviceCapabilities` (JSON string), optional `deviceQueueInfo`. Must look the ARN up in the seeded device registry and return `ResourceNotFoundException` (404) for an unknown ARN.
3. **CreateQuantumTask** — `POST /quantum-task` → 201. Input: `clientToken` (required, @idempotencyToken, 1..64), `deviceArn` (required), `shots` (required), `outputS3Bucket` (required), `outputS3KeyPrefix` (required), `action` (required JSON string), optional `deviceParameters`, `tags`, `jobToken`, `associations`. Output: `quantumTaskArn` (required). Must mint a real ARN (`arn:aws:braket:<region>:<account>:quantum-task/<uuid>`), store a record with every input field plus `status`, `createdAt`, and `outputS3Directory` derived from `outputS3Bucket`/`outputS3KeyPrefix`; reject an unknown `deviceArn` with `ValidationException`/`ResourceNotFoundException`, an OFFLINE device with `DeviceOfflineException` (424) and a RETIRED device with `DeviceRetiredException` (410). Repeating the same `clientToken` with the same body must return the SAME `quantumTaskArn` (idempotency), not a second task.
4. **GetQuantumTask** — `GET /quantum-task/{quantumTaskArn}`. Output must be reconstructed from the stored record: `quantumTaskArn`, `status` (CREATED|QUEUED|RUNNING|COMPLETED|FAILED|CANCELLING|CANCELLED), `deviceArn`, `deviceParameters`, `shots`, `outputS3Bucket`, `outputS3Directory`, `createdAt`, plus `endedAt`/`failureReason`/`tags`/`jobArn` when set. Unknown ARN → `ResourceNotFoundException` (404). A simple deterministic lifecycle advance (e.g. CREATED → QUEUED → COMPLETED on read-after-create, or on a monotonic clock) is acceptable provided it is persisted and observable, never a hardcoded status string.
5. **SearchQuantumTasks** — `POST /quantum-tasks`. Input `filters` (required), `maxResults`, `nextToken`; output `quantumTasks` (list<QuantumTaskSummary>, required) + `nextToken`. Must enumerate the stored tasks, apply the filters (`status`, `deviceArn`, `createdAt` ranges) and page with a real opaque token so a task created in step 3 is findable and a deleted/absent one is not.
6. **CancelQuantumTask** — `PUT /quantum-task/{quantumTaskArn}/cancel`. Input `quantumTaskArn` + `clientToken` (required). Output `quantumTaskArn` + `cancellationStatus` (CANCELLING|CANCELLED). Must mutate the stored record's status so a following `GetQuantumTask` reports CANCELLING/CANCELLED with `endedAt` set; cancelling an already-terminal (COMPLETED/FAILED) task must fail with `ConflictException` (409), and an unknown ARN with `ResourceNotFoundException` (404).

Every one of these six must compute its response from persisted state held in the service store reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back, no minting an ARN that is never stored.

## State Model

```text
BraketState {
    region: String,
    account_id: String,

    // Seeded at construction; devices are NOT created by the API.
    devices: Map<device_arn -> Device {
        device_arn: String,
        device_name: String,
        provider_name: String,
        device_type: DeviceType,      // QPU | SIMULATOR
        device_status: DeviceStatus,   // OFFLINE | ONLINE | RETIRED
        device_capabilities: String,   // JSON blob
        queue_depth: Option<DeviceQueueInfo>,
    }>,

    quantum_tasks: Map<quantum_task_arn -> QuantumTask {
        quantum_task_arn: String,
        status: QuantumTaskStatus,     // CREATED|QUEUED|RUNNING|COMPLETED|FAILED|CANCELLING|CANCELLED
        device_arn: String,
        device_parameters: String,     // defaulted to "{}" when omitted
        action: String,
        shots: i64,
        output_s3_bucket: String,
        output_s3_key_prefix: String,
        output_s3_directory: String,   // "<key_prefix>/<task-id>"
        created_at: Timestamp,
        ended_at: Option<Timestamp>,
        failure_reason: Option<String>,
        job_arn: Option<String>,
        job_token: Option<String>,
        associations: Vec<Association>,
        tags: Map<String,String>,
    }>,

    // clientToken -> already-minted ARN, for @idempotencyToken replay
    idempotency: Map<client_token -> String>,

    // opaque search token -> (offset, filter fingerprint)
    page_tokens: Map<token -> PageCursor { offset: usize, fingerprint: u64 }>,
}
```

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): CreateJob, GetJob, CancelJob, SearchJobs, CreateSpendingLimit, UpdateSpendingLimit, DeleteSpendingLimit, SearchSpendingLimits, TagResource, UntagResource, ListTagsForResource.

## Evidence

`services/service-com-amazonaws-braket-braket/OPERATIONS.md` lists all 17 operations with their routes, input/output members and modeled errors — the six above are quoted from it (CreateQuantumTask `POST /quantum-task -> 201` with required clientToken/deviceArn/shots/outputS3Bucket/outputS3KeyPrefix/action and output quantumTaskArn; GetQuantumTask `GET /quantum-task/{quantumTaskArn}`; SearchQuantumTasks `POST /quantum-tasks` @paginated(items=quantumTasks); CancelQuantumTask `PUT /quantum-task/{quantumTaskArn}/cancel` with ConflictException 409; GetDevice `GET /device/{deviceArn}`; SearchDevices `POST /devices` @paginated(items=devices)). `src/ops/create_quantum_task.rs` lines 3-12 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 183 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateQuantumTask"))`; every other file in `src/ops/` is the same generated stub, so no operation holds or reads state today and no test asserts any behavior.

## Steps to Reproduce

1. Start the emulator and point a Braket client at it (restJson1, signing name `braket`).
2. `SearchDevices` with filters `[{name: deviceStatus, operator: EQUAL, values: [ONLINE]}]` — expect a non-empty `devices` list from the seeded inventory; today: HTTP 501 NotImplemented.
3. `GetDevice` with one `deviceArn` from that list — expect deviceName/providerName/deviceType/deviceStatus/deviceCapabilities matching the seeded device; `GetDevice` on a bogus ARN must be 404 ResourceNotFoundException.
4. `CreateQuantumTask(clientToken="tok-1", deviceArn=<that ARN>, shots=100, action="{...}", outputS3Bucket="my-bucket", outputS3KeyPrefix="tasks")` — expect 201 and a real `quantumTaskArn`.
5. Repeat the identical CreateQuantumTask call with clientToken `tok-1` — expect the SAME quantumTaskArn and no second task.
6. `GetQuantumTask(quantumTaskArn)` — expect deviceArn, shots=100, outputS3Bucket="my-bucket", outputS3Directory starting with "tasks/", a createdAt timestamp and a status from the modeled enum, all matching what was created.
7. `SearchQuantumTasks` with filter `deviceArn EQUAL <that ARN>` — expect the new task in `quantumTasks`; with `maxResults=1` and two tasks present, expect a `nextToken` that returns the second task on the follow-up call.
8. `CancelQuantumTask(quantumTaskArn, clientToken="tok-2")` — expect cancellationStatus CANCELLING or CANCELLED, then `GetQuantumTask` must report that status with `endedAt` set; a second cancel of a terminal task must return 409 ConflictException, and cancelling an unknown ARN 404 ResourceNotFoundException.
