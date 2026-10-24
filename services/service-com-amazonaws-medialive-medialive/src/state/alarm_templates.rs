// SPDX-License-Identifier: CC0-1.0

//! The cloudwatch alarm TEMPLATE slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, its refusal, the
//! store's methods and the output projection — in the shape the
//! `alarm_template_groups` and `event_bridge_rule_templates` slices beside it
//! already use, so the slice can be read, and merged, on its own. The store
//! itself is the `alarm_templates` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module, and the
//! per-region idempotency key is the parent's `template_group_token_key`, shared
//! with every monitor template resource.
//!
//! A template belongs to a cloudwatch alarm template GROUP: the create resolves
//! the request's `GroupIdentifier` ("Can be either be its id or current name")
//! against the group store and records the group's ID, so `GroupId` is read back
//! out of state rather than echoed from the request.

use std::collections::HashMap;
use std::time::SystemTime;

use super::{template_group_token_key, ComAmazonawsMedialiveMedialiveState};

/// The id the first cloudwatch alarm template of a process gets.
///
/// `CloudWatchAlarmTemplateSummary$Id` is "A cloudwatch alarm template's id. AWS
/// provided templates have ids that start with `aws-`" and targets
/// `__stringMin7Max11PatternAws097` — seven characters at minimum, eleven at
/// most, i.e. seven digits on their own or `aws-` plus seven digits. A template
/// this emulator mints is caller-created, never AWS-provided, so it gets the bare
/// seven-digit form, minted from a counter so the `BTreeMap` key order equals the
/// mint order. A range of its own keeps the id spaces of the service's resource
/// types apart, as the group and template ranges beside it do.
const FIRST_ALARM_TEMPLATE_ID: u64 = 2_000_000;

/// The most cloudwatch alarm templates this emulator stores, across every region
/// this process has served.
///
/// A ceiling on the whole store, not a per-region or per-account quota of AWS's:
/// it exists so a create loop cannot grow the emulator's heap without bound,
/// which is why the refusal it produces names no region.
/// `CreateCloudWatchAlarmTemplate` models `TooManyRequestsException`, which is
/// that refusal once the store is full.
pub const MAX_ALARM_TEMPLATES: usize = 100;

/// The most `RequestId` entries kept for `CreateCloudWatchAlarmTemplate`
/// idempotency — the same bound, for the same reason, as
/// [`super::MAX_IDEMPOTENCY_TOKENS`] on channels: a token is only useful while
/// the template it created can still be returned.
pub const MAX_ALARM_TEMPLATE_TOKENS: usize = MAX_ALARM_TEMPLATES;

/// The longest `MetricName` a template may carry, from its target shape
/// `__stringMax64` ("The name of the metric associated with the alarm").
pub const MAX_METRIC_NAME_LEN: usize = 64;

/// The bounds `Period` carries, from its target shape `__integerMin10Max86400`
/// ("The period, in seconds, over which the specified statistic is applied").
pub const MIN_ALARM_PERIOD_SECONDS: i32 = 10;
pub const MAX_ALARM_PERIOD_SECONDS: i32 = 86_400;

/// The minimum `EvaluationPeriods` and `DatapointsToAlarm` carry, from their
/// shared target shape `__integerMin1`: a template compares at least one period
/// and, when it names one at all, needs at least one breaching datapoint.
pub const MIN_ALARM_DATAPOINTS: i32 = 1;

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_alarm_template`] stored
/// nothing. Each variant is one modelled refusal of
/// `CreateCloudWatchAlarmTemplate`.
#[derive(Debug)]
pub enum AlarmTemplateRefusal {
    /// No cloudwatch alarm template group in the request's region has that id or
    /// name, so there is no group to create the template in. Carries the
    /// identifier that resolved to nothing.
    GroupNotFound { identifier: String },
    /// A template in the same region already carries that name, which `Name` is
    /// documented to make unique there. Carries the name asked for and the id of
    /// the template that holds it.
    NameTaken { id: String, name: String },
    /// The store already holds [`MAX_ALARM_TEMPLATES`] templates.
    StoreFull,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::update_alarm_template`] changed
/// nothing. Each variant is one modelled refusal of
/// `UpdateCloudWatchAlarmTemplate`.
#[derive(Debug)]
pub enum AlarmTemplateUpdateRefusal {
    /// No template in the request's region has that id or name, so there is no
    /// template to change. Carries the identifier that resolved to nothing.
    NotFound { identifier: String },
    /// No cloudwatch alarm template group in the request's region has that id or
    /// name, so there is no group to move the template into. Carries the
    /// identifier that resolved to nothing.
    GroupNotFound { identifier: String },
    /// Another template in the same region already carries the name asked for,
    /// which `Name` is documented to make unique there. Carries the name and the
    /// id of the template that holds it.
    NameTaken { id: String, name: String },
}

/// What one update asks the store to change: a member the request carried is
/// `Some` and replaces the stored one, a member it omitted is `None` and leaves
/// that setting as it was.
#[derive(Debug, Clone, Default)]
pub struct CloudWatchAlarmTemplateChanges {
    /// The identifier of the group to move the template into ("Can be either be
    /// its id or current name"), resolved to a group id before anything is
    /// written.
    pub group_identifier: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub comparison_operator: Option<String>,
    pub datapoints_to_alarm: Option<i32>,
    pub evaluation_periods: Option<i32>,
    pub metric_name: Option<String>,
    pub period: Option<i32>,
    pub statistic: Option<String>,
    pub target_resource_type: Option<String>,
    pub threshold: Option<f64>,
    pub treat_missing_data: Option<String>,
}

/// One created cloudwatch alarm template, as `CreateCloudWatchAlarmTemplate`
/// stored it.
///
/// Every member the request supplied is kept verbatim; the group's NAME is not,
/// because the template records the group's ID, so renaming the group cannot
/// leave the template pointing at a name nothing holds.
#[derive(Debug, Clone)]
pub struct CloudWatchAlarmTemplateRecord {
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
    /// The `CloudWatchAlarmTemplateComparisonOperator` the request supplied (the
    /// handler refuses any value the model does not declare).
    pub comparison_operator: String,
    pub datapoints_to_alarm: Option<i32>,
    pub evaluation_periods: i32,
    pub metric_name: String,
    pub period: i32,
    /// The `CloudWatchAlarmTemplateStatistic` the request supplied.
    pub statistic: String,
    /// The `CloudWatchAlarmTemplateTargetResourceType` the request supplied.
    pub target_resource_type: String,
    pub threshold: f64,
    /// The `CloudWatchAlarmTemplateTreatMissingData` the request supplied.
    pub treat_missing_data: String,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

/// What one create asks the store to record, so the insert below takes one
/// argument rather than a dozen positional ones.
#[derive(Debug, Clone, Default)]
pub struct NewCloudWatchAlarmTemplate {
    pub name: String,
    pub description: Option<String>,
    pub comparison_operator: String,
    pub datapoints_to_alarm: Option<i32>,
    pub evaluation_periods: i32,
    pub metric_name: String,
    pub period: i32,
    pub statistic: String,
    pub target_resource_type: String,
    pub threshold: f64,
    pub treat_missing_data: String,
    pub tags: HashMap<String, String>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The next cloudwatch alarm template id: seven decimal digits, which is the
    /// bare (not AWS-provided) form of `__stringMin7Max11PatternAws097`.
    fn mint_alarm_template_id(&mut self) -> String {
        let id = FIRST_ALARM_TEMPLATE_ID + self.next_alarm_template_id;
        self.next_alarm_template_id += 1;
        id.to_string()
    }

    /// Store a cloudwatch alarm template in `region`, inside the group
    /// `group_identifier` names, and answer its minted id — or refuse.
    ///
    /// The group resolution, the name-uniqueness rule, the store's cap, the mint
    /// and the insert live together here, so a handler cannot check one of them
    /// and write past another; the handler's job is only to turn an
    /// [`AlarmTemplateRefusal`] into the operation's own modelled error.
    pub fn insert_alarm_template(
        &mut self,
        region: &str,
        group_identifier: &str,
        template: NewCloudWatchAlarmTemplate,
        now: SystemTime,
    ) -> Result<String, AlarmTemplateRefusal> {
        let Some(group) = self.alarm_template_group(region, group_identifier) else {
            return Err(AlarmTemplateRefusal::GroupNotFound {
                identifier: group_identifier.to_string(),
            });
        };
        let group_id = group.id.clone();

        if let Some(taken) = self.alarm_template_named(region, &template.name) {
            return Err(AlarmTemplateRefusal::NameTaken {
                id: taken.id.clone(),
                name: template.name,
            });
        }
        if self.alarm_templates.len() >= MAX_ALARM_TEMPLATES {
            return Err(AlarmTemplateRefusal::StoreFull);
        }

        let id = self.mint_alarm_template_id();
        let arn = ::engine::emulator_arn(
            "medialive",
            region,
            &format!("cloudwatch-alarm-template:{id}"),
        );
        // A template is ready the moment it exists — the model gives it no status
        // member and no transient state — so the clock owes it no transition, and
        // nothing has modified it since it was created.
        self.alarm_templates.insert(
            id.clone(),
            CloudWatchAlarmTemplateRecord {
                id: id.clone(),
                arn,
                region: region.to_string(),
                group_id,
                name: template.name,
                description: template.description,
                comparison_operator: template.comparison_operator,
                datapoints_to_alarm: template.datapoints_to_alarm,
                evaluation_periods: template.evaluation_periods,
                metric_name: template.metric_name,
                period: template.period,
                statistic: template.statistic,
                target_resource_type: template.target_resource_type,
                threshold: template.threshold,
                treat_missing_data: template.treat_missing_data,
                tags: template.tags,
                created_at: now,
                modified_at: now,
            },
        );
        Ok(id)
    }

    /// Apply `changes` to the cloudwatch alarm template `identifier` names in
    /// `region` and answer its id — or refuse, changing nothing.
    ///
    /// "Updates the specified cloudwatch alarm template": a member `changes`
    /// carries replaces the stored one and a member it omits leaves that setting
    /// as it was, so an update is a partial write rather than a replacement of
    /// the whole record. The id, the ARN, the region, the tags and `CreatedAt`
    /// are the template's identity and are never rewritten; `ModifiedAt` moves
    /// to `now` because this call is what modified it.
    ///
    /// The identifier resolution, the group resolution, the name-uniqueness rule
    /// and the write live together here, so a handler cannot check one of them
    /// and write past another; the handler's job is only to turn an
    /// [`AlarmTemplateUpdateRefusal`] into the operation's own modelled error.
    pub fn update_alarm_template(
        &mut self,
        region: &str,
        identifier: &str,
        changes: CloudWatchAlarmTemplateChanges,
        now: SystemTime,
    ) -> Result<String, AlarmTemplateUpdateRefusal> {
        let Some(template) = self.alarm_template(region, identifier) else {
            return Err(AlarmTemplateUpdateRefusal::NotFound {
                identifier: identifier.to_string(),
            });
        };
        let id = template.id.clone();

        // A `GroupIdentifier` is resolved against the groups of the request's
        // region, exactly as the create resolves it, so the stored `GroupId` is
        // a group that exists rather than whatever the request spelled.
        let group_id = match changes.group_identifier.as_deref() {
            Some(group_identifier) => {
                let Some(group) = self.alarm_template_group(region, group_identifier) else {
                    return Err(AlarmTemplateUpdateRefusal::GroupNotFound {
                        identifier: group_identifier.to_string(),
                    });
                };
                Some(group.id.clone())
            }
            None => None,
        };

        // A rename collides only with ANOTHER template: a request that renames a
        // template to the name it already carries asks for no change and is not
        // a conflict.
        if let Some(name) = changes.name.as_deref() {
            if let Some(taken) = self
                .alarm_template_named(region, name)
                .filter(|taken| taken.id != id)
            {
                return Err(AlarmTemplateUpdateRefusal::NameTaken {
                    id: taken.id.clone(),
                    name: name.to_string(),
                });
            }
        }

        let Some(template) = self.alarm_templates.get_mut(&id) else {
            return Err(AlarmTemplateUpdateRefusal::NotFound {
                identifier: identifier.to_string(),
            });
        };
        if let Some(group_id) = group_id {
            template.group_id = group_id;
        }
        if let Some(name) = changes.name {
            template.name = name;
        }
        if let Some(description) = changes.description {
            template.description = Some(description);
        }
        if let Some(comparison_operator) = changes.comparison_operator {
            template.comparison_operator = comparison_operator;
        }
        if let Some(datapoints_to_alarm) = changes.datapoints_to_alarm {
            template.datapoints_to_alarm = Some(datapoints_to_alarm);
        }
        if let Some(evaluation_periods) = changes.evaluation_periods {
            template.evaluation_periods = evaluation_periods;
        }
        if let Some(metric_name) = changes.metric_name {
            template.metric_name = metric_name;
        }
        if let Some(period) = changes.period {
            template.period = period;
        }
        if let Some(statistic) = changes.statistic {
            template.statistic = statistic;
        }
        if let Some(target_resource_type) = changes.target_resource_type {
            template.target_resource_type = target_resource_type;
        }
        if let Some(threshold) = changes.threshold {
            template.threshold = threshold;
        }
        if let Some(treat_missing_data) = changes.treat_missing_data {
            template.treat_missing_data = treat_missing_data;
        }
        template.modified_at = now;
        Ok(id)
    }

    /// The template `identifier` names in `region`, resolving it as the
    /// template's id or its current name — the two forms
    /// `GetCloudWatchAlarmTemplateRequest$Identifier` documents ("Can be either
    /// be its id or current name"). Templates in other regions are not visible: a
    /// template belongs to the region it was created in.
    pub fn alarm_template(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&CloudWatchAlarmTemplateRecord> {
        self.alarm_templates
            .get(identifier)
            .filter(|template| template.region == region)
            .or_else(|| self.alarm_template_named(region, identifier))
    }

    /// The template called `name` in `region`, if one exists. `Name` is
    /// documented as unique "within the scope of a resource type in a specific
    /// region", so at most one template answers — whichever group holds it, since
    /// the scope the documentation names is the resource type in the region, not
    /// one group of it.
    fn alarm_template_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&CloudWatchAlarmTemplateRecord> {
        self.alarm_templates
            .values()
            .find(|template| template.region == region && template.name == name)
    }

    /// The ids of the templates stored in the cloudwatch alarm template group
    /// `group_id` — the templates `DeleteCloudWatchAlarmTemplateGroup` documents
    /// must be "moved to another group or deleted" before the group goes.
    ///
    /// Derived from the templates that exist, which is also what
    /// `CloudWatchAlarmTemplateGroupSummary$TemplateCount` ("The number of
    /// templates in a group") is read from: a group carries no list of its own.
    pub fn alarm_template_ids_in_group(&self, group_id: &str) -> Vec<String> {
        self.alarm_templates
            .values()
            .filter(|template| template.group_id == group_id)
            .map(|template| template.id.clone())
            .collect()
    }

    /// Remove the cloudwatch alarm template `identifier` names in `region`,
    /// answering the record that was removed, or `None` when no template in that
    /// region answers to it.
    ///
    /// `DeleteCloudWatchAlarmTemplate` is documented "Deletes a cloudwatch alarm
    /// template", with no precondition of its own — hence no refusal type, as on
    /// the eventbridge rule template beside it: a template that exists in the
    /// request's region goes, and the miss is the only thing a handler has to
    /// refuse. Once it is gone every later read of either of its identifiers
    /// misses, its name is free again and the group that held it no longer counts
    /// it (so an emptied group becomes deletable).
    pub fn delete_alarm_template(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<CloudWatchAlarmTemplateRecord> {
        let id = self.alarm_template(region, identifier)?.id.clone();
        let removed = self.alarm_templates.remove(&id)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted template.
        self.alarm_template_tokens.forget(&id);
        Some(removed)
    }

    /// Record `token`, as presented in `region`, as having created the template
    /// `template_id`, evicting the oldest token first if that would push the map
    /// past [`MAX_ALARM_TEMPLATE_TOKENS`].
    pub fn record_alarm_template_token(&mut self, region: &str, token: &str, template_id: &str) {
        self.alarm_template_tokens.record(
            template_group_token_key(region, token),
            template_id,
            MAX_ALARM_TEMPLATE_TOKENS,
        );
    }

    /// The template a `RequestId` presented in `region` created, if that template
    /// still exists and belongs to that region.
    ///
    /// The region is part of the key AND re-checked on the record, so a token can
    /// only ever surface a template of the region the request was signed for.
    pub fn alarm_template_for_token(
        &self,
        region: &str,
        request_id: &str,
    ) -> Option<&CloudWatchAlarmTemplateRecord> {
        let id = self
            .alarm_template_tokens
            .created(&template_group_token_key(region, request_id))?;
        self.alarm_templates
            .get(id)
            .filter(|template| template.region == region)
    }
}

/// Project a stored [`CloudWatchAlarmTemplateRecord`] onto one of the
/// structurally identical per-operation template output shapes
/// (`CreateCloudWatchAlarmTemplateOutput`, `GetCloudWatchAlarmTemplateOutput`
/// and `UpdateCloudWatchAlarmTemplateOutput`).
///
/// One macro rather than a hand-written copy per operation, so a create and the
/// read that follows it cannot report the same stored template differently.
///
/// `Tags` reports the template's tag set WHATEVER it holds — an untagged template
/// reports the empty map, not an absent member — for the reason the shared
/// `template_group_output` states: the member is modelled as a `TagMap`, zero
/// tags is an empty map rather than an unknown one, and this service's other
/// resources already report it that way. `DatapointsToAlarm` and `Description`
/// keep their own rule: both are optional on the request, so a template created
/// without one reports it absent rather than inventing a value for it.
macro_rules! alarm_template_output {
    ($out:ident, $template:expr) => {{
        let template: &$crate::state::CloudWatchAlarmTemplateRecord = $template;
        $out {
            arn: Some(template.arn.clone()),
            comparison_operator: Some(template.comparison_operator.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(template.created_at)),
            datapoints_to_alarm: template.datapoints_to_alarm,
            description: template.description.clone(),
            evaluation_periods: Some(template.evaluation_periods),
            group_id: Some(template.group_id.clone()),
            id: Some(template.id.clone()),
            metric_name: Some(template.metric_name.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from(template.modified_at)),
            name: Some(template.name.clone()),
            period: Some(template.period),
            statistic: Some(template.statistic.clone()),
            tags: Some(template.tags.clone()),
            target_resource_type: Some(template.target_resource_type.clone()),
            threshold: Some(template.threshold),
            treat_missing_data: Some(template.treat_missing_data.clone()),
        }
    }};
}

pub(crate) use alarm_template_output;
