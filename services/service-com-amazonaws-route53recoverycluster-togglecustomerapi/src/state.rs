// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoverycluster-togglecustomerapi service.
//!
//! # What this service holds
//!
//! `ToggleCustomerAPI` is Route 53 ARC's **cluster (data-plane)** API: it reads and
//! toggles routing controls, and models no operation that can create or delete one.
//! Clusters, control panels and routing controls are created by the separate
//! recovery-control **config** (CONTROL plane) API, so WHICH of them exist — and their
//! names, their hosting control panel and cluster, and their owner — is that service's
//! store to answer, never a table of this one's. This module reaches it: [`stores`]
//! resolves both halves out of the ONE shared engine and [`DataPlane`] joins them, so a
//! resource a client creates through the control plane is addressable here immediately
//! and one it deletes there stops being addressable here.
//!
//! What this service OWNS is the switch itself
//! ([`ComAmazonawsRoute53recoveryclusterTogglecustomerapiState::routing_control_states`]):
//! a routing control's `On`/`Off` state is cluster (data-plane) state in Route 53 ARC —
//! the control plane neither sets it nor reports it, and only
//! `UpdateRoutingControlState` / `UpdateRoutingControlStates` move it. A routing control
//! the registry holds and that map does not is
//! [`INITIAL_ROUTING_CONTROL_STATE`], the state a newly created routing control has, so
//! `GetRoutingControlState` and `ListRoutingControls` stay pure reads: nothing is written
//! merely because a control was read.
//!
//! The safety rules that decide whether a switch may MOVE are the control plane's
//! records as well, so they are read from the registry too and no copy of them is kept
//! here: [`DataPlane::safety_rule_violation`] evaluates them for a proposed transaction,
//! which is what the two update operations ask before they write anything.
//!
//! No pagination-token table lives here either: `ListRoutingControls` encodes its cursor
//! (the filter it was issued for, plus the routing control the next page starts at) into
//! the opaque `NextToken` itself, so issuing a page costs no state write.
//!
//! # Which resources are addressable
//!
//! Every routing control and control panel the registry holds, whatever deployment
//! status the control plane reports for it: that status describes the propagation of the
//! resource DEFINITION, which the control plane's own describes report, while the switch
//! this service serves exists from the moment the resource does. So a caller that
//! creates a routing control and toggles it straight away is served, and one whose
//! resource the control plane has deleted is not.

use std::collections::BTreeMap;
use std::sync::MutexGuard;

/// The recovery-control **config** crate: the control plane that creates every resource
/// this data plane serves. Its store is the registry read here, through the shared
/// engine, so the two services cannot disagree about which routing controls exist.
use service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig as recovery_control_config;

use recovery_control_config::ops::types::RuleConfig;
use recovery_control_config::state::{
    status_at, RoutingControlRecord, SafetyRuleControls, SafetyRuleRecord, Store as Registry,
    STATUS_DEPLOYED,
};

/// The most routing control states one `UpdateRoutingControlStates` call may carry.
///
/// Route 53 ARC caps a batch update ("Routing controls per
/// `UpdateRoutingControlStates` request" in the service's quotas); a request over
/// the cap is answered with `ServiceLimitExceededException` (HTTP 402), whose own
/// documentation is "The request can't update that many routing control states at
/// the same time. Try again with fewer routing control states."
pub const MAX_UPDATE_ROUTING_CONTROL_STATE_ENTRIES: usize = 10;

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

/// The state a routing control reports until an update moves it: a newly created
/// routing control is `Off` in Route 53 ARC, so an absent switch is `Off` rather than a
/// value a read would have to write in.
pub const INITIAL_ROUTING_CONTROL_STATE: RoutingControlState = RoutingControlState::Off;

/// A routing control as this data plane reports it: what the control plane's registry
/// holds about it, joined with the switch this service owns.
#[derive(Debug, Clone)]
pub struct RoutingControl {
    /// ARN of this routing control, as the control plane minted it.
    pub routing_control_arn: String,
    /// Name of this routing control.
    pub routing_control_name: String,
    /// ARN of the control panel hosting it.
    pub control_panel_arn: String,
    /// Name of the control panel hosting it, when the registry still holds that panel.
    pub control_panel_name: Option<String>,
    /// ARN of the cluster the hosting control panel is on. Backs the "every entry of a
    /// batch update is in one cluster" rule, which is a cluster-endpoint property of
    /// this API.
    pub cluster_arn: String,
    /// Current on/off state.
    pub state: RoutingControlState,
    /// Account id of the routing control's owner.
    pub owner: String,
}

/// Per-service state for com-amazonaws-route53recoverycluster-togglecustomerapi. A handler reaches it through [`stores`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53recoveryclusterTogglecustomerapiState {
    /// The switch of every routing control an update has moved, keyed by the routing
    /// control's ARN. A control missing from this map is
    /// [`INITIAL_ROUTING_CONTROL_STATE`]; a switch whose routing control the control
    /// plane no longer holds is dropped by the next write
    /// ([`DataPlane::set_state`]), so the map is bounded by the registry and a deleted
    /// routing control leaves no state behind.
    pub routing_control_states: BTreeMap<String, RoutingControlState>,
}

/// The two stores a handler of this service works over: the control plane's registry of
/// what exists, and this service's own switches.
///
/// Held as the engine's clone-cheap handles, so a handler takes both locks in ONE place
/// ([`Stores::lock`]) and in one order.
pub struct Stores {
    registry: ::engine::ServiceState<Registry>,
    switches: ::engine::ServiceState<ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>,
}

impl Stores {
    /// Lock both stores and hand back the joined view.
    ///
    /// Always the registry first and the switches second: one order for every handler,
    /// so two requests cannot take the pair in opposite orders. The control plane never
    /// reaches for this service's store, so the pair has no cycle.
    pub fn lock(&self) -> DataPlane<'_> {
        let registry = self.registry.lock();
        let switches = self.switches.lock();
        DataPlane { registry, switches }
    }
}

/// The routing controls a cluster endpoint serves: the control plane's registry joined
/// with this service's switches, under one lock on each.
///
/// Every lookup resolves against the registry, so an ARN no control-plane resource was
/// minted for is unknown here however many times it has been asked for, and a resource
/// the control plane deleted is unknown again.
pub struct DataPlane<'a> {
    registry: MutexGuard<'a, Registry>,
    switches: MutexGuard<'a, ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>,
}

impl DataPlane<'_> {
    /// The routing control stored under `arn`, if the control plane holds one.
    pub fn routing_control(&self, arn: &str) -> Option<RoutingControl> {
        self.registry
            .routing_controls
            .get(arn)
            .map(|record| self.project(record))
    }

    /// Whether the control plane holds a control panel under `arn`.
    pub fn holds_control_panel(&self, arn: &str) -> bool {
        self.registry.control_panels.contains_key(arn)
    }

    /// The cluster hosting the control panel that hosts `arn`, if that routing control
    /// is known.
    pub fn cluster_of(&self, routing_control_arn: &str) -> Option<&str> {
        self.registry
            .routing_controls
            .get(routing_control_arn)
            .map(|record| record.cluster_arn.as_str())
    }

    /// The routing controls to report, in ARN order: every one the control plane holds,
    /// or just those hosted in `control_panel_arn` when a filter is given.
    ///
    /// The order is total and stable, which is what lets `ListRoutingControls`' pages
    /// walk the set without repeats or omissions.
    pub fn routing_controls_in(&self, control_panel_arn: Option<&str>) -> Vec<RoutingControl> {
        self.registry
            .routing_controls
            .values()
            .filter(|record| {
                control_panel_arn.is_none_or(|panel| record.control_panel_arn == panel)
            })
            .map(|record| self.project(record))
            .collect()
    }

    /// Move the switch of `routing_control_arn` to `state`, answering the state it was
    /// in before — or `None` when the control plane holds no such routing control, which
    /// is what the caller answers with `ResourceNotFoundException`.
    pub fn set_state(
        &mut self,
        routing_control_arn: &str,
        state: RoutingControlState,
    ) -> Option<RoutingControlState> {
        if !self.registry.routing_controls.contains_key(routing_control_arn) {
            return None;
        }
        let previous = self
            .switches
            .routing_control_states
            .insert(routing_control_arn.to_string(), state)
            .unwrap_or(INITIAL_ROUTING_CONTROL_STATE);

        // The registry decides which switches may exist, so every one it does not back
        // is dropped here: a routing control the control plane has deleted leaves no
        // state behind, and the map stays bounded by the registry rather than by how
        // many resources a caller has created and deleted over time.
        let registry = &self.registry;
        self.switches
            .routing_control_states
            .retain(|arn, _| registry.routing_controls.contains_key(arn));

        Some(previous)
    }

    /// The switch `routing_control_arn` is in right now — [`INITIAL_ROUTING_CONTROL_STATE`]
    /// while no update has moved it.
    pub fn state_of(&self, routing_control_arn: &str) -> RoutingControlState {
        self.switches
            .routing_control_states
            .get(routing_control_arn)
            .copied()
            .unwrap_or(INITIAL_ROUTING_CONTROL_STATE)
    }

    /// The state `routing_control_arn` would be in once `proposed` is applied: the state
    /// the transaction sets it to, or the one it is in now when the transaction does not
    /// name it.
    fn state_after(
        &self,
        routing_control_arn: &str,
        proposed: &[(String, RoutingControlState)],
    ) -> RoutingControlState {
        proposed
            .iter()
            .find(|(arn, _)| arn == routing_control_arn)
            .map_or_else(
                || self.state_of(routing_control_arn),
                |(_, state)| *state,
            )
    }

    /// The first deployed safety rule the transaction `proposed` would leave unsatisfied,
    /// or `None` when every rule that governs it holds — which is what decides whether
    /// the transaction may be written at all.
    ///
    /// A rule governs a transaction that changes one of the controls it guards (an
    /// assertion rule's `AssertedControls`, a gating rule's `TargetControls`), and is
    /// evaluated over the post-transaction state of the controls its configuration reads
    /// (the same `AssertedControls`, a gating rule's `GatingControls`) — so a gating
    /// control is not itself gated by its own rule. A rule whose ARN the caller listed in
    /// `overridden` is left out of the evaluation, and only a rule whose configuration has
    /// finished deploying guards anything.
    ///
    /// Reporting is pure: nothing here writes.
    pub fn safety_rule_violation(
        &self,
        proposed: &[(String, RoutingControlState)],
        overridden: Option<&[String]>,
    ) -> Option<SafetyRuleViolation> {
        let overridden = overridden.unwrap_or_default();

        // Creation order, so the rule a refusal names is stable rather than a function of
        // how the ARNs happen to sort.
        let mut rules: Vec<&SafetyRuleRecord> = self.registry.safety_rules.values().collect();
        rules.sort_by_key(|rule| rule.seq);

        for rule in rules {
            if overridden.contains(&rule.arn) {
                continue;
            }
            if status_at(rule.deploy_due) != STATUS_DEPLOYED {
                continue;
            }

            let (guarded, evaluated) = match &rule.controls {
                SafetyRuleControls::Assertion { asserted_controls } => {
                    (asserted_controls.as_slice(), asserted_controls.as_slice())
                }
                SafetyRuleControls::Gating {
                    gating_controls,
                    target_controls,
                } => (target_controls.as_slice(), gating_controls.as_slice()),
            };

            if !proposed.iter().any(|(arn, _)| guarded.contains(arn)) {
                continue;
            }
            let Some(required_on) = controls_required_on(&rule.rule_config, evaluated.len()) else {
                continue;
            };

            let would_be_on = evaluated
                .iter()
                .filter(|arn| self.state_after(arn, proposed) == RoutingControlState::On)
                .count();
            // `Inverted` is the logical negation of the rule, so it flips the verdict
            // rather than the count.
            if (would_be_on >= required_on) == rule.rule_config.inverted {
                return Some(SafetyRuleViolation {
                    safety_rule_arn: rule.arn.clone(),
                    message: safety_rule_violation_message(
                        &rule_condition(required_on, rule.rule_config.inverted),
                        proposed.iter().map(|(arn, _)| arn.as_str()),
                        &rule.arn,
                    ),
                });
            }
        }

        None
    }

    /// One registry record as the routing control this data plane reports.
    fn project(&self, record: &RoutingControlRecord) -> RoutingControl {
        RoutingControl {
            routing_control_arn: record.arn.clone(),
            routing_control_name: record.name.clone(),
            control_panel_arn: record.control_panel_arn.clone(),
            control_panel_name: self
                .registry
                .control_panels
                .get(&record.control_panel_arn)
                .map(|panel| panel.name.clone()),
            cluster_arn: record.cluster_arn.clone(),
            state: self.state_of(&record.arn),
            owner: record.owner.clone(),
        }
    }
}

/// A deployed safety rule a proposed transaction would leave unsatisfied.
///
/// Carries the rule's ARN as well as the sentence the refusal states, because that ARN is
/// what a caller puts in `SafetyRulesToOverride` to force the change through.
#[derive(Debug, Clone)]
pub struct SafetyRuleViolation {
    /// ARN of the rule whose configuration does not hold.
    pub safety_rule_arn: String,
    /// The `ValidationException$Message` the refusal states.
    pub message: String,
}

/// How many of the routing controls a rule evaluates must be `On` for its configuration
/// to hold, before `Inverted` is applied.
///
/// The modelled `RuleType` values define it
/// (`smithy_query(query="doc",shape="RuleType",model_path="services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json")`):
/// `ATLEAST` is the rule's own `Threshold` ("at least N routing controls must be set",
/// N being the threshold), `AND` is every control the rule evaluates ("a shortcut for
/// 'at least N', where N is the total number of controls in the rule") and `OR` is one
/// ("a shortcut for 'at least N', where N is 1"). A type outside that value set is no
/// modelled rule type and decides nothing, so such a rule guards nothing.
fn controls_required_on(config: &RuleConfig, evaluated: usize) -> Option<usize> {
    match config.r#type.as_str() {
        "ATLEAST" => Some(config.threshold.max(0).unsigned_abs() as usize),
        "AND" => Some(evaluated),
        "OR" => Some(1),
        _ => None,
    }
}

/// The condition a blocking safety rule imposes, in the words its `RuleConfig` reads as:
/// the quoted clause of [`safety_rule_violation_message`].
///
/// `required_on` is how many of the controls the rule evaluates its configuration needs
/// `On` ([`controls_required_on`]), so one clause covers every modelled `RuleType`: the
/// `Threshold` of an `ATLEAST` rule, the number of controls of an `AND` rule and one for
/// an `OR` rule, which the model defines as the same "at least N" with N fixed
/// (`smithy_query(query="doc",shape="RuleType",model_path="services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json")`).
/// `Inverted` is the logical negation of the rule ("if the rule would usually evaluate
/// true, it's evaluated as false, and vice versa"), so it states the negated clause:
/// fewer than N rather than at least N.
fn rule_condition(required_on: usize, inverted: bool) -> String {
    if inverted {
        format!("Fewer than {required_on} routing controls must be On")
    } else {
        format!("At least {required_on} routing controls must be On")
    }
}

/// The `ValidationException$Message` an update a safety rule refuses states.
///
/// Route 53 ARC words this refusal as the condition that does not hold, quoted, then the
/// remediation that gets the caller through it — the `AllowSafetyRulesOverride` guidance,
/// which is what `SafetyRulesToOverride` on the request does — and then two labelled
/// lines naming the routing controls the request asked to move and the rule that blocked
/// it, so a caller can retry overriding exactly that rule. Everything outside those three
/// slots is fixed text: the rule's name, how many of its controls are `On` and the raw
/// `RuleConfig` members are not stated.
fn safety_rule_violation_message<'a>(
    condition: &str,
    routing_control_arns: impl Iterator<Item = &'a str>,
    safety_rule_arn: &str,
) -> String {
    let routing_controls: Vec<&str> = routing_control_arns.collect();
    format!(
        "Can't update the routing control state because of a configured safety rule: \
         \"{condition}.\" To override the rule and change the state, see the guidance for \
         AllowSafetyRulesOverride in the Route 53 ARC Developer Guide. \nRouting control: \
         [{controls}] \nSafety rule: {safety_rule_arn}",
        controls = routing_controls.join(", "),
    )
}

/// The `ResourceNotFoundException$ResourceType` a routing control lookup reports.
///
/// The modelled member is documented "Hypothetical resource type that was not found",
/// so it names the KIND of resource the request addressed; both operations that take a
/// `RoutingControlArn` report it from here.
pub const ROUTING_CONTROL_RESOURCE_TYPE: &str = "RoutingControl";

/// The `ResourceNotFoundException$ResourceType` a control panel lookup reports.
pub const CONTROL_PANEL_RESOURCE_TYPE: &str = "ControlPanel";

/// The `ResourceNotFoundException$Message` for a `RoutingControlArn` that names no
/// routing control.
///
/// Route 53 ARC's cluster API words this refusal `Routing Control '<arn>' not found`:
/// the kind of resource that was looked up, then the ARN the request sent, quoted. Both
/// operations that take a `RoutingControlArn` state it from here, so they cannot word
/// the same refusal differently.
pub fn routing_control_not_found_message(routing_control_arn: &str) -> String {
    format!("Routing Control '{routing_control_arn}' not found")
}

/// The `ResourceNotFoundException$Message` for a `ControlPanelArn` filter that names no
/// control panel.
///
/// Route 53 ARC words the control-panel refusal the other way round —
/// `Control panel not found: '<arn>'`, the quoted ARN AFTER the diagnosis — so it is
/// stated here rather than folded into
/// [`routing_control_not_found_message`].
pub fn control_panel_not_found_message(control_panel_arn: &str) -> String {
    format!("Control panel not found: '{control_panel_arn}'")
}

/// This service's own store (the switches) in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoveryclusterTogglecustomerapiState>)
}

/// The control plane's registry in the SAME shared engine — resolved by the owning
/// crate's own accessor, so there is one copy of every cluster, control panel and
/// routing control rather than two that can drift, and one reading of where that store
/// lives.
pub fn registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<Registry>> {
    recovery_control_config::state::service_state(engine)
}

/// Both stores at once: what every handler of this service starts from.
pub fn stores<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Stores> {
    Some(Stores {
        registry: registry(engine)?,
        switches: service_state(engine)?,
    })
}
