Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# CreateGateway refuses every client-supplied ActivationKey: no over-the-wire path can register one

_Filed: 2026-10-24T08:44:27Z_
_Operation: CreateGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateGateway gates the create on an in-process activation-key registry that NO operation of the service (and no other wire-reachable endpoint) can ever populate, so every CreateGateway call made by a real client over the wire is refused with HTTP 400 ValidationException / ErrorCode `ActivationKeyNotFound`, and no gateway can ever exist in a running emulator.

Expected: a CreateGateway request carrying a well-formed ActivationKey (1..50, ^[0-9a-zA-Z\-]+$), a GatewayDisplayName and GatewayType=BACKUP_VM returns HTTP 200 with a generated GatewayArn, and the gateway is then visible to GetGateway/ListGateways — this is what a recorded AWS trace shows, because on real AWS the key is minted out of band by the appliance activation redirect and the API accepts it.

Actual: `handle` calls `store.activation_key_issued(&input.activation_key)` and returns ValidationException(ErrorCode=ActivationKeyNotFound) unless the key was previously inserted by the Rust-only helper `ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`, which is called from nothing but the crate's own integration tests. The emulator exposes no operation and no side channel to issue a key, so the refusal is unconditional for any HTTP client.

Consequence: the entire gateway resource is unreachable for clients — GetGateway, ListGateways (always empty), UpdateGatewayInformation, DeleteGateway, GetBandwidthRateLimitSchedule and gateway tagging (TagResource/UntagResource/ListTagsForResource) can never be exercised, because no gateway record can be created. Since the emulator cannot verify a key against a real appliance, a well-formed key should be accepted (optionally still refusing malformed keys, which the boundary validator already does).

## Evidence

Smithy model: CreateGateway input requires ActivationKey/GatewayDisplayName/GatewayType and declares only ValidationException, ThrottlingException and InternalServerException; output is GatewayArn (smithy_query operation=CreateGateway). The already-fixed bootstrap issue for this service specifies step 2 as `CreateGateway with ActivationKey="ACTIVATION-KEY123" ... -> expect 200 with a generated GatewayArn`; against the current code that same request returns 400 ValidationException.
Handler: src/ops/create_gateway.rs lines ~187-193 (`if !store.activation_key_issued(&input.activation_key) { return Err(... ACTIVATION_KEY_NOT_FOUND ...) }`).
State: src/state.rs `activation_keys: BTreeMap<String, SystemTime>` and `issue_activation_key` (line ~720); repo-wide grep for `issue_activation_key` finds callers ONLY in tests/all/gateway_lifecycle.rs, tests/all/negative_paths.rs — no handler and no wire endpoint writes the registry.
Tests: tests/all/negative_paths.rs::an_activation_key_the_service_never_issued_is_refused asserts the current refusal, and tests/all/gateway_lifecycle.rs seeds keys in-process via the engine handle, so no test exercises (or could exercise) a client-only CreateGateway; the gap is not covered by any test asserting real-service behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `backup-gateway` at it (awsJson1_0, POST / with X-Amz-Target: BackupOnPremises_v20210101.CreateGateway).
2. Call CreateGateway with ActivationKey="ACTIVATION-KEY123", GatewayDisplayName="my-gateway", GatewayType="BACKUP_VM".
3. Observe HTTP 400 with ValidationException and ErrorCode "ActivationKeyNotFound" instead of HTTP 200 with a generated GatewayArn.
4. Try any other well-formed key value (any string matching ^[0-9a-zA-Z\-]+$, length 1..50) — every one is refused the same way; there is no operation that can register a key first.
5. Call ListGateways — it returns an empty Gateways list, confirming no gateway can be created through the API, so GetGateway/UpdateGatewayInformation/DeleteGateway/GetBandwidthRateLimitSchedule/tagging of a gateway are all unreachable.
