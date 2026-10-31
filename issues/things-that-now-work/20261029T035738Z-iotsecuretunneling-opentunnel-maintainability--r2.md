Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# mint_tunnel_id hand-rolls UUID-v4 construction that the `uuid` crate already in Cargo.lock provides

_Filed: 2026-10-29T03:57:38Z_
_Operation: OpenTunnel | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::mint_tunnel_id` — the id minter `OpenTunnel` calls to produce the `tunnelId` it returns and stores — hand-rolls the whole UUID-v4 construction: it flips the version nibble and the RFC-4122 variant bits by hand, then hex-encodes and re-splits the 32 hex characters into the 8-4-4-4-12 hyphenated groups with a `format!` over five hard-coded byte ranges.

That is exactly what `uuid::Builder::from_random_bytes([u8; 16])` + `Uuid::to_string()` computes: `from_random_bytes` sets the same version-4 nibble and the same RFC-4122 variant bits, and `Display`/`to_string` renders the same lowercase hyphenated 8-4-4-4-12 form. The `uuid` crate is ALREADY in this workspace's `Cargo.lock`, and sibling service crates in this same repository declare it directly for precisely this purpose, so the dependency adds no new lockfile entry.

The duplicated logic is the risky part: the two bit-masks and the five `&hex[a..b]` index ranges are the UUID layout spelled out a second time in this crate. Any drift in them (an off-by-one in a slice bound, the variant mask applied to the wrong byte) silently produces an id that is no longer a well-formed UUID, and nothing in the crate would notice — the only test over the id checks the modelled `TunnelId` pattern, which any hex-and-hyphen string satisfies.

The fix is response-identical: the input bytes still come from `state::minted_bytes()`, so the same 16 bytes render to the same string, and `hex` stays a dependency for `mint_access_token`.

## Evidence

The hand-rolled span, `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:765-785`:

    /// Mint a fresh `tunnelId`.
    ///
    /// A random 128-bit value rendered in the UUID-v4 text form AWS uses for a
    /// tunnel id; it satisfies `TunnelId`'s modelled pattern
    /// `^[a-zA-Z0-9_\-+=:]{1,128}$`.
    pub fn mint_tunnel_id() -> String {
        let mut id = [0u8; 16];
        id.copy_from_slice(&minted_bytes()[..16]);
        // Version 4 and the RFC 4122 variant, so the id is well-formed as a UUID.
        id[6] = (id[6] & 0x0f) | 0x40;
        id[8] = (id[8] & 0x3f) | 0x80;
        let hex = hex::encode(id);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

Its one caller, `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/open_tunnel.rs:226`:

        let tunnel_id = crate::state::mint_tunnel_id();

The crate that replaces it is already in the lockfile — `Cargo.lock:17009-17011`:

    [[package]]
    name = "uuid"
    version = "1.29.0"

and a sibling service crate in this repository already declares it directly for the same job, `services/service-com-amazonaws-iotwireless-iotwireless/Cargo.toml:27-30`:

    # Mints the uuid `Id` CreateWirelessDevice returns for a new device. Already in the
    # lockfile as a dependency of the vendored framework crates, so declaring it adds no
    # new entry.
    uuid = { version = "1", features = ["v4"] }

This crate's manifest declares only `sha2` and `hex` for minting — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/Cargo.toml:24-26`:

    # Minting opaque tunnel ids and client access tokens (see `state::minted_bytes`).
    sha2 = { workspace = true }
    hex = { workspace = true }

Test coverage that must stay green and unchanged — `tests/all/tunnel_lifecycle.rs:83-91` and `:229-246` only pin the modelled pattern and uniqueness, which a `uuid`-rendered id satisfies identically:

    /// Whether `id` satisfies `TunnelId`'s modelled pattern
    /// `^[a-zA-Z0-9_\-+=:]{1,128}$`.
    fn is_modelled_tunnel_id(id: &str) -> bool {
    ...
        assert!(
            is_modelled_tunnel_id(&tunnel_id),
            "case {label:?}: tunnelId must satisfy the modelled pattern, got {tunnel_id:?}"
        );
    ...
        assert!(
            !minted.contains(&tunnel_id),
            "case {label:?}: tunnelId {tunnel_id:?} was already minted for another tunnel"
        );

Also exercised through `tests/all/tunnel_region_isolation.rs`, `tests/all/record_eviction_order.rs` and `tests/all/rotate_refusal_error_contract.rs`, all of which only consume the returned `tunnelId` as an opaque string.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs` at `mint_tunnel_id` (lines 765-785) and observe the two UUID version/variant bit-masks and the five hard-coded hex slice ranges that re-implement the UUID-v4 layout and its hyphenated text form.
2. Grep `Cargo.lock` for `name = "uuid"` and find it at line 17009 (version 1.29.0), i.e. the crate is already resolved in this workspace.
3. Grep `services/*/Cargo.toml` for `uuid = ` and find sibling service crates (e.g. `service-com-amazonaws-iotwireless-iotwireless/Cargo.toml:30`, `service-com-amazonaws-acm-certificatemanager/Cargo.toml:39`) declaring `uuid = { version = "1", features = ["v4"] }` for the same id-minting job, confirming the dependency is available to a service crate without a new lockfile entry.
4. Note that `uuid::Builder::from_random_bytes(bytes).into_uuid().to_string()` applies the same version-4 nibble and RFC-4122 variant bits and renders the same lowercase 8-4-4-4-12 form, so substituting it for lines 773-784 (keeping `minted_bytes()` as the byte source) leaves every response byte-identical and every existing test — `tests/all/tunnel_lifecycle.rs::is_modelled_tunnel_id` and the uniqueness assertion at :243-246 — unchanged.
