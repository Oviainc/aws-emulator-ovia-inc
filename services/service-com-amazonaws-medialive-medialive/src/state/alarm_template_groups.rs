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

use super::{template_group_token_key, ComAmazonawsMedialiveMedialiveState};

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
/// templates that exist, never carried on the group.
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
        let id = FIRST_ALARM_TEMPLATE_GROUP_ID + self.next_alarm_template_group_id;
        self.next_alarm_template_group_id += 1;
        id.to_string()
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
        if self.alarm_template_groups.len() >= MAX_ALARM_TEMPLATE_GROUPS {
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
/// `GetCloudWatchAlarmTemplateGroupOutput`).
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
