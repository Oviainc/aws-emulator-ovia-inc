Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The egress playback URL prefix (account label, hostname, /out/v1/<endpoint label>) is derived twice in state.rs

_Filed: 2026-10-27T04:29:02Z_
_Operation: CreateOriginEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`playback_url` and `cmaf_manifest_url` each independently re-derive the SAME three things: the account label, the egress hostname, and the endpoint's own path label — then append their own tail. The two copies have to agree for an endpoint's `url` and its CMAF children's `url`s to sit under one playback path, but nothing ties them together: changing the label derivation or the hostname in one leaves the other serving a different prefix. (The account-label half is written a third time in `mint_ingest_endpoints` for the ingest hostname.)

The defect is the duplicated derivation only. Extracting the shared prefix into one function that both call leaves every byte of every response, and every existing assertion, unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2583-2596`:

  pub fn playback_url(region: &str, channel_id: &str, endpoint_id: &str, manifest_name: &str, kind: PackagingKind) -> String {
      let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
      let endpoint_label = stable_label(&[channel_id, endpoint_id, region]);
      let manifest = kind.manifest_path(manifest_name);
      format!("https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}/{manifest}")
  }

Copy 2 — `src/state.rs:2600-2613`:

  pub fn cmaf_manifest_url(region: &str, channel_id: &str, endpoint_id: &str, manifest_id: &str, manifest_name: &str) -> String {
      let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
      let endpoint_label = stable_label(&[channel_id, endpoint_id, region]);
      let manifest_label = stable_label(&[channel_id, endpoint_id, manifest_id, region]);
      format!("https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}/{manifest_label}/{manifest_name}/index.m3u8")
  }

The two `let account_label = ...` / `let endpoint_label = ...` lines and the `https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}` prefix are byte-identical between them; only the tail differs. Third copy of the account-label rule: `src/state.rs:2105` in `mint_ingest_endpoints` (`let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);`).

Both copies are reached from the same write: `src/ops/create_origin_endpoint.rs:346-352` calls `crate::state::playback_url(...)` and line 375 calls `crate::state::rederive_cmaf_package(&mut record)`, which goes through `stored_cmaf_package` -> `cmaf_manifest_url` (`src/state.rs:2648-2654`).

Existing tests that pin both URLs and must stay green unchanged: `tests/all/channel_lifecycle.rs:619-700` (asserts the endpoint `url` starts with `https://`, carries the Region and ends in the packaging suffix, and that each child `url` contains `/{child_name}/` and ends with `/index.m3u8`); `tests/all/cmaf_child_manifest_name.rs:178`; `tests/all/cmaf_child_manifest_name_follows_the_endpoint.rs:254,273,481`; `tests/all/mutation_persistence.rs:789,812`; `tests/all/region_isolation.rs:230`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs and read playback_url (lines 2583-2596) and cmaf_manifest_url (lines 2600-2613) side by side.
2. Observe the identical account_label/endpoint_label derivations and the identical "https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}" prefix in both.
3. Change the hostname or the label derivation in one function only: the endpoint's own `url` and its CMAF children's `url`s now sit under different playback prefixes, with no compile error and no single place that states the rule.
