// SPDX-License-Identifier: CC0-1.0

//! @op CreateEnvironment
//! Mints an environment and persists it in [`crate::state`]: a new `environmentId`,
//! its ARN, `environmentState` `CREATING` (the clock owes it `CREATED`), and the ten
//! VLAN subnets materialised from `initialVlans`. Every other operation of this
//! service reads what this one stored.
//!
//! Those ten VLAN subnets are PROVISIONED, not just recorded: "Amazon EVS provisions
//! only the VLAN subnets" in `SELF_DEPLOYED` mode and provisions them plus VCF
//! otherwise, and they are subnets of the caller's own VPC — so each is created in the
//! account's Amazon EC2 store through EC2's own writer
//! ([`crate::state::ec2_resources::provision_vlan_subnets`]), and the `Vlan` reports
//! the `subnetId` and `availabilityZone` of the subnet EC2 holds.
//!
//! The request-level rules it enforces beyond the modelled constraints, each from the
//! documentation quoted under "Modeled contract" below:
//!
//! 1. `dedicatedHostId` and `placementGroupId` cannot be used together in one
//!    `HostInfoForCreate`.
//! 2. A VLAN subnet CIDR block is between /24 and /28, and no two of an
//!    environment's VLAN subnets may overlap.
//! 3. `hcxNetworkAclId` is required when `isHcxPublic` is true.
//! 4. An environment name is unique within the account and Region.
//! 5. `termsAccepted` must be true: it is the caller's confirmation that it holds the
//!    VCF licenses the environment needs, so an environment cannot be created without
//!    it.
//! 6. The `vpcId` is not already used by another deployed environment of this Region.
//! 7. The `vpcId` names a VPC the account holds, and the `serviceAccessSubnetId` a
//!    subnet of THAT VPC: an environment is deployed into the caller's existing
//!    network, so a request naming a VPC or subnet that does not exist is refused with
//!    a clause naming the id it gave ([`crate::state::ec2_resources`], which reads
//!    Amazon EC2's own store).
//! 8. Each VLAN subnet CIDR block is a block of that VPC's own address space and
//!    overlaps no subnet the VPC already holds: "Amazon EVS VLAN subnet CIDR blocks
//!    must not overlap with other subnets in the VPC" (`InitialVlans` doc) — rule 2 at
//!    the scope the doc states it, which is what lets each block become a real subnet
//!    of the VPC.
//!
//! Supplying a parameter the chosen VCF mode does not support is NOT one of them.
//! `SELF_DEPLOYED` "does not support" `licenseInfo`, `hosts`, `vcfHostnames`, `siteId`
//! and `connectivityInfo` in the sense that it does nothing with them — no hosts are
//! added and no VCF is installed in that mode — and the recorded wire confirms that a
//! request carrying all five of them together with `vcfVersion` `SELF_DEPLOYED` is NOT
//! refused for carrying them: the refusal it drew named the VPC and nothing else. The
//! members are therefore accepted and stored as supplied, exactly as `clientToken` is
//! ("If you supply input for this parameter, it will have no effect").
//!
//! The account's AWS Premium Support subscription is not a precondition either: this
//! emulator holds no account-subscription state to decide it from, and a guess either
//! way would refuse or admit every request alike. Only preconditions this service can
//! actually evaluate are enforced.
//!
//! Every one of those checks runs before anything is minted, and they are all
//! evaluated: a request that breaks several is refused ONCE, with one
//! `ValidationException` whose `fieldList` carries an entry per failed check and whose
//! `message` joins those entries ([`crate::state::aggregate_validation_error`]). A
//! refused request creates nothing.
//!
//! The store itself is bounded by [`crate::state::MAX_ENVIRONMENTS`]: this is the only
//! operation that grows it, and it refuses with the service's
//! `ServiceQuotaExceededException` once the cap is reached and no finished (`DELETED`)
//! record can be reclaimed.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEnvironment::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateEnvironment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")
//! ```
//!
//! Creates an Amazon EVS environment that runs VCF software, such as SDDC Manager, NSX
//! Manager, and vCenter Server. When you specify SELF_DEPLOYED for vcfVersion, Amazon EVS
//! provisions only the VLAN subnets; no hosts are added and no VCF installation is
//! performed. After the environment is created, you can add hosts with
//! CreateEnvironmentHost and install VCF yourself. The licenseInfo, hosts, vcfHostnames,
//! siteId, and connectivityInfo parameters are not supported in this mode. When you specify
//! any other VCF version, Amazon EVS installs and configures VCF for you. For more
//! information, see Self-deployed mode in the Amazon EVS User Guide. When Amazon EVS
//! installs VCF, the default ESX version for the selected VCF version will be used. After a
//! host is added with a specific ESX version, it can only be upgraded using vCenter
//! Lifecycle Manager. You cannot use the dedicatedHostId and placementGroupId parameters
//! together in the same CreateEnvironment action. This results in a ValidationException
//! response.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..100, pattern ^[!-~]+$, @idempotencyToken) — This
//!   parameter is not used in Amazon EVS currently. If you supply input for this parameter,
//!   it will have no effect. A unique, case-sensitive identifier that you provide to ensure
//!   the idempotency of the environment creation request. If you do not specify a client
//!   token, a randomly generated token is used for the request to ensure idempotency.
//! * `connectivityInfo` (ConnectivityInfo) — The connectivity configuration for the
//!   environment. Amazon EVS requires that you specify two route server peer IDs. During
//!   environment creation, the route server endpoints peer with the NSX edges over the NSX
//!   uplink subnet, providing BGP-based dynamic routing for overlay networks. Not supported
//!   when vcfVersion is SELF_DEPLOYED.
//! * `environmentName` (EnvironmentName, len 1..100, pattern ^[a-zA-Z0-9_-]+$) — The name to
//!   give to your environment. The name can contain only alphanumeric characters
//!   (case-sensitive), hyphens, and underscores. It must start with an alphanumeric
//!   character, and can't be longer than 100 characters. The name must be unique within the
//!   Amazon Web Services Region and Amazon Web Services account that you're creating the
//!   environment in.
//! * `hosts` (HostInfoForCreateList, len 4..4) — The ESX hosts to add to the environment. For
//!   each host, provide the desired hostname, EC2 SSH keypair name, and EC2 instance type.
//!   Optionally, provide a partition or cluster placement group, or use Amazon EC2 Dedicated
//!   Hosts. Not supported when vcfVersion is SELF_DEPLOYED. In that case, you can add hosts
//!   using CreateEnvironmentHost after the environment is created.
//! * `initialVlans` (InitialVlans) **required** — The initial VLAN subnets for the Amazon EVS
//!   environment. For each Amazon EVS VLAN subnet, you must specify a non-overlapping CIDR
//!   block. Amazon EVS VLAN subnets have a minimum CIDR block size of /28 and a maximum size
//!   of /24.
//! * `kmsKeyId` (String) — A unique ID for the customer-managed KMS key that is used to
//!   encrypt the VCF credential pairs for SDDC Manager, NSX Manager, and vCenter appliances.
//!   These credentials are stored in Amazon Web Services Secrets Manager.
//! * `licenseInfo` (LicenseInfoList, len 1..1) — The license information that Amazon EVS
//!   requires to create an environment. Amazon EVS requires two license keys: a VCF solution
//!   key and a vSAN license key. The VCF solution key must meet minimum core requirements,
//!   and the vSAN license key must meet minimum capacity requirements for your selected
//!   instance type. For information about minimum license requirements, see the VCF
//!   subscriptions section in the Amazon EVS User Guide. VCF licenses can be used for only
//!   one Amazon EVS environment. Amazon EVS does not support reuse of VCF licenses for
//!   multiple environments. VCF license information can be retrieved from the Broadcom
//!   portal. Not supported when vcfVersion is SELF_DEPLOYED.
//! * `serviceAccessSecurityGroups` (ServiceAccessSecurityGroups) — The security group that
//!   controls communication between the Amazon EVS control plane and VPC. The default
//!   security group is used if a custom security group isn't specified. The security group
//!   should allow access to the following. TCP/UDP access to the DNS servers HTTPS/SSH access
//!   to the host management VLAN subnet HTTPS/SSH access to the Management VM VLAN subnet You
//!   should avoid modifying the security group rules after deployment, as this can break the
//!   persistent connection between the Amazon EVS control plane and VPC. This can cause
//!   future environment actions like adding or removing hosts to fail.
//! * `serviceAccessSubnetId` (SubnetId, len 15..24, pattern
//!   ^subnet-[a-f0-9]{8}([a-f0-9]{9})?$) **required** — The subnet that is used to establish
//!   connectivity between the Amazon EVS control plane and VPC. The Amazon EVS control plane
//!   uses this subnet to interface with your environment. This includes validating DNS
//!   records and enabling Amazon EVS Connectors.
//! * `siteId` (String) — The Broadcom Site ID that is allocated to you as part of your
//!   electronic software delivery. This ID allows customer access to the Broadcom portal, and
//!   is provided to you by Broadcom at the close of your software contract or contract
//!   renewal. Amazon EVS uses the Broadcom Site ID that you provide to meet Broadcom VCF
//!   license usage reporting requirements for Amazon EVS. Not supported when vcfVersion is
//!   SELF_DEPLOYED.
//! * `tags` (RequestTagMap, len 1..200) — Metadata that assists with categorization and
//!   organization. Each tag consists of a key and an optional value. You define both. Tags
//!   don't propagate to any other cluster or Amazon Web Services resources.
//! * `termsAccepted` (Boolean) **required** — Confirmation that the customer has purchased
//!   and will continue to maintain the required number of VCF software licenses to cover all
//!   physical processor cores in the Amazon EVS environment. Information about your VCF
//!   software in Amazon EVS will be shared with Broadcom to verify license compliance. Amazon
//!   EVS does not validate license keys. To validate license keys, visit the Broadcom support
//!   portal.
//! * `vcfHostnames` (VcfHostnames) — The DNS hostnames for the virtual machines that host the
//!   VCF management appliances. Provide hostnames for vCenter, NSX Manager, SDDC Manager, and
//!   Cloud Builder. Not supported when vcfVersion is SELF_DEPLOYED.
//! * `vcfVersion` (VcfVersion, one of: SELF_DEPLOYED | VCF-5.2.1 | VCF-5.2.2) **required** —
//!   The VCF version to use for the environment. SELF_DEPLOYED: You install VCF yourself. The
//!   licenseInfo, hosts, vcfHostnames, siteId, and connectivityInfo parameters are not
//!   supported. Any other valid value: Amazon EVS installs and configures VCF for you in the
//!   version you specify.
//! * `vpcId` (VpcId, len 12..21, pattern ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$) **required** — A
//!   unique ID for the VPC that the environment is deployed inside. Amazon EVS requires that
//!   all VPC subnets exist in a single Availability Zone in a Region where the service is
//!   available. The VPC that you specify must have a valid DHCP option set with domain name,
//!   at least two DNS servers, and an NTP server. These settings are used to configure your
//!   VCF appliances and hosts. The VPC cannot be used with any other deployed Amazon EVS
//!   environment. Amazon EVS does not provide multi-VPC support for environments at this
//!   time. Amazon EVS does not support the following Amazon Web Services networking options
//!   for NSX overlay connectivity: cross-Region VPC peering, Amazon S3 gateway endpoints, or
//!   Amazon …
//!
//! ## Output
//!
//! * `environment` (Environment) — A description of the created environment.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the specified constraints.
//!   You will see this exception if invalid inputs are provided for any of the Amazon EVS
//!   environment operations, or if a list operation is performed on an environment resource
//!   that is still initializing.

#[allow(unused_imports)]
use super::types::*;

// This service's OWN reading of an IPv4 CIDR block, shared with
// `CreateEnvironmentHost`, which places a host's management address inside the
// `vmkManagement` block with it. It judges a block the REQUEST supplied, before any
// subnet exists for it; a block judged against what Amazon EC2 holds goes through EC2's
// own CIDR helpers instead ([`crate::state::ec2_resources`]), because that is the store
// the answer comes from.
use crate::state::Ipv4Cidr;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEnvironmentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEnvironmentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.evs#CreateEnvironment",
    extra_errors = ["InternalServerException", "ServiceQuotaExceededException"]
)]
#[derive(Debug)]
pub enum CreateEnvironmentErrorModeled {
    ValidationException(ValidationException),
    /// Widened in from the service's own error set (this operation does not list it):
    /// the one honest answer when this service's store cannot be reached at all.
    InternalServerException(InternalServerException),
    /// Widened in from the service's own error set (`TagResource` lists it): the
    /// refusal when this emulator's store is full, which is a quota condition and not
    /// a defect in the request.
    ServiceQuotaExceededException(ServiceQuotaExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_environment::CreateEnvironmentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEnvironmentError = crate::scaffolded::Scaffolded<CreateEnvironmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEnvironmentErrorModeled> for CreateEnvironmentError {
    fn from(modeled: CreateEnvironmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEnvironment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEnvironment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEnvironment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEnvironment<Eng>
{
    type Input = CreateEnvironmentInput;
    type Output = CreateEnvironmentOutput;
    type Error = CreateEnvironmentError;

    const NAME: &'static str = "CreateEnvironment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEnvironmentErrorModeled::InternalServerException(InternalServerException {
                message: "The Amazon EVS environment store is unavailable.".to_string(),
            })
        })?;
        // Authorization first, before anything about the request is evaluated: AWS
        // decides it at the IAM layer, and a create names no resource of its own yet.
        if let Some(denied) =
            crate::state::authorization::denied(&*self.engine, ctx, Self::NAME, None)
        {
            return Err(denied);
        }

        // Every request-level precondition, evaluated before anything is minted. They
        // are all collected rather than short-circuited: AWS reports a request's
        // failed preconditions together, so a caller can correct them in one round
        // trip instead of one refusal at a time.
        let mut failures: Vec<ValidationExceptionField> = Vec::new();
        failures.extend(terms_accepted_failure(&input));
        failures.extend(host_failures(input.hosts.as_deref().unwrap_or_default()));
        let vlan_plan = plan_vlans(&input.initial_vlans);
        failures.extend(vlan_failures(&input.initial_vlans, &vlan_plan));
        // The network the environment is deployed into, from Amazon EC2's own store,
        // read before this service's own lock is taken.
        //
        // LOCK ORDER. The checks and the provisioning below DO take EC2's store lock
        // while this service's guard is held, and `DeleteEnvironment` takes the two the
        // same way round; EC2 never reaches this service, so the order is consistent and
        // cannot cycle. Never add an EC2 read that takes the locks the other way round.
        failures.extend(crate::state::ec2_resources::network_failures(
            &*self.engine,
            &input.vpc_id,
            &input.service_access_subnet_id,
        ));

        let region = crate::state::request_region(ctx.parts());
        let mut state = state.lock();
        state.materialise_due_transitions();

        // The two preconditions that depend on stored state rather than on the
        // request alone, so they are gathered with the lock held.
        if let Some(name) = input.environment_name.as_deref() {
            if state.name_is_taken(&region, name) {
                failures.push(crate::state::field_failure(
                    "environmentName",
                    &format!("is already used by an environment in {region}"),
                ));
            }
        }
        if state.vpc_is_in_use(&region, &input.vpc_id) {
            failures.push(crate::state::field_failure(
                "vpcId",
                "is already used by another deployed Amazon EVS environment, which Amazon EVS \
                 does not support",
            ));
        }

        // The VLAN subnets are provisioned INSIDE that VPC, so each block must be a
        // block of the VPC's own address space and overlap no subnet the VPC already
        // holds ("Amazon EVS VLAN subnet CIDR blocks must not overlap with other
        // subnets in the VPC", `InitialVlans` doc) — the same overlap rule as among the
        // environment's own ten blocks, at the scope the doc states it.
        //
        // Judged only when there IS a VPC this environment can be deployed into: a
        // `vpcId` already refused (it names no VPC, or it carries another environment)
        // leaves nothing to place the blocks in, so the refusal names only the VPC —
        // exactly as an unresolved VPC leaves no VPC to judge the service access subnet
        // against. And only for the members no clause above already refuses, so one
        // member draws one clause.
        let vpc_refused = failures.iter().any(|failure| failure.name == "vpcId");
        if !vpc_refused {
            let placeable: Vec<crate::state::ec2_resources::VlanSubnetRequest<'_>> = {
                let refused: std::collections::BTreeSet<&str> =
                    failures.iter().map(|failure| failure.name.as_str()).collect();
                vlan_plan
                    .iter()
                    .filter(|planned| !refused.contains(planned.member))
                    .map(|planned| crate::state::ec2_resources::VlanSubnetRequest {
                        member: planned.member,
                        cidr: planned.cidr,
                    })
                    .collect()
            };
            failures.extend(crate::state::ec2_resources::vlan_placement_failures(
                &*self.engine,
                &input.vpc_id,
                &placeable,
            ));
        }

        if !failures.is_empty() {
            return Err(CreateEnvironmentErrorModeled::ValidationException(
                crate::state::aggregate_validation_error(failures),
            )
            .into());
        }
        let requested_hosts = input.hosts.unwrap_or_default();

        // The ten VLAN subnets have to fit in the account's EC2 store, and that is
        // asked BEFORE this service reserves anything of its own: a request that cannot
        // be served must not consume an environment slot (reserving one can purge the
        // longest-deleted record) or an environment id.
        if let Some((held, limit)) =
            crate::state::ec2_resources::subnet_room_shortfall(&*self.engine, vlan_plan.len())
        {
            return Err(subnet_room_refusal(held, limit, vlan_plan.len()));
        }

        // Bound the store before anything is minted: a generated emulator holds its
        // resources for the life of the process, so the create path is the one that
        // must refuse rather than grow without limit.
        if !state.reserve_environment_slot() {
            return Err(CreateEnvironmentErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "This emulator holds at most {} environments; delete one before \
                         creating another.",
                        crate::state::MAX_ENVIRONMENTS
                    ),
                },
            )
            .into());
        }

        let created_at_ms = crate::state::now_ms();
        let environment_id = state.mint_environment_id();

        // The ten VLAN subnets are REAL subnets of the caller's VPC: Amazon EVS
        // "provisions the VLAN subnets", so they are created in the account's own
        // Amazon EC2 store and each VLAN reports the id, the block and the Availability
        // Zone EC2 gave its subnet. The EC2 store is written while this service's lock
        // is held, and in the same order by `DeleteEnvironment`, so the two locks are
        // always taken this way round.
        let provisioning = {
            let planned: Vec<crate::state::ec2_resources::VlanSubnetRequest<'_>> = vlan_plan
                .iter()
                .map(|planned| crate::state::ec2_resources::VlanSubnetRequest {
                    member: planned.member,
                    cidr: planned.cidr,
                })
                .collect();
            crate::state::ec2_resources::provision_vlan_subnets(
                &*self.engine,
                &region,
                &input.vpc_id,
                &input.service_access_subnet_id,
                &planned,
            )
        };
        let provisioned = match provisioning {
            // One subnet per planned VLAN, in the order they were asked for. A shorter
            // answer than the plan would leave some VLAN naming no subnet, so it is an
            // internal inconsistency rather than something to paper over: the subnets
            // that WERE created go back, and nothing is stored.
            crate::state::ec2_resources::VlanSubnetProvisioning::Provisioned(subnets)
                if subnets.len() == vlan_plan.len() =>
            {
                subnets
            }
            crate::state::ec2_resources::VlanSubnetProvisioning::Provisioned(subnets) => {
                release(&*self.engine, &subnets);
                return Err(CreateEnvironmentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: format!(
                            "Amazon EC2 provisioned {provisioned} of the {planned} VLAN \
                             subnets the environment needs.",
                            provisioned = subnets.len(),
                            planned = vlan_plan.len(),
                        ),
                    },
                )
                .into());
            }
            // No EC2 store to provision them in is evidence of nothing, so the
            // environment is still created — with ids of this service's own, which is
            // all there is to name its VLANs by, and no Availability Zone.
            crate::state::ec2_resources::VlanSubnetProvisioning::NoEc2Store => Vec::new(),
            // A zone every subnet of the VPC is in is part of what a VLAN subnet IS, so
            // a Region that names none leaves nothing to provision: the same server
            // error EC2's own `CreateSubnet` answers for a Region with no zone, and
            // nothing was written.
            crate::state::ec2_resources::VlanSubnetProvisioning::ZoneUnknown => {
                return Err(CreateEnvironmentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: format!(
                            "No Availability Zone could be resolved for the VLAN subnets of an \
                             environment in {region}."
                        ),
                    },
                )
                .into());
            }
            // Asked before the slot was reserved, so this is only reachable if another
            // request filled the EC2 store in between; nothing was written either way.
            crate::state::ec2_resources::VlanSubnetProvisioning::SubnetLimitExceeded {
                held,
                limit,
            } => return Err(subnet_room_refusal(held, limit, vlan_plan.len())),
        };

        let vlans = vlan_plan
            .into_iter()
            .enumerate()
            .map(|(index, planned)| {
                let subnet = provisioned.get(index);
                crate::state::StoredVlan {
                    // The 802.1Q tag the traffic type is carried on: unique within the
                    // environment, assigned in the order the `InitialVlans` members are
                    // modelled, and clear of the reserved default VLAN 1.
                    vlan_id: FIRST_VLAN_ID + index as i32,
                    // The block of the subnet EC2 holds, which is the requested block in
                    // the canonical form EC2 stores one in — so `Vlan$cidr` and the
                    // subnet's `CidrBlock` are one block and cannot disagree.
                    cidr: match subnet {
                        Some(subnet) => subnet.cidr_block.clone(),
                        None => planned.cidr.to_string(),
                    },
                    function_name: planned.function_name.to_string(),
                    subnet_id: match subnet {
                        Some(subnet) => subnet.subnet_id.clone(),
                        None => state.mint_subnet_id(),
                    },
                    availability_zone: subnet
                        .map(|subnet| subnet.availability_zone.clone()),
                    vlan_state: crate::state::STATE_CREATING.to_string(),
                    is_public: planned.is_public,
                    network_acl_id: planned.network_acl_id.map(str::to_string),
                    created_at_ms,
                    modified_at_ms: created_at_ms,
                    eip_associations: Vec::new(),
                }
            })
            .collect();

        let environment = crate::state::StoredEnvironment {
            environment_arn: crate::state::environment_arn(&region, &environment_id),
            environment_id: environment_id.clone(),
            region,
            environment_name: input.environment_name,
            // Creation has started but is not finished: the clock owes this
            // environment, and its VLAN subnets, the move to CREATED.
            state: crate::state::STATE_CREATING.to_string(),
            state_details: None,
            created_at_ms,
            modified_at_ms: created_at_ms,
            vpc_id: input.vpc_id,
            service_access_subnet_id: input.service_access_subnet_id,
            vcf_version: input.vcf_version,
            terms_accepted: input.terms_accepted,
            kms_key_id: input.kms_key_id,
            site_id: input.site_id,
            license_info: input.license_info.unwrap_or_default(),
            connectivity_info: input.connectivity_info,
            vcf_hostnames: input.vcf_hostnames,
            service_access_security_groups: input.service_access_security_groups,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            vlans,
            requested_hosts,
            checks: Vec::new(),
            entitlements: std::collections::BTreeMap::new(),
            connectors: std::collections::BTreeMap::new(),
            hosts: std::collections::BTreeMap::new(),
            pending: Some(crate::state::PendingTransition::starting_now(
                crate::state::STATE_CREATED,
                crate::state::STATE_CREATED,
            )),
        };

        let transition_due_at = environment.pending.as_ref().map(|pending| pending.due_at);
        let created = match state.environments.entry(environment_id) {
            std::collections::btree_map::Entry::Vacant(slot) => slot.insert(environment).to_wire(),
            // Unreachable: ids are minted from a strictly increasing seed. Answering
            // honestly beats reporting some other environment as the created one.
            std::collections::btree_map::Entry::Occupied(taken) => {
                let message =
                    format!("The environment id {} is already in use.", taken.key());
                // Nothing of this environment is stored, so the subnets provisioned for
                // it are released rather than left in the account's EC2 store with no
                // environment to delete them.
                release(&*self.engine, &provisioned);
                return Err(CreateEnvironmentErrorModeled::InternalServerException(
                    InternalServerException { message },
                )
                .into());
            }
        };
        if let Some(due_at) = transition_due_at {
            state.note_transition_due(due_at);
        }
        Ok(CreateEnvironmentOutput {
            environment: Some(created),
        })
    }
}

/// The first 802.1Q VLAN tag this service assigns. VLAN 1 is the reserved default
/// VLAN of an 802.1Q trunk, so an environment's own VLANs start clear of it.
const FIRST_VLAN_ID: i32 = 100;

/// The refusal for an account whose Amazon EC2 store cannot take the environment's ten
/// VLAN subnets: a quota condition, not a defect in the request, so it is the
/// operation's `ServiceQuotaExceededException`. Stated in one place because both the
/// check before anything is reserved and the writer's own ceiling report it.
fn subnet_room_refusal(held: usize, limit: usize, planned: usize) -> CreateEnvironmentError {
    CreateEnvironmentErrorModeled::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: format!(
            "The account's Amazon EC2 store holds {held} of at most {limit} subnets, so the \
             {planned} VLAN subnets of an environment cannot be provisioned in it."
        ),
    })
    .into()
}

/// Give the subnets `provisioned` back to the account's Amazon EC2 store, for a create
/// that is refused after they were written: an environment that is not stored can never
/// be deleted, so nothing else would ever remove them.
fn release<Eng: Send + Sync + 'static>(
    engine: &Eng,
    provisioned: &[crate::state::ec2_resources::ProvisionedVlanSubnet],
) {
    let subnet_ids: Vec<String> = provisioned
        .iter()
        .map(|subnet| subnet.subnet_id.clone())
        .collect();
    crate::state::ec2_resources::release_vlan_subnets(engine, &subnet_ids);
}

/// The largest VLAN subnet prefix length: "Amazon EVS VLAN subnets have a minimum
/// CIDR block size of /28 and a maximum size of /24" (`InitialVlans` doc).
const MAX_VLAN_PREFIX_LEN: u8 = 28;
/// The smallest VLAN subnet prefix length. See [`MAX_VLAN_PREFIX_LEN`].
const MIN_VLAN_PREFIX_LEN: u8 = 24;

/// One VLAN subnet `CreateEnvironment` is about to materialise.
struct PlannedVlan<'a> {
    /// The `functionName` the VLAN carries — the `InitialVlans` member's own name,
    /// which is the form the `Vlan` doc gives (a `functionName` of `hcx`).
    function_name: &'static str,
    /// The path of the `InitialVlans` member this VLAN came from, so a refusal names
    /// the member the caller sent.
    member: &'static str,
    /// The CIDR block the request supplied for it.
    cidr: &'a str,
    /// Whether Amazon EVS provisions this VLAN as public.
    is_public: bool,
    /// The network ACL the VLAN uses, when the request named one.
    network_acl_id: Option<&'a str>,
}

/// The failure, if any, of the precondition that the caller accepted the terms:
/// `termsAccepted` is the "Confirmation that the customer has purchased and will
/// continue to maintain the required number of VCF software licenses to cover all
/// physical processor cores in the Amazon EVS environment" (its own doc), so an
/// environment cannot be created while it is false.
fn terms_accepted_failure(input: &CreateEnvironmentInput) -> Option<ValidationExceptionField> {
    (!input.terms_accepted).then(|| {
        crate::state::field_failure(
            "termsAccepted",
            "must be accepted (true) before an environment can be created",
        )
    })
}

/// Every host that names both placement mechanisms, refused through the shape's own
/// rule ([`crate::state::placement_conflict_failure`]), which `CreateEnvironmentHost`
/// applies to its single `host` for the same reason: "You cannot use the
/// `dedicatedHostId` and `placementGroupId` parameters together in the same
/// `HostInfoForCreate` object. This results in a `ValidationException` response."
fn host_failures(hosts: &[HostInfoForCreate]) -> Vec<ValidationExceptionField> {
    hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| host.dedicated_host_id.is_some() && host.placement_group_id.is_some())
        .map(|(index, _)| crate::state::placement_conflict_failure(&format!("hosts[{index}]")))
        .collect()
}

/// The ten VLAN subnets to materialise, in the order `InitialVlans` models its
/// members. The layout alone: the rules that govern the blocks are
/// [`vlan_failures`].
fn plan_vlans(initial: &InitialVlans) -> Vec<PlannedVlan<'_>> {
    vec![
        PlannedVlan {
            function_name: crate::state::VMK_MANAGEMENT_FUNCTION,
            member: "initialVlans.vmkManagement",
            cidr: &initial.vmk_management.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vmManagement",
            member: "initialVlans.vmManagement",
            cidr: &initial.vm_management.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vMotion",
            member: "initialVlans.vMotion",
            cidr: &initial.v_motion.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vSan",
            member: "initialVlans.vSan",
            cidr: &initial.v_san.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vTep",
            member: "initialVlans.vTep",
            cidr: &initial.v_tep.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "edgeVTep",
            member: "initialVlans.edgeVTep",
            cidr: &initial.edge_v_tep.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "nsxUplink",
            member: "initialVlans.nsxUplink",
            cidr: &initial.nsx_uplink.cidr,
            is_public: false,
            network_acl_id: None,
        },
        // Only the HCX VLAN can be public, and only it carries a network ACL id.
        PlannedVlan {
            function_name: "hcx",
            member: "initialVlans.hcx",
            cidr: &initial.hcx.cidr,
            is_public: initial.is_hcx_public,
            network_acl_id: initial.hcx_network_acl_id.as_deref(),
        },
        PlannedVlan {
            function_name: "expansionVlan1",
            member: "initialVlans.expansionVlan1",
            cidr: &initial.expansion_vlan1.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "expansionVlan2",
            member: "initialVlans.expansionVlan2",
            cidr: &initial.expansion_vlan2.cidr,
            is_public: false,
            network_acl_id: None,
        },
    ]
}

/// Every rule the request's VLAN subnets break: the HCX network ACL the public HCX
/// VLAN requires, and, per VLAN subnet, a parsable CIDR block of an allowed size that
/// overlaps none of the environment's other VLAN subnets.
fn vlan_failures(
    initial: &InitialVlans,
    planned: &[PlannedVlan<'_>],
) -> Vec<ValidationExceptionField> {
    let mut failures = Vec::new();

    // "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).
    if initial.is_hcx_public && initial.hcx_network_acl_id.is_none() {
        failures.push(crate::state::field_failure(
            "initialVlans.hcxNetworkAclId",
            "is required when isHcxPublic is true",
        ));
    }

    let mut networks: Vec<(&'static str, Ipv4Cidr)> = Vec::with_capacity(planned.len());
    for vlan in planned {
        let member = vlan.member;
        // The modelled `Cidr` pattern already refused anything that is not a
        // dotted-quad with a prefix length, so a parse failure here is a value that
        // pattern admits and no IPv4 subnet can be.
        let Some(network) = Ipv4Cidr::parse(vlan.cidr) else {
            failures.push(crate::state::field_failure(
                member,
                "is not a valid IPv4 CIDR block",
            ));
            continue;
        };
        let prefix_len = network.prefix_len;
        if !(MIN_VLAN_PREFIX_LEN..=MAX_VLAN_PREFIX_LEN).contains(&prefix_len) {
            failures.push(crate::state::field_failure(
                member,
                &format!(
                    "must have a prefix length between /{MIN_VLAN_PREFIX_LEN} and \
                     /{MAX_VLAN_PREFIX_LEN}, but was /{prefix_len}"
                ),
            ));
        }
        // "Amazon EVS VLAN subnet CIDR blocks must not overlap with other subnets"
        // (`InitialVlans` doc).
        if let Some((other, _)) = networks.iter().find(|(_, other)| other.overlaps(&network)) {
            failures.push(crate::state::field_failure(
                member,
                &format!("overlaps the CIDR block of {other}"),
            ));
        }
        networks.push((member, network));
    }

    failures
}

// The `InitialVlans` member a VLAN's `functionName` came from is carried on
// `PlannedVlan::member`, so no reverse lookup from the wire name is needed.
