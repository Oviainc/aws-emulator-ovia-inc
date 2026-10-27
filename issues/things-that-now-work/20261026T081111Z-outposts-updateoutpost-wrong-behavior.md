Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3af8ecaf4ce2325789e9d5260e121bf04ff83dc7

# UpdateOutpost writes SupportedHardwareType without reconciling the Outpost's installed hardware, leaving a form factor and an asset list the crate's own rule says cannot coexist

_Filed: 2026-10-26T08:11:11Z_
_Operation: UpdateOutpost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3af8ecaf4ce2325789e9d5260e121bf04ff83dc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateOutpost` applies `SupportedHardwareType` straight onto the stored `Outpost` projection and touches nothing else. But in this crate `SupportedHardwareType` is not a cosmetic label: it is the ONE input that selects which hardware the Outpost is installed with, and the install happens exactly once, at the moment the clock settles the Outpost's provisioning. After that instant `OutpostRecord::active_at` is `None`, so `settle_outposts` never revisits the record and no later form-factor change is ever installed for.

Two states the crate documents as impossible are therefore reachable through `UpdateOutpost`:

1. An Outpost created with NO `SupportedHardwareType` settles ACTIVE holding no asset (correct — it named no configuration). A later `UpdateOutpost` with `SupportedHardwareType: "RACK"` is accepted and the Outpost reports `SupportedHardwareType=RACK`, yet `ListAssets` on it answers an empty list forever, `GetOutpostSupportedInstanceTypes?AssetId=…` can never resolve anything, and `StartConnection`/`StartCapacityTask` can never name one of its assets. Two Outposts with identical persisted state (form factor RACK, lifecycle ACTIVE) report different hardware depending only on whether the form factor arrived on the create or on the update.
2. An Outpost created `RACK` settles with the whole `RACK_HARDWARE` configuration (powershelf, two switches, a networking asset and the compute/storage servers). A later `UpdateOutpost` with `SupportedHardwareType: "SERVER"` is accepted, so the Outpost now reports the SERVER form factor while still holding the RACK configuration — and `GetOutpostSupportedInstanceTypes` then filters the catalog by `SERVER` while weighing those rack assets' `ComputeAttributes`, which were built from the RACK profile.

The handler neither installs the newly named configuration, nor refuses the change, nor says anything about the hardware it leaves stale; its only comment about the write is that it is one of "the three members this operation takes besides the identifier".

## Evidence

The rule the crate states for itself, in `src/state.rs:2202-2213` (`installed_hardware`):

    /// Every hardware asset installed on one Outpost, in installation order.
    ///
    /// Hardware is what an Outpost IS: Amazon Web Services installs the configuration the
    /// Outpost's form factor names once the order for it is fulfilled, and the Outpost
    /// reaches a settled lifecycle status with that hardware in it — so an Outpost that
    /// has finished provisioning holds one asset per physical unit of its configuration,
    /// ... An Outpost whose form factor was never recorded names no configuration,
    /// so nothing is installed on it (see [`hardware_configuration`]).

and `src/state.rs:2100-2103` (`hardware_configuration`):

    /// `CreateOutpost` records only the form factor the caller named and the model documents no
    /// default, so an Outpost that named none names no configuration either: there is nothing
    /// for the service to install on it and it holds no asset.

The install is one-shot — `src/state.rs:2946-2970` (`settle_outposts`):

        let settled: Vec<(String, Vec<AssetInfo>)> = self
            .outposts
            .iter()
            .filter(|(_, record)| record.active_at.is_some_and(|due| now >= due))
            ...
            record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
            record.active_at = None;

The write that breaks the pairing — `src/ops/update_outpost.rs:174` then `185-194`:

        store.settle_outposts();
        ...
        // The three members this operation takes besides the identifier, each
        // written only when the request carried it. ...
        crate::state::apply(&mut record.outpost.name, input.name);
        crate::state::apply(&mut record.outpost.description, input.description);
        crate::state::apply(
            &mut record.outpost.supported_hardware_type,
            input.supported_hardware_type,
        );

and `apply` writes unconditionally when the member arrived (`src/state.rs:4107-4111`):

    pub fn apply<T>(stored: &mut Option<T>, update: Option<T>) {
        if let Some(value) = update { *stored = Some(value); }
    }

The create side, for contrast, is what records the form factor the install reads (`src/ops/create_outpost.rs:269`): `supported_hardware_type: crate::state::non_empty(input.supported_hardware_type),`.

Test coverage: `tests/all/outpost_hardware_installation.rs:11-16` pins rule 1 only for the form factor given at CREATE time ("Amazon Web Services installs the configuration the Outpost's form factor names... An Outpost whose form factor names no configuration this service holds is installed with nothing"), and its helper `an_outpost` (lines 74-84) only ever sets `SupportedHardwareType` on the create request. The one test that updates the member, `tests/all/private_connectivity_ownership.rs:72-127`, asserts only that the patched value is echoed back (`let reported = text(&patched["Outpost"], "SupportedHardwareType");`) and never looks at the Outpost's assets — so no test pins the current stale-hardware outcome.

## Steps to Reproduce

1. CreateSite, then CreateOutpost on it WITHOUT a SupportedHardwareType member; keep the OutpostId.
2. Wait past OUTPOST_PROVISIONING (500 ms) and GET /outposts/{OutpostId} — LifeCycleStatus is ACTIVE.
3. GET /outposts/{OutpostId}/assets — the empty list, as expected for an Outpost that named no configuration.
4. PATCH /outposts/{OutpostId} with {"SupportedHardwareType":"RACK"} — 200, and the answer reports SupportedHardwareType=RACK.
5. GET /outposts/{OutpostId}/assets again — still an empty list, for ever: the Outpost now has a form factor but holds none of the hardware `installed_hardware` says that form factor names. Compare with a second Outpost created WITH SupportedHardwareType=RACK, which reports the whole RACK_HARDWARE configuration.
6. For the mirror case: CreateOutpost with SupportedHardwareType=RACK, wait for ACTIVE, GET its assets (the rack configuration), then PATCH {"SupportedHardwareType":"SERVER"} and GET the assets again — the Outpost reports the SERVER form factor while still holding the RACK units.
