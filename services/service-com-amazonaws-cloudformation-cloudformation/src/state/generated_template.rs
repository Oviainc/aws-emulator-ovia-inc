// SPDX-License-Identifier: CC0-1.0

//! GENERATED TEMPLATES: the templates `CreateGeneratedTemplate` generates from resources a
//! stack already manages, and the generation each one's status reports the progress of.
//!
//! A generated template is a record of its own, not a stack's: `CreateGeneratedTemplate`
//! "creates a template from existing resources" and hands back the template's id, and
//! "you can check the status of the template generation using the DescribeGeneratedTemplate
//! API action". So everything that read reports lives on ONE [`StoredGeneratedTemplate`]
//! and the [`GeneratedResource`] rows under it, and the read is served from the record
//! rather than from the request.
//!
//! ## The generation rides the clock
//!
//! The generation is ASYNCHRONOUS: the call is answered while the template is still being
//! generated (the modelled `GeneratedTemplateStatus` has a `CREATE_IN_PROGRESS` value whose
//! documentation is "the creation of the template is in progress") and it reaches `COMPLETE`
//! a moment later. A record therefore carries the terminal status the clock owes in
//! [`StoredGeneratedTemplate::pending`], and
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] is what
//! materialises it: a caller reading straight after the call still sees the generation in
//! flight, one reading after the due instant sees it complete, and nothing sleeps or
//! advances a generation because it was read.
//!
//! ## The in-flight cap
//!
//! How many generations may be in flight AT ONCE is metered, and the quota is modelled:
//! `ConcurrentResourcesLimitExceededException` (HTTP 429, awsQuery code
//! `ConcurrentResourcesLimitExceeded`) is documented "No more than
//! 5 generated templates can be in an InProgress or Pending status at one time". That
//! documented number is [`MAX_CONCURRENT_GENERATIONS`], the set it meters is the records
//! [`StoredGeneratedTemplate::is_in_flight`] answers true for, and because the statuses it
//! counts are the ones the clock retires, the cap LIFTS on its own as the generations it
//! counted complete.
//!
//! ## The template body a generation produces
//!
//! What `GetGeneratedTemplate` answers is a TEMPLATE, rendered from the record rather than
//! stored as text: the resources the generation covers, each keyed by the logical id it has
//! in the final template ([`final_logical_ids`]) and carrying the `TemplateConfiguration`
//! the record holds. It is rendered on demand because the language is the READ's choice —
//! `GetGeneratedTemplateInput$Format` is "the language to use to retrieve for the generated
//! template", `JSON` or `YAML` — so one record answers either spelling of the same
//! template.
//!
//! Which generation that template is from is the documented part: "if the template is in an
//! InProgress or Pending status then the template returned will be the template when the
//! template was last in a Complete status", and "if the template has not yet been in a
//! Complete status then an empty template will be returned". A record therefore carries the
//! template of its last COMPLETE generation in [`StoredGeneratedTemplate::completed`], which
//! [`StoredGeneratedTemplate::settle`] snapshots as it materialises that completion, and a
//! record that has never completed renders the empty template.
//!
//! ## Deleting one
//!
//! `DeleteGeneratedTemplate` removes the record it addresses from the store, so the name and
//! the quotas it held are free again and no read answers for it afterwards: nothing in this
//! feature reports a generated template that was deleted, unlike a deleted stack, whose
//! summary and template stay addressable for a retention window. A delete DISCARDS the
//! record whatever status its generation is in — a generation still in flight
//! ([`StoredGeneratedTemplate::is_in_flight`]) goes with the record it belongs to, since
//! there is nothing left for it to conclude onto — and the only address it refuses is one no
//! record of the request's Region holds.
//!
//! ## Refusing one
//!
//! Every refusal of this surface is worded by the component that serves it, so the message a
//! client reads is that component's sentence followed by the SDK envelope of the call into it
//! ([`does_not_exist`], [`already_exists`]).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::serde_json::{Map, Value};

/// The `GeneratedTemplateStatus` values a generated template reports (the model's own enum,
/// documented value by value on `DescribeGeneratedTemplateOutput$Status`).
///
/// `COMPLETE` ("the template operation is complete") and `FAILED` ("the template operation
/// failed") are the terminal ones; every other value is a generation still in flight, which
/// is what [`StoredGeneratedTemplate::is_in_flight`] and so the in-flight cap key on.
pub mod status {
    pub const CREATE_PENDING: &str = "CREATE_PENDING";
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const UPDATE_PENDING: &str = "UPDATE_PENDING";
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const DELETE_PENDING: &str = "DELETE_PENDING";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const COMPLETE: &str = "COMPLETE";
    pub const FAILED: &str = "FAILED";
}

/// The `TemplateFormat` values a read may ask a generated template's body in (the model's
/// own enum: `JSON | YAML`), as `GetGeneratedTemplateInput$Format`.
pub mod format {
    pub const JSON: &str = "JSON";
    pub const YAML: &str = "YAML";
}

/// The `GeneratedTemplateDeletionPolicy` / `GeneratedTemplateUpdateReplacePolicy` values a
/// `TemplateConfiguration` carries (both modelled enums are `DELETE | RETAIN`).
pub mod policy {
    pub const DELETE: &str = "DELETE";
    pub const RETAIN: &str = "RETAIN";
}

/// The `GeneratedTemplateResourceStatus` values one resource of a generated template
/// reports (the model's enum: `PENDING | IN_PROGRESS | FAILED | COMPLETE`), as
/// `ResourceDetail$ResourceStatus`.
pub mod resource_status {
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const COMPLETE: &str = "COMPLETE";
}

/// The most generations that may be in a pending or in-progress status at one time, in one
/// Region.
///
/// The number the modelled `ConcurrentResourcesLimitExceededException` documents: "No more
/// than 5 generated templates can be in an InProgress or Pending status at one time". A
/// call beyond it is refused with that error and leaves no record behind.
pub const MAX_CONCURRENT_GENERATIONS: usize = 5;

/// The most generated templates one Region holds, past which `CreateGeneratedTemplate`
/// answers the modelled `LimitExceededException` ("The quota for the resource has already
/// been reached").
///
/// A caller chooses how many it creates, so the store bounds them by count. Chosen, like
/// every count in [`crate::state`] that no published quota fixes; it sits far above any
/// client workflow. What frees a record is the caller deleting it, the way a stack's
/// quota is freed by deleting the stack: `DeleteGeneratedTemplate` removes the record it
/// addresses, so the name and this quota it held are free again.
pub const MAX_GENERATED_TEMPLATES: usize = 100;

/// The most generated templates this store holds across EVERY Region together.
///
/// The Region a request is served in comes from its own credential scope, so it is a value
/// the CALLER chooses: a per-Region bound alone would cap only one factor of the store's
/// growth, since a caller signing for one Region string after another could keep creating
/// records. This is the flat ceiling on the product, past which `CreateGeneratedTemplate`
/// answers the same modelled `LimitExceededException` — the same shape
/// [`crate::state::MAX_LIVE_STACKS`] is to [`crate::state::MAX_STACKS`].
pub const MAX_STORED_GENERATED_TEMPLATES: usize = 5 * MAX_GENERATED_TEMPLATES;

/// The most resources one generated template carries.
///
/// Both modelled resource lists of this feature are `@length(min: 1, max: 500)` —
/// `CreateGeneratedTemplateInput$Resources` and `DescribeGeneratedTemplateOutput$Resources`
/// — so 500 is the most a generated template can hold or report.
pub const MAX_GENERATED_TEMPLATE_RESOURCES: usize = 500;

/// How long after the call that starts it a generation's completion becomes due.
///
/// Chosen (nothing documents how long generating a template takes): non-zero, so a caller
/// polling `DescribeGeneratedTemplate` straight after the call still sees the generation in
/// flight — and so the in-flight cap it counts against — and short enough that the poll
/// after it sees the generation complete.
pub const GENERATION_DELAY: Duration = Duration::from_millis(500);

/// How many summaries `ListGeneratedTemplates` answers for a request that names no
/// `MaxResults`.
///
/// Documented on `ListGeneratedTemplatesInput$MaxResults`: "By default the
/// ListGeneratedTemplates API action will return at most 50 results in each response. The
/// maximum value is 100." The maximum is the modelled `@range(min: 1, max: 100)` on that
/// member, which the request itself cannot exceed.
pub const DEFAULT_LISTED_TEMPLATES: usize = 50;

/// The `StatusReason` a generation that completed carries, as the service reports it for a
/// template whose generation is `COMPLETE`.
pub const GENERATION_COMPLETE_REASON: &str = "Template complete";

/// One resource of a generated template: what `ResourceDetail` reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedResource {
    /// `ResourceDetail$ResourceType`: "The type of the resource, such as
    /// `AWS::DynamoDB::Table`".
    pub resource_type: String,
    /// `ResourceDetail$LogicalResourceId`: "The logical id for this resource in the final
    /// generated template" — the one the base stack declares it under, or the one the
    /// request named for a resource it listed itself. `None` for a listed resource the
    /// request gave none for, since the member is optional and nothing here invents one.
    pub logical_resource_id: Option<String>,
    /// `ResourceDetail$ResourceIdentifier`: the primary identifier that identifies the
    /// resource, keyed by the identifier's PROPERTY name.
    ///
    /// Carries what the request supplied for a resource it listed
    /// (`ResourceDefinition$ResourceIdentifier` is required there, so a listed resource
    /// always has one). EMPTY for a resource taken from the base stack: which property
    /// holds a type's primary identifier is not something this emulator's per-type
    /// catalogue records (the same gap
    /// [`crate::state::resource_schema`](crate::state::resource_schema) leaves
    /// `primaryIdentifier` out of a published schema for), so the member is reported absent
    /// rather than under a property name nothing stands behind.
    pub resource_identifier: BTreeMap<String, String>,
}

/// The template the last COMPLETE generation of one record produced: the resources it
/// covered and the configuration assigned to them at that point.
///
/// Snapshotted rather than read live off the record, because "the template returned will be
/// the template when the template was last in a Complete status": a generation started
/// since (an `UpdateGeneratedTemplate`, which covers a different resource set) must not
/// change what a read answers until that generation itself completes.
#[derive(Debug, Clone)]
pub struct CompletedGeneration {
    pub resources: Vec<GeneratedResource>,
    pub deletion_policy: Option<String>,
    pub update_replace_policy: Option<String>,
}

/// The terminal status a generation is headed for, and the instant it is due.
#[derive(Debug, Clone)]
pub struct PendingGeneration {
    pub status: String,
    pub status_reason: Option<String>,
    pub due_at: SystemTime,
}

/// One generated template, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredGeneratedTemplate {
    /// `GeneratedTemplateId`: "The Amazon Resource Name (ARN) of the generated template.
    /// The format is
    /// `arn:${Partition}:cloudformation:${Region}:${Account}:generatedtemplate/${Id}`."
    pub generated_template_id: String,
    pub generated_template_name: String,
    /// The Region the generated template lives in: the one the call that created it was
    /// signed for. A name addresses one record of THAT Region, and the record's own ARN
    /// carries the same Region.
    pub region: String,
    /// `StackId`: "The stack ARN of the base stack if a base stack was provided when
    /// generating the template."
    pub stack_id: Option<String>,
    pub resources: Vec<GeneratedResource>,
    /// [`status`].
    pub status: String,
    pub status_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
    /// `TemplateConfiguration$DeletionPolicy`, as the request supplied it.
    pub deletion_policy: Option<String>,
    /// `TemplateConfiguration$UpdateReplacePolicy`, as the request supplied it.
    pub update_replace_policy: Option<String>,
    /// The terminal status the clock owes this generation, until
    /// [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] materialises
    /// it.
    pub pending: Option<PendingGeneration>,
    /// The template of the last generation of this record that reached `COMPLETE`, which is
    /// the one a read of its body answers. `None` until one has, since "if the template has
    /// not yet been in a Complete status then an empty template will be returned".
    pub completed: Option<CompletedGeneration>,
}

impl StoredGeneratedTemplate {
    /// A generated template as `CreateGeneratedTemplate` first stores it: in flight, with
    /// its completion due one [`GENERATION_DELAY`] from the call that started it.
    pub fn starting(
        generated_template_id: String,
        generated_template_name: String,
        region: String,
        now: SystemTime,
    ) -> Self {
        Self {
            generated_template_id,
            generated_template_name,
            region,
            stack_id: None,
            resources: Vec::new(),
            status: status::CREATE_IN_PROGRESS.to_string(),
            status_reason: None,
            creation_time: now,
            last_updated_time: now,
            deletion_policy: None,
            update_replace_policy: None,
            pending: Some(PendingGeneration {
                status: status::COMPLETE.to_string(),
                status_reason: Some(GENERATION_COMPLETE_REASON.to_string()),
                due_at: now + GENERATION_DELAY,
            }),
            completed: None,
        }
    }

    /// Start a NEW generation over a record that already exists, as
    /// `UpdateGeneratedTemplate` does: "updates a generated template... you can check the
    /// status of the update to the generated template using the DescribeGeneratedTemplate
    /// API action".
    ///
    /// The update is asynchronous exactly as the first generation was, so the record is left
    /// with its generation in flight and its conclusion due one [`GENERATION_DELAY`] on — a
    /// caller reading straight after the call sees `UPDATE_IN_PROGRESS`, and the template of
    /// the last COMPLETE generation stays the one a read of the body answers until this one
    /// concludes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.status = status::UPDATE_IN_PROGRESS.to_string();
        // The reason belongs to the status it was recorded for, and the generation this call
        // starts has not concluded yet.
        self.status_reason = None;
        self.last_updated_time = now;
        self.pending = Some(PendingGeneration {
            status: status::COMPLETE.to_string(),
            status_reason: Some(GENERATION_COMPLETE_REASON.to_string()),
            due_at: now + GENERATION_DELAY,
        });
    }

    /// Materialise the status transition the clock has already made due, if any.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what was
    /// owed at `now` even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = self.pending.take().expect("checked just above");
        self.status = pending.status;
        self.status_reason = pending.status_reason;
        // A generation that CONCLUDED produced a template, and that template is what every
        // later read of the body answers until another generation concludes — so the
        // resources and configuration it covered are snapshotted as the completion is
        // materialised, not read live off a record a later generation may have moved on.
        if self.status == status::COMPLETE {
            self.completed = Some(CompletedGeneration {
                resources: self.resources.clone(),
                deletion_policy: self.deletion_policy.clone(),
                update_replace_policy: self.update_replace_policy.clone(),
            });
        }
        // "The time the generated template was last updated": the generation concluding is
        // the update, so it is stamped with the instant that was due rather than with now.
        self.last_updated_time = pending.due_at;
    }

    /// Whether this generation is still in flight — a pending or in-progress status, which
    /// is the set [`MAX_CONCURRENT_GENERATIONS`] meters.
    pub fn is_in_flight(&self) -> bool {
        !matches!(self.status.as_str(), status::COMPLETE | status::FAILED)
    }

    /// Whether `name_or_id` addresses this record in `region`: `GeneratedTemplateName` is
    /// "the name or Amazon Resource Name (ARN) of a generated template".
    ///
    /// Nothing outside the request's own Region is addressable — the same name in another
    /// Region is another generated template, exactly as it is for a stack
    /// ([`crate::state::ComAmazonawsCloudformationCloudformationState::resolve`]).
    pub fn is_addressed_by(&self, region: &str, name_or_id: &str) -> bool {
        self.region == region
            && (self.generated_template_name == name_or_id
                || self.generated_template_id == name_or_id)
    }

    /// The `ResourceStatus` every resource of this template reports: the resources of a
    /// generation that has concluded are complete, and those of one still in flight are
    /// being processed.
    pub fn resource_status(&self) -> &'static str {
        if self.is_in_flight() {
            resource_status::IN_PROGRESS
        } else {
            resource_status::COMPLETE
        }
    }

    /// `TemplateProgress`: "a summary of the progress of the template generation", counted
    /// over the resources this template carries.
    pub fn progress(&self) -> GenerationProgress {
        let resources = i32::try_from(self.resources.len()).unwrap_or(i32::MAX);
        let in_flight = self.is_in_flight();
        GenerationProgress {
            resources_succeeded: if in_flight { 0 } else { resources },
            resources_processing: if in_flight { resources } else { 0 },
            // A generation here either concludes for every resource it covers or does not
            // conclude yet: no resource of it is left queued behind another, and none of
            // them fails on its own.
            resources_pending: 0,
            resources_failed: 0,
        }
    }

    /// The body of this generated template, in the language the read asked for.
    ///
    /// `Format` is "the language to use to retrieve for the generated template. Supported
    /// values are: JSON YAML"; a read that names neither is answered the JSON spelling,
    /// which is the language a template body is written in when nothing says otherwise.
    pub fn template_body(&self, format: Option<&str>) -> String {
        let document = self.last_complete_template();
        if format == Some(format::YAML) {
            yaml_text(&document)
        } else {
            json_text(&document)
        }
    }

    /// The template of this record's last COMPLETE generation, as a template document.
    ///
    /// "If the template has not yet been in a Complete status then an empty template will be
    /// returned" — the empty template, not an absent body.
    fn last_complete_template(&self) -> Value {
        let Some(completed) = self.completed.as_ref() else {
            return Value::Object(Map::new());
        };
        let mut resources = Map::new();
        let logical_ids = final_logical_ids(&completed.resources);
        for (logical_id, resource) in logical_ids.into_iter().zip(&completed.resources) {
            let mut declared = Map::new();
            declared.insert(
                "Type".to_string(),
                Value::String(resource.resource_type.clone()),
            );
            // "The DeletionPolicy assigned to resources in the generated template" — and the
            // same for UpdateReplacePolicy: the configuration is assigned to every resource
            // of the template, so it is rendered onto each one, and a configuration the
            // record does not carry assigns no attribute at all.
            for (attribute, configured) in [
                ("DeletionPolicy", completed.deletion_policy.as_deref()),
                (
                    "UpdateReplacePolicy",
                    completed.update_replace_policy.as_deref(),
                ),
            ] {
                if let Some(value) = configured.and_then(attribute_policy) {
                    declared.insert(attribute.to_string(), Value::String(value.to_string()));
                }
            }
            // The properties of the resource that the record knows: the primary-identifier
            // pairs it was identified by ("the key is the name of one of the primary
            // identifiers for the resource", which is a property of the resource). A
            // resource the record holds no property of declares none rather than an empty
            // map.
            if !resource.resource_identifier.is_empty() {
                declared.insert(
                    "Properties".to_string(),
                    Value::Object(
                        resource
                            .resource_identifier
                            .iter()
                            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                            .collect(),
                    ),
                );
            }
            resources.insert(logical_id, Value::Object(declared));
        }

        let mut document = Map::new();
        // The generated template's own id, carried by the template it generated, so the
        // document says which record it came out of.
        let mut metadata = Map::new();
        metadata.insert(
            "TemplateId".to_string(),
            Value::String(self.generated_template_id.clone()),
        );
        document.insert("Metadata".to_string(), Value::Object(metadata));
        // A generation may cover no resource at all ("if no resources are specified, the
        // template will be created without any resources"), and a template declares no
        // empty `Resources` section for that.
        if !resources.is_empty() {
            document.insert("Resources".to_string(), Value::Object(resources));
        }
        Value::Object(document)
    }
}

/// The counts `TemplateProgress` reports for one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationProgress {
    pub resources_succeeded: i32,
    pub resources_failed: i32,
    pub resources_processing: i32,
    pub resources_pending: i32,
}

/// The component that serves this surface, as the SDK envelope of one of its refusals names
/// the service that answered.
///
/// Template generation is not served by CloudFormation's own front end: the operations of
/// this feature are forwarded to a component of their own, and a refusal of this surface
/// therefore reaches a client as that component's sentence wrapped in the SDK's envelope for
/// the call into it, naming the component rather than CloudFormation.
const GENERATION_COMPONENT: &str = "CfnTemplateGenerationServiceLambda";

/// The status the envelope of a refusal of this surface states.
///
/// The envelope states the status of the CALL INTO [`GENERATION_COMPONENT`], which is the
/// client-error status that component answers a refused request with — not the status
/// CloudFormation maps the modelled error onto for its own caller, which is the one the
/// response itself carries (a not-found is 404 on the wire and 400 in the envelope).
const GENERATION_COMPONENT_REFUSAL_STATUS: u16 = 400;

/// The exception name the envelope states for a refusal of an address no record holds: the
/// modelled error's SHAPE name, which is what the SDK of the call into the component reports
/// — the awsQuery `<Code>` of the response this emulator sends is the model's own
/// `GeneratedTemplateNotFound`, and the two are separate values.
const NOT_FOUND_EXCEPTION: &str = "GeneratedTemplateNotFoundException";

/// The exception name the envelope states for a refusal of a name a record already holds.
const ALREADY_EXISTS_EXCEPTION: &str = "AlreadyExistsException";



/// `sentence` followed by the envelope the AWS SDK appends to a refusal of this surface:
/// `(Service: <component>; Status Code: <status>; Error Code: <exception>; Request ID: <id>;
/// Proxy: null)`.
///
/// `exception` is the name of the modelled error being worded. The request id is the only
/// per-run field, and it belongs to the RESPONSE rather than to this sentence: the marker
/// [`crate::state::REQUEST_ID_PLACEHOLDER`] stands in for it here and the layer that finishes
/// the response substitutes the one id that response reports. Nothing else of the request is
/// interpolated.
fn with_component_envelope(sentence: &str, exception: &str) -> String {
    format!(
        "{sentence} (Service: {GENERATION_COMPONENT}; Status Code: \
         {GENERATION_COMPONENT_REFUSAL_STATUS}; Error Code: {exception}; Request ID: {}; \
         Proxy: null)",
        crate::state::REQUEST_ID_PLACEHOLDER
    )
}

/// The sentence of the refusal answered when a request names a generated template this
/// account and Region holds no record of: fixed text apart from the identifier the request
/// supplied, which the sentence names whichever of the two addresses it is.
fn does_not_exist_sentence(name_or_id: &str) -> String {
    format!("Template with arn {name_or_id} does not exist")
}

/// The whole message answered when a request names a generated template this account and
/// Region holds no record of: the sentence above plus the envelope every refusal of this
/// surface carries ([`with_component_envelope`]).
pub fn does_not_exist(name_or_id: &str) -> String {
    with_component_envelope(&does_not_exist_sentence(name_or_id), NOT_FOUND_EXCEPTION)
}

/// The sentence of the modelled `AlreadyExistsException` ("the resource with the name
/// requested already exists") for a generated template name a record already holds — the
/// name a create asks for, or the one an update asks to rename a record to. Fixed text apart
/// from the name the request asked for.
fn already_exists_sentence(generated_template_name: &str) -> String {
    format!("Template with name {generated_template_name} already exists.")
}

/// The whole message of that refusal: the sentence above plus the envelope
/// ([`with_component_envelope`]).
pub fn already_exists(generated_template_name: &str) -> String {
    with_component_envelope(
        &already_exists_sentence(generated_template_name),
        ALREADY_EXISTS_EXCEPTION,
    )
}

/// The message of the modelled `ConcurrentResourcesLimitExceededException` for a call that
/// would start one more generation than may be in flight at once.
///
/// The sentence the model states for that error, with the number read off
/// [`MAX_CONCURRENT_GENERATIONS`] rather than written into the text twice. It is the only
/// refusal of this surface that does NOT go through [`with_component_envelope`]: the two that
/// do were worded from a recording of this surface answering them, and there is no evidence
/// of how AWS words THIS one — a sentence of this crate's own in that envelope would be an
/// invented message, so what it answers is the model's statement of the error and nothing
/// more. (See the open issue that still carries this cause.)
pub fn concurrent_generations_exceeded() -> String {
    format!(
        "No more than {MAX_CONCURRENT_GENERATIONS} generated templates can be in an \
         InProgress or Pending status at one time"
    )
}

/// The message of the modelled `LimitExceededException` ("the quota for the resource has
/// already been reached") for a call that would leave a generated template covering more
/// than [`MAX_GENERATED_TEMPLATE_RESOURCES`] resources.
pub fn resource_limit_exceeded() -> String {
    format!(
        "Limit for resources in a generated template exceeded: no more than {} resources \
         can be included",
        MAX_GENERATED_TEMPLATE_RESOURCES
    )
}

/// Fold one resource onto the set a generated template covers.
///
/// A resource that names a logical id the set already carries for the same type is the SAME
/// resource of the generated template, not a second one, so the identifier it was submitted
/// with replaces the one that row carried rather than being appended beside it — a template
/// keys its resources by logical id, so two rows under one id would not be a template. One
/// that names no id the set carries is a resource of its own.
pub fn fold(resources: &mut Vec<GeneratedResource>, incoming: GeneratedResource) {
    let already = incoming.logical_resource_id.as_ref().and_then(|logical| {
        resources.iter_mut().find(|resource| {
            resource.logical_resource_id.as_ref() == Some(logical)
                && resource.resource_type == incoming.resource_type
        })
    });
    match already {
        Some(resource) => resource.resource_identifier = incoming.resource_identifier,
        None => resources.push(incoming),
    }
}

/// The resources of a generated template that `logical_ids` does NOT name.
///
/// `UpdateGeneratedTemplateInput$RemoveResources` is "a list of logical ids for resources to
/// remove from the generated template", and the logical id a resource of the template has is
/// the one it is keyed by in the final template ([`final_logical_ids`]) — the same id every
/// read of the record reports — so that is the id a removal addresses. A logical id the
/// template carries no resource under removes nothing.
pub fn without_logical_ids(
    resources: &[GeneratedResource],
    logical_ids: &[String],
) -> Vec<GeneratedResource> {
    final_logical_ids(resources)
        .into_iter()
        .zip(resources)
        .filter(|(logical_id, _)| !logical_ids.contains(logical_id))
        .map(|(_, resource)| resource.clone())
        .collect()
}

/// The logical id each covered resource has IN THE FINAL GENERATED TEMPLATE, index by
/// index with `resources`.
///
/// A template keys its resources by logical id, and `ResourceDetail$LogicalResourceId` is
/// "the logical id for this resource in the final generated template" — while the logical
/// id a request may supply for a resource it lists
/// (`ResourceDefinition$LogicalResourceId`) is OPTIONAL. So the id supplied for a resource
/// is the one it keeps, and a resource that was given none is still keyed by something: an
/// id derived from its own resource type, which is the only thing known about it that says
/// what it is. A derived id stays clear of every supplied one, and a collision either way
/// is broken by the smallest ordinal suffix that is still free, so the ids of one template
/// are pairwise distinct — "unique within the template" is what a template keying its
/// resources by them requires — and the same record always renders the same ones.
pub fn final_logical_ids(resources: &[GeneratedResource]) -> Vec<String> {
    let supplied: BTreeSet<&str> = resources
        .iter()
        .filter_map(|resource| resource.logical_resource_id.as_deref())
        .collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut ids = Vec::with_capacity(resources.len());
    for resource in resources {
        let asked_for = resource.logical_resource_id.is_some();
        let base = match resource.logical_resource_id.as_deref() {
            Some(id) => id.to_string(),
            None => logical_id_of_type(&resource.resource_type),
        };
        let mut candidate = base.clone();
        let mut ordinal = 1usize;
        // A derived id yields to every id the request asked for, wherever in the list that
        // resource sits, so nothing takes a key a resource of this template was named by.
        while used.contains(&candidate)
            || (!asked_for && supplied.contains(candidate.as_str()))
        {
            ordinal += 1;
            candidate = format!("{base}{ordinal}");
        }
        used.insert(candidate.clone());
        ids.push(candidate);
    }
    ids
}

/// A logical id derived from a resource type: the type's own segments past the vendor
/// prefix, letters and digits only (`AWS::DynamoDB::Table` -> `DynamoDBTable`).
///
/// A logical id is "alphanumeric (A-Za-z0-9) and unique within the template", so everything
/// a type name carries that an id may not hold is dropped.
fn logical_id_of_type(resource_type: &str) -> String {
    let mut id = String::new();
    for (position, segment) in resource_type.split("::").enumerate() {
        if position == 0 && matches!(segment, "AWS" | "Alexa" | "Custom") {
            continue;
        }
        id.extend(segment.chars().filter(|c| c.is_ascii_alphanumeric()));
    }
    if id.is_empty() {
        // Nothing of the type name survives as an id (a type that is punctuation alone):
        // the resource still needs a key, so it falls back to the generic noun and the
        // ordinal suffix above is what keeps those distinct.
        "Resource".to_string()
    } else {
        id
    }
}

/// The resource-attribute spelling of a configured `DeletionPolicy` / `UpdateReplacePolicy`.
///
/// The configuration is modelled as an enum of `DELETE | RETAIN`, while the attribute it is
/// assigned to in a template takes that attribute's own documented values (`Delete` /
/// `Retain`), so the rendered template speaks the attribute's vocabulary rather than the
/// API's.
fn attribute_policy(configured: &str) -> Option<&'static str> {
    match configured {
        policy::DELETE => Some("Delete"),
        policy::RETAIN => Some("Retain"),
        _ => None,
    }
}

/// A template document as JSON text.
fn json_text(document: &Value) -> String {
    // Indented, because a template body is a document a caller reads and edits. Infallible
    // for a document built here (no non-string map key and no non-finite number), and a
    // body is never absent, so the empty template is the fallback.
    ::serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_string())
}

/// A template document as YAML text.
///
/// "You can write CloudFormation templates in JSON or YAML", so the YAML spelling is
/// emitted as YAML — through the same maintained YAML implementation this crate reads a
/// submitted body with — rather than handed back as its JSON spelling.
fn yaml_text(document: &Value) -> String {
    let node = yaml_node(document);
    let mut text = String::new();
    let mut emitter = ::yaml_rust2::YamlEmitter::new(&mut text);
    if emitter.dump(&node).is_err() {
        return "{}".to_string();
    }
    // The emitter opens with YAML's document-start marker; a template body carries the
    // document itself, so the marker line — and only it — is dropped.
    let body = match text.strip_prefix("---") {
        Some(rest) => rest.strip_prefix('\n').unwrap_or(rest),
        None => text.as_str(),
    };
    if body.is_empty() {
        "{}".to_string()
    } else {
        body.to_string()
    }
}

/// One template node as the YAML implementation's own tree.
fn yaml_node(value: &Value) -> ::yaml_rust2::Yaml {
    use ::yaml_rust2::Yaml;

    match value {
        Value::Null => Yaml::Null,
        Value::Bool(flag) => Yaml::Boolean(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Yaml::Integer(integer),
            None => Yaml::Real(number.to_string()),
        },
        Value::String(text) => Yaml::String(text.clone()),
        Value::Array(items) => Yaml::Array(items.iter().map(yaml_node).collect()),
        Value::Object(members) => {
            let mut mapping = ::yaml_rust2::yaml::Hash::new();
            for (key, member) in members {
                mapping.insert(Yaml::String(key.clone()), yaml_node(member));
            }
            Yaml::Hash(mapping)
        }
    }
}
