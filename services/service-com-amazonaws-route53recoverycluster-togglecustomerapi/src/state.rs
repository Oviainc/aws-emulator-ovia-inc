// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoverycluster-togglecustomerapi service.
//!
//! # What this service holds
//!
//! `ToggleCustomerAPI` is Route 53 ARC's **cluster (data-plane)** API: it reads
//! and toggles routing controls, and models no operation that can create or
//! delete one. Routing controls, control panels and clusters are created by the
//! separate recovery-control-**config** CONTROL plane. So this state is a
//! registry the data plane reads and mutates in place:
//!
//! - [`ComAmazonawsRoute53recoveryclusterTogglecustomerapiState::routing_controls`]
//!   — every routing control, keyed by its ARN. A `BTreeMap` so the key order is
//!   total and stable, which is what makes `ListRoutingControls`' paging walk the
//!   full set without repeats or omissions even while states are being toggled.
//! - [`ComAmazonawsRoute53recoveryclusterTogglecustomerapiState::control_panels`]
//!   — every control panel, keyed by ARN, carrying the cluster it is hosted on and
//!   the routing controls it hosts. It backs `ListRoutingControls`' `ControlPanelArn`
//!   filter and `UpdateRoutingControlStates`' "every entry in one cluster" rule.
//!
//! # Seeding rule
//!
//! Nothing in this API can create a routing control, so an empty registry would
//! make the whole service unexercisable. The fleet below is therefore
//! materialised by [`Default`], i.e. by `engine.state::<_>()` on the FIRST touch
//! of this service's store, from the fixed table [`SEED_FLEET`]:
//!
//! - seeding is request-independent and happens exactly once per engine, so no
//!   operation ever writes state merely because it was read: `GetRoutingControlState`
//!   and `ListRoutingControls` are pure reads of this registry, and only the two
//!   update operations mutate it;
//! - every seeded control starts `Off`, the state a newly created routing control
//!   has in Route 53 ARC, and a state change only ever comes from
//!   `UpdateRoutingControlState` / `UpdateRoutingControlStates`;
//! - the ARNs are built through [`::engine::emulator_arn`] with the canonical
//!   emulator account, in the real `controlpanel/<id>/routingcontrol/<id>` shape of
//!   a recovery-control ARN.
//!
//! When the recovery-control-config emulator grows a registry of its own, this
//! module's [`Default`] is the single place that should read it (through a crate
//! shared by both services) instead of the fixed table; no handler needs to change.
//!
//! No pagination-token table lives here: `ListRoutingControls` encodes its cursor
//! (the filter it was issued for, plus the offset) into the opaque `NextToken`
//! itself, so issuing a page costs no state write and a read stays a read.

use std::collections::BTreeMap;

/// The AWS service that owns a routing control's ARN namespace: routing controls,
/// control panels and clusters are all recovery-**control** (control-plane)
/// resources, which this data-plane service only reads and toggles.
const CONTROL_PLANE_SERVICE: &str = "route53-recovery-control";

/// Recovery-control ARNs carry no region (they are cluster-global), so the region
/// segment of every ARN built here is empty.
const ARN_REGION: &str = "";

/// The most routing control states one `UpdateRoutingControlStates` call may carry.
///
/// Route 53 ARC caps a batch update ("Routing controls per
/// `UpdateRoutingControlStates` request" in the service's quotas); a request over
/// the cap is answered with `ServiceLimitExceededException` (HTTP 402), whose own
/// documentation is "The request can't update that many routing control states at
/// the same time. Try again with fewer routing control states."
pub const MAX_UPDATE_ROUTING_CONTROL_STATE_ENTRIES: usize = 10;

/// One control panel of the seed fleet: the cluster hosting it, its id/name, and
/// the routing controls it hosts.
#[derive(Debug, Clone, Copy)]
pub struct SeedControlPanel {
    /// Id of the cluster this control panel is hosted on.
    pub cluster_id: &'static str,
    /// Id and name of the control panel (Route 53 ARC panel names are ASCII).
    pub control_panel_id: &'static str,
    /// Ids/names of the routing controls hosted in this control panel.
    pub routing_control_ids: &'static [&'static str],
}

/// The fleet [`Default`] materialises — see this module's "Seeding rule".
///
/// Deliberately spans two clusters, with one cluster hosting two control panels,
/// so the whole modelled surface is reachable: the `ControlPanelArn` filter, a
/// batch update across two panels of ONE cluster (allowed), and a batch update
/// spanning two clusters (refused).
pub const SEED_FLEET: &[SeedControlPanel] = &[
    SeedControlPanel {
        cluster_id: "EmulatorCluster1",
        control_panel_id: "EmulatorControlPanelA",
        routing_control_ids: &[
            "EmulatorRoutingControl1",
            "EmulatorRoutingControl2",
            "EmulatorRoutingControl3",
        ],
    },
    SeedControlPanel {
        cluster_id: "EmulatorCluster1",
        control_panel_id: "EmulatorControlPanelB",
        routing_control_ids: &["EmulatorRoutingControl4"],
    },
    SeedControlPanel {
        cluster_id: "EmulatorCluster2",
        control_panel_id: "EmulatorControlPanelC",
        routing_control_ids: &["EmulatorRoutingControl5"],
    },
];

/// The ARN of a recovery-control cluster.
pub fn cluster_arn(cluster_id: &str) -> String {
    ::engine::emulator_arn(
        CONTROL_PLANE_SERVICE,
        ARN_REGION,
        &format!("cluster/{cluster_id}"),
    )
}

/// The ARN of a control panel.
pub fn control_panel_arn(control_panel_id: &str) -> String {
    ::engine::emulator_arn(
        CONTROL_PLANE_SERVICE,
        ARN_REGION,
        &format!("controlpanel/{control_panel_id}"),
    )
}

/// The ARN of a routing control hosted in `control_panel_id`.
pub fn routing_control_arn(control_panel_id: &str, routing_control_id: &str) -> String {
    ::engine::emulator_arn(
        CONTROL_PLANE_SERVICE,
        ARN_REGION,
        &format!("controlpanel/{control_panel_id}/routingcontrol/{routing_control_id}"),
    )
}

/// A routing control's state: the on/off switch itself.
///
/// The wire form is the modelled `RoutingControlState` enum — `On` traffic flows
/// to the cell, `Off` it does not — whose value set is
/// [`crate::ops::types::ROUTING_CONTROL_STATE_VALUES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingControlState {
    /// Traffic flows to the cell.
    On,
    /// Traffic does not flow to the cell.
    Off,
}

impl RoutingControlState {
    /// The value this state is sent and received as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    /// Parse a wire value, or `None` when it is not a modelled
    /// `RoutingControlState` (the caller answers that with a `ValidationException`).
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "On" => Some(Self::On),
            "Off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// A routing control, as this service stores it. The members mirror the modelled
/// `RoutingControl` shape, with `state` as the typed on/off switch.
#[derive(Debug, Clone)]
pub struct RoutingControl {
    /// ARN of this routing control; the key it is stored under.
    pub routing_control_arn: String,
    /// Name of this routing control.
    pub routing_control_name: String,
    /// ARN of the control panel hosting it.
    pub control_panel_arn: String,
    /// Name of the control panel hosting it.
    pub control_panel_name: String,
    /// Current on/off state.
    pub state: RoutingControlState,
    /// Account id of the routing control's owner.
    pub owner: String,
}

/// A control panel: a cluster-hosted group of routing controls.
#[derive(Debug, Clone)]
pub struct ControlPanel {
    /// ARN of this control panel; the key it is stored under.
    pub arn: String,
    /// Name of this control panel.
    pub name: String,
    /// ARN of the cluster hosting it. Backs the "every entry of a batch update is
    /// in one cluster" rule, which is a cluster-endpoint property of the API.
    pub cluster_arn: String,
    /// ARNs of the routing controls hosted in this control panel.
    pub routing_control_arns: Vec<String>,
}

/// Per-service state for com-amazonaws-route53recoverycluster-togglecustomerapi. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRoute53recoveryclusterTogglecustomerapiState {
    /// Every routing control, keyed by ARN (total, stable order for paging).
    pub routing_controls: BTreeMap<String, RoutingControl>,
    /// Every control panel, keyed by ARN.
    pub control_panels: BTreeMap<String, ControlPanel>,
}

impl Default for ComAmazonawsRoute53recoveryclusterTogglecustomerapiState {
    /// Materialise the seed fleet — see this module's "Seeding rule". Runs once
    /// per engine, on the first touch of this service's store.
    fn default() -> Self {
        let owner = ::engine::emulator_account_id().to_string();
        let mut routing_controls = BTreeMap::new();
        let mut control_panels = BTreeMap::new();

        for panel in SEED_FLEET {
            let panel_arn = control_panel_arn(panel.control_panel_id);
            let mut hosted = Vec::with_capacity(panel.routing_control_ids.len());
            for control_id in panel.routing_control_ids {
                let arn = routing_control_arn(panel.control_panel_id, control_id);
                hosted.push(arn.clone());
                routing_controls.insert(
                    arn.clone(),
                    RoutingControl {
                        routing_control_arn: arn,
                        routing_control_name: (*control_id).to_string(),
                        control_panel_arn: panel_arn.clone(),
                        control_panel_name: panel.control_panel_id.to_string(),
                        // A routing control starts Off; only an update moves it.
                        state: RoutingControlState::Off,
                        owner: owner.clone(),
                    },
                );
            }
            control_panels.insert(
                panel_arn.clone(),
                ControlPanel {
                    arn: panel_arn,
                    name: panel.control_panel_id.to_string(),
                    cluster_arn: cluster_arn(panel.cluster_id),
                    routing_control_arns: hosted,
                },
            );
        }

        Self {
            routing_controls,
            control_panels,
        }
    }
}

impl ComAmazonawsRoute53recoveryclusterTogglecustomerapiState {
    /// The routing control stored under `arn`, if any.
    pub fn routing_control(&self, arn: &str) -> Option<&RoutingControl> {
        self.routing_controls.get(arn)
    }

    /// The cluster hosting the control panel that hosts `arn`, if that routing
    /// control is known.
    pub fn cluster_of(&self, routing_control_arn: &str) -> Option<&str> {
        let control = self.routing_controls.get(routing_control_arn)?;
        let panel = self.control_panels.get(&control.control_panel_arn)?;
        Some(panel.cluster_arn.as_str())
    }

    /// The routing controls to report, in ARN order: every one in the registry, or
    /// just those hosted in `control_panel_arn` when a filter is given.
    pub fn routing_controls_in(&self, control_panel_arn: Option<&str>) -> Vec<&RoutingControl> {
        self.routing_controls
            .values()
            .filter(|control| {
                control_panel_arn.is_none_or(|panel| control.control_panel_arn == panel)
            })
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>)
}
