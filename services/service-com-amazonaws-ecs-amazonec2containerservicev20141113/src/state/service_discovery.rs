// SPDX-License-Identifier: CC0-1.0

//! The Cloud Map instances Amazon ECS registers for the tasks of a service that names a
//! `serviceRegistries` entry — the registration that makes a service registry do something.
//!
//! `ServiceRegistry` is "The details for the service registry ... When you add, update, or
//! remove the service registries configuration, Amazon ECS starts a new deployment. New
//! tasks are registered and deregistered to the updated service registry configuration":
//! the registry is a CLOUD MAP service, and what Amazon ECS puts in it is one INSTANCE per
//! task of the ECS service, carrying the address and the port that task is reached at. So a
//! client that discovers instances of the Cloud Map service finds the service's tasks.
//!
//! The registration is reconciled rather than appended: the instances Amazon ECS owns in a
//! Cloud Map service are exactly the ones its live tasks need, so a task that stopped is
//! deregistered and a service whose registries were removed leaves none behind. An instance
//! registered through Cloud Map's own `RegisterInstance` carries no
//! [`CLOUD_MAP_ATTRIBUTE_ECS_SERVICE_NAME`] and is never touched.
//!
//! LOCK ORDER: [`reconcile_cloud_map_instances`] takes the CLOUD MAP store's lock and holds
//! no guard of this service's store while it does — the same one-directional discipline
//! every other cross-store access here follows, which is what keeps a cycle between the two
//! stores unreachable.

use super::*;

/// `AWS_INSTANCE_IPV4`: "If the service configuration includes an `A` record, the IPv4
/// address that you want Route 53 to return in response to DNS queries"
/// (`RegisterInstanceRequest$Attributes`) — for a task, the address of the elastic network
/// interface it was allocated.
pub const CLOUD_MAP_ATTRIBUTE_IPV4: &str = "AWS_INSTANCE_IPV4";

/// `AWS_INSTANCE_PORT`: "the port on the endpoint that you want Route 53 to send requests
/// to" — for a task, the port the registry entry named.
pub const CLOUD_MAP_ATTRIBUTE_PORT: &str = "AWS_INSTANCE_PORT";

/// The attribute naming the ECS cluster the registered task runs in.
pub const CLOUD_MAP_ATTRIBUTE_ECS_CLUSTER_NAME: &str = "ECS_CLUSTER_NAME";

/// The attribute naming the ECS service whose scheduler placed the registered task. It is
/// also what marks an instance as Amazon ECS's to reconcile.
pub const CLOUD_MAP_ATTRIBUTE_ECS_SERVICE_NAME: &str = "ECS_SERVICE_NAME";

/// The attribute naming the task-definition family the registered task runs.
pub const CLOUD_MAP_ATTRIBUTE_ECS_TASK_DEFINITION_FAMILY: &str = "ECS_TASK_DEFINITION_FAMILY";

/// The attribute naming the Region the registered task runs in.
pub const CLOUD_MAP_ATTRIBUTE_REGION: &str = "REGION";

/// One Cloud Map instance Amazon ECS holds for one task of one service registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudMapRegistration {
    /// The Cloud Map service the instance belongs to: the id in the entry's `registryArn`.
    pub cloud_map_service_id: String,
    /// `RegisterInstanceRequest$InstanceId` — the ECS task id, so one task is one instance
    /// and a re-reconcile of the same task replaces nothing.
    pub instance_id: String,
    /// The attributes the instance carries, ordered by key so two reconciles of the same
    /// task compare equal.
    pub attributes: Vec<(String, String)>,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every Cloud Map instance this store's services need, as of the state it holds now.
    ///
    /// One per (`RUNNING` task, registry entry) pair of every ACTIVE service that names a
    /// registry: a task Amazon ECS has started is one a client should discover, and a task
    /// that is still provisioning or has stopped serves nothing, so it is not published.
    ///
    /// A task is registered at the address of the interface it was allocated, so only a task
    /// that HOLDS one is registered: `AWS_INSTANCE_IPV4` is the address a DNS query is
    /// answered with, and a task whose network mode gives it no interface of its own is
    /// reached at its container instance's address, which this store does not hold.
    ///
    /// At most one entry per (Cloud Map service, task): a service carries at most
    /// [`MAX_SERVICE_REGISTRIES`] registries, and an id already registered for is not added
    /// twice — so the list is bounded by the tasks this store holds ([`MAX_TASKS`]).
    pub fn cloud_map_registrations(&self) -> Vec<CloudMapRegistration> {
        let mut registrations = Vec::new();
        let mut registered = HashSet::new();
        for service in self.services.values() {
            if service.status != SERVICE_ACTIVE || service.service_registries.is_empty() {
                continue;
            }
            let region = arn_region(&service.arn);
            for task in self.tasks.values() {
                if task.cluster_name != service.cluster_name
                    || task.service_name.as_deref() != Some(service.name.as_str())
                    || !task_serves(task)
                {
                    continue;
                }
                let Some(address) = task
                    .attachments
                    .iter()
                    .find_map(AttachmentRecord::private_ipv4_address)
                else {
                    continue;
                };
                for registry in &service.service_registries {
                    let Some(arn) = registry.registry_arn.as_deref() else {
                        continue;
                    };
                    let cloud_map_service_id = registry_service_id(arn).to_string();
                    if !registered.insert((cloud_map_service_id.clone(), task.task_id.clone())) {
                        continue;
                    }
                    let mut attributes = vec![
                        (
                            CLOUD_MAP_ATTRIBUTE_IPV4.to_string(),
                            address.to_string(),
                        ),
                        (
                            CLOUD_MAP_ATTRIBUTE_ECS_CLUSTER_NAME.to_string(),
                            service.cluster_name.clone(),
                        ),
                        (
                            CLOUD_MAP_ATTRIBUTE_ECS_SERVICE_NAME.to_string(),
                            service.name.clone(),
                        ),
                        (
                            CLOUD_MAP_ATTRIBUTE_ECS_TASK_DEFINITION_FAMILY.to_string(),
                            task.family.clone(),
                        ),
                    ];
                    if let Some(region) = region.as_deref() {
                        attributes.push((
                            CLOUD_MAP_ATTRIBUTE_REGION.to_string(),
                            region.to_string(),
                        ));
                    }
                    // The port the entry named: its own `port` for a registry that carries
                    // one, the container port otherwise — the two forms a `serviceRegistries`
                    // entry names a port in.
                    if let Some(port) = registry.port.or(registry.container_port) {
                        attributes
                            .push((CLOUD_MAP_ATTRIBUTE_PORT.to_string(), port.to_string()));
                    }
                    attributes.sort();
                    registrations.push(CloudMapRegistration {
                        cloud_map_service_id,
                        instance_id: task.task_id.clone(),
                        attributes,
                    });
                }
            }
        }
        registrations.sort_by(|left, right| {
            (&left.cloud_map_service_id, &left.instance_id)
                .cmp(&(&right.cloud_map_service_id, &right.instance_id))
        });
        registrations
    }
}

/// Whether a task is one a client should discover: Amazon ECS registers a task's instance
/// once the task is `RUNNING`, so a task that is still provisioning or starting — and one
/// that is stopping or stopped — is not published as an endpoint a query would be answered
/// with.
fn task_serves(task: &TaskRecord) -> bool {
    task.desired_status != STATUS_STOPPED && task.last_status == STATUS_RUNNING
}

/// The Region segment of an ARN (`arn:aws:<service>:<region>:…`), or `None` for a value that
/// is not one.
fn arn_region(arn: &str) -> Option<String> {
    let mut segments = arn.split(':');
    (segments.next()? == "arn")
        .then(|| segments.nth(2).map(str::to_string))
        .flatten()
        .filter(|region| !region.is_empty())
}

/// Make the Cloud Map instances Amazon ECS owns exactly `desired`, in the Cloud Map
/// (service discovery) service's OWN store through the one shared engine — so an instance
/// this registers is one Cloud Map returns from its own reads (`DiscoverInstances`,
/// `ListInstances`, `GetInstance`) and counts in `InstanceCount`.
///
/// An instance carrying no [`CLOUD_MAP_ATTRIBUTE_ECS_SERVICE_NAME`] was registered through
/// Cloud Map's own API and is left exactly as it is. One that does is Amazon ECS's: it is
/// updated when the task it stands for changed, and deregistered when `desired` no longer
/// holds it.
///
/// `InstancesRevision` is "A unique string that identifies the request and that allows
/// failed requests to be retried without the risk of running the operation twice", bumped
/// per Cloud Map service exactly when this reconcile changed that service's instances, so a
/// reconcile that found everything already in place moves nothing.
///
/// Nothing happens when the Cloud Map store is unreachable (the engine is not the shared
/// in-memory one): there is then no Cloud Map service to register into and none Amazon ECS
/// could have registered into, so there is nothing to add and nothing to clean up — which is
/// why this answers nothing rather than failing the request that called it.
pub fn reconcile_cloud_map_instances<Eng: Send + Sync + 'static>(
    engine: &Eng,
    desired: &[CloudMapRegistration],
) {
    let Some(state) =
        ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::service_state(
            engine,
        )
    else {
        return;
    };
    let mut store = state.lock();
    for (service_id, service) in store.services.iter_mut() {
        let wanted: Vec<&CloudMapRegistration> = desired
            .iter()
            .filter(|registration| &registration.cloud_map_service_id == service_id)
            .collect();
        let mut changed = false;
        service.instances.retain(|instance_id, instance| {
            let ecs_owned = instance
                .attributes
                .contains_key(CLOUD_MAP_ATTRIBUTE_ECS_SERVICE_NAME);
            let keep = !ecs_owned
                || wanted
                    .iter()
                    .any(|registration| &registration.instance_id == instance_id);
            changed |= !keep;
            keep
        });
        // An instance Amazon ECS registers for a task it placed is reported as healthy
        // where the service's health checking is the CUSTOM kind, whose initial status the
        // registrar supplies (`AWS_INIT_HEALTH_STATUS`); otherwise it starts `UNKNOWN`, as
        // any registration does, and the service's own checker is what moves it.
        let health_status = if service.custom_health_checked() {
            ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::HEALTH_STATUS_HEALTHY
        } else {
            ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::HEALTH_STATUS_UNKNOWN
        };
        for registration in wanted {
            let attributes: ::std::collections::HashMap<String, String> =
                registration.attributes.iter().cloned().collect();
            if let Some(held) = service.instances.get(&registration.instance_id) {
                if held.attributes == attributes {
                    continue;
                }
            } else if service.instances.len()
                >= ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::MAX_INSTANCES_PER_SERVICE
            {
                // The Cloud Map service holds its quota of instances, so this task has
                // nowhere to be registered; the ones already there are left alone.
                continue;
            }
            service.instances.insert(
                registration.instance_id.clone(),
                ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::InstanceRecord {
                    instance_id: registration.instance_id.clone(),
                    attributes,
                    // Nothing supplied a `CreatorRequestId`: this registration is Amazon
                    // ECS's own, not a client call being made idempotent.
                    creator_request_id: None,
                    health_status: health_status.to_string(),
                    pending_health_status: None,
                },
            );
            changed = true;
        }
        if changed {
            service.instances_revision += 1;
        }
    }
}
