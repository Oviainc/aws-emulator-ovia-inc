Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# CreateConnection's remoteAccount is stored write-only: never checked against the Environment's remoteIdentifierType and never part of idempotency

_Filed: 2026-10-24T12:45:29Z_
_Operation: CreateConnection | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnection$remoteAccount` (`RemoteAccountIdentifier`) is the partner-verifiable identity of the remote side of the connection. The handler copies it into `StoredConnection.remote_account` and nothing in the crate ever reads that field again, so the member has NO observable effect on any response or on later state:

1. It is not validated against the Environment that dictates it. `Environment$remoteIdentifierType` is documented as "the type of identifying information that should be supplied to the `remoteAccount` parameter of a `CreateConnection` call for this specific Environment" (enum `account` | `email`), and the seeded catalog does set it (src/state.rs: `account` for the Azure/GoogleCloud environments, `email` for the Lumen/Megaport ones). A CreateConnection that omits `remoteAccount` entirely, or supplies an e-mail-shaped identifier on an environment whose `remoteIdentifierType` is `account` (or vice versa), is accepted with HTTP 200 and a `requested` connection. On the real service the environment declares the identifier type precisely because the create is held to it, so the proposal can be verified by the partner; here the member is free-form and unchecked.
2. It is excluded from the `@idempotencyToken` replay comparison. `IdempotentCreate::matches` compares environmentId, bandwidth, attachPoint, description and tags only, so a retry carrying the SAME `clientToken` but a DIFFERENT `remoteAccount` is replayed as the first connection, while a differing description or tag set is refused as a token/parameter mismatch. The one member identifying the remote party can therefore be silently swapped on a retry.

Expected: `remoteAccount` participates in the create — rejected (modelled client/validation error) when it does not match the Environment's declared `remoteIdentifierType`, and treated like every other create parameter when a `clientToken` is replayed. Actual: accepted unconditionally, stored where no code path reads it, and invisible to the idempotency check.

## Evidence

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs line 343 (`remote_account: input.remote_account.clone()` — the only use of the member) and lines 177-206 (the idempotency replay calls `created.matches(&input.environment_id, &input.bandwidth, &input.attach_point, input.description.as_ref(), input.tags.as_ref())` — `remote_account` is not an argument). A repo-wide grep for `remote_account` finds exactly three hits: the struct field (src/state.rs line 106), the deserialized input member (src/ops/types/create_connection_types.rs line 21) and that single write — no reader.

Model (smithy_query): CreateConnection input `remoteAccount` -> RemoteAccountIdentifier union; `Environment$remoteIdentifierType` -> RemoteAccountIdentifierType enum [account, email] documented as the type that should be supplied to `remoteAccount` for that Environment. `Connection` has no `remoteAccount` member, so the stored value is unobservable by design — the only way the input can matter is the create-time check, which is absent.

Test coverage: none. tests/all/emulator.rs line 152 always sends `remoteAccount: {identifier: EMULATOR_ACCOUNT_ID}`; no test varies it, omits it, or asserts any consequence (grep for remoteAccount/remoteIdentifierType over tests/ returns that one line). Distinct from the open CreateConnection issue 20261024T004734Z-interconnect-createconnection-validation, which is about the unchecked `attachPoint`.

## Steps to Reproduce

1. Boot the emulator and ListEnvironments; pick an environment whose `remoteIdentifierType` is "account" (e.g. the Azure-provider seed) and one of its available bandwidths.
2. CreateConnection on that environment with NO `remoteAccount` member at all -> observe HTTP 200 and a stored `requested` connection; the environment's declared identifier requirement is never enforced.
3. CreateConnection on the same environment with `remoteAccount: {identifier: "someone@example.com"}` (allowed by the member's `^[-a-zA-Z0-9_@\.]+$` pattern but not an account identifier) -> observe HTTP 200; no modelled refusal.
4. Idempotency: CreateConnection with clientToken="t1" and `remoteAccount: {identifier: "111111111111"}`, then repeat with clientToken="t1" and `remoteAccount: {identifier: "222222222222"}` (everything else identical) -> observe the first connection replayed with HTTP 200, whereas the same retry with a changed `description` is refused with InterconnectValidationException.
5. GetConnection / ListConnections on the created id -> no response member reports the remote account, so nothing the caller sent in `remoteAccount` is ever observable.
