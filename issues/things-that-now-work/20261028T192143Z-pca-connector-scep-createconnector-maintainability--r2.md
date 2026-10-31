Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# Replace the hand-rolled UUID version/variant and hyphenation span in new_uuid with the uuid crate

_Filed: 2026-10-28T19:21:42Z_
_Operation: CreateConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::new_uuid()` — the identifier mint every connector ARN, challenge ARN and challenge `Password` in this crate comes from (first called by `CreateConnector`'s handler) — derives 16 bytes from a SHA-256 digest and then hand-rolls the two things the `uuid` crate already does verbatim: setting the RFC 9562 version and variant nibbles, and rendering the 16 bytes as the canonical 8-4-4-4-12 hyphenated lowercase form built out of hex slices.

`uuid` 1.29.0 is already in this workspace's `Cargo.lock` and is used directly by sibling crates in this repo, and `::uuid::Builder::from_random_bytes(bytes)` performs exactly the two nibble writes quoted below before `Uuid::hyphenated().to_string()` renders exactly the same five-group lowercase string. The hand-rolled span therefore duplicates library logic that can drift from it (a bad slice index, a wrong mask) while buying nothing.

The fix is confined to the span: the randomness source stays the existing per-process seed + per-call `RandomState` + counter + clock SHA-256 digest, so the module's documented "no RNG crate and no entropy of this crate's own invention" property is untouched, every minted id is byte-for-byte what it is today, and no response or test changes.

## Evidence

Hand-rolled span — `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:888-901`:
```
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
```
Its own doc states the rule it is re-implementing — `src/state.rs:866-868`: "The version and variant nibbles are set as RFC 9562 §5.4 specifies for a random-bytes UUID, so the result is a well-formed UUID and not merely 32 hex characters."

The crate that already implements that rule is in the lock file — `Cargo.lock:16921-16923`:
```
[[package]]
name = "uuid"
version = "1.29.0"
```
and is used directly elsewhere in this repo, e.g. `services/common/src/management_events.rs:988`: `::uuid::Uuid::new_v4().hyphenated().to_string()`.

Caller this is filed under — `src/ops/create_connector.rs:297`: `let connector_id = crate::state::new_uuid();` (also `src/ops/create_challenge.rs:305` and `:310` for the challenge id and the `Password`).

Existing tests that cover the span and must stay green unchanged: `tests/all/connector_lifecycle.rs:154-156` (`/// True when `value` has the UUID form every id in this service's ARNs, and its challenge ... fn is_uuid(value: &str) -> bool`), asserted at `tests/all/connector_lifecycle.rs:345` ("the connector id must be a uuid"), `:733` ("the challenge id must be a uuid") and `:737` ("the password must be in UUID format"); plus the ARN-pattern assertions in `tests/all/resource_arn_validation.rs:258-274`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs` and read `new_uuid()` at lines 869-901: after the SHA-256 digest it writes `bytes[6] = (bytes[6] & 0x0f) | 0x40;` and `bytes[8] = (bytes[8] & 0x3f) | 0x80;` and then re-assembles the hyphenated form from `::hex::encode` slices.
2. Grep `Cargo.lock` for `name = "uuid"` — version 1.29.0 is already in the dependency graph (line 16921), and `services/common/src/management_events.rs:988` shows the repo already using it for exactly this rendering.
3. Compare: `::uuid::Builder::from_random_bytes(bytes).into_uuid().hyphenated().to_string()` performs the same two nibble writes and the same 8-4-4-4-12 lowercase rendering, so lines 888-901 collapse to one call with identical output.
4. Run the crate's tests (`tests/all/connector_lifecycle.rs` `is_uuid` assertions at lines 345, 733, 737) before and after: every minted id and password is unchanged, so no response and no test changes.
