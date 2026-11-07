// SPDX-License-Identifier: CC0-1.0

//! The cloudwatch alarm template group slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, its refusal, the
//! store's methods and the output projection — so the slice can be read, and
//! merged, on its own. The store itself is the `alarm_template_groups` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module, and the
//! per-region idempotency key is the parent's `template_group_token_key`, shared
//! with the eventbridge rule template groups.

use std::collections::HashMap;
use std::time::SystemTime;

use super::{
    template_group_token_key, ComAmazonawsMedialiveMedialiveState, TemplateGroupDeleteRefusal,
};

/// The id the first cloudwatch alarm template group of a process gets.
///
/// `CloudWatchAlarmTemplateGroupSummary$Id` is "A cloudwatch alarm template
/// group's id. AWS provided template groups have ids that start with `aws-`", and
/// it targets `__stringMin7Max11PatternAws097` — seven characters at minimum,
/// eleven at most, i.e. seven digits on their own or `aws-` plus seven digits. A
/// group this emulator mints is caller-created, never AWS-provided, so it gets
/// the bare seven-digit form, minted from a counter so the `BTreeMap` key order
/// equals the mint order.
const FIRST_ALARM_TEMPLATE_GROUP_ID: u64 = 1_000_000;

/// The most cloudwatch alarm template groups this emulator stores, across every
/// region this process has served.
///
/// A ceiling on the whole store, not a per-region or per-account quota of AWS's:
/// it exists so a create loop cannot grow the emulator's heap without bound,
/// which is why the refusal it produces names no region.
/// `CreateCloudWatchAlarmTemplateGroup` models `TooManyRequestsException`, which
/// is that refusal once the store is full.
pub const MAX_ALARM_TEMPLATE_GROUPS: usize = 100;

/// The most `RequestId` entries kept for `CreateCloudWatchAlarmTemplateGroup`
/// idempotency — the same bound, for the same reason, as
/// [`super::MAX_IDEMPOTENCY_TOKENS`] on channels: a token is only useful while
/// the group it created can still be returned.
pub const MAX_ALARM_TEMPLATE_GROUP_TOKENS: usize = MAX_ALARM_TEMPLATE_GROUPS;

/// The `message` a not-found refusal carries when what the identifier failed to
/// resolve to is a resource of the CLOUDWATCH ALARM-TEMPLATE hierarchy — a
/// template group or one of the templates inside it: NONE, the member is left
/// unset.
///
/// `NotFoundException`'s single member `Message` (jsonName `message`) is
/// OPTIONAL and its documentation is a placeholder
/// (`smithy_query(query="shape",shape="NotFoundException")`), so the model
/// states no message rule and the member may legitimately be absent. The
/// recorded behaviour of this hierarchy is that it IS absent: the 404 answered
/// for a group identifier that resolves to nothing carries the status and the
/// error code and no `message` member at all, and so does the 404 answered for
/// a template identifier. So this is a `None` and not an empty string — an
/// empty string is a present member whose value is empty, which is a different
/// error document.
///
/// This is the same absence the sibling monitor hierarchy already answers with
/// ([`super::EVENT_BRIDGE_RULE_TEMPLATE_NOT_FOUND_MESSAGE`]), while the
/// resource families OUTSIDE the two template hierarchies keep composing their
/// own per-resource sentence with [`super::not_found_message`].
///
/// ONE constant, so every place that reports such a resource missing — its own
/// get/update/delete at both levels, the group resolution of the template
/// create, of the template update and of the two listings, and the signal-map
/// writes that resolve a group — cannot answer it two ways.
pub const CLOUD_WATCH_ALARM_TEMPLATE_NOT_FOUND_MESSAGE: Option<String> = None;

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_alarm_template_group`]
/// stored nothing. Each variant is one modelled refusal of
/// `CreateCloudWatchAlarmTemplateGroup`.
#[derive(Debug)]
pub enum AlarmTemplateGroupRefusal {
    /// A group in the same region already carries that name, which `Name` is
    /// documented to make unique there. Carries the name asked for and the id of
    /// the group that holds it.
    NameTaken { id: String, name: String },
    /// The store already holds [`MAX_ALARM_TEMPLATE_GROUPS`] groups.
    StoreFull,
}

/// One created cloudwatch alarm template group, as
/// `CreateCloudWatchAlarmTemplateGroup` stored it.
///
/// `TemplateCount` is NOT stored: `CloudWatchAlarmTemplateGroupSummary` documents
/// it as "The number of templates in a group", so it is derived from the
/// templates that exist
/// ([`ComAmazonawsMedialiveMedialiveState::alarm_template_ids_in_group`]), never
/// carried on the group.
#[derive(Debug, Clone)]
pub struct CloudWatchAlarmTemplateGroupRecord {
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A group belongs to one
    /// region: `Name` is documented as unique "within the scope of a resource
    /// type in a specific region", and a group is only visible to requests signed
    /// for the region that created it.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The next cloudwatch alarm template group id: seven decimal digits, which
    /// is the bare (not AWS-provided) form of `__stringMin7Max11PatternAws097`.
    fn mint_alarm_template_group_id(&mut self) -> String {
        (FIRST_ALARM_TEMPLATE_GROUP_ID + self.mints.next(|mints| &mut mints.alarm_template_group))
            .to_string()
    }

    /// Store a cloudwatch alarm template group in `region` and answer its minted
    /// id, or refuse.
    ///
    /// The name-uniqueness rule, the store's cap, the mint and the insert live
    /// together here, so a handler cannot check one of them and write past
    /// another; the handler's job is only to turn an
    /// [`AlarmTemplateGroupRefusal`] into the operation's own modelled error.
    pub fn insert_alarm_template_group(
        &mut self,
        region: &str,
        name: String,
        description: Option<String>,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Result<String, AlarmTemplateGroupRefusal> {
        if let Some(taken) = self.alarm_template_group_named(region, &name) {
            return Err(AlarmTemplateGroupRefusal::NameTaken {
                id: taken.id.clone(),
                name,
            });
        }
        if self.account_alarm_template_groups() >= MAX_ALARM_TEMPLATE_GROUPS {
            return Err(AlarmTemplateGroupRefusal::StoreFull);
        }

        let id = self.mint_alarm_template_group_id();
        let arn = ::engine::emulator_arn(
            "medialive",
            region,
            &format!("cloudwatch-alarm-template-group:{id}"),
        );
        // A group is ready the moment it exists — the model gives it no status
        // member and no transient state — so the clock owes it no transition, and
        // nothing has modified it since it was created.
        self.alarm_template_groups.insert(
            id.clone(),
            CloudWatchAlarmTemplateGroupRecord {
                id: id.clone(),
                arn,
                region: region.to_string(),
                name,
                description,
                tags,
                created_at: now,
                modified_at: now,
            },
        );
        Ok(id)
    }

    /// How many of the stored groups this ACCOUNT created — what
    /// [`MAX_ALARM_TEMPLATE_GROUPS`] bounds.
    ///
    /// The groups the SERVICE publishes are seeded into the same store (see
    /// [`super::managed_monitor_templates`]) and no create brought them there,
    /// so they hold none of the room a create loop could otherwise grow without
    /// bound.
    fn account_alarm_template_groups(&self) -> usize {
        self.alarm_template_groups
            .keys()
            .filter(|id| !super::is_published_id(id))
            .count()
    }

    /// The group `identifier` names in `region`, resolving it as the group's id
    /// or its current name — the two forms
    /// `GetCloudWatchAlarmTemplateGroupRequest$Identifier` documents ("Can be
    /// either be its id or current name"). Groups in other regions are not
    /// visible: a group belongs to the region it was created in.
    pub fn alarm_template_group(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&CloudWatchAlarmTemplateGroupRecord> {
        self.alarm_template_groups
            .get(identifier)
            .filter(|group| group.region == region)
            .or_else(|| self.alarm_template_group_named(region, identifier))
    }

    /// The cloudwatch alarm template groups stored in `region`, in the order a
    /// listing reports them ([`super::listing_position`]: the groups the SERVICE
    /// published first, then the account's own in mint order), beginning at the
    /// group `start` names when a `NextToken` carried a resume point.
    ///
    /// The store itself is private, so this is how a listing reads it — the same
    /// reading the listing of the TEMPLATES inside them already uses
    /// ([`ComAmazonawsMedialiveMedialiveState::alarm_templates_in`]): a scan from
    /// the resume point in that order, which is why a token naming a group
    /// deleted since the previous page resumes at the next surviving id instead
    /// of failing. Groups of other regions are invisible, for the reason
    /// [`Self::alarm_template_group`] states.
    pub fn alarm_template_groups_in<'a>(
        &'a self,
        region: &'a str,
        start: Option<&'a str>,
    ) -> impl Iterator<Item = &'a CloudWatchAlarmTemplateGroupRecord> + 'a {
        let mut ordered: Vec<&'a CloudWatchAlarmTemplateGroupRecord> = self
            .alarm_template_groups
            .values()
            .filter(|group| group.region == region)
            .collect();
        ordered.sort_by_key(|group| super::listing_position(&group.id));
        let from = start.map(super::listing_position);
        ordered
            .into_iter()
            .filter(move |group| from.is_none_or(|first| super::listing_position(&group.id) >= first))
    }

    /// Project a stored group onto the `CloudWatchAlarmTemplateGroupSummary` a
    /// listing reports.
    ///
    /// Every member the summary shares with the flat group shape
    /// ([`alarm_template_group_output`]) carries the stored value, so a listing
    /// and the `GetCloudWatchAlarmTemplateGroup` beside it cannot disagree about
    /// one group. Where the two shapes differ, the summary adds `TemplateCount` —
    /// "The number of templates in a group", which is derived from the templates
    /// that exist right now
    /// ([`ComAmazonawsMedialiveMedialiveState::alarm_template_ids_in_group`])
    /// rather than from anything recorded on the group, so it tracks every later
    /// template create and delete.
    pub fn alarm_template_group_summary(
        &self,
        group: &CloudWatchAlarmTemplateGroupRecord,
    ) -> crate::ops::types::CloudWatchAlarmTemplateGroupSummary {
        crate::ops::types::CloudWatchAlarmTemplateGroupSummary {
            arn: group.arn.clone(),
            created_at: ::aws_smithy_types::DateTime::from(group.created_at),
            description: group.description.clone(),
            id: group.id.clone(),
            modified_at: Some(::aws_smithy_types::DateTime::from(group.modified_at)),
            name: group.name.clone(),
            tags: Some(group.tags.clone()),
            template_count: i32::try_from(self.alarm_template_ids_in_group(&group.id).len())
                .unwrap_or(i32::MAX),
        }
    }

    /// Remove the cloudwatch alarm template group `identifier` names in
    /// `region`, answering the record that was removed — or refuse.
    ///
    /// `DeleteCloudWatchAlarmTemplateGroup` is documented "Deletes a cloudwatch
    /// alarm template group. You must detach this group from all signal maps and
    /// ensure its existing templates are moved to another group or deleted", so a
    /// group a signal map still names is refused and nothing is removed, and so is
    /// a group that still holds cloudwatch alarm TEMPLATES of its own. Once
    /// removed the group is GONE from the store, so every later read of either of
    /// its identifiers misses and its name is free again.
    ///
    /// A group the SERVICE publishes ([`super::managed_monitor_templates`]) is
    /// refused before either precondition is read: it is not the account's
    /// resource, so no state the account could change would make it deletable.
    ///
    /// The resolution, the precondition and the removal live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`TemplateGroupDeleteRefusal`] into the operation's own
    /// modelled error.
    pub fn delete_alarm_template_group(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Result<CloudWatchAlarmTemplateGroupRecord, TemplateGroupDeleteRefusal> {
        let id = self
            .alarm_template_group(region, identifier)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?
            .id
            .clone();
        if super::is_published_id(&id) {
            return Err(TemplateGroupDeleteRefusal::Published);
        }

        let signal_map_ids = self.signal_maps_using_alarm_template_group(&id);
        let template_ids = self.alarm_template_ids_in_group(&id);
        if !signal_map_ids.is_empty() || !template_ids.is_empty() {
            return Err(TemplateGroupDeleteRefusal::StillHeld {
                signal_map_ids,
                template_ids,
            });
        }

        let removed = self
            .alarm_template_groups
            .remove(&id)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted group.
        self.alarm_template_group_tokens.forget(&id);
        Ok(removed)
    }

    /// The group called `name` in `region`, if one exists. `Name` is documented
    /// as unique "within the scope of a resource type in a specific region", so
    /// at most one group answers.
    fn alarm_template_group_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&CloudWatchAlarmTemplateGroupRecord> {
        self.alarm_template_groups
            .values()
            .find(|group| group.region == region && group.name == name)
    }

    /// Apply the members an `UpdateCloudWatchAlarmTemplateGroup` request
    /// supplied onto the group `identifier` names in `region`, and answer that
    /// group's id — or refuse, changing nothing.
    ///
    /// "Updates the specified cloudwatch alarm template group": a member the
    /// request carries replaces the stored one, a member it omits leaves that
    /// value as it was, and `ModifiedAt` becomes the instant of the call, which
    /// is when the record was last written. The identifier resolves through
    /// [`Self::alarm_template_group`], so an update reaches its group by either
    /// form `Identifier` documents ("Can be either be its id or current name") —
    /// and a group the SERVICE publishes is refused, for the reason
    /// [`Self::delete_alarm_template_group`] states.
    pub fn update_alarm_template_group(
        &mut self,
        region: &str,
        identifier: &str,
        description: Option<String>,
        now: SystemTime,
    ) -> Result<String, super::PublishedWriteRefusal> {
        let id = self
            .alarm_template_group(region, identifier)
            .ok_or(super::PublishedWriteRefusal::NotFound)?
            .id
            .clone();
        if super::is_published_id(&id) {
            return Err(super::PublishedWriteRefusal::Published);
        }
        let group = self
            .alarm_template_groups
            .get_mut(&id)
            .ok_or(super::PublishedWriteRefusal::NotFound)?;
        if let Some(description) = description {
            group.description = Some(description);
        }
        group.modified_at = now;
        Ok(id)
    }

    /// Record `token`, as presented in `region`, as having created the group
    /// `group_id`, evicting the oldest token first if that would push the map
    /// past [`MAX_ALARM_TEMPLATE_GROUP_TOKENS`].
    pub fn record_alarm_template_group_token(
        &mut self,
        region: &str,
        token: &str,
        group_id: &str,
    ) {
        self.alarm_template_group_tokens.record(
            template_group_token_key(region, token),
            group_id,
            MAX_ALARM_TEMPLATE_GROUP_TOKENS,
        );
    }

    /// The group a `RequestId` presented in `region` created, if that group still
    /// exists and belongs to that region.
    ///
    /// The region is part of the key AND re-checked on the record, so a token can
    /// only ever surface a group of the region the request was signed for.
    pub fn alarm_template_group_for_token(
        &self,
        region: &str,
        request_id: &str,
    ) -> Option<&CloudWatchAlarmTemplateGroupRecord> {
        let id = self
            .alarm_template_group_tokens
            .created(&template_group_token_key(region, request_id))?;
        self.alarm_template_groups
            .get(id)
            .filter(|group| group.region == region)
    }
}

/// Project a stored [`CloudWatchAlarmTemplateGroupRecord`] onto one of the
/// structurally identical per-operation group output shapes
/// (`CreateCloudWatchAlarmTemplateGroupOutput`,
/// `GetCloudWatchAlarmTemplateGroupOutput`,
/// `UpdateCloudWatchAlarmTemplateGroupOutput`).
///
/// One macro rather than a hand-written copy per operation, so a create and the
/// read that follows it cannot report the same stored group differently; the
/// projection itself is the parent module's `template_group_output`, shared with
/// the eventbridge rule template groups beside it.
macro_rules! alarm_template_group_output {
    ($out:ident, $group:expr) => {
        $crate::state::template_group_output!(
            $out,
            $crate::state::CloudWatchAlarmTemplateGroupRecord,
            $group
        )
    };
}

pub(crate) use alarm_template_group_output;
