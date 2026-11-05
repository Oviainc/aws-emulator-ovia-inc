// SPDX-License-Identifier: CC0-1.0

//! What a `serviceRegistries` entry has to name, and what it is refused for.
//!
//! The registry is a CLOUD MAP service (`ServiceRegistry$registryArn`: "The Amazon Resource
//! Name (ARN) of the service registry. The currently supported service registry is Cloud
//! Map"), so every rule here is decided against THAT service's own record — whether Cloud
//! Map holds it at all, and which `DnsRecord$Type` values it publishes — together with the
//! network mode of the revision whose tasks the entry would register.
//!
//! Shared by every operation that attaches a registry, so `CreateService`, `UpdateService`
//! and `CreateTaskSet` cannot answer differently about the same entry.
//!
//! LOCK ORDER: the Cloud Map read takes that store's lock, holds no guard of this service's
//! store while it does, and hands back owned values — so it is called with NO guard of ours
//! held, which is what keeps a lock cycle between the two stores unreachable.

use super::*;

/// `ClientException$message` for a `serviceRegistries` entry naming a Cloud Map service
/// that service holds no record of.
///
/// Amazon ECS validates the entry by asking Cloud Map for it: the refusal it answers reports
/// that the registry could not be validated and quotes the error Route 53 / Cloud Map gave
/// back, and names no value from the request.
pub const SERVICE_REGISTRY_NOT_FOUND_MESSAGE: &str =
    "The service registry could not be validated. Route 53 error: \"ServiceNotFound.\"";

/// The Cloud Map `DnsRecord$Type` that publishes a PORT beside the address it resolves to,
/// and so the only record type a task reached through the host's or the bridge's network
/// can be registered in.
pub const DNS_RECORD_TYPE_SRV: &str = "SRV";

/// The Cloud Map `DnsRecord$Type` values that resolve to an ADDRESS ONLY — IPv4 `A` and
/// IPv6 `AAAA` — so a task registered in one of them is reached at no port of its own and
/// has to own the address the record carries.
pub const DNS_RECORD_TYPES_ADDRESS_ONLY: [&str; 2] = ["A", "AAAA"];

/// How many `serviceRegistries` entries one service may carry. `ServiceRegistry`'s own doc:
/// "Each service may be associated with one service registry. Multiple service registries
/// for each service are not supported."
///
/// The list is caller-supplied and every entry costs a lookup in another service's store
/// and one Cloud Map instance per task, so the bound is what keeps the work ONE request can
/// ask for finite — checked before any per-entry work and before either store is locked.
pub const MAX_SERVICE_REGISTRIES: usize = 1;

/// `InvalidParameterException$message` for a `serviceRegistries` list longer than
/// [`MAX_SERVICE_REGISTRIES`], worded from the sentence the model states the rule in.
pub const MULTIPLE_SERVICE_REGISTRIES_MESSAGE: &str =
    "Multiple service registries for each service are not supported.";

/// The refusal a `serviceRegistries` list earns for its LENGTH alone, or `None` when it
/// names at most [`MAX_SERVICE_REGISTRIES`] entries.
pub fn service_registry_count_violation(
    registries: &[ServiceRegistry],
) -> Option<&'static str> {
    (registries.len() > MAX_SERVICE_REGISTRIES).then_some(MULTIPLE_SERVICE_REGISTRIES_MESSAGE)
}

/// The Cloud Map service id each `serviceRegistries` entry names, deduplicated in the order
/// the list named them.
pub fn requested_registry_ids(registries: &[ServiceRegistry]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for registry in registries {
        let Some(arn) = registry.registry_arn.as_deref() else {
            continue;
        };
        let id = registry_service_id(arn).to_string();
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    ids
}

/// The Cloud Map service id ONE `registryArn` names: the last segment of
/// `arn:aws:servicediscovery:<region>:<account>:service/<id>`, which is what Cloud Map
/// stores a service under, and the value itself when it carries no path.
pub fn registry_service_id(registry_arn: &str) -> &str {
    registry_arn.rsplit('/').next().unwrap_or(registry_arn)
}

/// Which of `ids` the Cloud Map (service discovery) service still holds a service for and
/// the `DnsRecord$Type` values each of those services publishes, read from THAT service's
/// own store through the one shared engine — so a registry created, reconfigured or deleted
/// over its API is seen here with no propagation step and no second copy of it kept in this
/// service's state.
///
/// A service Cloud Map holds is a KEY of the result; its record types are the value, empty
/// for an HTTP-only service that configures no `DnsConfig` at all. So an id the map does not
/// hold is one Cloud Map has no service for ([`service_registry_refusal`]), and an id it
/// holds with no record types publishes no DNS record to be held to
/// ([`service_registry_binding_violation`]).
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own store
/// being unreachable.
pub fn cloud_map_service_dns_records<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ids: &[String],
) -> Option<HashMap<String, Vec<String>>> {
    let state =
        ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        ids.iter()
            .filter_map(|id| {
                let service = store.services.get(id)?;
                let record_types = service
                    .dns_config
                    .as_ref()
                    .map(|config| {
                        config
                            .dns_records
                            .iter()
                            .map(|record| record.r#type.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                Some((id.clone(), record_types))
            })
            .collect(),
    )
}

/// The refusal a `serviceRegistries` list a request NAMES earns, or `None` when Cloud Map
/// holds the service every entry names.
///
/// `known` is what [`cloud_map_service_dns_records`] read for [`requested_registry_ids`] of
/// the same list: an id it does not hold names a registry Amazon ECS cannot validate, so the
/// request is refused and the service is neither created nor changed.
pub fn service_registry_refusal(
    registries: &[ServiceRegistry],
    known: &HashMap<String, Vec<String>>,
) -> Option<&'static str> {
    requested_registry_ids(registries)
        .into_iter()
        .any(|id| !known.contains_key(&id))
        .then_some(SERVICE_REGISTRY_NOT_FOUND_MESSAGE)
}

/// The refusal a `serviceRegistries` list earns against the revision whose tasks it would
/// register, or `None` when every entry names what that revision and its registry need.
///
/// The rule, stated once for every operation that attaches a registry: what an entry has to
/// name is decided by the Cloud Map service's own DNS record type together with the
/// revision's `networkMode`, because between them they fix how a task of that revision is
/// reached.
///
/// * `ServiceRegistry$containerName` is "The container name value ... It's already specified
///   in the task definition", so a name none of the revision's `containerDefinitions`
///   carries names no container to register and is refused first, whatever the record type.
/// * An address-only record ([`DNS_RECORD_TYPES_ADDRESS_ONLY`]) resolves to an address its
///   owner holds alone, and a task on the host's or the bridge's network shares the
///   container instance's address with every other task on it — so that pairing is refused
///   and `SRV` is named as the record type that works.
/// * "If the task definition that your service task specifies uses the `bridge` or `host`
///   network mode, you must specify a `containerName` and `containerPort` combination from
///   the task definition": such a task is reached at the port the container was published
///   on, so an entry naming neither has nothing to register.
/// * "If the task definition ... uses the `awsvpc` network mode and a type SRV DNS record is
///   used, you must specify either a `containerName` and `containerPort` combination or a
///   `port` value. However, you can't specify both": an `SRV` record publishes exactly ONE
///   port, so the entry has to say which one it is exactly once — a half pair, no source at
///   all, or two sources are all refused.
/// * A pair the entry does name has to be one the revision declares — the container one of
///   its `containerDefinitions` and the port one that container publishes — which is the
///   same "from the task definition" rule a load-balancer binding is held to
///   ([`load_balancer_binding_violation`]).
///
/// `dns_records` is what [`cloud_map_service_dns_records`] read for
/// [`requested_registry_ids`] of the same list: an entry whose registry it has no record
/// types for is held only to the rules that do not depend on one.
pub fn service_registry_binding_violation(
    definition: Option<&TaskDefinitionRecord>,
    registries: &[ServiceRegistry],
    dns_records: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let definition = definition?;
    let mode = definition.network_mode.as_str();
    let on_the_host_network = mode == NETWORK_MODE_HOST || mode == NETWORK_MODE_BRIDGE;
    for registry in registries {
        let registry_arn = registry.registry_arn.as_deref().unwrap_or_default();
        let record_types = dns_records
            .get(registry_service_id(registry_arn))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let container = match registry.container_name.as_deref() {
            None => None,
            Some(container_name) => {
                let Some(container) = definition
                    .container_definitions
                    .iter()
                    .find(|container| container.name.as_deref() == Some(container_name))
                else {
                    return Some(format!(
                        "The container {container_name} is not valid. The value for \
                         'containerName' must already be specified in the task definition. \
                         Registry: {registry_arn}"
                    ));
                };
                Some((container_name, container))
            }
        };
        if on_the_host_network {
            if let Some(record_type) = record_types
                .iter()
                .find(|record_type| DNS_RECORD_TYPES_ADDRESS_ONLY.contains(&record_type.as_str()))
            {
                return Some(format!(
                    "The specified serviceRegistries value is configured to use a type \
                     '{record_type}' DNS record, which is not supported when specifying \
                     '{NETWORK_MODE_HOST}' or '{NETWORK_MODE_BRIDGE}' for networkMode. Change \
                     to '{DNS_RECORD_TYPE_SRV}' record, which is the supported DNS record \
                     type. Registry: {registry_arn}"
                ));
            }
            if registry.container_name.is_none() || registry.container_port.is_none() {
                return Some(format!(
                    "When specifying '{NETWORK_MODE_HOST}' or '{NETWORK_MODE_BRIDGE}' for \
                     networkMode, values for 'containerName' and 'containerPort' must be \
                     specified from the task definition. Registry: {registry_arn}"
                ));
            }
        }
        if record_types
            .iter()
            .any(|record_type| record_type == DNS_RECORD_TYPE_SRV)
        {
            // ONE port, named exactly once: the complete pair, or `port`, and never a half
            // pair and never both.
            let pair = registry.container_name.is_some() && registry.container_port.is_some();
            let half_pair = registry.container_name.is_some() != registry.container_port.is_some();
            if half_pair || pair == registry.port.is_some() {
                return Some(format!(
                    "When specifying a value for serviceRegistries that is configured to use \
                     a type '{DNS_RECORD_TYPE_SRV}' DNS record, you must also enter a value \
                     for either 'port' or the 'containerName' and 'containerPort' \
                     combination, but not both. Registry: {registry_arn}"
                ));
            }
        }
        let (Some((container_name, container)), Some(container_port)) =
            (container, registry.container_port)
        else {
            continue;
        };
        let declared = container
            .port_mappings
            .iter()
            .flatten()
            .any(|mapping| port_mapping_covers(mapping, container_port));
        if !declared {
            return Some(format!(
                "The container {container_name} did not have a container port \
                 {container_port} defined."
            ));
        }
    }
    None
}
