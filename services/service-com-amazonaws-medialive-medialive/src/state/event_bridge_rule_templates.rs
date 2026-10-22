// SPDX-License-Identifier: CC0-1.0

//! The eventbridge rule TEMPLATE slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, its refusal, the
//! store's methods and the output projection — in the shape the
//! `alarm_template_groups` slice beside it already uses, so the slice can be
//! read, and merged, on its own. The store itself is the
//! `event_bridge_rule_templates` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module, and the
//! per-region idempotency key is the parent's `template_group_token_key`, shared
//! with every monitor template resource.
//!
//! A template belongs to an eventbridge rule template GROUP: the create resolves
//! the request's `GroupIdentifier` ("Can be either be its id or current name")
//! against the group store and records the group's ID, so `GroupId` is read back
//! out of state rather than echoed from the request.

use std::collections::HashMap;
use std::time::SystemTime;

use super::{template_group_token_key, ComAmazonawsMedialiveMedialiveState};

/// The id the first eventbridge rule template of a process gets.
///
/// `EventBridgeRuleTemplateSummary$Id` is "An eventbridge rule template's id. AWS
/// provided templates have ids that start with `aws-`" and targets
/// `__stringMin7Max11PatternAws097` — seven characters at minimum, eleven at
/// most, i.e. seven digits on their own or `aws-` plus seven digits. A template
/// this emulator mints is caller-created, never AWS-provided, so it gets the bare
/// seven-digit form, minted from a counter so the `BTreeMap` key order equals the
/// mint order. A range of its own keeps the id spaces of the service's resource
/// types apart, as the group and SdiSource ranges do.
const FIRST_EVENT_BRIDGE_RULE_TEMPLATE_ID: u64 = 6_000_000;

/// The most eventbridge rule templates this emulator stores, across every region
/// this process has served.
///
/// A ceiling on the whole store, not a per-region or per-account quota of AWS's:
/// it exists so a create loop cannot grow the emulator's heap without bound,
/// which is why the refusal it produces names no region.
/// `CreateEventBridgeRuleTemplate` models `TooManyRequestsException`, which is
/// that refusal once the store is full.
pub const MAX_EVENT_BRIDGE_RULE_TEMPLATES: usize = 100;

/// The most `RequestId` entries kept for `CreateEventBridgeRuleTemplate`
/// idempotency — the same bound, for the same reason, as
/// [`super::MAX_IDEMPOTENCY_TOKENS`] on channels: a token is only useful while
/// the template it created can still be returned.
pub const MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS: usize = MAX_EVENT_BRIDGE_RULE_TEMPLATES;

/// The most `EventTargets` one template may carry.
///
/// From the model: `EventBridgeRuleTemplateSummary$EventTargetCount` is "The
/// number of targets configured to send matching events" and targets
/// `__integerMax5` (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateSummary")`), so a template the service can
/// report has at most five targets.
pub const MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS: usize = 5;

/// The longest target `Arn` a template's `EventTargets` entry may carry, from
/// `EventBridgeRuleTemplateTarget$Arn`'s target shape
/// `__stringMin1Max2048PatternArn`.
pub const MAX_EVENT_TARGET_ARN_LEN: usize = 2048;

/// Every `EventBridgeRuleTemplateEventType` the model declares, spelled exactly
/// as it declares them (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateEventType")`).
///
/// "The type of event to match with the rule" is a closed enum, so a value
/// outside this list names no event the service can match and is refused.
pub const EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES: [&str; 13] = [
    "MEDIALIVE_MULTIPLEX_ALERT",
    "MEDIALIVE_MULTIPLEX_STATE_CHANGE",
    "MEDIALIVE_CHANNEL_ALERT",
    "MEDIALIVE_CHANNEL_INPUT_CHANGE",
    "MEDIALIVE_CHANNEL_STATE_CHANGE",
    "MEDIAPACKAGE_INPUT_NOTIFICATION",
    "MEDIAPACKAGE_KEY_PROVIDER_NOTIFICATION",
    "MEDIAPACKAGE_HARVEST_JOB_NOTIFICATION",
    "SIGNAL_MAP_ACTIVE_ALARM",
    "MEDIACONNECT_ALERT",
    "MEDIACONNECT_SOURCE_HEALTH",
    "MEDIACONNECT_OUTPUT_HEALTH",
    "MEDIACONNECT_FLOW_STATUS_CHANGE",
];

/// Whether `event_type` is one of the values
/// [`EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES`] lists.
pub fn is_event_type(event_type: &str) -> bool {
    EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES.contains(&event_type)
}

/// The AWS service segments an `EventTargets` entry's ARN may name:
/// `EventBridgeRuleTemplateTarget$Arn` is documented "Target ARNs must be either
/// an SNS topic or CloudWatch log group" (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateTarget")`) — `sns` for a topic and `logs` for a
/// CloudWatch Logs log group.
const EVENT_TARGET_SERVICES: [&str; 2] = ["sns", "logs"];

/// Whether `arn` is an ARN this template may send matching events to: an ARN in
/// shape ([`super::is_resource_arn`], one definition of that shape for the whole
/// service) naming one of [`EVENT_TARGET_SERVICES`].
pub fn is_event_target_arn(arn: &str) -> bool {
    if !super::is_resource_arn(arn) {
        return false;
    }
    arn.split(':')
        .nth(2)
        .is_some_and(|service| EVENT_TARGET_SERVICES.contains(&service))
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_event_bridge_rule_template`]
/// stored nothing. Each variant is one modelled refusal of
/// `CreateEventBridgeRuleTemplate`.
#[derive(Debug)]
pub enum EventBridgeRuleTemplateRefusal {
    /// No eventbridge rule template group in the request's region has that id or
    /// name, so there is no group to create the template in. Carries the
    /// identifier that resolved to nothing.
    GroupNotFound { identifier: String },
    /// A template in the same region already carries that name, which `Name` is
    /// documented to make unique there. Carries the name asked for and the id of
    /// the template that holds it.
    NameTaken { id: String, name: String },
    /// The store already holds [`MAX_EVENT_BRIDGE_RULE_TEMPLATES`] templates.
    StoreFull,
}

/// One created eventbridge rule template, as `CreateEventBridgeRuleTemplate`
/// stored it.
///
/// `EventTargetCount` is NOT stored: `EventBridgeRuleTemplateSummary` documents
/// it as "The number of targets configured to send matching events", so it is
/// derived from [`Self::event_targets`] on every read. Neither is the group's
/// name: the template records the group's ID, so renaming the group cannot leave
/// the template pointing at a name nothing holds.
#[derive(Debug, Clone)]
pub struct EventBridgeRuleTemplateRecord {
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A template belongs to one
    /// region: `Name` is documented as unique "within the scope of a resource
    /// type in a specific region", and a template is only visible to requests
    /// signed for the region that created it.
    pub region: String,
    /// The id of the group this template was created in, resolved from the
    /// request's `GroupIdentifier`.
    pub group_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The `EventType` the request supplied, one of the
    /// `EventBridgeRuleTemplateEventType` values the model declares (the
    /// boundary validator refuses any other).
    pub event_type: String,
    /// The target ARNs, in the order the request listed them —
    /// `EventBridgeRuleTemplateTarget` carries exactly one member, `Arn`
    /// ("Target ARNs must be either an SNS topic or CloudWatch log group").
    pub event_targets: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

/// What one create asks the store to record, so the insert below takes one
/// argument per member rather than seven positional ones.
#[derive(Debug, Clone, Default)]
pub struct NewEventBridgeRuleTemplate {
    pub name: String,
    pub event_type: String,
    pub event_targets: Vec<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The next eventbridge rule template id: seven decimal digits, which is the
    /// bare (not AWS-provided) form of `__stringMin7Max11PatternAws097`.
    fn mint_event_bridge_rule_template_id(&mut self) -> String {
        let id = FIRST_EVENT_BRIDGE_RULE_TEMPLATE_ID + self.next_event_bridge_rule_template_id;
        self.next_event_bridge_rule_template_id += 1;
        id.to_string()
    }

    /// Store an eventbridge rule template in `region`, inside the group
    /// `group_identifier` names, and answer its minted id — or refuse.
    ///
    /// The group resolution, the name-uniqueness rule, the store's cap, the mint
    /// and the insert live together here, so a handler cannot check one of them
    /// and write past another; the handler's job is only to turn an
    /// [`EventBridgeRuleTemplateRefusal`] into the operation's own modelled
    /// error.
    pub fn insert_event_bridge_rule_template(
        &mut self,
        region: &str,
        group_identifier: &str,
        template: NewEventBridgeRuleTemplate,
        now: SystemTime,
    ) -> Result<String, EventBridgeRuleTemplateRefusal> {
        let Some(group) = self.event_bridge_rule_template_group(region, group_identifier) else {
            return Err(EventBridgeRuleTemplateRefusal::GroupNotFound {
                identifier: group_identifier.to_string(),
            });
        };
        let group_id = group.id.clone();

        if let Some(taken) = self.event_bridge_rule_template_named(region, &template.name) {
            return Err(EventBridgeRuleTemplateRefusal::NameTaken {
                id: taken.id.clone(),
                name: template.name,
            });
        }
        if self.event_bridge_rule_templates.len() >= MAX_EVENT_BRIDGE_RULE_TEMPLATES {
            return Err(EventBridgeRuleTemplateRefusal::StoreFull);
        }

        let id = self.mint_event_bridge_rule_template_id();
        let arn = ::engine::emulator_arn(
            "medialive",
            region,
            &format!("eventbridge-rule-template:{id}"),
        );
        // A template is ready the moment it exists — the model gives it no status
        // member and no transient state — so the clock owes it no transition, and
        // nothing has modified it since it was created.
        self.event_bridge_rule_templates.insert(
            id.clone(),
            EventBridgeRuleTemplateRecord {
                id: id.clone(),
                arn,
                region: region.to_string(),
                group_id,
                name: template.name,
                description: template.description,
                event_type: template.event_type,
                event_targets: template.event_targets,
                tags: template.tags,
                created_at: now,
                modified_at: now,
            },
        );
        Ok(id)
    }

    /// The template `identifier` names in `region`, resolving it as the
    /// template's id or its current name — the two forms
    /// `GetEventBridgeRuleTemplateRequest$Identifier` documents ("Can be either
    /// be its id or current name"). Templates in other regions are not visible: a
    /// template belongs to the region it was created in.
    pub fn event_bridge_rule_template(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&EventBridgeRuleTemplateRecord> {
        self.event_bridge_rule_templates
            .get(identifier)
            .filter(|template| template.region == region)
            .or_else(|| self.event_bridge_rule_template_named(region, identifier))
    }

    /// The template called `name` in `region`, if one exists. `Name` is
    /// documented as unique "within the scope of a resource type in a specific
    /// region", so at most one template answers — whichever group holds it, since
    /// the scope the documentation names is the resource type in the region, not
    /// one group of it.
    fn event_bridge_rule_template_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&EventBridgeRuleTemplateRecord> {
        self.event_bridge_rule_templates
            .values()
            .find(|template| template.region == region && template.name == name)
    }

    /// The ids of the templates stored in the eventbridge rule template group
    /// `group_id` — the templates `DeleteEventBridgeRuleTemplateGroup` documents
    /// must be "moved to another group or deleted" before the group goes.
    ///
    /// Derived from the templates that exist, which is also what
    /// `EventBridgeRuleTemplateGroupSummary$TemplateCount` ("The number of
    /// templates in a group") is read from: a group carries no list of its own.
    pub fn event_bridge_rule_template_ids_in_group(&self, group_id: &str) -> Vec<String> {
        self.event_bridge_rule_templates
            .values()
            .filter(|template| template.group_id == group_id)
            .map(|template| template.id.clone())
            .collect()
    }

    /// Remove the eventbridge rule template `identifier` names in `region`,
    /// answering the record that was removed, or `None` when no template in that
    /// region answers to it.
    ///
    /// `DeleteEventBridgeRuleTemplate` is documented "Deletes an eventbridge rule
    /// template", with no precondition of its own — hence no refusal type: a
    /// template that exists in the request's region goes, and the miss is the
    /// only thing a handler has to refuse. Once it is gone every later read of
    /// either of its identifiers misses, its name is free again and the group
    /// that held it no longer counts it.
    pub fn delete_event_bridge_rule_template(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<EventBridgeRuleTemplateRecord> {
        let id = self
            .event_bridge_rule_template(region, identifier)?
            .id
            .clone();
        let removed = self.event_bridge_rule_templates.remove(&id)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted template.
        self.event_bridge_rule_template_tokens.forget(&id);
        Some(removed)
    }

    /// Record `token`, as presented in `region`, as having created the template
    /// `template_id`, evicting the oldest token first if that would push the map
    /// past [`MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS`].
    pub fn record_event_bridge_rule_template_token(
        &mut self,
        region: &str,
        token: &str,
        template_id: &str,
    ) {
        self.event_bridge_rule_template_tokens.record(
            template_group_token_key(region, token),
            template_id,
            MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS,
        );
    }

    /// The template a `RequestId` presented in `region` created, if that template
    /// still exists and belongs to that region.
    ///
    /// The region is part of the key AND re-checked on the record, so a token can
    /// only ever surface a template of the region the request was signed for.
    pub fn event_bridge_rule_template_for_token(
        &self,
        region: &str,
        request_id: &str,
    ) -> Option<&EventBridgeRuleTemplateRecord> {
        let id = self
            .event_bridge_rule_template_tokens
            .created(&template_group_token_key(region, request_id))?;
        self.event_bridge_rule_templates
            .get(id)
            .filter(|template| template.region == region)
    }
}

/// Project a stored [`EventBridgeRuleTemplateRecord`] onto one of the
/// structurally identical per-operation template output shapes
/// (`CreateEventBridgeRuleTemplateOutput` and
/// `GetEventBridgeRuleTemplateOutput` today;
/// `UpdateEventBridgeRuleTemplateOutput` when its own issue lands).
///
/// One macro rather than a hand-written copy per operation, so a create and the
/// read that follows it cannot report the same stored template differently.
/// `Tags` and `EventTargets` are reported only when the template carries them: an
/// empty map or list would claim a value the caller never supplied.
macro_rules! event_bridge_rule_template_output {
    ($out:ident, $template:expr) => {{
        let template: &$crate::state::EventBridgeRuleTemplateRecord = $template;
        $out {
            arn: Some(template.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(template.created_at)),
            description: template.description.clone(),
            event_targets: if template.event_targets.is_empty() {
                None
            } else {
                Some(
                    template
                        .event_targets
                        .iter()
                        .map(|arn| $crate::ops::types::EventBridgeRuleTemplateTarget {
                            arn: arn.clone(),
                        })
                        .collect(),
                )
            },
            event_type: Some(template.event_type.clone()),
            group_id: Some(template.group_id.clone()),
            id: Some(template.id.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from(template.modified_at)),
            name: Some(template.name.clone()),
            tags: if template.tags.is_empty() {
                None
            } else {
                Some(template.tags.clone())
            },
        }
    }};
}

pub(crate) use event_bridge_rule_template_output;
