// SPDX-License-Identifier: CC0-1.0

//! Whether a launched task can FETCH the images its containers run from where it was
//! placed, and the stop a task that cannot earns.
//!
//! A container runs an image, and the image is fetched from its registry before the
//! container starts — so a task whose network placement has no path to that registry never
//! runs at all. `Task$stopCode` is "the stop code indicating why a task was stopped", and
//! the `TaskStopCode` member for a task that failed before its containers started is
//! [`STOP_CODE_TASK_FAILED_TO_START`]; `Task$stoppedReason` ("The reason that the task was
//! stopped") carries the fetch failure itself, which the container runtime words as a
//! `CannotPullContainerError` ([`cannot_pull_container_message`]).
//!
//! The path is Amazon VPC's, read off the placement's own subnet:
//!
//! * a subnet whose route table sends `0.0.0.0/0` to a PUBLIC NAT gateway reaches the
//!   internet — "with a public NAT gateway, internet-bound traffic from a private subnet
//!   can be routed to the NAT gateway" (`CreateNatGateway`), which a PRIVATE one does not
//!   do;
//! * a subnet whose route table sends `0.0.0.0/0` to an internet gateway reaches it only
//!   for a task that was given a public address — "to use an internet gateway, your
//!   subnet's route table must contain a route that directs internet-bound traffic to the
//!   internet gateway" (Amazon VPC User Guide) and the instance must have a public address,
//!   which for an `awsvpc` task is `AwsVpcConfiguration$assignPublicIp`
//!   ([`ASSIGN_PUBLIC_IP_ENABLED`]);
//! * a subnet with no route off the VPC at all can still reach AMAZON ECR, through the
//!   endpoints that service publishes: an interface endpoint for `ecr.api`, one for
//!   `ecr.dkr` and a gateway endpoint for Amazon S3, which the image layers come from
//!   (Amazon ECR User Guide, "Amazon ECR interface VPC endpoints"). A public registry is
//!   not an AWS service and publishes no endpoint, so an image held in one needs the
//!   internet.
//!
//! The verdict is reached at LAUNCH, by the call that places the task, and stored on the
//! task ([`TaskRecord::launch_failure`]); the clock then stops the task at the instant it
//! would otherwise have started ([`TaskRecord::fail_launch`]), so a caller reading in
//! between still sees the task come up.
//!
//! LOCK ORDER: the cross-service read here takes the Amazon EC2 store's lock, holds no
//! other while it does, and hands back owned values — so it is called with NO guard of
//! this service's store held, which is what keeps a lock cycle between the two stores
//! unreachable. The same contract [`unattached_target_groups`] has.

use super::*;

use ::service_com_amazonaws_ec2_amazonec2::ops::types::{Route, RouteTable};
use ::service_com_amazonaws_ec2_amazonec2::state::{
    CONNECTIVITY_TYPE_PUBLIC, NAT_GATEWAY_STATE_AVAILABLE, NAT_GATEWAY_STATE_PENDING,
    ROUTE_STATE_BLACKHOLE, VPC_ENDPOINT_STATE_AVAILABLE, VPC_ENDPOINT_STATE_PENDING,
    VPC_ENDPOINT_TYPE_GATEWAY, VPC_ENDPOINT_TYPE_INTERFACE,
};
use ::service_com_amazonaws_ec2_amazonec2::ComAmazonawsEc2Amazonec2State;

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all: the
/// `TaskStopCode` enum's `TaskFailedToStart` member
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
pub const STOP_CODE_TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The destination of the route that carries traffic OFF a VPC: "a route table contains a
/// set of rules, called routes" and the one for everything not inside the VPC is the
/// default route (Amazon VPC User Guide, "Route tables").
pub const DEFAULT_IPV4_DESTINATION: &str = "0.0.0.0/0";

/// The id prefix of an internet gateway, which EC2 spells `igw-<hex>` — what a route's
/// `GatewayId` carries when the route points at one.
pub const INTERNET_GATEWAY_ID_PREFIX: &str = "igw-";

/// The registry a reference that names none is pulled from: the Docker Hub registry a
/// container runtime resolves a bare name (`nginx:latest`) against.
pub const DOCKER_HUB_REGISTRY: &str = "registry-1.docker.io";

/// The endpoint service suffix of the Amazon ECR API, as an endpoint's `ServiceName`
/// spells it (`com.amazonaws.<region>.ecr.api`).
const ECR_API_ENDPOINT_SUFFIX: &str = ".ecr.api";

/// The endpoint service suffix of the Amazon ECR Docker registry
/// (`com.amazonaws.<region>.ecr.dkr`).
const ECR_DKR_ENDPOINT_SUFFIX: &str = ".ecr.dkr";

/// The endpoint service suffix of Amazon S3, which an ECR image's layers are fetched from
/// (`com.amazonaws.<region>.s3`).
const S3_ENDPOINT_SUFFIX: &str = ".s3";

/// The host part of an Amazon ECR registry URI (`<account>.dkr.ecr.<region>.<suffix>`),
/// which is what makes an image one the ECR endpoints can reach.
const ECR_REGISTRY_INFIX: &str = ".dkr.ecr.";

/// What ONE launched task's image fetch depends on: where the launch placed it, and what
/// its containers run.
///
/// Collected from the stored tasks under this service's lock
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::launches_awaiting_image_pull`])
/// and answered against the Amazon EC2 store under that store's own, so neither lock is
/// ever held while the other is taken.
#[derive(Debug, Clone)]
pub struct TaskLaunch {
    /// The task the verdict belongs to.
    pub task_arn: String,
    /// `AwsVpcConfiguration$subnets`: "The IDs of the subnets associated with the task or
    /// service", in the order the launch named them.
    pub subnets: Vec<String>,
    /// `AwsVpcConfiguration$assignPublicIp`: "Whether the task's elastic network interface
    /// receives a public IP address", and `None` when the launch named no value.
    pub assign_public_ip: Option<String>,
    /// The images the task's containers run, as `Container$image` holds them.
    pub images: Vec<String>,
}

/// The `stoppedReason` of a task whose containers could not be launched because an image
/// could not be fetched from where the task is placed.
///
/// The error CLASS is the container runtime's `CannotPullContainerError`, which is what
/// Amazon ECS surfaces for a failed fetch; the variable parts are the image the request
/// asked for and the registry that reference resolves to ([`registry_host`]), with the
/// network condition a placement that cannot reach it produces.
pub fn cannot_pull_container_message(image: &str) -> String {
    let host = registry_host(image);
    format!(
        "CannotPullContainerError: failed to resolve ref {image}: failed to do request: \
         Get \"https://{host}/v2/\": dial tcp {host}:443: i/o timeout"
    )
}

/// The REGISTRY an image reference is fetched from: the part before the first `/` when
/// that part is a host — it carries a `.` or a `:`, or is `localhost` — and
/// [`DOCKER_HUB_REGISTRY`] otherwise, which is how a container runtime resolves a
/// reference that names no registry.
pub fn registry_host(image: &str) -> &str {
    match image.split_once('/') {
        Some((host, _))
            if host.contains('.') || host.contains(':') || host == "localhost" =>
        {
            host
        }
        _ => DOCKER_HUB_REGISTRY,
    }
}

/// Decide the image fetch of every task that has just been launched, and record the
/// failure on each task whose placement cannot make it.
///
/// Called by the operations that LAUNCH a task, with this service's guard dropped: the
/// verdict is a question about the Amazon VPC networking the launch named, so it is read
/// out of the Amazon EC2 service's own store through the one shared engine, under that
/// store's lock and no other.
///
/// Nothing is recorded for a placement this emulator holds no network facts about — a
/// subnet the EC2 store does not hold is not evidence of a blocked path — and nothing is
/// recorded twice: a task already carrying a verdict is not re-decided.
pub fn evaluate_image_pulls<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let launches = state.lock().launches_awaiting_image_pull();
    if launches.is_empty() {
        return;
    }
    let Some(ec2) = ::service_com_amazonaws_ec2_amazonec2::state::service_state(engine) else {
        return;
    };
    let failures: Vec<(String, String)> = {
        let store = ec2.lock();
        launches
            .iter()
            .filter_map(|launch| {
                unfetchable_image(&store, launch)
                    .map(|reason| (launch.task_arn.clone(), reason))
            })
            .collect()
    };
    if failures.is_empty() {
        return;
    }
    state.lock().record_launch_failures(&failures);
}

/// Why the containers of `launch` cannot be launched where the task is placed — the
/// `stoppedReason` of the first image no path reaches — or `None` when every image can be
/// fetched, or when this emulator holds no network facts about the placement at all.
fn unfetchable_image(store: &ComAmazonawsEc2Amazonec2State, launch: &TaskLaunch) -> Option<String> {
    // The subnet the task's elastic network interface was given, which is the one the
    // launch named first
    // ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::mint_task_attachments`]).
    let subnet_id = launch.subnets.first()?;
    let vpc_id = store.subnets.get(subnet_id)?.vpc_id.clone()?;
    let internet =
        subnet_reaches_internet(store, subnet_id, &vpc_id, launch.assign_public_ip.as_deref());
    launch.images.iter().find_map(|image| {
        let reachable = internet || endpoints_reach_registry(store, &vpc_id, image);
        (!reachable).then(|| cannot_pull_container_message(image))
    })
}

/// Whether traffic leaving `subnet_id` reaches the internet, for a task whose
/// `assignPublicIp` is `assign_public_ip`.
///
/// Answered off the route table that APPLIES to the subnet ([`effective_route_table`]) and
/// the default route in it, which is the rule the Amazon VPC User Guide states for every
/// kind of subnet: traffic bound for anything outside the VPC goes where that route sends
/// it, and a subnet whose table has no such route keeps it inside the VPC.
fn subnet_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    subnet_id: &str,
    vpc_id: &str,
    assign_public_ip: Option<&str>,
) -> bool {
    let Some(table) = effective_route_table(store, subnet_id, vpc_id) else {
        return false;
    };
    table
        .routes
        .iter()
        .flatten()
        .filter(|route| {
            route.destination_cidr_block.as_deref() == Some(DEFAULT_IPV4_DESTINATION)
                // `Route$state` is `blackhole` while "the specified gateway isn't attached
                // to the VPC, or the specified NAT instance has been terminated", and such
                // a route carries nothing.
                && route.state.as_deref() != Some(ROUTE_STATE_BLACKHOLE)
        })
        .any(|route| route_reaches_internet(store, route, assign_public_ip))
}

/// Whether ONE default route carries traffic to the internet, for a task whose
/// `assignPublicIp` is `assign_public_ip`.
fn route_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    route: &Route,
    assign_public_ip: Option<&str>,
) -> bool {
    if let Some(nat_gateway_id) = route.nat_gateway_id.as_deref() {
        return nat_gateway_reaches_internet(store, nat_gateway_id);
    }
    if route
        .gateway_id
        .as_deref()
        .is_some_and(|gateway_id| gateway_id.starts_with(INTERNET_GATEWAY_ID_PREFIX))
    {
        // An internet gateway carries traffic for an address that is reachable from the
        // internet, which for an `awsvpc` task is the public address `assignPublicIp` asks
        // for — "If you don't specify an IP address ... the task won't have access to the
        // internet" is the same rule the member's own doc states for a task in a public
        // subnet.
        return assign_public_ip == Some(ASSIGN_PUBLIC_IP_ENABLED);
    }
    // A transit gateway carries the traffic off the VPC, to whatever its own route table
    // sends it to.
    route.transit_gateway_id.is_some()
}

/// Whether the NAT gateway `nat_gateway_id` carries internet-bound traffic.
///
/// "With a public NAT gateway, internet-bound traffic from a private subnet can be routed
/// to the NAT gateway ... With a private NAT gateway, internet-bound traffic from a private
/// subnet can't be routed to the gateway" (`CreateNatGateway`), and a gateway that is being
/// deleted, is deleted, or failed to come up carries nothing.
fn nat_gateway_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    nat_gateway_id: &str,
) -> bool {
    let Some(gateway) = store.nat_gateways.get(nat_gateway_id) else {
        return false;
    };
    let public = gateway
        .connectivity_type
        .as_deref()
        .unwrap_or(CONNECTIVITY_TYPE_PUBLIC)
        == CONNECTIVITY_TYPE_PUBLIC;
    let in_service = matches!(
        gateway.state.as_deref(),
        Some(NAT_GATEWAY_STATE_AVAILABLE | NAT_GATEWAY_STATE_PENDING)
    );
    public && in_service
}

/// Whether the VPC `vpc_id` holds the endpoints that reach the registry `image` is held
/// in, with no route off the VPC at all.
///
/// Only Amazon ECR is reachable this way, and only through all three of the endpoints that
/// service's own documentation names: an interface endpoint for the ECR API, one for its
/// Docker registry, and a gateway endpoint for Amazon S3, which the image layers are
/// fetched from.
fn endpoints_reach_registry(
    store: &ComAmazonawsEc2Amazonec2State,
    vpc_id: &str,
    image: &str,
) -> bool {
    if !is_ecr_image(image) {
        return false;
    }
    [
        (ECR_API_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_INTERFACE),
        (ECR_DKR_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_INTERFACE),
        (S3_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_GATEWAY),
    ]
    .iter()
    .all(|(suffix, endpoint_type)| {
        store.vpc_endpoints.values().any(|endpoint| {
            endpoint.vpc_id.as_deref() == Some(vpc_id)
                && endpoint.vpc_endpoint_type.as_deref() == Some(*endpoint_type)
                && endpoint
                    .service_name
                    .as_deref()
                    .is_some_and(|name| name.ends_with(*suffix))
                // An endpoint Amazon EC2 is still bringing into service is one the VPC
                // has; one that was deleted, rejected or failed is not.
                && matches!(
                    endpoint.state.as_deref(),
                    Some(VPC_ENDPOINT_STATE_AVAILABLE | VPC_ENDPOINT_STATE_PENDING)
                )
        })
    })
}

/// Whether `image` names a repository in Amazon ECR, whose registry is an AWS service and
/// so has VPC endpoints: an ECR image is pulled from `<account>.dkr.ecr.<region>.<suffix>`
/// (Amazon ECR User Guide, the registry URI a pull uses).
fn is_ecr_image(image: &str) -> bool {
    let host = registry_host(image);
    host.contains(ECR_REGISTRY_INFIX) && host.contains(".amazonaws.")
}

/// The route table that APPLIES to `subnet_id`: the one a caller explicitly associated
/// with it, and otherwise the MAIN table of its VPC.
///
/// "Each subnet in your VPC must be associated with a route table ... A subnet can be
/// explicitly associated with a route table, or implicitly or explicitly associated with
/// the main route table" (Amazon VPC User Guide, "Route tables"), so exactly one table
/// decides where the subnet's traffic goes. The explicit half is Amazon EC2's own reading
/// of its own records
/// ([`ComAmazonawsEc2Amazonec2State::route_table_associated_with_subnet`]), so an
/// association made through that service resolves here exactly as it does there.
fn effective_route_table<'store>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    subnet_id: &str,
    vpc_id: &str,
) -> Option<&'store RouteTable> {
    store
        .route_table_associated_with_subnet(subnet_id)
        .and_then(|route_table_id| store.route_tables.get(&route_table_id))
        .or_else(|| {
            store.route_tables.values().find(|table| {
                table.vpc_id.as_deref() == Some(vpc_id)
                    && table
                        .associations
                        .iter()
                        .flatten()
                        .any(|association| association.main == Some(true))
            })
        })
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every task whose image fetch has not been decided yet: one that is on its way UP
    /// (its `desiredStatus` is `RUNNING` and it has not started) and that was launched
    /// into an `awsvpc` network placement, with the placement and the images its
    /// containers run.
    ///
    /// A task that shares the networking of the host that runs it carries no placement of
    /// its own, so nothing here decides for it: the path it fetches an image over is that
    /// host's.
    pub fn launches_awaiting_image_pull(&self) -> Vec<TaskLaunch> {
        self.tasks
            .values()
            .filter(|task| {
                task.launch_failure.is_none()
                    && task.desired_status == STATUS_RUNNING
                    && task.is_pending()
            })
            .filter_map(|task| {
                let awsvpc = task.awsvpc_configuration.as_ref()?;
                Some(TaskLaunch {
                    task_arn: task.task_arn.clone(),
                    subnets: awsvpc.subnets.clone(),
                    assign_public_ip: awsvpc.assign_public_ip.clone(),
                    images: task
                        .containers
                        .iter()
                        .filter_map(|container| container.image.clone())
                        .collect(),
                })
            })
            .collect()
    }

    /// Record, on each named task, why its containers cannot be launched where it is
    /// placed — the `stoppedReason` the stop the clock then owes carries
    /// ([`TaskRecord::fail_launch`]).
    ///
    /// Only a task that is still on its way up takes a verdict: one that has started has
    /// already fetched its images, and one that is stopping is on its way out either way.
    /// Nothing a client reads changes here, so the task's `version` does not move — the
    /// stop itself is the change.
    pub fn record_launch_failures(&mut self, failures: &[(String, String)]) {
        for (task_arn, reason) in failures {
            if let Some(task) = self.tasks.get_mut(task_arn) {
                if task.launch_failure.is_none()
                    && task.desired_status == STATUS_RUNNING
                    && task.is_pending()
                {
                    task.launch_failure = Some(reason.clone());
                }
            }
        }
    }
}

impl TaskRecord {
    /// Stop this task because Amazon ECS could not launch its containers at all, as of
    /// `at`: the task never ran, so it gets no `startedAt`, its containers no exit code,
    /// and the stop carries [`STOP_CODE_TASK_FAILED_TO_START`] with `reason` as its
    /// `stoppedReason`.
    ///
    /// The task is on its way out from this instant — `desiredStatus` is `STOPPED`, which
    /// is what a `ListTasks` narrowed to stopped tasks reports it under — and `STOPPED`
    /// itself falls due one [`TRANSITION_SPAN`] later, the same two-step stop every other
    /// stop of a task is recorded as.
    pub fn fail_launch(&mut self, reason: &str, at: SystemTime) {
        self.desired_status = STATUS_STOPPED.to_string();
        self.stopped_reason = Some(reason.to_string());
        self.stop_code = Some(STOP_CODE_TASK_FAILED_TO_START.to_string());
        self.stopping_at = Some(DateTime::from(at));
        self.pending_due_at = None;
        self.running_due_at = None;
        self.stopped_due_at = Some(at + TRANSITION_SPAN);
        self.version += 1;
    }
}
