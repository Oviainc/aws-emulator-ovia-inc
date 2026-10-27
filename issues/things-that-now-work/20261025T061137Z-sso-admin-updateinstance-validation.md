Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Refuse UpdateInstance while the instance is CREATE_IN_PROGRESS

_Filed: 2026-10-25T06:11:37Z_
_Operation: UpdateInstance | Class: validation_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states, in `DescribeInstance`'s own documentation, that only two operations may be called against an instance that is still being created: "While the instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance operations."

This crate deliberately makes that state REAL and observable: `CreateInstance` stores the new instance as `CREATE_IN_PROGRESS` with its activation owed one `INSTANCE_ACTIVATION_SPAN` later, and `state.rs` documents the span as "never zero, so a caller that reads the instance straight after creating it still observes the transient status the model declares". So there is a genuine window in which an instance of this emulator carries `Status: CREATE_IN_PROGRESS`.

`UpdateInstance`'s handler never asks what status the instance it resolved is in. It resolves the record with `store.instance_mut(...)` and then writes `Name`, `PermissionSetsEnabled` and/or `EncryptionConfiguration` straight onto it, answering 200. A caller can therefore enable permission sets on — or re-key the encryption of — an instance whose creation has not finished, which is exactly the call the quoted sentence says is not available yet; the next `DescribeInstance` reports `Status: CREATE_IN_PROGRESS` together with the mutation that should not have been accepted.

The handler already performs three request-level precondition checks of its own in this same `handle` body (the Encryption/PermissionSetsEnabled exclusivity, the `PermissionSetsEnabled == false` refusal, and the `KeyType`/`KmsKeyArn` pairing), each returning the modelled `ValidationException`, so the missing check is the state-of-the-resource one, not a missing error channel: `UpdateInstance` models `ValidationException`, `ConflictException` and `ResourceNotFoundException`.

(The same unguarded window exists on the other writes reachable during it — `CreateApplication`, `CreateTrustedTokenIssuer`, `TagResource` — which resolve the instance and write without consulting its status either; this issue is scoped to `UpdateInstance`, the write a client is most likely to make immediately after `CreateInstance`.)

## Evidence

Smithy model — `DescribeInstance` operation documentation (quoted verbatim in the crate's own handler header at services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_instance.rs:64-70):

    //! Returns the details of an instance of IAM Identity Center. The status can be one of the
    //! following: CREATE_IN_PROGRESS - The instance is in the process of being created. When
    //! the instance is ready for use, DescribeInstance returns the status of ACTIVE. While the
    //! instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and
    //! DeleteInstance operations.

The state is real and observable — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:119-127:

    /// How long after `CreateInstance` the new instance's move from `CREATE_IN_PROGRESS` to
    /// `ACTIVE` falls due.
    /// … it is never zero, so a caller that reads the instance straight after creating it still
    /// observes the transient status the model declares …
    pub const INSTANCE_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

and services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:1150 (`Instance::creating`):

    instance.status = STATUS_CREATE_IN_PROGRESS.to_string();

The handler does the opposite of the quoted sentence — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/update_instance.rs:281-302:

    let instance = store.instance_mut(&input.instance_arn).ok_or_else(|| { … })?;

    // Only the members the request CARRIES are written: an absent member leaves the
    // stored value alone, …
    if let Some(name) = input.name {
        instance.name = Some(name);
    }
    if input.permission_sets_enabled == Some(true) {
        // "Enables permission sets for this Identity Center instance" …
        instance.permission_sets_enabled = true;
    }

No status is consulted anywhere in the handler: `grep -n "CREATE_IN_PROGRESS\|is_active\|status ==" src/ops/` matches only `state.rs`, `create_instance.rs`'s and `describe_instance.rs`'s doc headers, and `shared_types.rs`'s enum value list — no handler branches on an instance's `Status`.

Test coverage today: services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/instance_create.rs:248-255 holds the activation instant open (`hold_activation`) and asserts the instance reads back `CREATE_IN_PROGRESS`, but only ever calls `DescribeInstance` inside that window; services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/instance_update.rs exercises `UpdateInstance` against an already-ACTIVE instance. Nothing pins that a write during `CREATE_IN_PROGRESS` must succeed, so no existing test states a reason for the current behaviour.

## Steps to Reproduce

1. Point an awsJson1_1 client at the emulator with an account that holds no instance (the state a completed DeleteInstance leaves, as `tests/all/instance_create.rs::empty_account_router` sets up).
2. Call `CreateInstance` with `{"Name":"Mine"}` and capture `InstanceArn` from the 200 response.
3. Within `state::INSTANCE_ACTIVATION_SPAN` of that call (or with the activation instant held open, as the existing test's `hold_activation` helper does), call `DescribeInstance` with that ARN and observe `"Status": "CREATE_IN_PROGRESS"`.
4. Still inside that window, call `UpdateInstance` with `{"InstanceArn": <arn>, "PermissionSetsEnabled": true}` (or `{"InstanceArn": <arn>, "Name": "Renamed"}`).
5. Expected per the quoted model sentence: the request is refused (the modelled `ValidationException`/`ConflictException` channel this operation declares), because only `DescribeInstance` and `DeleteInstance` may be called while the instance is `CREATE_IN_PROGRESS`.
6. Actual: HTTP 200 with an empty body, and a following `DescribeInstance` reports `"Status": "CREATE_IN_PROGRESS"` together with `"PermissionSetsEnabled": true` (or the new `Name`) — a mutation accepted on an instance whose creation had not finished.
