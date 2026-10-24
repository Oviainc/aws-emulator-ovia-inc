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
//! evaluated document is passed through verbatim; for `AWS::ECR::PublicRepository` they are
//! not (`RepositoryName` / `RepositoryCatalogData.RepositoryDescription` against
//! `repositoryName` / `catalogData.description`), so that arm maps the members the resource
//! reference publishes one by one, and sends nothing for a property the template leaves
//! out; and for `AWS::ECS::Cluster` they are the same members in a different CASE, with one
//! renamed, so that arm re-cases them.
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

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::Value;

use super::provisioner::provisioner_for;
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

/// What deploying a stack's template left its resources as.
#[derive(Debug, Clone, Default)]
pub struct Materialised {
    /// The stack's resources, in the order the template declares them: each one as
    /// [`super::template::provision`] computed it, with `materialised` set and any
    /// attribute the owning service reported for it applied.
    pub resources: Vec<StoredResource>,
    /// The reason the stack cannot reach its completed status: an owning service refused
    /// to create one of the resources, or to delete one the deployment replaced. `None`
    /// when every resource this emulator provisions was created.
    pub failure: Option<String>,
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
/// tags and the `aws:cloudformation:*` ones. A resource the deployment carries forward keeps
/// the tags it was created with: nothing is re-tagged for a resource nothing changed about.
pub async fn reconcile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    stack: &StackProvenance,
    declared: Vec<StoredResource>,
    previous: &[StoredResource],
) -> Materialised {
    let mut outcome = Materialised::default();
    let mut declared = declared.into_iter();
    for mut resource in declared.by_ref() {
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
                let applied = match provisioner_for(&resource.resource_type) {
                    Some(provisioner) => {
                        provisioner
                            .update(ctx, region, &resource, &declared_properties)
                            .await
                    }
                    None => Ok(None),
                };
                match applied {
                    // The owning service applied it, so the configuration in force is the
                    // DECLARED one — which is what the stack records, and what a later
                    // deployment and drift detection compare against.
                    Ok(Some(updated)) => {
                        resource.attributes.extend(updated.attributes);
                        resource.properties = declared_properties;
                    }
                    Ok(None) => {
                        outcome.failure = Some(format!(
                            "The following resource(s) failed to update: [{}]. NotImplemented: \
                             this emulator does not change the properties of an existing {} in \
                             place; the resource was left as it is.",
                            resource.logical_id, resource.resource_type
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                    Err(refusal) => {
                        // The owning service refused the change: nothing changed there, so
                        // the deployment reports that refusal rather than a configuration
                        // nothing applied.
                        outcome.failure = Some(format!(
                            "The following resource(s) failed to update: [{}]. {refusal}",
                            resource.logical_id
                        ));
                        outcome.resources.push(resource);
                        break;
                    }
                }
            }
            outcome.resources.push(resource);
            continue;
        }
        let created = match provisioner_for(&resource.resource_type) {
            // The resource is created WITH the tags CloudFormation gives it: the stack's
            // own, plus the three that name the stack and this resource's place in it.
            Some(provisioner) => {
                let tags = stack.tags_for(&resource.logical_id);
                provisioner.create(ctx, region, &resource, &tags).await
            }
            // No provisioner for this type: nothing is asked of any service, and the
            // resource stays DECLARED-ONLY.
            None => {
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
                adopt_minted_identity(&mut resource, created.physical_resource_id);
                resource.attributes.extend(created.attributes);
                resource.materialised = true;
            }
            Err(refusal) => {
                outcome.failure = Some(format!(
                    "The following resource(s) failed to create: [{}]. {refusal}",
                    resource.logical_id
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
    let deleted = delete(ctx, region, &replaced).await;
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
    outcome
}

/// Record the identity an owning service ASSIGNED to a resource it created: the id its
/// `Ref` resolves to from here on, and the `Id`/`<Type>Id` attributes read off that id
/// ([`super::template::identity_attributes`]), which the naming rules had computed from the
/// name instead.
///
/// `None` is every type whose physical id CloudFormation computes, which the create already
/// carried out to that service: nothing to adopt.
fn adopt_minted_identity(resource: &mut StoredResource, assigned: Option<String>) {
    let Some(assigned) = assigned else {
        return;
    };
    for (attribute, value) in
        super::template::identity_attributes(&resource.resource_type, &assigned)
    {
        // An attribute the convention already gave the resource is re-pointed; a new one
        // is added only while the resource is under its attribute bound, which is what
        // caps the map.
        if resource.attributes.contains_key(&attribute)
            || resource.attributes.len() < super::template::MAX_ATTRIBUTES_PER_RESOURCE
        {
            resource.attributes.insert(attribute, value);
        }
    }
    resource.physical_resource_id = assigned;
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
    /// refused to delete one of the resources.
    pub failure: Option<String>,
}

/// Delete the resources a stack created, newest first — the reverse of the order they were
/// created in, so a resource is deleted before whatever it depends on.
///
/// [`Deleted::failure`] names the first resource whose owning service refused to delete
/// it; the stack's deletion cannot be reported as complete then, and the resources it had
/// already deleted are reported as deleted so a retry does not ask for them again. A
/// resource the service no longer has is a success: the request asks for a state the
/// service is already in.
pub async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
) -> Deleted {
    let mut outcome = Deleted {
        resources: resources.to_vec(),
        failure: None,
    };
    // Newest first, over the indices of the set being reported back, so each resource's
    // record is updated in place as its deletion succeeds.
    for index in (0..outcome.resources.len()).rev() {
        // Nothing was created for a declared-only resource, so there is nothing to
        // delete.
        if !outcome.resources[index].materialised {
            continue;
        }
        let resource = &outcome.resources[index];
        let deleted = match provisioner_for(&resource.resource_type) {
            Some(provisioner) => provisioner.delete(ctx, region, resource).await,
            None => Ok(()),
        };
        match deleted {
            Ok(()) => outcome.resources[index].materialised = false,
            Err(refusal) => {
                outcome.failure = Some(format!(
                    "The following resource(s) failed to delete: [{}]. {refusal}",
                    outcome.resources[index].logical_id
                ));
                return outcome;
            }
        }
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
    Err(refusal_of(status, &document))
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
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&::percent_encoding::utf8_percent_encode(name, FORM_VALUE).to_string());
        body.push('=');
        body.push_str(&::percent_encoding::utf8_percent_encode(value, FORM_VALUE).to_string());
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
    Err(Refusal {
        code: xml_element(&document, "Code")
            .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}")),
        message: xml_element(&document, "Message").unwrap_or_default(),
    })
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
    Err(refusal_of(status, &document))
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

/// A service's error answer as the code it carries and the message it worded. The code is
/// the last segment of the awsJson `__type`
/// (`com.amazonaws.firehose#ResourceInUseException` -> `ResourceInUseException`), which is
/// what a client branches on.
fn refusal_of(status: u16, document: &Value) -> Refusal {
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
        .unwrap_or_default()
        .to_string();
    Refusal { code, message }
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
