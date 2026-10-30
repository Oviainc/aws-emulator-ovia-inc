Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f30b839d650f592709ba7d30fd0311226ca1f682

# The "a DELETED environment is gone for addressing purposes" rule is written out inline in thirteen handlers, while the crate single-homes the same rule for connectors

_Filed: 2026-10-29T16:27:30Z_
_Operation: CreateEnvironmentConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f30b839d650f592709ba7d30fd0311226ca1f682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate resolves "the environment a request addresses" through four state-module lookups — `environment_in_region`, `environment_in_region_mut`, `environment_by_arn`, `environment_by_arn_mut` — each of which single-homes HALF of the addressing rule (the Region scope) with its own doc. The OTHER half of the same rule, "a record that has reached `DELETED` resolves to nothing", is not in the state module at all: it is restated as a `.filter(|environment| environment.state != crate::state::STATE_DELETED)` immediately after the lookup in THIRTEEN handlers, each with its own prose copy of the justification.

The crate already demonstrates the single-homed form for the sibling record kind: a connector's identical rule lives in `StoredConnector::is_live` and is reached through `live_connector` / `live_connector_mut`, whose own doc says it exists so "a reader and a writer resolve exactly the same records" — and the handlers that use a connector call those helpers rather than re-deriving the predicate. Hosts likewise go through `StoredEnvironment::live_hosts`. Only the ENVIRONMENT, the parent resource every one of those handlers resolves first, has its liveness rule copied per handler, so the thirteen copies can drift: one handler can be changed (or a new handler added) that resolves a `DELETED` environment while the other twelve refuse it, and nothing links them.

The fix is a pure extraction — a `live_environment_in_region(_mut)` / `live_environment_by_arn(_mut)` pair in `crate::state` beside the four lookups it wraps, called from the thirteen handlers. Every response and every existing test stays byte-identical: the predicate is the same comparison against the same constant in all thirteen places today.

(Scope note: `GetEnvironment` and `ListEnvironments` deliberately DO report a `DELETED` environment — pinned by tests/all/environment_lifecycle.rs:646-660 — so they are not among the thirteen and are not changed by the extraction.)

## Evidence

The rule, single-homed for the sibling record kind — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:797-812:
```
    /// Whether this record is still a connector a request can USE: one whose teardown
    /// has not started.
    ///
    /// A `DELETING` connector's connection to the appliance is being torn down and a
    /// `DELETED` one is gone, so neither is a connector further work can be tracked
    /// through ("The resource might not be specified correctly, or it may have a
    /// `state` of `DELETED`", `ResourceNotFoundException` doc). ...
    ///
    /// The one predicate behind [`live_connector`] and [`live_connector_mut`], so a
    /// reader and a writer resolve exactly the same records.
    pub fn is_live(&self) -> bool {
        self.state != CONNECTOR_DELETING && self.state != CONNECTOR_DELETED
    }
```
and the two helpers that compose it with the map lookup — state.rs:3246-3270:
```
pub fn live_connector<'a>(
    environment: &'a StoredEnvironment,
    connector_id: &str,
) -> Option<&'a StoredConnector> {
    environment
        .connectors
        .get(connector_id)
        .filter(|connector| connector.is_live())
}
...
pub fn live_connector_mut<'a>(
    environment: &'a mut StoredEnvironment,
    connector_id: &str,
) -> Option<&'a mut StoredConnector> {
    environment
        .connectors
        .get_mut(connector_id)
        .filter(|connector| connector.is_live())
}
```

The environment lookups, which single-home only the REGION half — state.rs:2891-2904:
```
    /// The environment `environment_id` names, if this service holds it IN `region`.
    ///
    /// An Amazon EVS environment is a regional resource: it exists at the endpoint of
    /// the Region it was created in and nowhere else, so a lookup from another Region
    /// is a miss, exactly as the ARN this service minted for it says.
    pub fn environment_in_region(
        &self,
        region: &str,
        environment_id: &str,
    ) -> Option<&StoredEnvironment> {
        self.environments
            .get(environment_id)
            .filter(|environment| environment.region == region)
    }
```
(and `environment_in_region_mut` state.rs:2906-2915, `environment_by_arn` state.rs:2917-2930, `environment_by_arn_mut` state.rs:2932-2941 — none of the four mentions `DELETED`).

The thirteen inline copies of the missing half, each `.filter(|environment| environment.state != crate::state::STATE_DELETED)` applied to one of those four lookups:
* src/ops/associate_eip_to_vlan.rs:243
* src/ops/create_entitlement.rs:218
* src/ops/create_environment_connector.rs:262
* src/ops/create_environment_host.rs:267
* src/ops/delete_entitlement.rs:228
* src/ops/delete_environment_connector.rs:238
* src/ops/delete_environment_host.rs:236
* src/ops/disassociate_eip_from_vlan.rs:220
* src/ops/get_depot_url.rs:207
* src/ops/list_tags_for_resource.rs:173
* src/ops/tag_resource.rs:212
* src/ops/untag_resource.rs:190
* src/ops/update_environment_connector.rs:241

Two of the thirteen, showing that the prose justification is copied with the predicate — src/ops/create_environment_connector.rs:255-267:
```
        // Regional resource: an environment exists at the endpoint of the Region it was
        // created in, so a connector can only be created in one from that Region. An
        // environment that has reached DELETED is gone as far as a create is concerned:
        // "The resource might not be specified correctly, or it may have a state of
        // DELETED" (`ResourceNotFoundException` doc).
        let environment = state
            .environment_in_region_mut(&region, &input.environment_id)
            .filter(|environment| environment.state != crate::state::STATE_DELETED)
            .ok_or_else(|| {
                CreateEnvironmentConnectorErrorModeled::ResourceNotFoundException(
                    crate::state::environment_not_found(&input.environment_id),
                )
            })?;
```
and src/ops/get_depot_url.rs:200-212:
```
        // Regional resource: an environment exists at the endpoint of the Region it was
        // created in, so a request from any other Region is a miss — as is one naming an
        // environment that has reached DELETED: "The resource might not be specified
        // correctly, or it may have a state of DELETED" (`ResourceNotFoundException`
        // doc).
        let environment = state
            .environment_in_region_mut(&region, &input.environment_id)
            .filter(|environment| environment.state != crate::state::STATE_DELETED)
```

Existing tests that pin the behaviour the extraction must leave unchanged: tests/all/entitlements.rs:554 (`an_environment_that_has_reached_deleted_holds_no_new_entitlement`), tests/all/connectors.rs:808 (`an_environment_that_has_reached_deleted_holds_no_new_or_updated_connector`), tests/all/tagging_resource_arn_resolution.rs:273 (`a_resource_arn_stops_resolving_once_its_environment_reaches_deleted`), tests/all/depot_url.rs:357 (`a_deleted_environment_has_no_depot_url`), and — for the two readers that deliberately DO report a DELETED environment — tests/all/environment_lifecycle.rs:646-660.

## Steps to Reproduce

1. Open src/state.rs:3246-3270 and src/state.rs:797-812: the connector's "still usable" rule is one predicate, reached through two helpers, so no handler restates it.
2. Open src/state.rs:2891-2941: none of the four environment lookups mentions `DELETED`; they scope only by Region.
3. Grep the crate for `state != crate::state::STATE_DELETED`: thirteen handler copies come back (associate_eip_to_vlan.rs:243, create_entitlement.rs:218, create_environment_connector.rs:262, create_environment_host.rs:267, delete_entitlement.rs:228, delete_environment_connector.rs:238, delete_environment_host.rs:236, disassociate_eip_from_vlan.rs:220, get_depot_url.rs:207, list_tags_for_resource.rs:173, tag_resource.rs:212, untag_resource.rs:190, update_environment_connector.rs:241), each applied to one of those four lookups.
4. Change the predicate in ONE of them (for example widen it to exclude `STATE_DELETING` as well) and note that nothing links the other twelve: the thirteen addressing paths can disagree about which environments resolve.
5. Confirm a behaviour-preserving fix by stating the rule once in crate::state beside the four lookups and re-running tests/all/entitlements.rs:554, tests/all/connectors.rs:808, tests/all/tagging_resource_arn_resolution.rs:273, tests/all/depot_url.rs:357 and tests/all/environment_lifecycle.rs:646-660 unchanged.
