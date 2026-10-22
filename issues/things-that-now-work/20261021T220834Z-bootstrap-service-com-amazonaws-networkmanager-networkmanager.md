Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the networkmanager emulator

_Filed: 2026-10-21T22:08:34Z_
_Operation: service-com-amazonaws-networkmanager-networkmanager | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 95 operations in `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds no fields at all (`// TODO: add service-specific state fields`). No client workflow can run.

Implement a FIRST, SMALL slice: the global-network / site lifecycle, the core resource of Network Manager (restJson1, SDK id `NetworkManager`, signing name `networkmanager`). Six operations, in this order:

1. **CreateGlobalNetwork** — `POST /global-networks`. Input: `Description` (0..256), `Tags` (list<Tag>). Must mint a fresh `GlobalNetworkId` (e.g. `global-network-<16 hex>`), build `GlobalNetworkArn` (`arn:aws:networkmanager::<account>:global-network/<id>`), set `CreatedAt` to now, `State` = `PENDING`/`AVAILABLE` (an emulator may settle immediately to `AVAILABLE`), store the record, and return the stored `GlobalNetwork` (`GlobalNetworkId`, `GlobalNetworkArn`, `Description`, `CreatedAt`, `State`, `Tags`).
2. **DescribeGlobalNetworks** — `GET /global-networks`. Input: `GlobalNetworkIds` (query `globalNetworkIds`), `MaxResults` (1..500, query `maxResults`), `NextToken` (query `nextToken`). Output: `GlobalNetworks` (list<GlobalNetwork>), `NextToken`. Must list exactly the networks previously created, filter by `GlobalNetworkIds` when supplied, honour `MaxResults` and emit/consume `NextToken` for a stable continuation; a deleted network must not appear.
3. **CreateSite** — `POST /global-networks/{GlobalNetworkId}/sites`. Input: `GlobalNetworkId` (label, required), `Description`, `Location`, `Tags`. Must 404 `ResourceNotFoundException` for an unknown `GlobalNetworkId`; otherwise mint `SiteId` (`site-<hex>`), build `SiteArn`, set `CreatedAt`/`State`, persist under that global network and return the stored `Site` (`SiteId`, `SiteArn`, `GlobalNetworkId`, `Description`, `Location`, `CreatedAt`, `State`, `Tags`).
4. **GetSites** — `GET /global-networks/{GlobalNetworkId}/sites`. Input: `GlobalNetworkId` (required), `SiteIds` (query `siteIds`), `MaxResults`, `NextToken`. Output: `Sites`, `NextToken`. Must return only the sites of that global network, filtered by `SiteIds` when given, paginated; unknown global network => 404.
5. **UpdateGlobalNetwork** — `PATCH /global-networks/{GlobalNetworkId}`. Input: `GlobalNetworkId`, `Description`. Must mutate the persisted record (and only the supplied field) and return the updated `GlobalNetwork`; a following DescribeGlobalNetworks must show the new `Description`. Unknown id => 404.
6. **DeleteGlobalNetwork** — `DELETE /global-networks/{GlobalNetworkId}`. Must 409 `ConflictException` while the network still has child sites/devices (real service refuses to delete a non-empty global network — so DeleteSite, `DELETE /global-networks/{GlobalNetworkId}/sites/{SiteId}`, is included in the slice as the way to empty it), otherwise remove the record and return the final `GlobalNetwork` with `State` = `DELETING`. A later DescribeGlobalNetworks must not list it, and GetSites on it must 404.

NEVER STUBS: each of these must compute its response from persisted state. No hardcoded ids echoed back without storage, no echoed input structures, no empty 200s. Create-then-read must return exactly what was created, including generated ids, ARNs and timestamps.

## State Model

In `ComAmazonawsNetworkmanagerNetworkmanagerState` (reached via `crate::state::service_state(&*self.engine)`):

- `global_networks: BTreeMap<String /*GlobalNetworkId*/, GlobalNetworkRecord>` where `GlobalNetworkRecord { id: String, arn: String, description: Option<String>, created_at: SystemTime, state: GlobalNetworkState, tags: Vec<Tag> }`
- `sites: BTreeMap<String /*SiteId*/, SiteRecord>` where `SiteRecord { id: String, arn: String, global_network_id: String, description: Option<String>, location: Option<Location>, created_at: SystemTime, state: SiteState, tags: Vec<Tag> }`
- `devices: BTreeMap<String /*DeviceId*/, DeviceRecord>` — reserved for the next slice (CreateDevice/GetDevices), keyed the same way with `global_network_id` + `site_id` back-references, so the DeleteGlobalNetwork conflict check can already consult it.
- `id_counter: u64` — monotonic source for generated `global-network-`/`site-`/`device-` suffixes, so ids are unique and deterministic per run.
- Pagination: `NextToken` is an opaque encoding of the key offset into the relevant BTreeMap (e.g. base64 of the last returned id), validated on input; an unparseable token must yield `ValidationException` (400).

All remaining operations (core networks, attachments, peerings, policies, route analysis, tagging, etc.) correctly stay at 501 and will be filed as individual gap issues later.

## Evidence

Spec: `services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json` via OPERATIONS.md — CreateGlobalNetwork `POST /global-networks` -> `GlobalNetwork`; DescribeGlobalNetworks `GET /global-networks` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=GlobalNetworks)`; CreateSite `POST /global-networks/{GlobalNetworkId}/sites` -> `Site`; GetSites `GET /global-networks/{GlobalNetworkId}/sites` `@paginated(items=Sites)`; UpdateGlobalNetwork `PATCH /global-networks/{GlobalNetworkId}`; DeleteSite `DELETE /global-networks/{GlobalNetworkId}/sites/{SiteId}`; DeleteGlobalNetwork `DELETE /global-networks/{GlobalNetworkId}`, all declaring `ResourceNotFoundException (404)`, `ConflictException (409)` and `ValidationException (400)`. Shape members from `smithy_query shape=GlobalNetwork,Site,Device` (GlobalNetworkId/GlobalNetworkArn/Description/CreatedAt/State/Tags; SiteId/SiteArn/GlobalNetworkId/Description/Location/CreatedAt/State/Tags). Implementation: `src/ops/create_global_network.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 163 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateGlobalNetwork"))`; all 95 op files are the same scaffold. `src/state.rs:8` still has `// TODO: add service-specific state fields`, so nothing can persist. Only test present is the generated `tests/state_roundtrip.rs`; no operation has behavioral coverage.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client with `--endpoint-url` at it, service `networkmanager`.
2. `aws networkmanager create-global-network --description "demo"` — expect 200 with a generated `GlobalNetworkId`, `GlobalNetworkArn`, `CreatedAt` and `State`; today this returns HTTP 501 NotImplemented.
3. `aws networkmanager describe-global-networks` — expect the just-created network in `GlobalNetworks` with the same id and description.
4. `aws networkmanager create-site --global-network-id <id> --description "hq" --location Address=...,Latitude=...,Longitude=...` — expect 200 with a generated `SiteId` and `GlobalNetworkId` echoing the parent.
5. `aws networkmanager get-sites --global-network-id <id>` — expect exactly that one site; repeat with `--max-results 1` across a second site to confirm `NextToken` paging works.
6. `aws networkmanager update-global-network --global-network-id <id> --description "demo2"`, then `describe-global-networks` — expect the new description from stored state.
7. `aws networkmanager delete-global-network --global-network-id <id>` while the site exists — expect `ConflictException` (409).
8. `aws networkmanager delete-site --global-network-id <id> --site-id <site>`, then `delete-global-network --global-network-id <id>` — expect 200, and a following `describe-global-networks` must not list it while `get-sites --global-network-id <id>` returns `ResourceNotFoundException` (404).
9. `aws networkmanager create-site --global-network-id global-network-00000000000000000` (never created) — expect `ResourceNotFoundException` (404), not 501 and not a success.
