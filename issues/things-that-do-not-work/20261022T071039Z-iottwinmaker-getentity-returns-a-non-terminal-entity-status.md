Issue-Source: trace-sweep-divergence
Issue-Trace: iottwinmaker-get-entity-not-found
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iottwinmaker-get-entity-not-found
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker|GetEntity|2xx->2xx|$.description
Issue-Key: iottwinmaker|GetEntity|2xx->2xx|$.status.error
Issue-Key: iottwinmaker|GetEntity|2xx->2xx|$.status.state
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# iottwinmaker GetEntity returns a non-terminal entity status and drops modeled response members

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEntity

## Description
GetEntity on an entity that exists in the emulator answers 200 but with a status/body shape that does not match the API: the entity is reported as still CREATING and two modeled response members are absent entirely.

Cause 1 of 1: GetEntity response for an existing entity (fixture iottwinmaker-get-entity-not-found, step task#7).

AWS: Smithy (services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model) models GetEntity as GET /workspaces/{workspaceId}/entities/{entityId} -> 200 GetEntityResponse, whose required member `status` targets the Status structure {state: State, error: ErrorDetails}. State is an enum with values CREATING | UPDATING | DELETING | ACTIVE | ERROR, i.e. CREATING is a transient provisioning state and ACTIVE is the terminal state of a usable entity. GetEntityResponse also models the optional `description` member (Description, @length 0..2048). The recorded response for an entity that has been created and read back carries status.state = "ACTIVE", a present (empty) `status.error` object, and a present (empty) `description` string. Recorded behavior only for the exact emptiness of those two members; the ACTIVE terminal state is what the model's State enum and the recording agree on.

Emulator: the same GET answers 200 with status.state = "CREATING" (expected "ACTIVE"), with `status.error` absent (expected an object; replay reports `$.status.error: missing (expected {}, got None)`) and with `description` absent (expected a string; `$.description: missing (expected '', got None)`).

Why: a client that creates an entity and then polls GetEntity waits for status.state to reach ACTIVE before using it. Against the emulator the state never leaves CREATING, so any create-then-wait workflow (SDK waiters, CDK/Terraform-style provisioners, test harnesses) spins until it times out rather than proceeding. Separately, SDKs and generated models that expect the Status structure and the description member to be materialized see absent fields where AWS supplies present-but-empty ones, so code that dereferences status.error or description without a null guard fails on the emulator but not on AWS.

Fix: the iottwinmaker GetEntity handler and the entity state it reads from — entity records must settle in the terminal ACTIVE state once creation completes (rather than being persisted/returned with the initial CREATING state), and the GetEntity serializer must emit the modeled `status.error` and `description` members for an entity whose values are empty instead of omitting them.

Traces: iottwinmaker-get-entity-not-found#task#7 (GetEntity, expected 200 / actual 200, diverging keys $.status.state, $.status.error, $.description).

## Evidence
Fixture iottwinmaker-get-entity-not-found, step task#7, operation GetEntity, service iottwinmaker.
Expected status 200, actual status 200.
Diverging keys reported by replay:
  $.status.state: value_mismatch (expected 'ACTIVE', got 'CREATING')
  $.status.error: missing (expected {}, got None)
  $.description: missing (expected '', got None)
Divergence key: service=iottwinmaker|GetEntity|status=200->200|keys=$.description,$.status.error,$.status.state
Model evidence: Smithy GetEntity -> GET /workspaces/{workspaceId}/entities/{entityId}, 200 GetEntityResponse; required member status -> Status{state: State, error: ErrorDetails}; State enum values CREATING, UPDATING, DELETING, ACTIVE, ERROR; optional member description -> Description (@length 0..2048).

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay wire fixture iottwinmaker-get-entity-not-found (trace set simparity-wire-aws-v1) and let it run through step task#7, which creates a workspace/entity and then issues GET https://<endpoint>/workspaces/<workspaceId>/entities/<entityId> (GetEntity).
3. Inspect the 200 body: status.state is "CREATING" where the recording has "ACTIVE", and the status.error and description members are absent where the recording has them present (empty object / empty string).
4. Equivalently with the SDK: iottwinmaker create-workspace + create-entity, then get-entity, and observe the state never reaches ACTIVE.
