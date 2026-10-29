Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 180e00f18303c31290b0095c063d702b5ec407d7

# StartConnection resolves its AssetId by existence alone, so an Outpost-server installation tunnel is minted for a rack's POWERSHELF, SWITCH, NETWORKING or STORAGE unit

_Filed: 2026-10-29T04:46:09Z_
_Operation: StartConnection | Class: validation_

> _Note: this issue was identified against the code base at commit `180e00f18303c31290b0095c063d702b5ec407d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartConnection` is the call that installs an Outpost SERVER, and its `AssetId` names the server being installed. The handler checks only that SOME Outpost of the request's region holds an asset with that id (`store.outpost_with_asset(...)`), throws the asset record away, and mints a WireGuard tunnel for it. Since the crate now installs a whole rack of hardware when an order on a RACK Outpost is fulfilled (`settle_outposts` -> `installed_hardware` -> `RACK_HARDWARE`), the ids a caller reads out of `ListAssets` include units this crate itself recorded as `POWERSHELF`, `SWITCH`, `NETWORKING` and `STORAGE`. Naming any one of them is answered 200 with a persisted connection, and `GetConnection` then reports an Outpost-server installation tunnel whose `AssetId` is a power shelf.

The handler states the rule it does not keep: the comment above the check says the `AssetId` names one Outpost SERVER, and the only thing the code derives from it is that the id exists. The crate already knows how to ask the narrower question: the sibling `GetOutpostSupportedInstanceTypes` resolves the same member through `asset_of_outpost`, keeps the `AssetInfo`, and refuses hardware that is not of the kind that operation is about ("a storage server, a powershelf, a switch or a networking asset hosts no instance"). `StartConnection` is the one path that resolves an asset without looking at what the asset IS.

The gap is a residual of a premise that has since become false: this handler's own module doc still asserts that no order is ever carried as far as being delivered and installed, i.e. that only assets recorded straight into the store could ever reach the check — while the handler body immediately below it settles the install an order's fulfilment owes. Once orders began delivering rack hardware, a non-server unit became reachable here for the first time.

(Note the per-member distinction: `StartConnection` already models `NotFoundException` for an asset it cannot resolve and `ValidationException` for a bad request, so there is an error channel for the refusal; what is missing is that the resolved asset's recorded kind is never consulted.)

## Evidence

The whole of what the handler derives from `AssetId` — `src/ops/start_connection.rs:179-191`:

        // A connection installs one Outpost SERVER, named by the `AssetId` of that
        // unit of hardware, so an asset no Outpost of this store holds has no
        // connection to start: the not-found error this call models, worded by the
        // identifier echo of the value as supplied.
        if store.outpost_with_asset(&region, &input.asset_id).is_none() {
            return Err(StartConnectionErrorModeled::NotFoundException(NotFoundException {

and what that helper answers — `src/state.rs:4625-4632`:

    pub fn outpost_with_asset(&self, region: &str, asset_id: &str) -> Option<&OutpostRecord> {
        self.outposts_in(region).find(|record| {
            record
                .assets
                .iter()
                .any(|asset| asset.asset_id.as_deref() == Some(asset_id))
        })
    }

It matches ANY asset of ANY Outpost of the region and hands back the Outpost, never the asset — so the unit's `AssetType` is unreachable from here.

What a rack Outpost's assets are in this crate — `src/state.rs:2492-2541`:

    const RACK_HARDWARE: &[HardwareUnit] = &[
        HardwareUnit {
            asset_type: ASSET_TYPE_POWERSHELF,
            rack_elevation: Some(1.0),
        },
        HardwareUnit {
            asset_type: ASSET_TYPE_SWITCH,
            ...
        HardwareUnit {
            asset_type: ASSET_TYPE_STORAGE,
            rack_elevation: Some(12.0),
        },
    ];

against what an Outposts SERVER is — `src/state.rs:2543-2553`:

    /// The hardware an Outposts SERVER is installed with: the one server it is.
    ...
    const SERVER_HARDWARE: &[HardwareUnit] = &[HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: None,
    }];

The SIBLING in this same crate that resolves the same member and does consult the kind — `src/ops/get_outpost_supported_instance_types.rs:286-300` resolves it as `store.asset_of_outpost(&outpost_id, asset_id)` and keeps the record, then `:370-374`:

        Hardware::Outpost => true,
        // Not compute hardware: a storage server, a powershelf, a switch or a
        // networking asset hosts no instance, so it supports no instance type.
        Hardware::Asset(None) => false,

The premise the handler was written under, now false, in the same file — `src/ops/start_connection.rs:12-16`:

    //! The asset has to be one an Outpost of the store holds, as it does at AWS. No
    //! operation in this API records hardware on an Outpost — hardware arrives with
    //! a fulfilled order, and no order this emulator accepts is ever carried as far as
    //! being delivered and installed — so that precondition is reachable only for an
    //! Outpost whose assets were recorded through the store.

contradicted by the body eleven lines down — `src/ops/start_connection.rs:174-178`:

        // An Outpost's hardware is installed when an order placed on it has delivered
        // some, so the installation the clock has already made due is materialised
        // before the `AssetId` is resolved against it: the asset ids a caller read out
        // of `ListAssets` are the ones it can start a connection for.
        store.settle_outposts();

and by the suite that pins order-driven installation — `tests/all/outpost_order_driven_lifecycle.rs:17-20`: "What owes the transition is the FULFILMENT of an order placed on the Outpost: when the hardware that order delivers has been installed, the order is complete and the Outpost reports the terminal status holding the configuration its form factor names."

The model's own words for the operation and the member: `smithy_query(query="doc",shape="StartConnection")` — "Amazon Web Services uses this action to install Outpost servers. Starts the connection required for Outpost server installation." — and `smithy_query(query="doc",shape="StartConnectionRequest",member="AssetId")` — "The ID of the Outpost server."

Test coverage today: `tests/all/outpost_server_connection.rs` is the suite for this operation and every row records its hardware with NO asset type — `:209-217` builds each unit as `asset(asset_id, "rack-1", None, None, None)`, and `asset()` (`tests/all/outpost_assets.rs:60-76`) maps that third argument to `asset_type: asset_type.map(str::to_string)`. So no existing case starts a connection for a unit with a RECORDED non-server `AssetType`, and nothing pins the current acceptance of one.

## Steps to Reproduce

1. CreateSite, then CreateOutpost on it with SupportedHardwareType=RACK; keep the OutpostId.
2. CreateOrder on that Outpost with one LineItem, and wait until GetOrder reports the order COMPLETED — the rack is then installed (settle_outposts).
3. GET /outposts/{OutpostIdentifier}/assets — note the AssetId of the unit reported with "AssetType":"POWERSHELF" (RACK_HARDWARE's first unit), or of a SWITCH / NETWORKING / STORAGE unit.
4. POST /connections with {"AssetId":"<that powershelf AssetId>","ClientPublicKey":"<43 base64 chars>=","NetworkInterfaceDeviceIndex":0}.
5. Observe HTTP 200 with a ConnectionId and an UnderlayIpAddress: an Outpost-server installation connection was started for a power shelf.
6. GET /connections/{ConnectionId} — the stored record reports that tunnel, while GET /outposts/{OutpostIdentifier}/supportedInstanceTypes?AssetId=<same id> shows the crate refusing the same unit elsewhere as hardware that is not a server.
