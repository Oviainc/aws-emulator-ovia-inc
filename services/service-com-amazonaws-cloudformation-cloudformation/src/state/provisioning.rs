// SPDX-License-Identifier: CC0-1.0

//! Creating a stack's declared resources in the services that own them, and deleting them
//! when the stack is deleted.
//!
//! ## The rule this module implements
//!
//! A stack is not just a record of a template. When CloudFormation creates a stack it
//! calls the OWNING service's own create API once per resource the template's `Resources`
//! section declares, and it deletes those same resources when the stack is deleted. What
//! each resource IS — the physical id a `Ref` resolves to and the attributes an
//! `Fn::GetAtt` resolves to — is computed by [`super::template`]; what this module adds is
//! the call that makes the resource exist, carrying the tags CloudFormation tags it with
//! ([`StackProvenance`]).
//!
//! The call is made in-process through the handler's own callback into this emulator
//! ([`smithy_server_runtime::OperationContext::callback`]), signed with the emulator's
//! credentials: the resource is created by the SAME code path a client's own
//! `CreateDeliveryStream` or `CreateBucket` goes through, so a stack cannot create a
//! resource that differs from one the service would have created itself.
//!
//! There are TWO shapes that call takes, and which one a type uses follows from what its
//! properties are:
//!
//! * on the owning service's WIRE PROTOCOL, through [`call_json_rpc`] here — for a type
//!   whose properties ARE the create request's own members, like
//!   `AWS::KinesisFirehose::DeliveryStream`, where the evaluated property DOCUMENT is
//!   passed through verbatim with only the name CloudFormation decides set on the way out.
//!   A typed SDK builder cannot carry a document through: it would need every member of
//!   every destination configuration mapped by hand, which is precisely the mapping that
//!   would drift from the model as the service grows;
//! * through the owning service's official AWS SDK client over this same in-process
//!   callback — for a type whose properties are NOT the create's members and whose service
//!   speaks a protocol other than awsJson (`AWS::S3::Bucket` on restXml through
//!   `aws-sdk-s3`, `AWS::IAM::Role` on awsQuery through `aws-sdk-iam`), where the members
//!   the create takes are named one by one anyway, so the typed builder costs nothing and
//!   the protocol's serialization is the SDK's rather than this crate's.
//!
//! Both shapes ride the one signed, in-process transport below ([`dispatch`]), so neither
//! reaches the network and both are served by the router that serves a client's own call.
//!
//! ## What this module owns, and what [`super::provisioner`] owns
//!
//! This module is the part that is the SAME for every resource type: reconciling what a
//! deployment declares against what the stack already has ([`reconcile`]), deleting what
//! it replaced or what the stack's own deletion owes ([`delete`]), and the in-process,
//! signed transport each of those calls rides on ([`call_json_rpc`] for a service whose
//! model declares awsJson, [`call_query`] for one that declares awsQuery — the wire is the
//! called service's, not this one's). The part that is
//! per type — which service owns it, and the create, update and delete calls that make its
//! resources exist, change and stop existing — is [`super::provisioner`], reached only
//! through
//! [`provisioner_for`](super::provisioner::provisioner_for), so adding a resource type
//! never touches the algorithm here.
//!
//! A type whose properties are not the create's own members is mapped the one way its
//! owning API takes them, and no further: an `AWS::SQS::Queue`'s properties are the
//! queue's ATTRIBUTES, named identically on both sides, so they are passed through as the
//! `CreateQueue` `Attributes` map ([`queue_attributes`]) rather than as request members.
//! Either way the mapping is the documented correspondence between the type's property
//! names and the API's, never a per-member translation table.
//!
//! ## What a resource type needs before a stack can create it
//!
//! [`provisioner_for`](super::provisioner::provisioner_for) maps a template resource type
//! to the operations of the service that owns it. The type usually names that service
//! itself: its middle segment is the service model's own `awsService.cloudFormationName`
//! (Amazon Data Firehose's model carries `cloudFormationName: KinesisFirehose`, so its
//! streams are `AWS::KinesisFirehose::DeliveryStream`). Where CloudFormation's own
//! resource reference publishes a different name the published one wins — Amazon ECR
//! Public's model carries `cloudFormationName: ECRPUBLIC`, but its repositories are
//! published as `AWS::ECR::PublicRepository`, beside the private registry's
//! `AWS::ECR::Repository` — because the type is what a template spells, not what a model
//! suggests.
//!
//! What a type's PROPERTIES are decides how its create call is built, which is why that
//! half lives per type in [`super::provisioner`]: for
//! `AWS::KinesisFirehose::DeliveryStream` the properties ARE the request's members, so the
//! evaluated document is passed through verbatim; for `AWS::ECR::PublicRepository` and the
//! `AWS::OpenSearchServerless::*` types they are not (`RepositoryName` /
//! `RepositoryCatalogData.RepositoryDescription` against `repositoryName` /
//! `catalogData.description`, and `Name`/`Type`/`Policy` against `name`/`type`/`policy`), so
//! those arms map the members the resource reference publishes one by one, and send nothing
//! for a property the template leaves out; and for `AWS::ECS::Cluster` they are the same
//! members in a different CASE, with one renamed, so that arm re-cases them.
//!
//! A type with no entry there is left DECLARED-ONLY: [`StoredResource::materialised`]
//! stays `false` and nothing is created in any service. Such a resource still carries the
//! physical id and the attributes CloudFormation DOCUMENTS for its type, because those are
//! computed from the template and the stack's own identity; that is what lets a stack's
//! `Outputs` name a resource of a type no service here creates yet. The per-resource reads
//! (`DescribeStackResources` and its siblings) report it as the stack DECLARED it, with
//! the status of the deployment that recorded it — this emulator's inherited scope for
//! such a type, not a claim that the resource exists in that service. Adding a provisioner
//! for another type is a local change: one arm in
//! [`provisioner_for`](super::provisioner::provisioner_for) and the create/delete pair it
//! names, and that type's resources then really are created and deleted.
//!
//! ## Whose the resource's IDENTITY is
//!
//! For most types CloudFormation decides it: "Ref returns the bucket name", the queue url
//! it built, the stream name the template asked for — [`super::template`] computes the
//! physical resource id from the template and the stack, and the create call carries it
//! OUT to the owning service as the name of the resource to make.
//!
//! For a type whose owning service ASSIGNS the resource's identity it is the other way
//! round ([`super::template::identity_is_service_assigned`]): FIS mints an experiment
//! template's id, AWS WAF Classic answers `CreateIPSet` with an `IPSetId` of its own and
//! "Ref returns the IPSet ID", and Amazon EMR answers `RunJobFlow` with the `j-` job-flow id
//! an `AWS::EMR::Cluster`'s `Ref` returns — so the id a stack's `Ref`, its `Outputs` and its
//! per-resource reads report is the one that service answered, and the name CloudFormation
//! computed is only what the create asked for. [`reconcile`] adopts that id ([`Created`])
//! and the `Id`/`<Type>Id` attributes are re-pointed at it
//! ([`adopt_minted_identity`]), while a later deployment of the same resource keeps it
//! rather than recomputing one ([`super::StoredStack::deploy_template`]): the stack's own
//! deletion has to address the resource by the id its service knows it by.
//!
//! A type that DOES have an entry is never passed off as created: whatever its owning
//! service answers short of success — a modelled refusal, the 501 of an operation this
//! emulator has not implemented, or an answer that identifies nothing at all — leaves the
//! stack `CREATE_FAILED` with that answer as the reason, because the one thing an
//! attempted create that did not answer 2xx tells you is that the resource was not
//! created. A declaration that is missing a property its resource type REQUIRES is
//! refused the same way, naming that property.
//!
//! ## An identifier the owning service assigns
//!
//! Most types' physical resource id is a name the template asked for, which is why
//! [`super::template::provision`] can compute it. Some are SERVER-minted instead: an
//! OpenSearch Serverless collection's `Ref` is its `CollectionId`, which only the create
//! reports. Such a type's create answers that identifier ([`Created`]) and it replaces the
//! computed one, and
//! [`identity_is_service_assigned`](super::template::identity_is_service_assigned) is what
//! tells a re-deploy to carry the recorded identifier forward rather than read the
//! resource as replaced.
//!
//! A resource of ANOTHER such resource — a Resolver rule routing through a Resolver
//! endpoint of the same template — names it by a reference the declaration resolved before
//! either existed, so that property holds the stand-in the naming rules computed. Once the
//! endpoint's create has answered, a property of a resource this walk has not reached yet
//! whose value IS that stand-in is re-pointed at the minted identifier
//! ([`repoint_references`]): the create carries the identifier the owning service knows its
//! dependency by. A LATER deployment resolves its declarations against the identifiers the
//! stack already has ([`super::StoredStack::deploy_template`]), which is what makes a
//! re-deploy of the same template read as unchanged.
//!
//! ## Identity, and the isolation these calls can and cannot carry
//!
//! The call is signed with the EMULATOR's own credentials, because that is what they are
//! for: "The emulator's own (root) credentials, `ctx.callback().credentials()` and
//! `ctx.callback().aws_credentials_provider()`, only sign this operation's in-process
//! calls to other services" (`AGENTS.md`). The caller's own signature cannot be reused —
//! the emulator never holds a client's secret (its `CredentialStore` holds only
//! credentials an operation minted), so no signature of the caller's could be produced
//! here.
//!
//! What the call is signed with is not what DECIDES it, though. A stack operation acts as
//! the stack's service role — "CloudFormation uses the role's credentials to make calls on
//! your behalf" — so the permissions that role carries are what say whether the change this
//! call asks for may be made at all, and a call that role is denied is never sent
//! ([`super::service_role`], asked in [`call_json_rpc`] and [`call_query`] before either
//! writes a request).
//!
//! Nor is there state for a credential to select. Every service of this emulator reaches
//! its store by TYPE out of the ONE `InMemoryEngine` the router builds and hands to all of
//! them (`AGENTS.md`: "`engine` is deliberately ONE crate shared by every service crate
//! and the router"; `::engine::InMemoryEngine::state::<T>()` takes no identity), and every
//! ARN it mints carries the single canonical account id
//! (`::emulator_server::EMULATOR_ACCOUNT_ID`). A stack's stream therefore lands in exactly
//! the store a client's own `CreateDeliveryStream` writes to — which is the point of
//! provisioning it — and the account half of a credential scope has one value, so the
//! REGION is the only part there is to propagate: these calls carry the stack's own
//! Region, the one the call that created it was signed for, on both the create and the
//! delete.
//!
//! What this module does keep apart is one STACK's resources from another's: a stack's
//! deletion deletes the resources ITS OWN records say it created
//! ([`StoredResource::materialised`]), never everything an owning service happens to hold.
//! Whether a resource another stack (or a client) already created can be deployed over is
//! the OWNING service's answer, not this module's: Firehose refuses the create with its own
//! uniqueness rule ("This name must be unique per Amazon Web Services account in the same
//! Amazon Web Services Region"), which leaves the second stack `CREATE_FAILED` with that
//! refusal and the existing stream untouched, while S3 answers a re-create of a bucket the
//! caller already owns with 200 in us-east-1 ("for legacy compatibility, if you re-create
//! an existing bucket that you already own…"), so that create is accepted and the bucket
//! is left exactly as it was — nothing is re-created and nothing in it is cleared. The
//! stack's own deletion then deletes what ITS records say it created, so a bucket it
//! deployed over is deleted by the stack that recorded it, and one that still holds objects
//! is refused by S3 and reported as `DELETE_FAILED`. Both answers are held to by
//! `core/router/tests/all/cloudformation_provisions_stack_resources.rs` and by
//! `tests/all/stack_resource_provisioning.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::Value;

use super::provisioner::{provisioner_for, Provisioner};
use super::StoredResource;

/// The awsJson media types the services this module calls speak, named once here because
/// [`call_json_rpc`] takes the one the called service's own model declares: an answer is
/// rendered in it, and a call written in the other version is not the wire that service
/// publishes.
pub(super) const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";
pub(super) const AWS_JSON_1_1: &str = "application/x-amz-json-1.1";

/// The media type a REST-JSON service's requests and answers carry
/// ([`call_rest_json`]): a `restJson1` service publishes `application/json`, not one of
/// the awsJson versions above.
pub(super) const REST_JSON: &str = "application/json";

/// The media type a REST-XML service's requests and answers carry ([`call_rest_xml`]): a
/// `restXml` service (Amazon Route 53) takes an XML document rather than a JSON one.
pub(super) const REST_XML: &str = "application/xml";

/// What the service that owns a resource answered when it created it.
///
/// `physical_resource_id` is set only by a type whose id that service ASSIGNS (an FIS
/// experiment template's `EXT…`, an AWS WAF Classic IPSet's `IPSetId`), which no naming rule
/// could have computed: it replaces the computed id, because it is what `Ref` resolves to
/// and what a client calls the service with. For every other type the computed id IS the one
/// the resource was created under (a queue's name, a cluster's name), so the arm leaves it
/// alone.
#[derive(Debug, Clone, Default)]
pub(super) struct Created {
    pub(super) physical_resource_id: Option<String>,
    pub(super) attributes: BTreeMap<String, String>,
}

impl Created {
    /// What a create that reported only ATTRIBUTES answered: the resource keeps the
    /// physical id the naming rules computed.
    pub(super) fn attributes(attributes: BTreeMap<String, String>) -> Self {
        Self {
            physical_resource_id: None,
            attributes,
        }
    }
}

/// The tag key CloudFormation records the NAME of the stack a resource belongs to under,
/// on the resource itself.
pub const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
/// The tag key it records that stack's ID under.
pub const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
/// The tag key it records the resource's LOGICAL id in that stack under.
pub const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// The stack a deployment is provisioning for, as far as the resources it creates are
/// concerned: the identity CloudFormation records ON each of them, and the stack-level tags
/// it propagates TO each of them. The Region those resources live in travels beside this as
/// [`reconcile`]'s own argument, because it is also what the credential scope of every call
/// this module makes names.
///
/// ## The rule this carries
///
/// CloudFormation does not only create a stack's resources — it TAGS them. The stack-level
/// `Tags` a stack operation supplies are "propagated to the resources that CloudFormation
/// creates in the stack", and CloudFormation adds three tags of its own naming the stack
/// and the resource's place in it ([`STACK_NAME_TAG`], [`STACK_ID_TAG`],
/// [`LOGICAL_ID_TAG`]). So the owning service's own tag read on the handle a stack
/// publishes for a resource answers with those tags — which is what tag-based cost
/// allocation, tag-based access control and anything keying off `aws:cloudformation:*` see.
///
/// Applying them is PER TYPE, in [`super::provisioner`], because the tag set reaches each
/// owning API its own way (a `Tags` member of the create, a map, a tagging call of its own);
/// what is the same for every type is the set itself, which is [`Self::tags_for`].
#[derive(Debug, Clone, Default)]
pub struct StackProvenance {
    /// `Stack$StackName`.
    pub stack_name: String,
    /// `Stack$StackId`.
    pub stack_id: String,
    /// The stack-level `Tags` in force, in the order the stack records them.
    pub tags: Vec<(String, String)>,
}

impl StackProvenance {
    /// The tags CloudFormation applies to the resource `logical_id` names: the stack's own,
    /// then the three CloudFormation records itself — which are CloudFormation's to set, so
    /// a stack tag spelled with one of those reserved keys does not displace them.
    ///
    /// A tag with an empty key is not a tag and is left out, and a key stated twice keeps
    /// its first position with the later value, so what reaches an owning service is a
    /// well-formed tag set however the stack itself was tagged.
    pub fn tags_for(&self, logical_id: &str) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = Vec::new();
        let mut put = |key: &str, value: &str| {
            if key.is_empty() {
                return;
            }
            match tags
                .iter_mut()
                .find(|(existing, _)| existing.as_str() == key)
            {
                Some((_, held)) => *held = value.to_string(),
                None => tags.push((key.to_string(), value.to_string())),
            }
        };
        for (key, value) in &self.tags {
            put(key, value);
        }
        put(STACK_NAME_TAG, &self.stack_name);
        put(STACK_ID_TAG, &self.stack_id);
        put(LOGICAL_ID_TAG, logical_id);
        tags
    }
}

/// One transition a stack operation made ONE of its resources go through, as
/// CloudFormation records a `StackEvent` for it.
///
/// A stack operation is not one transition but a walk over the template's resources, and
/// CloudFormation records the walk: an `*_IN_PROGRESS` event when it starts acting on a
/// resource, a second one that says the create was initiated in the service that owns it,
/// and a `*_COMPLETE` or `*_FAILED` event when that service has answered.
/// That is what names the resource a deployment failed on, and what carries the owning
/// service's own refusal ([`Self::reason`]) — neither of which the STACK's own events can
/// say, since they describe the operation as a whole.
///
/// WHEN each of those happened is not recorded here. The calls below run in-process, one
/// after another, so the wall clock while they return says nothing about the walk: the
/// instant a resource was scheduled and the instant it finished follow from the dependency
/// graph and from the signals a wait condition waited for, which is
/// [`super::timeline`]'s to compute.
///
/// Collected here rather than written straight to the store because the calls that make a
/// resource exist run with the store UNLOCKED: the operation that drove them records them
/// on the stack ([`super::StoredStack::schedule_resource_events`]) when it takes the lock
/// again.
#[derive(Debug, Clone)]
pub struct ResourceTransition {
    /// `StackEvent$LogicalResourceId`: the name the template gives the resource.
    pub logical_id: String,
    /// `StackEvent$ResourceType`.
    pub resource_type: String,
    /// `StackEvent$PhysicalResourceId`, which a resource nothing has created yet — and one
    /// whose create was refused — does not have.
    pub physical_resource_id: Option<String>,
    /// `StackEvent$ResourceStatus`, from the model's `ResourceStatus` enum
    /// ([`super::resource_status`]).
    pub status: String,
    /// `StackEvent$ResourceStatusReason`: "Success/failure message associated with the
    /// resource" — the owning service's refusal for a failed transition, and nothing for
    /// one that succeeded.
    pub reason: Option<String>,
    /// `StackEvent$ResourceProperties`: "BLOB of the properties used to create the
    /// resource" — the properties document the template declares for it, which is what the
    /// deployment asked the owning service for.
    pub resource_properties: Option<String>,
}

impl ResourceTransition {
    /// The transition a resource is STARTING: `*_IN_PROGRESS`, reporting the identifier the
    /// resource has at that point — the one it already carries for a change to a resource
    /// that exists, and none for a create, where the owning service has not answered yet.
    fn starting(resource: &StoredResource, status: &str) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: None,
            status: status.to_string(),
            reason: None,
            resource_properties: resource.properties_blob(),
        }
    }

    /// The transition that OPENS a resource's create: `CREATE_IN_PROGRESS` before anything
    /// exists.
    ///
    /// `StackEvent$PhysicalResourceId` is "the name or unique identifier associated with
    /// the physical instance of the resource", and there is no such instance yet, so this
    /// event names an EMPTY one — the member is reported, and what it reports is that
    /// nothing has been created under it so far.
    fn creating(resource: &StoredResource) -> Self {
        Self {
            physical_resource_id: Some(String::new()),
            ..Self::starting(resource, super::resource_status::CREATE_IN_PROGRESS)
        }
    }

    /// The transition that reports the create of a resource was INITIATED: the second
    /// `CREATE_IN_PROGRESS` CloudFormation records for it, once the request to create it
    /// has gone to the service that owns it, naming the identifier it is being created
    /// under.
    fn initiated(resource: &StoredResource) -> Self {
        Self {
            reason: Some(super::RESOURCE_CREATION_INITIATED.to_string()),
            ..Self::finished(resource, super::resource_status::CREATE_IN_PROGRESS, None)
        }
    }

    /// The transition a resource FINISHED: the status it reached, the identifier it carries
    /// now, and the reason a failure came with.
    fn finished(resource: &StoredResource, status: &str, reason: Option<String>) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: super::physical_resource_id(resource),
            status: status.to_string(),
            reason,
            resource_properties: resource.properties_blob(),
        }
    }

    /// The transition a resource failed: as [`Self::finished`], with no identifier —
    /// nothing was created, so there is no physical id to report.
    fn failed(resource: &StoredResource, status: &str, reason: String) -> Self {
        Self {
            physical_resource_id: None,
            ..Self::finished(resource, status, Some(reason))
        }
    }
}

/// What deploying a stack's template left its resources as.
#[derive(Debug, Clone, Default)]
pub struct Materialised {
    /// The stack's resources, in the order the template declares them: each one as
    /// [`super::template::provision`] computed it, with `materialised` set and any
    /// attribute the owning service reported for it applied.
    pub resources: Vec<StoredResource>,
    /// The reason the stack cannot reach its completed status: an owning service refused
    /// to create one of the resources, or to delete one the deployment replaced, or
    /// CloudFormation itself refused the deployment before any service was asked anything.
    /// `None` when every resource this emulator provisions was created.
    ///
    /// This is the STACK-level reason, and WHOSE failure it was decides what it says. A
    /// resource an owning service refused is reported as the summary of which logical
    /// resources failed ([`super::resource_failure_summary`]), with why each one failed on
    /// that resource's own transition ([`ResourceTransition::reason`]). A deployment
    /// CLOUDFORMATION refused — a dynamic reference it could not resolve
    /// ([`super::dynamic_references::resolve_declared`]) — is its own diagnostic: no
    /// resource was asked of anybody, so there is no resource whose failure it could be
    /// summarising.
    pub failure: Option<String>,
    /// Every resource transition this deployment made, oldest first — the stack's
    /// per-resource event records.
    pub transitions: Vec<ResourceTransition>,
    /// The resources this deployment CREATED whose owning service does not report them
    /// ready yet, and the ones it DELETED whose owning service still holds them: the call
    /// was accepted and has not been carried out, so CloudFormation holds the resource — and
    /// the operation — open until that service says otherwise
    /// ([`super::stabilising`]).
    pub stabilising: Vec<Stabilising>,
}

/// One resource whose creation an owning service accepted and has not finished, or whose
/// deletion it accepted and has not finished.
///
/// What the stack needs in order to go on asking that service about it: which resource it
/// is, which answer ends the wait, and the state that service reported when the call
/// answered.
#[derive(Debug, Clone)]
pub struct Stabilising {
    /// Which of that service's answers ends this wait
    /// ([`super::stabilising::Awaited`]): the resource is ready, or the resource is gone.
    pub awaited: super::stabilising::Awaited,
    pub logical_id: String,
    pub resource_type: String,
    /// The identifier the owning service assigned, which the read that ends the wait
    /// addresses the resource by.
    pub physical_resource_id: String,
    /// That service's own word for the state the resource is in, as its API spells it.
    pub reported_state: String,
}

/// What the service that owns a resource says about a resource it has CREATED: whether the
/// resource has reached the state that service reports it ready in.
///
/// Answered by the type's own arm ([`super::provisioner::Provisioner::readiness`]) from
/// that service's own read, so it is never a status this crate guessed. A type whose create
/// leaves the resource ready — which is most of them — answers [`Self::Ready`] without
/// asking anything.
#[derive(Debug, Clone)]
pub(super) enum Readiness {
    /// The resource is in the state that service reports it ready in, so its creation is
    /// finished.
    Ready,
    /// Not yet: the state that service reports the resource in right now.
    Pending(String),
    /// That service reports the resource FAILED: nothing is going to bring it up, so the
    /// resource's creation failed with this reason rather than waiting the window out.
    Failed(String),
}

/// What the service that owns a resource says about a resource it has been asked to DELETE:
/// whether it still holds the resource.
///
/// Answered by the type's own arm ([`super::provisioner::Provisioner::removal`]) from that
/// service's own read, so it is never a status this crate guessed. A type whose delete takes
/// the resource away — which is most of them — answers [`Self::Gone`] without asking
/// anything.
#[derive(Debug, Clone)]
pub(super) enum Removal {
    /// That service no longer holds the resource, so its deletion is finished.
    Gone,
    /// Not yet: the state that service reports the resource being removed in right now.
    Pending(String),
}

/// Ask the services that own a Region's still-creating and still-deleting resources whether
/// they are done yet, and record what they answer.
///
/// This is the other half of [`super::stabilising`]: a resource whose owning service
/// accepted the create without reporting the resource ready, or accepted the delete while
/// still holding the resource, holds its stack's operation open, and what ENDS that wait is
/// that service's own answer about the resource — which only a call to it can give. `settle`
/// cannot make that call (it runs under the store lock, and a read of this store cannot reach
/// another service), so the operations that REPORT a stack's progress ask here, before they
/// take the lock: a client polling a stack's resources is exactly the caller that needs the
/// answer.
///
/// What this records was owed at the instant the owning service reached it, whether or not
/// anybody called: the resource became ready, or went away, then, and a wait that has run out
/// ([`super::stabilising::StabilisationBarrier::has_expired`]) failed then. The one thing it
/// changes in another service is what that failure owes: a failed create is rolled back, so
/// the resources it had made are removed again — the same work
/// [`crate::ops::create_stack`]'s own failure path does, and for the same reason
/// [`super::settle_due_rollbacks`] performs a failed update's rollback here rather than under
/// the store lock. Every other call this makes is that service's own READ.
///
/// Only the stacks of the Region the request was signed for are advanced: that is the set
/// the operation calling this reports on, and the Region is what the credential scope of
/// every call made here names.
pub async fn advance_stabilisations(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsCloudformationCloudformationState>,
) {
    let region = super::request_region(ctx.parts());
    let region = region.as_str();
    // Which resources are still waiting, read under a lock of its own: the readiness calls
    // below reach other services' handlers, which take those services' locks, so a client
    // reading THIS service must not be blocked behind them.
    let waiting: Vec<(String, Vec<super::stabilising::StabilisationBarrier>)> = {
        let store = state.lock();
        store
            .stacks
            .iter()
            .filter(|stack| stack.region == region)
            .filter_map(|stack| {
                let open = stack.open_stabilisations();
                if open.is_empty() {
                    None
                } else {
                    Some((stack.stack_id.clone(), open))
                }
            })
            .collect()
    };
    for (stack_id, barriers) in waiting {
        for barrier in barriers {
            // WHICH of the owning service's answers ends this wait: that the resource it
            // created is ready, or that the resource it deleted is gone. A removal that is
            // still in flight holds the operation exactly as a creation that is still coming
            // up does, so the two answers are read as the one thing they both are — whether
            // the call that service accepted has been carried out.
            let readiness = match provisioner_for(&barrier.resource_type) {
                Some(provisioner) => match barrier.awaited {
                    super::stabilising::Awaited::Ready => {
                        provisioner
                            .readiness(ctx, region, &barrier.physical_resource_id)
                            .await
                    }
                    super::stabilising::Awaited::Gone => provisioner
                        .removal(ctx, region, &barrier.physical_resource_id)
                        .await
                        .map(|removal| match removal {
                            Removal::Gone => Readiness::Ready,
                            Removal::Pending(state) => Readiness::Pending(state),
                        }),
                },
                // Nothing owns the type any more, so there is nothing to ask: the wait is
                // over, because what opened it was that provisioner's answer.
                None => Ok(Readiness::Ready),
            };
            let now = SystemTime::now();
            // The identifiers the owning service publishes for a resource that has only NOW
            // come up — an endpoint a still-creating resource did not have — read before the
            // lock is taken, like every other call this module makes. Asked only where that
            // service has just reported the resource ready; a read it refuses leaves the
            // records as they stand, since one unanswered read is no evidence about the
            // resource.
            let ready_attributes = if matches!(readiness, Ok(Readiness::Ready))
                && barrier.awaited == super::stabilising::Awaited::Ready
            {
                match provisioner_for(&barrier.resource_type) {
                    Some(provisioner) => provisioner
                        .ready_attributes(
                            ctx,
                            region,
                            &barrier.resource_type,
                            &barrier.physical_resource_id,
                        )
                        .await
                        .unwrap_or_default(),
                    None => BTreeMap::new(),
                }
            } else {
                BTreeMap::new()
            };
            // What a failure the answer makes due owes besides the stack's own failed
            // status: the resources a rolled-back CREATE has to remove again. Taken under
            // the lock and performed with it RELEASED, as every other call this module
            // makes is.
            let unwinding = {
                let mut store = state.lock();
                let Some(stack) = store.resolve_mut(region, &stack_id) else {
                    continue;
                };
                let taken = match readiness {
                    Ok(Readiness::Ready) => {
                        stack.record_ready_attributes(&barrier.logical_id, &ready_attributes);
                        stack.release_stabilisation(&barrier, now);
                        None
                    }
                    // Still coming up, or still being removed: the wait holds, unless it has
                    // run out — which is the instant the call the wait was for failed.
                    Ok(Readiness::Pending(state)) => {
                        if barrier.has_expired(now) {
                            stack.fail_stabilisation(
                                now,
                                &barrier,
                                &barrier.not_stabilised_reason(&state),
                            )
                        } else {
                            None
                        }
                    }
                    // That service says the resource will not come up, so waiting the window
                    // out would report a resource as creating that nothing is creating.
                    Ok(Readiness::Failed(reason)) => {
                        stack.fail_stabilisation(now, &barrier, &reason)
                    }
                    // The service could not be asked at all. One unanswered read is not
                    // evidence about the resource — a service is allowed to throttle or be
                    // briefly unreachable — so the wait holds and the next read asks again;
                    // only the window running out ends it.
                    Err(refusal) => {
                        if barrier.has_expired(now) {
                            stack.fail_stabilisation(
                                now,
                                &barrier,
                                &handler_failure_reason(
                                    provisioner_for(&barrier.resource_type),
                                    &refusal,
                                ),
                            )
                        } else {
                            None
                        }
                    }
                };
                // The records the stack holds are what the rollback takes down and reports
                // ([`unwind`]), read off the stack under the same lock the resources were —
                // with what that rollback does with their `DeletionPolicy`.
                taken.map(|resources| {
                    (
                        resources,
                        stack.held_resource_ids(),
                        stack.rolled_back_resource_policies(),
                    )
                })
            };
            // "ROLLBACK — Roll back the stack creation. This is the default": the resources
            // the failed create had already made are removed again in the services that own
            // them, exactly as `crate::ops::create_stack`'s own failure path removes them,
            // and only a delete one of those services REFUSES changes the status the
            // rollback settles into.
            let Some((resources, held, policies)) = unwinding else {
                continue;
            };
            let deleted = unwind(ctx, region, &resources, &held, policies).await;
            let mut store = state.lock();
            let Some(stack) = store.resolve_mut(region, &stack_id) else {
                continue;
            };
            stack.schedule_resource_events(&deleted.transitions);
            stack.record_materialised_resources(deleted.resources);
            // A resource whose owning service accepted the delete and still holds it holds
            // this rollback open, exactly as the resource that failed to come up held the
            // operation this rollback follows.
            stack.record_stabilisations(&deleted.removing, SystemTime::now());
            if let Some(failure) = deleted.failure {
                stack.fail_transition(SystemTime::now(), super::status::ROLLBACK_FAILED, failure);
            }
        }
    }
}

/// Reconcile what a stack's newly deployed template declares with what the stack already
/// has: create what is new in the service that owns it, carry forward what is unchanged,
/// and delete what the deployment replaced or removed.
///
/// `region` is the stack's own Region: the resources live there, and it is what the
/// credential scope of every call this module makes names. `previous` is the set the
/// stack's last completed deployment left (empty for a create), which is what makes a
/// re-deploy of the same template a no-op rather than a second create — and what keeps a
/// resource another deployment created from being forgotten, so the stack's deletion still
/// deletes it.
///
/// Every dynamic reference the declarations carry is resolved FIRST, by CloudFormation
/// itself ([`super::dynamic_references::resolve_declared`]): one it cannot honour refuses
/// the whole deployment before any owning service is asked anything, so nothing is created
/// and [`Materialised::failure`] is the resolver's own diagnostic rather than a summary of
/// resources that failed.
///
/// `refused` is a refusal CLOUDFORMATION itself reached before this walk — the deployment's
/// template imports an export nothing publishes
/// ([`super::StoredStack::unresolved_import_failure`]) — and it is answered exactly as the
/// gate above answers one of its own: the walk is not entered, nothing is asked of any
/// owning service, and [`Materialised::failure`] is that diagnostic. `None` is a deployment
/// CloudFormation has nothing against, which is every rollback and every revert.
///
/// The resources are created in the order the template declares them. The first refusal
/// from an owning service stops the walk and is reported as [`Materialised::failure`]: the
/// resources created before it are still reported (with their `materialised` flag set), so
/// the stack's deletion can clean them up, and the ones after it are reported as the
/// declaration left them.
///
/// A resource the stack already has and this deployment re-declares with DIFFERENT
/// properties is UPDATED IN PLACE, through the owning service's own update API
/// ([`super::provisioner::Provisioner::update`]): the resource keeps its identity, so
/// nothing is created and nothing is deleted, and the stack records the declared
/// configuration because that is the one now in force. Only a type with no update path
/// there, and a change the owning service refuses, leave the deployment with a failure —
/// never a success that dropped the caller's change.
///
/// `stack` is the stack the resources belong to: every resource this CREATES is created
/// carrying the tags CloudFormation gives it ([`StackProvenance::tags_for`]), so the
/// owning service's own tag read on the handle the stack publishes answers with the stack's
/// tags and the `aws:cloudformation:*` ones. A resource the deployment carries FORWARD is
/// brought back into step with that same set
/// ([`super::provisioner::Provisioner::retag`]), since a deployment can state the stack's
/// tags differently with no resource's declaration changing at all; nothing is called about
/// a resource whose tags already agree with it, and a resource whose configuration change
/// the owning service refused is not re-tagged either.
///
/// Every resource the walk ACTS ON is recorded as a pair of transitions
/// ([`Materialised::transitions`]) — the one that opens it and the one that ends it — which
/// is what lets the stack's event stream name the resource a deployment failed on and carry
/// the owning service's own refusal. A resource the deployment leaves exactly as it found it
/// transitions through nothing and is recorded nowhere, and neither is one the walk never
/// reached.
///
/// `removal_policies` is what this walk does with the `DeletionPolicy` of a resource it
/// REMOVES — one the stack has and this deployment no longer declares ([`Policies`]). An
/// ordinary deployment reads the attribute, which is where CloudFormation stops managing the
/// resource; a deployment that is a stack's ROLLBACK passes what the operation being rolled
/// back was asked for ([`super::StoredStack::rolled_back_resource_policies`]), since
/// `RetainExceptOnCreate` is documented to override that attribute for a resource the
/// operation itself created. The OLD physical resource of a replacement is governed by
/// `UpdateReplacePolicy` instead, so that walk reads no deletion policy either way.
pub async fn reconcile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    stack: &StackProvenance,
    declared: Vec<StoredResource>,
    previous: &[StoredResource],
    refused: Option<String>,
    removal_policies: Policies,
) -> Materialised {
    let mut outcome = Materialised {
        // A deployment CLOUDFORMATION itself refused before this walk: nothing is asked of
        // any owning service for it, and every declaration is recorded as the declaration
        // left it by the tail below — the same treatment the gate just below gives a
        // refusal of its own.
        failure: refused,
        ..Materialised::default()
    };
    // The identifiers the owning services MINTED in this walk, each against the stand-in
    // the naming rules had computed for that resource ([`repoint_references`]).
    let mut minted: Vec<Minted> = Vec::new();
    let mut declared = declared.into_iter();
    // Every `{{resolve:...}}` DYNAMIC REFERENCE the declarations carry is resolved by
    // CLOUDFORMATION ITSELF, here, before anything is asked of any owning service: a
    // reference no store can satisfy has no value, so there is nothing to ask any service
    // for. Such a reference is therefore CloudFormation's OWN refusal of the deployment —
    // no provider is called for ANY of the resources, and the reason the stack reports is
    // the resolver's diagnostic (which reference could not be honoured, and why) rather
    // than a summary of resources that tried and failed
    // ([`super::resource_failure_summary`]).
    //
    // A deployment already refused above is not resolved against any store at all: that
    // refusal stands, and nothing is read for a deployment nothing will deploy.
    let mut resolutions = if outcome.failure.is_some() {
        BTreeMap::new()
    } else {
        match super::dynamic_references::resolve_declared(ctx, region, declared.as_slice())
            .await
        {
            Ok(resolutions) => resolutions,
            Err(reason) => {
                outcome.failure = Some(reason);
                BTreeMap::new()
            }
        }
    };
    // A deployment the gate above refused deploys nothing: the walk is not entered, and
    // every declaration is recorded as the declaration left it by the tail below.
    while outcome.failure.is_none() {
        let Some(mut resource) = declared.next() else {
            break;
        };
        // The resource's record carries the document the stack RECORDS — a secure
        // reference's value is never one of them — while the owning service is asked for
        // the resource with the resolved one.
        let resolved = resolutions
            .remove(&resource.logical_id)
            .unwrap_or_else(|| {
                super::dynamic_references::Resolution::verbatim(&resource.properties)
            });
        resource.properties = resolved.recorded;
        let mut deployed_properties = resolved.deployed;
        // A property of this resource naming one created EARLIER in this walk carries the
        // stand-in id that resource had when the declaration was resolved, which is not the
        // id its owning service went on to mint. The reference is re-pointed at the minted
        // one before anything is asked about this resource, so the create carries the
        // identifier its service knows the dependency by — and so the comparison below
        // reads an UNCHANGED declaration as unchanged. Both documents the resolution left
        // are re-pointed, since both name that dependency.
        repoint_references(&mut resource.properties, &minted);
        repoint_references(&mut deployed_properties, &minted);
        if let Some(existing) = previous
            .iter()
            .find(|existing| existing.materialised && is_same_resource(existing, &resource))
        {
            // The owning service already created exactly this resource and the deployment
            // did not change its identity, so nothing is created again; what that service
            // reported about it, and the configuration it really has, are carried forward.
            let declared_properties = resource.properties.clone();
            let changed = declared_properties != existing.properties;
            carry_forward(existing, &mut resource);
            if changed {
                // ...but this deployment asks for a DIFFERENT configuration of a resource
                // that already exists. CloudFormation applies such a change THROUGH the
                // owning service's own update API, and the stack completes: the resource
                // keeps its identity, so nothing is created and nothing is deleted. The
                // call that applies it is the type's own entry
                // ([`super::provisioner::Provisioner::update`]); a type with no update
                // path there has nothing asked of its service, and the change is reported
                // as NOT applied rather than as a success that dropped the caller's
                // change.
                outcome.transitions.push(ResourceTransition::starting(
                    &resource,
                    super::resource_status::UPDATE_IN_PROGRESS,
                ));
                let applied = match provisioner_for(&resource.resource_type) {
                    Some(provisioner) => {
                        // The call is the stack's, made under its service role and about
                        // THIS resource ([`super::service_role`]).
                        super::service_role::on_resource(
                            super::service_role::target_of(&resource),
                            provisioner.update(ctx, region, &resource, &deployed_properties),
                        )
                        .await
                    }
                    None => Ok(None),
                };
                match applied {
                    // The owning service applied it, so the configuration in force is the
                    // DECLARED one — which is what the stack records, and what a later
                    // deployment and drift detection compare against. The transition that
                    // ENDS the resource is pushed after the tags are in step too, below, so
                    // `UPDATE_COMPLETE` means both halves of the change landed.
                    Ok(Some(updated)) => {
                        resource.attributes.extend(updated.attributes);
                        resource.properties = declared_properties.clone();
                    }
                    Ok(None) => {
                        let unsupported = format!(
                            "NotImplemented: this emulator does not change the properties of \
                             an existing {} in place; the resource was left as it is.",
                            resource.resource_type
                        );
                        outcome.failure = Some(super::resource_failure_summary(
                            super::failed_action::UPDATE,
                            [resource.logical_id.as_str()],
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(unsupported),
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                    Err(refusal) => {
                        // The owning service refused the change: nothing changed there, so
                        // the deployment reports that refusal rather than a configuration
                        // nothing applied.
                        outcome.failure = Some(super::resource_failure_summary(
                            super::failed_action::UPDATE,
                            [resource.logical_id.as_str()],
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(handler_failure_reason(
                                provisioner_for(&resource.resource_type),
                                &refusal,
                            )),
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                }
            }
            // The tags CloudFormation gives the resource are brought back into step AFTER
            // the configuration change, whether or not the declaration changed: a stack
            // operation's `Tags` are propagated to the resources in the stack, and a
            // deployment can state them differently with no resource's declaration changing
            // at all — while a configuration change the owning service REFUSED has already
            // left the loop, so a refused deployment does not re-tag what it could not
            // change. Only a type with a re-tagging arm has anything asked of its service
            // ([`super::provisioner::Provisioner::retag`]), and that arm is idempotent, so a
            // resource whose tags already agree with the set has nothing called about it.
            let retagged = match provisioner_for(&resource.resource_type) {
                Some(provisioner) => {
                    let tags = stack.tags_for(&resource.logical_id);
                    super::service_role::on_resource(
                        super::service_role::target_of(&resource),
                        provisioner.retag(ctx, region, &resource, &deployed_properties, &tags),
                    )
                    .await
                }
                None => Ok(()),
            };
            match retagged {
                // Both halves of the change landed, which is what ends a resource this
                // deployment acted on.
                Ok(()) => {
                    if changed {
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_COMPLETE,
                            None,
                        ));
                    }
                }
                Err(refusal) => {
                    if !changed {
                        // A tag-only change opens the resource's transition pair here: the
                        // configuration half never asked anything of the owning service.
                        outcome.transitions.push(ResourceTransition::starting(
                            &resource,
                            super::resource_status::UPDATE_IN_PROGRESS,
                        ));
                    }
                    outcome.failure = Some(super::resource_failure_summary(
                        super::failed_action::UPDATE,
                        [resource.logical_id.as_str()],
                    ));
                    outcome.transitions.push(ResourceTransition::finished(
                        &resource,
                        super::resource_status::UPDATE_FAILED,
                        Some(handler_failure_reason(
                            provisioner_for(&resource.resource_type),
                            &refusal,
                        )),
                    ));
                    outcome.resources.push(resource);
                    break;
                }
            }
            outcome.resources.push(resource);
            continue;
        }
        // The deployment is CREATING this resource: the stack records it as one it created
        // once this is through, whether the create reaches an owning service or not, so the
        // transition that opens it is recorded here.
        outcome
            .transitions
            .push(ResourceTransition::creating(&resource));
        // Which service owns the type, resolved ONCE here: the create below and the
        // readiness read that follows it are two questions for the same provisioner, and
        // one lookup is what keeps them from ever being asked of different ones.
        let owner = provisioner_for(&resource.resource_type);
        let created = match owner {
            // The resource is created WITH the tags CloudFormation gives it: the stack's
            // own, plus the three that name the stack and this resource's place in it.
            Some(provisioner) => {
                let tags = stack.tags_for(&resource.logical_id);
                // Asked for with the DEPLOYED document: the record keeps the one the stack
                // reports, so the create carries a resolved dynamic reference's value
                // while nothing the stack reports does.
                let asked = StoredResource {
                    properties: deployed_properties,
                    ..resource.clone()
                };
                super::service_role::on_resource(
                    super::service_role::target_of(&asked),
                    provisioner.create(ctx, region, &asked, &tags),
                )
                .await
            }
            // No provisioner for this type: nothing is asked of any service, and the
            // resource stays DECLARED-ONLY — which the stack still records as a resource it
            // created, under the identifier the naming rules computed, so the completion of
            // that record is recorded too.
            None => {
                outcome
                    .transitions
                    .push(ResourceTransition::initiated(&resource));
                outcome.transitions.push(ResourceTransition::finished(
                    &resource,
                    super::resource_status::CREATE_COMPLETE,
                    None,
                ));
                outcome.resources.push(resource);
                continue;
            }
        };
        match created {
            // The owning service created it, and what IT answered wins over the computed
            // value: that is the resource's real identity — including the physical id
            // itself, where that service ASSIGNS the id rather than taking it from a name
            // the template or CloudFormation decided.
            Ok(created) => {
                let computed = resource.physical_resource_id.clone();
                let computed_attributes = resource.attributes.clone();
                adopt_minted_identity(&mut resource, created.physical_resource_id);
                // An EMPTY stand-in is no identifier a reference could have resolved to,
                // and matching one would rewrite every empty value — and every empty
                // segment of a longer one — so there is nothing to re-point for it, exactly
                // as for an attribute the naming rules left empty below.
                if !computed.is_empty() && resource.physical_resource_id != computed {
                    minted.push(Minted {
                        // The stand-in is an identifier CloudFormation COMPUTED whenever it
                        // is not the name the declaration itself asked for, so a longer
                        // value carrying it carries it because a reference to this resource
                        // put it there — see [`Minted::composed`].
                        composed: super::declared_name(&resource).as_deref() != Some(&*computed),
                        stand_in: computed.clone(),
                        assigned: resource.physical_resource_id.clone(),
                    });
                }
                resource.attributes.extend(created.attributes);
                // An ATTRIBUTE the owning service reports differently from the value the
                // naming rules computed is re-pointed exactly as the physical id is: a
                // later resource's property that `Fn::GetAtt` resolved to the stand-in has
                // to reach its own service as the value this resource REALLY has — a key
                // ARN carrying the key id KMS minted, say — or that service is handed an
                // identifier nothing answers to.
                for (attribute, stand_in) in &computed_attributes {
                    let Some(assigned) = resource.attributes.get(attribute) else {
                        continue;
                    };
                    // An attribute whose computed value IS the identifier `Ref` resolved
                    // to is NOT re-pointed: the uniform convention gives every resource an
                    // `Id`/`<Type>Id` and a `Name`/`<Type>Name` read off its physical id,
                    // so for a type whose id is a NAME those stand-ins are that name — and
                    // another resource's property carrying it (an `AWS::IAM::ManagedPolicy`
                    // naming the role it is attached to) carries the name on purpose. Only
                    // the physical id's own pair above may rewrite it, which it does when
                    // the owning service assigned a different identity.
                    if stand_in == &computed {
                        continue;
                    }
                    if !stand_in.is_empty() && assigned != stand_in {
                        minted.push(Minted {
                            stand_in: stand_in.clone(),
                            assigned: assigned.clone(),
                            // An attribute is an ARN, an endpoint or a url — a value a
                            // property carries WHOLE.
                            composed: false,
                        });
                    }
                }
                resource.materialised = true;
                outcome
                    .transitions
                    .push(ResourceTransition::initiated(&resource));
                // The create was ACCEPTED; whether the resource is usable is the owning
                // service's to say. A type whose create leaves it coming up answers the
                // state it is in, and the resource's creation is held open until that
                // service reports it ready ([`super::stabilising`]) — CloudFormation does
                // not report a resource created on the strength of an accepted call.
                let readiness = match owner {
                    Some(provisioner) => {
                        provisioner
                            .readiness(ctx, region, &resource.physical_resource_id)
                            .await
                    }
                    None => Ok(Readiness::Ready),
                };
                match readiness {
                    Ok(Readiness::Ready) => {
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::CREATE_COMPLETE,
                            None,
                        ));
                    }
                    Ok(Readiness::Pending(state)) => {
                        // The terminal transition is planned here as it is for any other
                        // resource; the gate this records is what holds it back until the
                        // owning service reports the resource ready.
                        outcome.stabilising.push(Stabilising {
                            awaited: super::stabilising::Awaited::Ready,
                            logical_id: resource.logical_id.clone(),
                            resource_type: resource.resource_type.clone(),
                            physical_resource_id: resource.physical_resource_id.clone(),
                            reported_state: state,
                        });
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::CREATE_COMPLETE,
                            None,
                        ));
                    }
                    // The resource exists in that service and is not going to come up, or
                    // that service could not be asked about it at all: either way the
                    // create did not go through, so the deployment stops here exactly as it
                    // does on a refused create. The record keeps `materialised`, so the
                    // rollback still deletes what the create made.
                    Ok(Readiness::Failed(reason)) => {
                        outcome.failure = Some(super::resource_failure_summary(
                            super::failed_action::CREATE,
                            [resource.logical_id.as_str()],
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::CREATE_FAILED,
                            Some(reason),
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                    Err(refusal) => {
                        outcome.failure = Some(super::resource_failure_summary(
                            super::failed_action::CREATE,
                            [resource.logical_id.as_str()],
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::CREATE_FAILED,
                            Some(handler_failure_reason(owner, &refusal)),
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                }
            }
            Err(refusal) => {
                outcome.failure = Some(super::resource_failure_summary(
                    super::failed_action::CREATE,
                    [resource.logical_id.as_str()],
                ));
                outcome.transitions.push(ResourceTransition::failed(
                    &resource,
                    super::resource_status::CREATE_FAILED,
                    handler_failure_reason(owner, &refusal),
                ));
                outcome.resources.push(resource);
                break;
            }
        }
        outcome.resources.push(resource);
    }
    // Whatever the failure stopped: nothing was created for it, so it is recorded as the
    // declaration left it — except where the stack ALREADY had that resource, which still
    // exists in its owning service and keeps the record that says so, so the stack's
    // deletion still deletes it.
    for mut resource in declared {
        repoint_references(&mut resource.properties, &minted);
        if let Some(existing) = previous
            .iter()
            .find(|existing| existing.materialised && is_same_resource(existing, &resource))
        {
            carry_forward(existing, &mut resource);
        }
        outcome.resources.push(resource);
    }

    // The resources this deployment replaced or removed exist in their owning services
    // and the stack no longer records them, so the deployment deletes them — a resource
    // the stack stops recording must not be left behind. Judged against what the stack
    // now records (not against the walk above, which a failed create may have stopped
    // early), so one logical resource is never recorded twice.
    let replaced: Vec<StoredResource> = previous
        .iter()
        .filter(|existing| {
            existing.materialised
                && !outcome
                    .resources
                    .iter()
                    .any(|recorded| is_same_resource(existing, recorded))
        })
        .cloned()
        .collect();
    if replaced.is_empty() {
        return outcome;
    }
    if outcome.failure.is_some() {
        // A create failed, so this deployment is not the one that replaces anything: the
        // resources it would have deleted are still there, and the stack keeps their
        // records so its own deletion still deletes them.
        outcome.resources.extend(replaced);
        return outcome;
    }
    // The stack held every one of these records before this deployment stopped recording
    // it, so each one's removal is reported.
    let held: BTreeSet<String> = replaced
        .iter()
        .map(|resource| resource.logical_id.clone())
        .collect();
    // WHY the stack stopped recording a resource is what decides the fate of the physical
    // resource. One whose logical id the deployment no longer declares at all is REMOVED
    // from the stack, which is the event its own `DeletionPolicy` governs; one whose logical
    // id the stack still records, under a new identity, is the OLD half of a REPLACEMENT,
    // whose fate is the separate `UpdateReplacePolicy` attribute's — so the two are taken
    // down as two walks, each reading the attribute that governs it ([`Policies`]).
    let (replacements, removals): (Vec<StoredResource>, Vec<StoredResource>) = replaced
        .into_iter()
        .partition(|existing| {
            outcome
                .resources
                .iter()
                .any(|recorded| recorded.logical_id == existing.logical_id)
        });
    // The deletions already opened, carried across the two walks: a logical id cannot be in
    // both, and sharing the set is what keeps that true of the records as well.
    let mut opened = BTreeSet::new();
    // A deployment is not a `DeleteStack`, so no `DeletionMode` chose for it: a resource an
    // owning service refuses to delete is this deployment's failure.
    let mut deleted = delete_pass(
        ctx,
        region,
        &replacements,
        &held,
        &mut opened,
        OnRefusal::Stop,
        Policies::Ignored,
    )
    .await;
    if deleted.failure.is_some() {
        // The walk stopped on a refusal, so the resources the second one would have taken
        // down were never asked about: their records are kept exactly as the stack held
        // them, so a later deletion still deletes them.
        deleted.resources.extend(removals);
    } else {
        let mut removed = delete_pass(
            ctx,
            region,
            &removals,
            &held,
            &mut opened,
            OnRefusal::Stop,
            removal_policies,
        )
        .await;
        deleted.resources.append(&mut removed.resources);
        deleted.transitions.append(&mut removed.transitions);
        deleted.removing.append(&mut removed.removing);
        deleted.failure = removed.failure;
    }
    // A resource an owning service refused to delete still exists there, so the stack
    // keeps its record: the deployment failed, and a later deletion is what has to delete
    // it. The ones that were deleted carry `materialised = false` and are not kept.
    outcome.resources.extend(
        deleted
            .resources
            .into_iter()
            .filter(|resource| resource.materialised),
    );
    outcome.failure = deleted.failure;
    // The removals happened after the creates above, which is the order they are reported
    // in.
    outcome.transitions.extend(deleted.transitions);
    // ...and a resource an owning service has accepted the delete of without having removed
    // it yet holds this deployment open exactly as a resource it is still bringing up does:
    // the update is not complete while the resource it replaced is still there
    // ([`super::stabilising`]).
    outcome.stabilising.extend(deleted.removing);
    outcome
}

/// How deep [`repoint_references`] walks a property document.
///
/// The document's nesting is the REQUESTER's, and a recursive walk bounded by its input
/// exhausts the native stack — which aborts the process rather than failing one stack. Past
/// this depth the values are left exactly as they are: a reference to another resource is a
/// property of it, never a hundred levels inside one.
const MAX_REPOINTED_NESTING: usize = 64;

/// Re-point every reference inside one property document from the stand-in identifier a
/// resource was given before its create to the identifier its owning service MINTED.
///
/// The rule: a reference to a resource resolves to the identifier that resource really has,
/// and for a type whose identity its owning service assigns
/// ([`super::template::identity_is_service_assigned`]) that identifier only exists once the
/// create has answered. A declaration is resolved before any of them have, so a property
/// naming such a resource (`ResolverRuleId: !Ref Rule`) holds the stand-in the naming rules
/// computed; `minted` pairs each stand-in with what its service answered, and a property
/// whose value IS one of those stand-ins becomes the minted identifier.
///
/// The same holds for an ATTRIBUTE the owning service reports differently from the computed
/// one (`KmsMasterKeyId: !GetAtt [Key, Arn]`, where the ARN carries the key id KMS minted):
/// [`reconcile`] pairs those too, so a property reaches its own service as the value the
/// resource it names really publishes.
///
/// Matched WHOLE value — or, where the stand-in is an identifier CloudFormation computed
/// rather than a name the declaration asked for ([`Minted::composed`]), as one whole
/// slash-separated SEGMENT of a longer value, which is the one form a template composes an
/// identifier into (`integrations/<IntegrationId>`). Never as any shorter run of
/// characters: a stand-in is often the NAME the declaration asked for, so a longer value
/// that merely contains one (an association named `<rule name>-assoc`) is a value of its
/// own and is left exactly as written.
///
/// Called by [`reconcile`] for the identifiers minted in the walk it is driving. A resource
/// is never re-pointed with its OWN pair — it cannot reference itself, and for a type whose
/// stand-in is the name it declares that pair would rewrite that very property. The
/// identifiers a stack ALREADY has are not substituted at all:
/// [`super::StoredStack::deploy_template`] resolves the declarations against them instead.
fn repoint_references(properties: &mut Value, minted: &[Minted]) {
    repoint(properties, minted, 0);
}

/// One identifier an owning service MINTED in a deployment walk, against the stand-in the
/// naming rules had computed for the same resource ([`repoint_references`]).
#[derive(Debug, Clone)]
struct Minted {
    /// What a reference to the resource was resolved to before its create.
    stand_in: String,
    /// What that reference has to carry now.
    assigned: String,
    /// Whether the stand-in may also be matched INSIDE a longer value, as one whole
    /// slash-separated segment of it.
    ///
    /// True only where the stand-in is an identifier CloudFormation COMPUTED rather than
    /// the name the declaration asked for: such a value is not one a template composed out
    /// of its own strings, so a longer value carrying it carries it because a reference to
    /// this resource put it there. That is how an API Gateway v2 route names the
    /// integration it is wired to — `Target: !Join ['/', ['integrations', !Ref
    /// Integration]]`, which is the `integrations/<IntegrationId>` form that service
    /// accepts — so without this such a route would be created pointing at an integration
    /// id nothing minted.
    composed: bool,
}

/// [`repoint_references`] over one value, `depth` levels into the document.
fn repoint(properties: &mut Value, minted: &[Minted], depth: usize) {
    if minted.is_empty() || depth >= MAX_REPOINTED_NESTING {
        return;
    }
    match properties {
        Value::String(text) => {
            if let Some(minted) = minted
                .iter()
                .find(|minted| minted.stand_in.as_str() == text.as_str())
            {
                *text = minted.assigned.clone();
                return;
            }
            for minted in minted
                .iter()
                .filter(|minted| minted.composed && !minted.stand_in.contains('/'))
            {
                if text.split('/').any(|segment| segment == minted.stand_in) {
                    *text = text
                        .split('/')
                        .map(|segment| {
                            if segment == minted.stand_in {
                                minted.assigned.as_str()
                            } else {
                                segment
                            }
                        })
                        .collect::<Vec<&str>>()
                        .join("/");
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                repoint(item, minted, depth + 1);
            }
        }
        Value::Object(members) => {
            for value in members.values_mut() {
                repoint(value, minted, depth + 1);
            }
        }
        _ => {}
    }
}

/// Record the identity an owning service ASSIGNED to a resource it created: the id its
/// `Ref` resolves to from here on, and the `Id`/`<Type>Id` attributes read off that id,
/// which the naming rules had computed from the name instead.
///
/// Taking an identifier over is [`super::template::adopt_identity`], the one place that
/// step is written — the same one a binding's composite primary identifier goes through —
/// so the attributes and the id a resource reports cannot drift apart whichever of them
/// produced it.
///
/// `None` is every type whose physical id CloudFormation computes, which the create already
/// carried out to that service: nothing to adopt.
fn adopt_minted_identity(resource: &mut StoredResource, assigned: Option<String>) {
    let Some(assigned) = assigned else {
        return;
    };
    super::template::adopt_identity(resource, assigned);
}

/// Record what an owning service already created: the resource exists there with the
/// identity and the configuration it was created with, so the new record carries both
/// rather than the deployment's own idea of them.
///
/// For a resource whose configuration this deployment CHANGES, [`reconcile`] replaces the
/// properties again with the declared ones once the owning service has applied them — so
/// the record still states the configuration the resource really has, never one that was
/// only asked for.
fn carry_forward(existing: &StoredResource, resource: &mut StoredResource) {
    resource.materialised = true;
    for (name, value) in &existing.attributes {
        resource.attributes.insert(name.clone(), value.clone());
    }
    // The identifier the resource really has. For a type whose physical id CloudFormation
    // decides, the two are already equal ([`is_same_resource`] compared them); for one its
    // owning service ASSIGNED, this is the only place the minted identifier comes from, so
    // a re-deploy keeps reporting the `j-` job-flow id EMR gave the cluster rather than the
    // name CloudFormation computed for the record.
    resource.physical_resource_id = existing.physical_resource_id.clone();
    // The properties the resource really has are the ones it was created with. For a
    // resource this deployment keeps unchanged they are the same document; where they
    // differ, this is what keeps the stack from recording a configuration nothing applied.
    resource.properties = existing.properties.clone();
}

/// Whether two records are the same resource: the same logical id, of the same type, with
/// the same physical id — the three things that decide whether a deployment keeps a
/// resource or replaces it. The one place that rule is written, so the record a re-deploy
/// carries forward ([`super::StoredStack::deploy_template`]) and the resource
/// [`reconcile`] leaves alone cannot drift apart.
///
/// A resource whose physical id its owning service ASSIGNED is matched by the same rule:
/// the deploying record already carries the id that service minted, because
/// [`super::StoredStack::deploy_template`] carried it forward — nothing here could
/// recompute it.
pub fn is_same_resource(left: &StoredResource, right: &StoredResource) -> bool {
    left.logical_id == right.logical_id
        && left.resource_type == right.resource_type
        && left.physical_resource_id == right.physical_resource_id
}

/// What deleting a stack's resources left.
#[derive(Debug, Clone, Default)]
pub struct Deleted {
    /// The stack's resources as the deletion left them: one whose deletion the owning
    /// service carried out no longer carries `materialised`, so a retried delete does not
    /// ask for it again.
    pub resources: Vec<StoredResource>,
    /// The reason the stack's deletion cannot be reported as complete: an owning service
    /// refused to delete one of the resources — the stack-level summary of which ones
    /// ([`super::resource_failure_summary`]), with that service's own refusal carried on
    /// the resource's transition.
    pub failure: Option<String>,
    /// Every resource transition this deletion made, oldest first — the stack's
    /// per-resource event records, exactly as [`Materialised::transitions`] carries a
    /// deployment's.
    pub transitions: Vec<ResourceTransition>,
    /// The resources whose deletion the owning service ACCEPTED while still holding them:
    /// the call went through and the resource is not gone yet, so the operation that deleted
    /// it is held open until that service no longer holds it
    /// ([`super::stabilising`]) — a stack operation is never reported complete while a
    /// resource it deleted is still there. Recorded by the operation that drove the teardown
    /// ([`super::StoredStack::record_stabilisations`]), exactly as
    /// [`Materialised::stabilising`] is.
    pub removing: Vec<Stabilising>,
}

/// What a teardown does with a resource the service that owns it REFUSES to delete.
///
/// `DeleteStackInput$DeletionMode` is what chooses: "STANDARD — use the standard behavior",
/// under which the refusal is the stack's failure, and "FORCE_DELETE_STACK — delete the stack
/// if it's stuck in a DELETE_FAILED state due to resource deletion failure", under which the
/// refused resource is left where it is and the stack's deletion goes on without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnRefusal {
    /// The refusal STOPS the teardown and is the stack's own failure: nothing after the
    /// refused resource is asked about ([`Deleted::failure`] names it).
    Stop,
    /// The refused resource is ABANDONED — nothing more is asked of its owning service, so
    /// it stays exactly as it is — and the walk carries on with the rest, so the teardown
    /// reports no failure of its own.
    Abandon,
}

/// Delete the resources a stack created, newest first — the reverse of the order they were
/// created in, so a resource is deleted before whatever it depends on.
///
/// [`Deleted::failure`] names the first resource whose owning service refused to delete
/// it; the stack's deletion cannot be reported as complete then, and the resources it had
/// already deleted are reported as deleted so a retry does not ask for them again. A
/// resource the service no longer has is a success: the request asks for a state the
/// service is already in. Under [`OnRefusal::Abandon`] a refusal is not a failure at all:
/// that resource is left alone and the walk goes on, so the teardown answers no failure and
/// the stack's deletion completes around what could not be deleted.
///
/// `held` is the logical ids the stack HOLDS a record of
/// ([`super::StoredStack::held_resource_ids`]) — the resources this teardown is responsible
/// for. Every one of them goes through its deletion and is recorded as the pair of
/// transitions it went through ([`Deleted::transitions`]), which is what names the resource
/// a deletion failed on and what makes the teardown's order observable: a resource of a type
/// this emulator provisions NOWHERE has no owning service to ask, so nothing is asked of
/// anybody, but the stack owned the record and its removal is reported exactly as any other
/// resource's is. A record the stack does NOT hold — one whose create an owning service
/// refused, one a failed deployment never reached — has nothing to remove and is recorded
/// nowhere.
///
/// What becomes of each PHYSICAL resource is its own `DeletionPolicy`, where this walk is one
/// that reads the attribute ([`Policies`]): `Retain` is asked of nobody and recorded
/// `DELETE_SKIPPED`, `Snapshot` is the owning service's delete taking a final snapshot
/// ([`final_snapshot_identifier`]), and `Delete` — the default, and what a walk that does not
/// read the attribute does — is the delete this walk has always made.
///
/// A resource whose owning service ACCEPTED the delete without having removed the resource
/// yet is reported in [`Deleted::removing`]: the operation that drove this teardown holds its
/// completion on those, so no stack operation is reported complete while a resource it
/// deleted is still there.
pub async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    held: &BTreeSet<String>,
    on_refusal: OnRefusal,
    policies: Policies,
) -> Deleted {
    delete_pass(
        ctx,
        region,
        resources,
        held,
        &mut BTreeSet::new(),
        on_refusal,
        policies,
    )
    .await
}

/// Whether a teardown reads each resource's own `DeletionPolicy`
/// ([`super::deletion_policy`]) for what becomes of the physical resource.
///
/// The attribute says what to do with a resource "when its stack is deleted", and
/// CloudFormation reads it wherever the stack STOPS MANAGING the resource: the stack's own
/// deletion, and a deployment that no longer declares it. The OLD physical resource of a
/// REPLACEMENT is a different event governed by a different attribute —
/// `UpdateReplacePolicy`, whose action this service reports
/// ([`super::change_plan::policy_action`]) and whose documented default is to delete that
/// resource — so a walk taking one of those down reads no deletion policy for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policies {
    /// Each resource's own `DeletionPolicy` decides what becomes of it.
    Honoured,
    /// The physical resource is deleted whatever `DeletionPolicy` its declaration states.
    Ignored,
}

/// What this walk does with one resource: its own `DeletionPolicy` where the walk reads the
/// attribute, and the documented default ([`super::deletion_policy::DELETE`]) otherwise —
/// which is also what a declaration naming no policy, and one naming a value that is none
/// of the three, gets.
fn policy_of(resource: &StoredResource, policies: Policies) -> &str {
    match policies {
        Policies::Ignored => super::deletion_policy::DELETE,
        Policies::Honoured => resource
            .deletion_policy
            .as_deref()
            .unwrap_or(super::deletion_policy::DELETE),
    }
}

/// How many digest characters the identifier of a `Snapshot` policy's snapshot ends in.
///
/// Enough that the name cannot be one a caller happened to create a snapshot of its own
/// under, and short enough to leave the resource's own identifier legible inside the length
/// every owning service's final-snapshot member bounds (Amazon RDS's
/// `FinalDBSnapshotIdentifier` admits 255 characters).
const FINAL_SNAPSHOT_DIGEST_CHARS: usize = 8;

/// What the digest of a final snapshot's identifier is derived under, which keeps it apart
/// from every other identifier this crate derives from the same resource.
const FINAL_SNAPSHOT_SALT: &str = "final-snapshot";

/// The identifier CloudFormation asks the owning service to take the snapshot of a
/// `Snapshot` policy's resource under.
///
/// Derived from the resource's own physical id, so the snapshot a deletion leaves behind
/// names the resource whose contents it holds, and a deletion ASKED AGAIN (a stack whose
/// earlier deletion failed on another resource) asks for that same snapshot rather than a
/// second one. The form is the one those services' final-snapshot members admit — the
/// resource's identifier, which is one of their own identifiers already, and hexadecimal
/// digits after a single hyphen.
fn final_snapshot_identifier(resource: &StoredResource) -> String {
    format!(
        "{}-{}",
        resource.physical_resource_id.trim_end_matches('-'),
        super::template::content_digest(
            FINAL_SNAPSHOT_SALT,
            &resource.physical_resource_id,
            FINAL_SNAPSHOT_DIGEST_CHARS,
        )
    )
}

/// One pass of [`delete`] over the set, recording what it transitioned.
///
/// `opened` is the logical ids whose deletion has already been OPENED, which this pass adds
/// to: each resource's `DELETE_IN_PROGRESS` is recorded the first time a pass reaches it,
/// and a RE-ASK of one an owning service has not carried out yet ([`unwind`]) is the same
/// deletion still in progress rather than a second one. A resource an earlier pass never
/// reached — one after the refusal that stopped it — is opened by the pass that does reach
/// it.
///
/// `policies` is whether this walk reads each resource's own `DeletionPolicy` ([`Policies`]).
async fn delete_pass(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    held: &BTreeSet<String>,
    opened: &mut BTreeSet<String>,
    on_refusal: OnRefusal,
    policies: Policies,
) -> Deleted {
    let mut outcome = Deleted {
        resources: resources.to_vec(),
        failure: None,
        transitions: Vec::new(),
        removing: Vec::new(),
    };
    // Newest first, over the indices of the set being reported back, so each resource's
    // record is updated in place as its deletion succeeds.
    for index in (0..outcome.resources.len()).rev() {
        let resource = &outcome.resources[index];
        // Which service is asked to delete the resource. `None` is a type this emulator
        // provisions nowhere: there is no API to call for it.
        let owner = provisioner_for(&resource.resource_type);
        if !resource.materialised {
            // Nothing exists in any owning service for this resource, so nothing is asked
            // of anybody. The stack still HELD the record, though, and its teardown is
            // reported exactly as every other resource's is — whether an owning service
            // had one to delete is not something a stack's event stream says. Two records
            // have no teardown at all: one whose create an owning service refused or never
            // reached (the stack does not hold it), and one an earlier pass of this same
            // walk already took down.
            if owner.is_some()
                || !held.contains(&resource.logical_id)
                || opened.contains(&resource.logical_id)
            {
                continue;
            }
        }
        let policy = policy_of(resource, policies);
        if policy == super::deletion_policy::RETAIN {
            // "CloudFormation keeps the resource without deleting the resource or its
            // contents when its stack is deleted": nothing at all is asked of the owning
            // service, so the resource stays exactly as it is and goes on serving its own
            // clients, and the one thing the stack records about it is that its deletion was
            // SKIPPED — once, however many passes of one walk reach the record. The record
            // stops carrying `materialised`: the stack has RELEASED the resource, so no
            // later deletion of this stack's own owes a call about it.
            let first = opened.insert(resource.logical_id.clone());
            outcome.resources[index].materialised = false;
            if first {
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_SKIPPED,
                    None,
                ));
            }
            continue;
        }
        if opened.insert(resource.logical_id.clone()) {
            outcome.transitions.push(ResourceTransition::starting(
                resource,
                super::resource_status::DELETE_IN_PROGRESS,
            ));
        }
        // "For resources that support snapshots, CloudFormation creates a snapshot for the
        // resource before deleting it": the snapshot is the OWNING service's own, and the
        // call that takes it is that service's delete carrying the final-snapshot
        // identifier CloudFormation names it under
        // ([`super::provisioner::Provisioner::delete`]). A type whose owning service takes
        // no snapshot on the way out is deleted as it is for the default policy — there is
        // no snapshot of it to take.
        let final_snapshot = (policy == super::deletion_policy::SNAPSHOT)
            .then(|| final_snapshot_identifier(resource));
        let deleted = match owner {
            Some(provisioner) => {
                super::service_role::on_resource(
                    super::service_role::target_of(resource),
                    provisioner.delete(ctx, region, resource, final_snapshot.as_deref()),
                )
                .await
            }
            None => Ok(()),
        };
        match deleted {
            Ok(()) => {
                // The delete was ACCEPTED; whether the resource is gone is the owning
                // service's to say. A type whose delete that service documents as
                // asynchronous leaves the resource in a state it is still being removed from
                // and answers which one ([`super::provisioner::Provisioner::removal`]), and
                // the operation is then held open until that service no longer holds it
                // ([`super::stabilising`]) — CloudFormation does not report an operation
                // complete while a resource it deleted is still there. An unanswered read is
                // no evidence that the resource is still there, so it opens no wait: the
                // delete itself went through.
                let removal = match owner {
                    Some(provisioner) => {
                        provisioner
                            .removal(
                                ctx,
                                region,
                                &outcome.resources[index].physical_resource_id,
                            )
                            .await
                    }
                    None => Ok(Removal::Gone),
                };
                if let Ok(Removal::Pending(state)) = removal {
                    let resource = &outcome.resources[index];
                    outcome.removing.push(Stabilising {
                        awaited: super::stabilising::Awaited::Gone,
                        logical_id: resource.logical_id.clone(),
                        resource_type: resource.resource_type.clone(),
                        physical_resource_id: resource.physical_resource_id.clone(),
                        reported_state: state,
                    });
                }
                outcome.resources[index].materialised = false;
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_COMPLETE,
                    None,
                ));
            }
            Err(refusal) => {
                let reason = handler_failure_reason(
                    provisioner_for(&outcome.resources[index].resource_type),
                    &refusal,
                );
                if on_refusal == OnRefusal::Abandon {
                    // ABANDONED: nothing more is asked of the owning service, so the
                    // resource stays exactly as it is and goes on serving its own clients.
                    // The record reports that its deletion was SKIPPED, carrying why, and
                    // the walk carries on with the resources declared before it — the
                    // teardown has no failure of its own, so the stack's deletion completes.
                    let resource = &outcome.resources[index];
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        super::resource_status::DELETE_SKIPPED,
                        Some(reason),
                    ));
                    continue;
                }
                outcome.failure = Some(super::resource_failure_summary(
                    super::failed_action::DELETE,
                    [outcome.resources[index].logical_id.as_str()],
                ));
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_FAILED,
                    Some(reason),
                ));
                return outcome;
            }
        }
    }
    outcome
}

/// How many times the UNWIND of a failed operation asks the owning services to delete
/// what that operation had already made.
///
/// A service that answers a create while the resource is still coming up refuses to
/// delete it until it is up (Amazon Data Firehose answers `DeleteDeliveryStream` for a
/// stream it is still creating with "cannot be deleted in CREATING state"), and
/// CloudFormation's rollback is what waits for that: `ROLLBACK_IN_PROGRESS` is a status a
/// stack sits in until the resources are gone. The wait is BOUNDED — these attempts, one
/// [`UNWIND_RETRY_DELAY`] apart — so a service that refuses the delete for any other
/// reason is reported rather than waited on forever.
///
/// The count is what makes the bound cover a resource that is merely coming up: together
/// with [`UNWIND_RETRY_DELAY`] it has to outlast the longest span a called service gives
/// one of those (a few seconds — Amazon Data Firehose gives a stream three), or a rollback
/// would report a resource it could have removed as one it could not.
const UNWIND_ATTEMPTS: usize = 20;

/// How long the unwind waits before asking again. Chosen: shorter than the status spans
/// the services this emulator calls give a resource that is coming up, so the first
/// attempt after one of those elapses carries the delete through.
const UNWIND_RETRY_DELAY: ::std::time::Duration = ::std::time::Duration::from_millis(200);

/// Delete what a FAILED create had already made, for the stack operation that is rolling
/// back — [`delete`], re-asked while an owning service has not let a resource go yet.
///
/// The resources still carrying `materialised` are the ones left to delete, which is what
/// each re-ask narrows to, so nothing is asked for twice. [`Deleted::failure`] is what the
/// LAST attempt answered: a refusal that outlives the bound above is the rollback's own
/// failure, reported on the stack rather than passed off as a rollback that removed what
/// it did not.
///
/// `held` is the set [`delete`] documents: the records the stack holds, which is what this
/// takes down and reports. A rollback is the stack giving up the resources the failed
/// operation had made, so what becomes of each PHYSICAL resource is its own
/// `DeletionPolicy` — unless the operation was asked for `RetainExceptOnCreate`, the one
/// member documented to override that policy for a resource the operation itself created
/// ([`super::StoredStack::rolled_back_resource_policies`], which is what `policies` carries
/// here).
pub async fn unwind(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    held: &BTreeSet<String>,
    policies: Policies,
) -> Deleted {
    // The deletions already opened, carried across the attempts: a re-ask of one still in
    // flight does not open it again, and one a stopped pass never reached is opened by the
    // pass that does reach it.
    let mut opened = BTreeSet::new();
    // An unwind is not a `DeleteStack`, so no `DeletionMode` chose for it: a refusal that
    // outlives the retries is the rollback's own failure.
    let mut outcome = delete_pass(
        ctx,
        region,
        resources,
        held,
        &mut opened,
        OnRefusal::Stop,
        policies,
    )
    .await;
    for _ in 1..UNWIND_ATTEMPTS {
        if outcome.failure.is_none() {
            break;
        }
        ::tokio::time::sleep(UNWIND_RETRY_DELAY).await;
        let remaining = ::std::mem::take(&mut outcome.resources);
        // What the earlier attempts transitioned happened BEFORE what this one does. The
        // refusal this attempt is about to retry was not that resource's OUTCOME — its
        // deletion is still in progress, which is the whole point of the re-ask — so the
        // failed terminal the last pass recorded is dropped, and only the terminal of the
        // attempt nothing follows stands.
        let mut transitioned: Vec<ResourceTransition> =
            ::std::mem::take(&mut outcome.transitions)
                .into_iter()
                .filter(|transition| {
                    transition.status != super::resource_status::DELETE_FAILED
                })
                .collect();
        // The waits the earlier attempts opened are the deletions they carried through, and
        // they stand: this attempt is about the resources those passes could NOT delete.
        let mut removing = ::std::mem::take(&mut outcome.removing);
        outcome = delete_pass(
            ctx,
            region,
            &remaining,
            held,
            &mut opened,
            OnRefusal::Stop,
            policies,
        )
        .await;
        transitioned.append(&mut outcome.transitions);
        outcome.transitions = transitioned;
        removing.append(&mut outcome.removing);
        outcome.removing = removing;
    }
    outcome
}

/// A refusal another service answered with: its error CODE and its message, kept apart so
/// a caller branches on the code (as a client does) and never on text a request can
/// influence.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl ::std::fmt::Display for Refusal {
    /// One line, as the stack's status reason carries it.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        if self.message.is_empty() {
            write!(formatter, "{}", self.code)
        } else {
            write!(formatter, "{}: {}", self.code, self.message)
        }
    }
}

/// What stands in for an error code an answer did not carry: not a code any service
/// answers, so it can never collide with one. An answer that identifies nothing is still
/// a failed create — the status it came back with is what the stack's reason reports.
const UNNAMED_CODE_PREFIX: &str = "HTTP ";

/// The SDK service NAME the diagnostic below names a called service by, per SIGNING name.
///
/// A resource provider calls the service that owns the type through an AWS SDK, and what it
/// reports when that call is refused is the SDK's exception message: the service's own
/// message followed by the SDK's own diagnostic of the call ([`sdk_diagnostic`]). The
/// service in that diagnostic is the SDK's name for it, which is NOT the signing name the
/// request was signed with — Amazon OpenSearch Service signs `es` and its SDK names it
/// `OpenSearch`, that model's own `sdkId`.
///
/// One entry per service whose reported wording this emulator pins; a service with no entry
/// has its message reported exactly as it worded it, with no diagnostic invented for it.
const SDK_SERVICE_NAMES: &[(&str, &str)] = &[("es", "OpenSearch"), ("iam", "Iam")];

/// The diagnostic an AWS SDK appends to the message of the exception it raises for a refused
/// call: which service refused it, the HTTP status it answered with, the id of the call and
/// how many attempts the SDK made at it.
///
/// `None` for a service [`SDK_SERVICE_NAMES`] does not name, which leaves the message as
/// that service worded it.
///
/// The request id is this call's own, minted per call as every identifier this service hands
/// out is: the in-process answer carries none of its own, and the id identifies the CALL
/// rather than anything about the resource.
fn sdk_diagnostic(signing_name: &str, status: u16, code: &str) -> Option<String> {
    let service = SDK_SERVICE_NAMES
        .iter()
        .find(|(name, _)| *name == signing_name)
        .map(|(_, service)| *service)?;
    Some(format!(
        "(Service: {service}, Status Code: {status}, Request ID: {}) \
         (SDK Attempt Count: {})",
        super::new_request_id(),
        sdk_attempt_count(code),
    ))
}

/// How many attempts an AWS SDK makes at a call whose refusal it classifies as worth
/// retrying: its default maximum, reached because every attempt was refused the same way.
const SDK_RETRIED_ATTEMPTS: u32 = 3;

/// How many it makes at a refusal it does not retry: the one call it was asked for.
const SDK_SINGLE_ATTEMPT: u32 = 1;

/// The classes of refusal an SDK retries, as [`HANDLER_ERROR_CLASSES`] names them: a
/// throttled call, an exhausted service limit and a service that failed are conditions that
/// can clear on their own, where a refusal of the REQUEST cannot and is raised on the first
/// attempt.
const SDK_RETRIED_CLASSES: [&str; 3] =
    ["Throttling", "ServiceLimitExceeded", "GeneralServiceException"];

/// The attempt count the diagnostic reports for a refusal carrying `code`, from the class
/// that code falls in ([`handler_error_code`]) — never from the message, which a request can
/// influence.
fn sdk_attempt_count(code: &str) -> u32 {
    if SDK_RETRIED_CLASSES.contains(&handler_error_code(code)) {
        SDK_RETRIED_ATTEMPTS
    } else {
        SDK_SINGLE_ATTEMPT
    }
}

/// One refused answer's message as the SDK of the called service raises it: the message that
/// service worded, then that SDK's diagnostic of the call where this crate words one
/// ([`sdk_diagnostic`]).
pub(super) fn sdk_message(signing_name: &str, status: u16, code: &str, message: &str) -> String {
    match sdk_diagnostic(signing_name, status, code) {
        Some(diagnostic) if message.is_empty() => diagnostic,
        Some(diagnostic) => format!("{message} {diagnostic}"),
        None => message.to_string(),
    }
}

/// Each `HandlerErrorCode` a provider's refusal is reported under, with the fragments an
/// owning service's own error CODE carries when the refusal falls in that class.
///
/// The classes are the model's own `HandlerErrorCode` values
/// ([`crate::ops::types::HANDLER_ERROR_CODE_VALUES`], which the tests below hold every
/// class here to), and that is what a client reads to decide whether a failed resource is
/// worth retrying — a throttled create is, a malformed declaration never is. The class is
/// decided by the error CODE the owning service answered, the value a client of that
/// service branches on, and never by message text a request can influence.
///
/// Walked in order, so a code carrying more than one fragment takes the first: the narrower
/// class comes before the wider one it reads like (`RequestLimitExceeded` is throttling, not
/// an exhausted quota).
const HANDLER_ERROR_CLASSES: &[(&str, &[&str])] = &[
    (
        "AccessDenied",
        &["AccessDenied", "Forbidden", "Unauthorized", "AuthorizationError"],
    ),
    (
        "InvalidCredentials",
        &["InvalidClientTokenId", "SignatureDoesNotMatch", "ExpiredToken"],
    ),
    (
        "Throttling",
        &["Throttl", "TooManyRequests", "RequestLimitExceeded", "SlowDown"],
    ),
    (
        "ServiceLimitExceeded",
        &["LimitExceeded", "QuotaExceeded", "TooManyEntries"],
    ),
    ("AlreadyExists", &["AlreadyExists", "AlreadyOwnedBy", "Duplicate"]),
    ("NotFound", &["NotFound", "NoSuch", "DoesNotExist"]),
    (
        "ResourceConflict",
        &["Conflict", "InUse", "InvalidState", "Concurrent"],
    ),
    (
        "InternalFailure",
        &["InternalFailure", "SerializationException"],
    ),
    (
        "GeneralServiceException",
        &["InternalError", "InternalServerError", "ServiceUnavailable", "ServiceFailure"],
    ),
];

/// The class a refusal falls in when nothing above matches: the owning service refused the
/// request the declaration asked for, which is what every validation refusal is.
///
/// Read by [`super::provisioner::Provisioner::handler_message`] too: it is the class whose
/// own wording some providers carry inside the message.
pub(super) const HANDLER_ERROR_INVALID_REQUEST: &str = "InvalidRequest";

/// The lowest HTTP status that is the owning SERVICE failing rather than the request being
/// wrong, for an answer that carried no error code at all ([`UNNAMED_CODE_PREFIX`]).
const SERVER_ERROR_STATUS: u16 = 500;

/// The class an answer that named no error at all falls in when its status says the owning
/// service failed rather than that the request was wrong.
const HANDLER_ERROR_SERVICE_FAILED: &str = "GeneralServiceException";

/// The `HandlerErrorCode` an owning service's refusal is reported under — see
/// [`HANDLER_ERROR_CLASSES`].
fn handler_error_code(code: &str) -> &'static str {
    if let Some(status) = code.strip_prefix(UNNAMED_CODE_PREFIX) {
        // An answer that named no error at all: only the status it came back with says
        // whether the service failed or the request was refused.
        return match status.parse::<u16>() {
            Ok(status) if status >= SERVER_ERROR_STATUS => HANDLER_ERROR_SERVICE_FAILED,
            _ => HANDLER_ERROR_INVALID_REQUEST,
        };
    }
    HANDLER_ERROR_CLASSES
        .iter()
        .find(|(_, fragments)| fragments.iter().any(|fragment| code.contains(fragment)))
        .map_or(HANDLER_ERROR_INVALID_REQUEST, |(class, _)| *class)
}

/// The `ResourceStatusReason` the event for a resource its provider refused to act on
/// carries: "Success/failure message associated with the resource".
///
/// CloudFormation does not report its own words for such a failure — it reports the
/// provider's, quoted, beside the token of the resource request it answered and the
/// `HandlerErrorCode` class that refusal falls in ([`handler_error_code`]), so a client
/// reads WHY the resource failed and whether the failure is of a kind worth retrying.
///
/// The token is this resource request's own, minted per request as every identifier this
/// service hands out is.
///
/// `owner` is the provisioner of the type whose resource failed, because the message inside
/// the quotes is the type's HANDLER's
/// ([`super::provisioner::Provisioner::handler_message`]): for a type whose provider words a
/// refusal through the class it put it in, that wording is part of what CloudFormation
/// quotes. A type this emulator has no provisioner for reports the refusal as it came.
fn handler_failure_reason(owner: Option<Provisioner>, refusal: &Refusal) -> String {
    // A service that worded no message at all still named the error, and the code is then
    // all there is to report.
    let message = if refusal.message.is_empty() {
        refusal.code.as_str()
    } else {
        refusal.message.as_str()
    };
    let class = handler_error_code(&refusal.code);
    let message = match owner {
        Some(provisioner) => provisioner.handler_message(class, message),
        None => message.to_string(),
    };
    format!(
        "Resource handler returned message: \"{message}\" (RequestToken: {}, \
         HandlerErrorCode: {class})",
        super::new_request_id(),
    )
}


/// The refusal the stack's SERVICE ROLE answers a call of `operation` on `signing_name`
/// with, or `None` where nothing refuses it — so a call the role's policies deny is never
/// made ([`super::service_role`]).
///
/// The refusal is CLOUDFORMATION's, not the owning service's: the call was not made, so
/// there is no answer of that service's to report. Its code is the one an unauthorized call
/// is classified under ([`handler_error_code`]'s `AccessDenied` class, which is what tells a
/// client whether the failed resource is worth retrying) rather than the particular spelling
/// the called service would have used, and its message is the sentence AWS words such a
/// refusal with, naming the principal, the action and the resource. `operation` is `None` for
/// a wire this cannot read an operation name off, which is judged by nothing.
fn denied_by_the_service_role(signing_name: &str, operation: Option<&str>) -> Option<Refusal> {
    let message = super::service_role::refusal(signing_name, operation?)?;
    Some(Refusal {
        code: super::service_role::ACCESS_DENIED.to_string(),
        message,
    })
}

/// Call one awsJson operation of another emulated service in-process and read its
/// answer document. `Err(refusal)` is the service's own refusal: its code, and the message
/// it worded.
///
/// `content_type` is the awsJson version that service's model declares
/// ([`AWS_JSON_1_0`] / [`AWS_JSON_1_1`]), which is what its answers are rendered in.
///
/// A 501 — the owning operation is still one of this emulator's stubs — is a refusal like
/// any other, so a resource of a type this emulator DOES provision is never passed off as
/// created when nothing created it; the type's own entry in
/// [`provisioner_for`](super::provisioner::provisioner_for) is what decides whether a
/// create is attempted at all.
pub(super) async fn call_json_rpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    content_type: &str,
    target: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    // The call is made under the stack's service role, so what that role is DENIED is not
    // made at all: the operation an awsJson request invokes is the part of its `X-Amz-Target`
    // after the service shape's name ([`super::service_role::refusal`]).
    if let Some(denied) = denied_by_the_service_role(signing_name, target.rsplit('.').next())
    {
        return Err(denied);
    }
    let body = serde_json::to_vec(request).map_err(|error| Refusal {
        code: "SerializationException".to_string(),
        message: error.to_string(),
    })?;
    let headers = vec![
        ("content-type", content_type.to_string()),
        ("x-amz-target", target.to_string()),
    ];
    let (status, payload) = dispatch(ctx, signing_name, region, "POST", "/", headers, body)
        .await
        .map_err(|message| Refusal {
            code: "InternalFailure".to_string(),
            message,
        })?;
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of(signing_name, status, &document))
}

/// The media type an awsQuery request carries: its parameters are a form body, not a
/// document ("the request is sent as `application/x-www-form-urlencoded`"). Read only by
/// [`call_query`], which is the one place this module writes such a body.
const FORM_URLENCODED: &str = "application/x-www-form-urlencoded";

/// Call one awsQuery operation of another emulated service in-process and read the XML
/// document it answers with. `Err(refusal)` is the service's own refusal: the
/// `awsQueryError` code it carries, and the message it worded.
///
/// `version` is the API version that service's model declares (`2010-03-31` for Amazon
/// SNS), which an awsQuery request names beside the action. `parameters` are the request's
/// members in the awsQuery spelling — `Attributes.entry.<n>.key`, `Tags.member.<n>.Key` —
/// percent-encoded here, so a value carrying `&` or `=` cannot be read as a second
/// parameter.
///
/// A 501 is a refusal like any other, exactly as it is for [`call_json_rpc`]: the
/// difference between the two is only the wire the called service publishes.
pub(super) async fn call_query(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    version: &str,
    action: &str,
    parameters: &[(String, String)],
) -> Result<String, Refusal> {
    // Under the stack's service role, exactly as [`call_json_rpc`] is: an awsQuery request
    // names the operation it invokes in its `Action` parameter.
    if let Some(denied) = denied_by_the_service_role(signing_name, Some(action)) {
        return Err(denied);
    }
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&percent_encoded(name));
        body.push('=');
        body.push_str(&percent_encoded(value));
    }
    let headers = vec![("content-type", FORM_URLENCODED.to_string())];
    let (status, payload) =
        dispatch(ctx, signing_name, region, "POST", "/", headers, body.into_bytes())
        .await
        .map_err(|message| Refusal {
            code: "InternalFailure".to_string(),
            message,
        })?;
    let document = String::from_utf8_lossy(&payload).into_owned();
    if (200..300).contains(&status) {
        return Ok(document);
    }
    // An awsQuery error answer is `<ErrorResponse><Error><Code/><Message/></Error>…`, and
    // the code is the one the model's `awsQueryError` trait fixes for that error shape
    // (`NotFoundException` -> `NotFound`), which is what a client branches on.
    let code = xml_element(&document, "Code")
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    Err(Refusal {
        message: sdk_message(
            signing_name,
            status,
            &code,
            &xml_element(&document, "Message").unwrap_or_default(),
        ),
        code,
    })
}

/// One value as a single form component, URI path segment or query value: everything
/// outside RFC 3986's unreserved set escaped ([`FORM_VALUE`]), which is what an AWS SDK
/// sends for an awsQuery parameter or a non-greedy `@httpLabel` and what the SigV4
/// canonical request assumes.
///
/// The ONE spelling of that encoding here, so a parameter of an awsQuery call
/// ([`call_query`]) and a name written into a REST URI are escaped identically.
///
/// A value that comes from the TEMPLATE — the name a declaration asks for — therefore
/// reaches the called operation as one value rather than as extra URI structure, so a name
/// outside the called model's own pattern is refused by that operation's validation instead
/// of addressing a different one.
pub(super) fn percent_encoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, FORM_VALUE).to_string()
}

/// Call one REST-JSON operation of another emulated service in-process and read its answer
/// document. `Err(refusal)` is that service's own refusal, exactly as for
/// [`call_json_rpc`].
///
/// A `restJson1` service binds each operation to an HTTP method and a URI rather than to an
/// `X-Amz-Target` header (AWS FIS: `POST /experimentTemplates`,
/// `DELETE /experimentTemplates/{id}`), so the caller passes the binding the called
/// operation's model declares and this writes the request on it. `body` is `None` for an
/// operation whose input is entirely in the URI.
pub(super) async fn call_rest_json(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).map_err(|error| Refusal {
            code: "SerializationException".to_string(),
            message: error.to_string(),
        })?,
        None => Vec::new(),
    };
    let headers = vec![("content-type", REST_JSON.to_string())];
    let (status, answer) = dispatch(ctx, signing_name, region, method, uri, headers, payload)
        .await
        .map_err(|message| Refusal {
            code: "InternalFailure".to_string(),
            message,
        })?;
    let document: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of(signing_name, status, &document))
}

/// Call one REST-XML operation of another emulated service in-process and read the XML
/// document it answers with. `Err(refusal)` is that service's own refusal: the error CODE
/// it carries, and the message it worded.
///
/// A `restXml` service (Amazon Route 53) binds each operation to a method and a URI like a
/// REST-JSON one, and takes an XML document rather than a JSON one — so the caller passes
/// the binding the called operation's model declares and the body it writes, and what comes
/// back is the answer's text, read with [`xml_element`] and [`xml_scalar_children`].
/// `body` is `None` for an operation whose input is entirely in the URI.
///
/// A 501 is a refusal like any other, exactly as it is for [`call_json_rpc`]: the
/// difference between the two is only the wire the called service publishes.
pub(super) async fn call_rest_xml(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&str>,
) -> Result<String, Refusal> {
    let payload = body.map(str::as_bytes).unwrap_or_default().to_vec();
    let headers = vec![("content-type", REST_XML.to_string())];
    let (status, answer) = dispatch(ctx, signing_name, region, method, uri, headers, payload)
        .await
        .map_err(|message| Refusal {
            code: "InternalFailure".to_string(),
            message,
        })?;
    let document = String::from_utf8_lossy(&answer).into_owned();
    if (200..300).contains(&status) {
        return Ok(document);
    }
    // A restXml error answer is `<ErrorResponse><Error><Type/><Code/><Message/></Error>…`,
    // and the code is the one the model's error shape carries (`NoSuchHostedZone`), which is
    // what a client branches on.
    let code = xml_element(&document, "Code")
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    Err(Refusal {
        message: sdk_message(
            signing_name,
            status,
            &code,
            &xml_element(&document, "Message").unwrap_or_default(),
        ),
        code,
    })
}

/// One element carrying TEXT, as a restXml request body spells a scalar member:
/// `<Name>value</Name>`, with the five predefined entities escaped by the same maintained
/// XML implementation that reads the answers back ([`xml_element`]) — so a value carrying
/// `&` or `<` reaches the called operation as the text it is rather than as markup.
///
/// The element NAME is the called model's own member name, never caller text.
pub(super) fn xml_leaf(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", ::quick_xml::escape::escape(value))
}

/// What an awsQuery parameter name or value is percent-encoded against: everything
/// outside the UNRESERVED set of RFC 3986 (`A-Z a-z 0-9 - _ . ~`) is escaped, which is
/// what an AWS SDK sends and what the SigV4 canonical request assumes.
const FORM_VALUE: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The text of the first `<tag>` element of an XML document, with its entities decoded, or
/// `None` where the document carries no such element.
///
/// A LEAF element's text: the character data between that element's own tags, concatenated,
/// which is what the members read through it are (`<TopicArn>`, `<Code>`, `<Message>`).
/// It keeps no element depth, so reading a member that HOLDS other elements — or one whose
/// name repeats inside itself — needs a depth-aware reader rather than this helper.
///
/// Read with the same XML reader the emulator's own restXml services use, so an escaped
/// value (`&amp;`, `&lt;`) comes back as the text the answering service wrote rather than
/// as its escaping.
pub(super) fn xml_element(document: &str, tag: &str) -> Option<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if start.local_name().as_ref() == tag.as_bytes() => {
                inside = true;
            }
            Ok(Event::Text(found)) if inside => {
                text.push_str(&found.xml10_content().unwrap_or_default());
            }
            Ok(Event::End(end)) if inside && end.local_name().as_ref() == tag.as_bytes() => {
                return Some(text);
            }
            // An empty element (`<TopicArn/>`) is the empty value of that element.
            Ok(Event::Empty(empty)) if empty.local_name().as_ref() == tag.as_bytes() => {
                return Some(String::new());
            }
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

/// The text of EVERY `<tag>` element of an XML document, in document order, with their
/// entities decoded.
///
/// [`xml_element`] reads the first one; a LIST member of an awsQuery answer repeats its
/// element once per entry (`<Tags><member><Key/><Value/></member>…`), so a reader of one
/// such member reads them all. Two members of the same list are read as PARALLEL lists and
/// zipped, which is sound exactly where the answering service reports both on every entry.
pub(super) fn xml_elements(document: &str, tag: &str) -> Vec<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut found = Vec::new();
    let mut reading: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if start.local_name().as_ref() == tag.as_bytes() => {
                reading = Some(String::new());
            }
            Ok(Event::Text(text)) => {
                if let Some(held) = reading.as_mut() {
                    held.push_str(&text.xml10_content().unwrap_or_default());
                }
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == tag.as_bytes() => {
                if let Some(held) = reading.take() {
                    found.push(held);
                }
            }
            // An empty element (`<Value/>`) is the empty value of that element.
            Ok(Event::Empty(empty)) if empty.local_name().as_ref() == tag.as_bytes() => {
                found.push(String::new());
            }
            Ok(Event::Eof) | Err(_) => return found,
            Ok(_) => {}
        }
    }
}

/// The map an awsQuery answer carries under `parent`, as the JSON object the rest of this
/// module compares: one entry per `<entry><key>…</key><value>…</value></entry>`, which is
/// how the query protocol serializes a map member (`Attributes` on
/// `GetTopicAttributes`).
///
/// Only the entries of THAT parent element: the walk starts at its start tag and stops at
/// its end tag, so a second map elsewhere in the document is not folded into this one. An
/// entry with no `<key>` is not an entry and is skipped; a `<value>` the answer leaves
/// empty is the empty string, which is what the answering service reported.
pub(super) fn xml_entry_map(document: &str, parent: &str) -> ::serde_json::Map<String, Value> {
    use ::quick_xml::events::Event;

    const ENTRY: &[u8] = b"entry";
    const KEY: &[u8] = b"key";
    const VALUE: &[u8] = b"value";

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut entries = ::serde_json::Map::new();
    let mut inside = false;
    // The entry being read: its key and its value, each `Some` once its element opened.
    let mut key: Option<String> = None;
    let mut value: Option<String> = None;
    let mut reading: Option<&[u8]> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => match start.local_name().as_ref() {
                name if name == parent.as_bytes() => inside = true,
                ENTRY if inside => {
                    key = None;
                    value = None;
                }
                KEY if inside => {
                    key = Some(String::new());
                    reading = Some(KEY);
                }
                VALUE if inside => {
                    value = Some(String::new());
                    reading = Some(VALUE);
                }
                _ => {}
            },
            Ok(Event::Text(found)) if reading.is_some() => {
                let text = found.xml10_content().unwrap_or_default();
                let held = match reading {
                    Some(KEY) => key.as_mut(),
                    Some(VALUE) => value.as_mut(),
                    _ => None,
                };
                if let Some(held) = held {
                    held.push_str(&text);
                }
            }
            Ok(Event::End(end)) => match end.local_name().as_ref() {
                name if name == parent.as_bytes() => return entries,
                ENTRY if inside => {
                    if let Some(key) = key.take() {
                        entries.insert(key, Value::String(value.take().unwrap_or_default()));
                    }
                }
                KEY | VALUE => reading = None,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => return entries,
            Ok(_) => {}
        }
    }
}

/// Each DIRECT child element of `parent` that holds TEXT, as the JSON object the rest of
/// this module compares: one entry per such element, keyed by its own name.
///
/// This is how an awsQuery answer carries a resource's scalar members — Amazon RDS reports
/// a DB instance as `<DBInstance><DBInstanceClass>db.t3.micro</DBInstanceClass>…` — so it
/// is the reader for a service whose answer is a STRUCTURE rather than the entry map
/// [`xml_entry_map`] reads.
///
/// Only the first `parent` element, and only its own children: the walk starts at that
/// element's start tag and stops at its end tag, so a second structure elsewhere in the
/// document (the next DB instance of a list) is not folded into this one. A child that
/// HOLDS other elements is not a scalar and is left out entirely rather than reported as
/// the concatenation of its descendants' text; an empty element (`<Iops/>`) is the empty
/// value of that member, which is what the answering service reported.
pub(super) fn xml_scalar_children(
    document: &str,
    parent: &str,
) -> ::serde_json::Map<String, Value> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut members = ::serde_json::Map::new();
    let mut inside = false;
    // How many elements are open BELOW `parent`: a direct child opens at 0, and only a
    // child read at that level is one of this structure's own members.
    let mut depth = 0usize;
    // The direct child being read, while it is still a candidate scalar, and its text.
    let mut member: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                if !inside {
                    inside = name == parent;
                    continue;
                }
                if depth == 0 {
                    member = Some(name);
                    text.clear();
                } else {
                    // The child this is nested in holds elements, so it is not a scalar.
                    member = None;
                }
                depth += 1;
            }
            Ok(Event::Text(found)) if inside && depth == 1 => {
                text.push_str(&found.xml10_content().unwrap_or_default());
            }
            Ok(Event::Empty(empty)) if inside && depth == 0 => {
                let name = String::from_utf8_lossy(empty.local_name().as_ref()).into_owned();
                members.insert(name, Value::String(String::new()));
            }
            Ok(Event::End(end)) if inside => {
                let name = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                if depth == 0 {
                    if name == parent {
                        return members;
                    }
                    continue;
                }
                if depth == 1 {
                    if let Some(read) = member.take() {
                        if read == name {
                            members.insert(read, Value::String(::std::mem::take(&mut text)));
                        }
                    }
                }
                depth -= 1;
            }
            Ok(Event::Eof) | Err(_) => return members,
            Ok(_) => {}
        }
    }
}

/// A service's error answer as the code it carries and the message it worded. The code is
/// the last segment of the awsJson `__type`
/// (`com.amazonaws.firehose#ResourceInUseException` -> `ResourceInUseException`), which is
/// what a client branches on.
///
/// The message is the one an SDK of that service would raise ([`sdk_message`]), because what
/// a resource provider reports is what its own SDK call raised.
fn refusal_of(signing_name: &str, status: u16, document: &Value) -> Refusal {
    let code = document
        .get("__type")
        .or_else(|| document.get("code"))
        .or_else(|| document.get("Code"))
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    let message = document
        .get("message")
        .or_else(|| document.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Refusal {
        message: sdk_message(signing_name, status, &code, message),
        code,
    }
}

/// Sign a request with the emulator's own credentials and dispatch it in-process to the
/// service its credential scope names.
///
/// The request is signed because the emulator verifies every request signature when
/// authentication is on; the credential scope is also what routes it, so the service that
/// owns the resource is the one that serves the call.
///
/// `method` and `uri` are the called operation's own HTTP binding: an RPC protocol binds
/// every operation to `POST /` and names what it acts on in its document — or, for
/// awsQuery, in its form body — while a REST protocol binds each operation to its own
/// method and path.
async fn dispatch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>), String> {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    for (name, value) in &headers {
        request = request.header(*name, value);
    }
    let mut request = request.body(body).map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = callback_credentials(ctx).await?;
    sign_callback_request(
        &mut request,
        signing_name,
        region,
        &access_key_id,
        &secret_access_key,
    )?;

    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| error.to_string())?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &ctx.callback().http_connector(),
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok((status, payload))
}

/// SigV4-sign one in-process request to another emulated service, in place.
///
/// The settings are the ones the emulator's own verifier re-signs with
/// (`vendor/server/src/request_authentication.rs`), so the signature this produces is the
/// one that verifier computes over the same request — which is what lets a stack create a
/// resource with request authentication ON. Public so a test can hold it to exactly that
/// (`tests/all/callback_signature.rs`).
pub fn sign_callback_request(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Result<(), String> {
    use ::aws_sigv4::http_request::{
        sign, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
    };

    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name("aws-emulator-cloudformation")
            .access_key_id(access_key_id)
            .secret_access_key(secret_access_key)
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let signable_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        signable_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}

/// The credentials a callback signs with: the emulator's own, through the provider the
/// framework hands every handler — which falls back to its placeholder pair when the
/// router vends none (request authentication off, where nothing verifies a signature).
async fn callback_credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = ctx
        .callback()
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        credentials.access_key_id().to_string(),
        credentials.secret_access_key().to_string(),
    ))
}

#[cfg(test)]
mod handler_error_class_tests {
    use super::{
        handler_error_code, sdk_attempt_count, HANDLER_ERROR_CLASSES, HANDLER_ERROR_INVALID_REQUEST,
        HANDLER_ERROR_SERVICE_FAILED, SDK_RETRIED_ATTEMPTS, SDK_RETRIED_CLASSES,
        SDK_SINGLE_ATTEMPT,
    };
    use crate::ops::types::HANDLER_ERROR_CODE_VALUES;

    /// Every class the SDK diagnostic counts attempts for is one the classification above
    /// can actually answer, so the retried set cannot drift from the class table.
    #[test]
    fn every_retried_class_is_one_the_classification_answers() {
        for class in SDK_RETRIED_CLASSES {
            assert!(
                HANDLER_ERROR_CLASSES.iter().any(|(named, _)| *named == class),
                "case {class:?}: not a class the refusal classification answers, which are \
                 {HANDLER_ERROR_CLASSES:?}"
            );
        }
    }

    /// The attempt count follows the CLASS of the refusal: the SDK's default maximum for a
    /// condition that can clear on its own, one attempt for a refusal of the request itself.
    #[test]
    fn the_attempt_count_follows_the_class() {
        for (code, expected) in [
            ("LimitExceededException", SDK_RETRIED_ATTEMPTS),
            ("ThrottlingException", SDK_RETRIED_ATTEMPTS),
            ("ServiceUnavailable", SDK_RETRIED_ATTEMPTS),
            ("HTTP 503", SDK_RETRIED_ATTEMPTS),
            ("ValidationException", SDK_SINGLE_ATTEMPT),
            ("InvalidTypeException", SDK_SINGLE_ATTEMPT),
            ("ResourceNotFoundException", SDK_SINGLE_ATTEMPT),
            ("HTTP 400", SDK_SINGLE_ATTEMPT),
        ] {
            let actual = sdk_attempt_count(code);
            assert_eq!(
                actual, expected,
                "case {code:?} ({}): expected {expected} attempt(s), got {actual}",
                handler_error_code(code)
            );
        }
    }

    /// Every class this module reports is one the MODEL declares: the classification is a
    /// mapping onto `HandlerErrorCode`, so a class spelled any other way would put a value
    /// on the wire that enum does not have.
    #[test]
    fn every_reported_class_is_one_the_model_declares() {
        for class in HANDLER_ERROR_CLASSES
            .iter()
            .map(|(class, _)| *class)
            .chain([HANDLER_ERROR_INVALID_REQUEST, HANDLER_ERROR_SERVICE_FAILED])
        {
            assert!(
                HANDLER_ERROR_CODE_VALUES.contains(&class),
                "case {class:?}: not a HandlerErrorCode the model declares, which are \
                 {HANDLER_ERROR_CODE_VALUES:?}"
            );
        }
    }

    /// And so is whatever the classification answers for an arbitrary code, including one
    /// no class matches and an answer that named no error at all.
    #[test]
    fn every_answer_is_one_the_model_declares() {
        for code in [
            "DBSubnetGroupNotFoundFault",
            "InvalidParameterValue",
            "ThrottlingException",
            "BucketAlreadyOwnedByYou",
            "ResourceInUseException",
            "LimitExceededException",
            "AccessDeniedException",
            "a-code-no-service-answers",
            "",
            "HTTP 501",
            "HTTP 400",
            "HTTP not-a-status",
        ] {
            let class = handler_error_code(code);
            assert!(
                HANDLER_ERROR_CODE_VALUES.contains(&class),
                "case {code:?}: answered {class:?}, which is not a HandlerErrorCode the \
                 model declares"
            );
        }
    }
}
