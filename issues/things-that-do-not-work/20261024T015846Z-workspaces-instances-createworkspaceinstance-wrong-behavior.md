Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# A reused ClientToken replays the first instance id even when the request body differs, instead of answering ConflictException

_Filed: 2026-10-24T01:58:46Z_
_Operation: CreateWorkspaceInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Idempotency is keyed on the ClientToken alone: `state.client_tokens` maps token -> instance id and the handler returns the stored id for any later call carrying that token, whatever the rest of the request says.

Expected: an idempotency token identifies one specific request. A retry carrying the same ClientToken but a different ManagedInstance / BillingConfiguration / Tags is a mismatched retry, which AWS refuses — here with the modelled ConflictException (HTTP 409, whose ResourceId/ResourceType members name the conflicting resource) rather than silently handing back an instance built from different parameters.

Actual: the second call returns 200 with the first call's WorkspaceInstanceId and the first call's stored configuration, so a caller that changed its request believes the new configuration was applied. ConflictException is unreachable in this handler — nothing in the file can construct it.

## Evidence

Spec: CreateWorkspaceInstance is @idempotent, ClientToken is @idempotencyToken, and ConflictException (HTTP 409, "Signals a conflict with the current state of the resource", required ResourceId/ResourceType) is a modelled error of this operation with no other path able to raise it. Mismatched-idempotency-token refusal is established AWS behaviour for client-token creates (EC2's IdempotentParameterMismatch, ConflictException on modern APIs). Handler: src/ops/create_workspace_instance.rs lines ~167-175 returns the stored id on token hit before looking at any other member. Tests only replay IDENTICAL bodies (tests/all/workspace_instance_lifecycle.rs `an_idempotency_token_replays_one_instance`), so the mismatch case is uncovered.

## Steps to Reproduce

1. Call CreateWorkspaceInstance with {"ClientToken":"tok-1","ManagedInstance":{"InstanceType":"t3.medium"},"Tags":[{"Key":"env","Value":"dev"}]} — note the id
2. Call CreateWorkspaceInstance again with {"ClientToken":"tok-1","ManagedInstance":{"InstanceType":"m5.large"},"BillingConfiguration":{"BillingMode":"MONTHLY"}}
3. Emulator answers 200 with the first id; GetWorkspaceInstance/ListTagsForResource show the FIRST request's configuration and tags, and no ConflictException (409) is ever produced
