Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 44485854c238b5a692f0cba9ea9d9988a6365132

# Replace the hand-assembled version-4 uuid minting in state.rs with the `uuid` crate, which is already in the workspace Cargo.lock

_Filed: 2026-10-29T00:49:27Z_
_Operation: CreateDataflowEndpointGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `44485854c238b5a692f0cba9ea9d9988a6365132`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::new_uuid` is the id minter every create of this service calls (`CreateConfig`, `CreateDataflowEndpointGroup`, `CreateDataflowEndpointGroupV2`, `CreateEphemeris`, `CreateMissionProfile`, `RegisterAgent`, `ReserveContact`, and `parse_satellites`). It hand-assembles a version-4 UUID: a per-process `RandomState`-derived seed, an `AtomicU64` counter, a `SystemTime` nanosecond reading, a SHA-256 digest, a manual truncation to 16 bytes, manual setting of the version nibble and variant bits, `hex::encode`, and a manual 8-4-4-4-12 re-slicing of the hex string.

The reason the doc comment gives for not using the `uuid` crate is that "taking a new dependency rewrites the workspace `Cargo.lock`". That premise does not hold: `uuid` 1.29.0 is ALREADY a member of the workspace `Cargo.lock`, with the `rand`/`getrandom` dependencies the v4 generator needs. So the hand-rolled span is replaceable by `::uuid::Uuid::new_v4()` plus `to_string()`, which produces exactly the lowercase hyphenated version-4 form this function documents as its contract, and the manual digest/bit-twiddling/slicing (including the `expect("16 of 32 digest bytes")` at state.rs:989) goes away.

No response and no test changes: every id this service hands out is still a fresh lowercase hyphenated v4 uuid, which is all the model constrains (`Uuid` = `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) and all the test suite asserts.

## Evidence

The hand-rolled span and the justification it rests on — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:962-1005`:
```
/// A fresh lowercase, hyphenated uuid — the `Uuid` shape every id member of this
/// service targets (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`),
/// in the version-4 layout (version nibble `4`, variant bits `10`).
...
/// It does NOT use the `uuid` crate, which would otherwise be the obvious choice: taking
/// a new dependency rewrites the workspace `Cargo.lock`, the one file every concurrently
/// landing change in this repository also rewrites, and this change has to merge beside
/// them. `sha2` and `hex` are already in this crate's dependency closure, so minting the
/// id from them leaves the lockfile untouched.
pub fn new_uuid() -> String {
    use ::sha2::Digest as _;

    let minted = IDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let mut bytes = <[u8; 16]>::try_from(&digest.finalize()[..16]).expect("16 of 32 digest bytes");

    // The version-4 layout: `4` in the high nibble of byte 6, `10` in the top two bits
    // of byte 8. Everything else stays as digested.
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
}
```
(plus the `IDS_MINTED` counter at `src/state.rs:946` and `process_seed` at `src/state.rs:953-960`, which exist only to feed it.)

The crate the lockfile already carries — `Cargo.lock:16986-16997`:
```
[[package]]
name = "uuid"
version = "1.29.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4a0e2d11620c50695fe12bbbfde96b3477fe606e7284c22d505c12ce51214514"
dependencies = [
 "getrandom 0.4.3",
 "js-sys",
 "rand 0.10.3",
 "sha1_smol",
 "wasm-bindgen",
]
```
so `uuid` is not "a new dependency" to the workspace lockfile at all, and `Uuid::new_v4().to_string()` is the documented API that renders exactly the lowercase hyphenated version-4 form.

Tests that cover it, and that stay unchanged: the one shape check every suite asserts an id through — `tests/all/scheduling_loop.rs:269-282`:
```
/// Whether `id` is spelled the way the modelled `Uuid` requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) — the shape of every
/// id this service mints, so the check is written once for every suite that asserts one.
pub(crate) fn is_uuid(id: &str) -> bool {
```
called from `tests/all/dataflow_endpoint_groups.rs:280`, `:450`, `tests/all/dataflow_endpoint_groups_v2.rs:240`, `tests/all/ephemeris_catalog.rs:122`, `tests/all/agent_lifecycle.rs:312`, `tests/all/agent_registration_tags.rs:147`, `tests/all/agent_configuration_reads.rs:182`, `:343`, `tests/all/tag_rules.rs:397`, `tests/all/satellite_reads.rs:81`, `tests/all/satellite_catalog.rs:174`, `:534`, `tests/all/dataflow_endpoint_group_listing.rs:121` and `tests/all/dataflow_endpoint_group_reads.rs:171` — none of them inspects the bytes, the counter or the digest, only the modelled spelling.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:962-1005`: the doc comment declines the `uuid` crate on the ground that "taking a new dependency rewrites the workspace `Cargo.lock`".
2. Read `Cargo.lock:16986-16997`: `uuid` 1.29.0 is already a package of that lockfile, with `rand`/`getrandom` behind it.
3. Confirm the span is pure id assembly with no service behaviour: the SHA-256 digest, the `expect("16 of 32 digest bytes")`, the version/variant bit masks and the manual hex re-slicing, plus the `IDS_MINTED` static and `process_seed` that exist only for it.
4. Confirm the only property the service and its tests depend on is the modelled spelling: `tests/all/scheduling_loop.rs:272` `is_uuid` checks five hex groups of 8-4-4-4-12 in lower case, which `::uuid::Uuid::new_v4().to_string()` satisfies — so replacing the span leaves every response and every existing test unchanged.
