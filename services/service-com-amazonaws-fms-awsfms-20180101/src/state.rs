// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fms-awsfms-20180101 service.
//!
//! Firewall Manager's control plane is organisation-scoped: ONE administrator
//! account association, and a flat set of policies keyed by the `PolicyId` the
//! service mints. Both live here so every operation reads and writes the same
//! store; the helpers below are the bits of derivation several handlers share
//! (the request's Region, the policy ARN, the `PolicySummary` projection and the
//! pagination token codec), kept in one place so two handlers cannot disagree.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AppsListData, DiscoveredResource, Policy, PolicySummary, ProtocolsListData,
    ResourceNotFoundException, ResourceSet, Tag,
};

/// The Region an emulated service reports when the request carries no SigV4
/// credential scope to read one from (an unsigned request).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace / signing name of this service, per its Smithy model
/// (`aws.api#service` arnNamespace `fms`).
const ARN_NAMESPACE: &str = "fms";

/// The Firewall Manager administrator account association.
///
/// `role_status` is an `AccountRoleStatus` value (see
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`]); the association is
/// returned ready, so it is `READY` from the moment it is written.
///
/// `associated_at` is recorded here but not yet reported by any implemented
/// operation: it is what the later `ListAdminAccountsForOrganization` slice
/// reads, so it is stored now rather than being a derivation a reader computes.
///
/// `removal_due` is the instant a requested disassociation completes — see
/// [`ComAmazonawsFmsAwsfms20180101State::settle_admin_account`]. It is `None`
/// for an association no caller has asked to tear down.
#[derive(Debug, Clone)]
pub struct AdminAccountRecord {
    pub account_id: String,
    pub role_status: String,
    pub associated_at: SystemTime,
    pub removal_due: Option<SystemTime>,
}

impl AdminAccountRecord {
    /// Whether this association still makes its account the organisation's
    /// default administrator AT `now`.
    ///
    /// A `DELETED` role is the terminal state of a completed disassociation: the
    /// record is kept so `GetAdminAccount` can report what became of the account,
    /// but the administrator slot is free again. A disassociation whose
    /// completion instant ([`Self::removal_due`]) has passed is already the same
    /// thing — the transition is owed whether or not anybody has materialised it
    /// — so this predicate is derived from the CLOCK rather than from waiting for
    /// a writer, and no reader of the store can disagree with another about
    /// whether an administrator exists.
    /// [`ComAmazonawsFmsAwsfms20180101State::settle_admin_account_at`] is what
    /// persists the status that follows from it.
    ///
    /// It takes `now` so one request decides from ONE reading of a clock that is
    /// not monotonic: an operation that checks the association and then writes to
    /// it must not be able to see it active and then gone.
    pub fn is_active_at(&self, now: SystemTime) -> bool {
        self.role_status != ROLE_STATUS_DELETED && self.removal_due.is_none_or(|due| now < due)
    }

    /// [`Self::is_active_at`] at the current instant, for a caller that makes ONE
    /// decision from it.
    pub fn is_active(&self) -> bool {
        self.is_active_at(SystemTime::now())
    }

    /// Whether a disassociation of this association has been requested and has
    /// not completed yet.
    pub fn removal_pending(&self) -> bool {
        self.removal_due.is_some()
    }
}

/// `AccountRoleStatus` of an administrator account whose role exists and is
/// usable. One of [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_READY: &str = "READY";

/// `AccountRoleStatus` of an administrator account whose role deletion has been
/// requested (by `DisassociateAdminAccount`) and has not completed yet. One of
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_PENDING_DELETION: &str = "PENDING_DELETION";

/// `AccountRoleStatus` of an administrator account whose role deletion has
/// completed: the account is no longer the default administrator. One of
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_DELETED: &str = "DELETED";

/// How long a requested disassociation stays `PENDING_DELETION` before the role
/// is `DELETED`.
///
/// A CHOSEN span, not a documented one: neither the model nor the Firewall
/// Manager documentation states how long tearing the administrator role down
/// takes, only that the role passes through the non-terminal
/// `PENDING_DELETION` / `DELETING` statuses of `AccountRoleStatus` on the way.
/// It is deliberately non-zero, so a caller reading the association straight
/// after asking for the disassociation still observes that transient status,
/// and short enough that a caller that waits does not wait long.
pub const DISASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// The `CustomerPolicyScopeIdType` key under which a policy's `IncludeMap` /
/// `ExcludeMap` holds ACCOUNT IDS (the only other key, `ORG_UNIT`, holds
/// organizational-unit ids). One of
/// [`crate::ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES`].
pub const SCOPE_ID_TYPE_ACCOUNT: &str = "ACCOUNT";

/// The default `CustomerPolicyStatus` of a policy an administrator created: the
/// administrator can manage and delete it. One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";

/// The default `ResourceSetStatus` of a resource set an administrator created:
/// it is in the administrator's Region scope, so the administrator can manage
/// and delete it. One of [`crate::ops::types::RESOURCE_SET_STATUS_VALUES`].
pub const RESOURCE_SET_STATUS_ACTIVE: &str = "ACTIVE";

/// `ThirdPartyFirewallAssociationStatus` of a third-party firewall that was
/// never associated with this organisation. One of
/// [`crate::ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES`].
pub const THIRD_PARTY_FIREWALL_NOT_EXIST: &str = "NOT_EXIST";

/// `ThirdPartyFirewallAssociationStatus` of an association that is being torn
/// down: the transient status a `DisassociateThirdPartyFirewall` starts.
pub const THIRD_PARTY_FIREWALL_OFFBOARDING: &str = "OFFBOARDING";

/// `ThirdPartyFirewallAssociationStatus` of a torn-down association: the
/// terminal status the clock makes due after [`ASSOCIATION_SETTLE`].
pub const THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE: &str = "OFFBOARD_COMPLETE";

/// `MarketplaceSubscriptionOnboardingStatus` of an account with no Marketplace
/// subscription to a third-party firewall. One of
/// [`crate::ops::types::MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES`].
pub const MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION: &str = "NO_SUBSCRIPTION";

/// How long a third-party firewall association stays in a transient status
/// (`ONBOARDING` / `OFFBOARDING`) before the clock makes the terminal one due.
///
/// An EMULATOR-CHOSEN span, not an AWS-documented duration: Firewall Manager
/// publishes no settling time for an association. It is deliberately NON-ZERO
/// and inside the emulator's 250-750 ms band, so a caller that reads the status
/// straight after the call still observes the transient status, and
/// deliberately short so a test can wait it out. The due instant is STORED on
/// the record (see [`ThirdPartyFirewallRecord::settles_at`]); nothing sleeps
/// for it.
pub const ASSOCIATION_SETTLE: Duration = Duration::from_millis(400);

/// One stored Firewall Manager policy: the modelled `Policy` exactly as the
/// caller sent it (plus the identifiers the service minted), its ARN, the
/// current update token, and its tags.
///
/// `created_at` / `updated_at` are recorded but not yet reported: no modelled
/// member of `Policy` or `PolicySummary` carries them, and they are what the
/// later compliance-status slice (`ListComplianceStatus`, which reports a
/// policy's last evaluation) reads.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub policy_id: String,
    pub policy_arn: String,
    pub update_token: String,
    pub policy: Policy,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PolicyRecord {
    /// The stored policy as `GetPolicy`/`PutPolicy` return it: the caller's own
    /// members, with the service-owned `PolicyId` and `PolicyUpdateToken`
    /// filled in from the record so a read never reports a stale token.
    pub fn policy(&self) -> Policy {
        let mut policy = self.policy.clone();
        policy.policy_id = Some(self.policy_id.clone());
        policy.policy_update_token = Some(self.update_token.clone());
        policy
    }

    /// The `PolicySummary` projection `ListPolicies` reports, every member read
    /// off the stored policy (`SecurityServiceType` is the stored
    /// `SecurityServicePolicyData.Type`).
    pub fn summary(&self) -> PolicySummary {
        PolicySummary {
            policy_arn: Some(self.policy_arn.clone()),
            policy_id: Some(self.policy_id.clone()),
            policy_name: Some(self.policy.policy_name.clone()),
            resource_type: Some(self.policy.resource_type.clone()),
            security_service_type: Some(self.policy.security_service_policy_data.r#type.clone()),
            remediation_enabled: self.policy.remediation_enabled,
            delete_unused_fm_managed_resources: self.policy.delete_unused_fm_managed_resources,
            policy_status: self.policy.policy_status.clone(),
        }
    }
}

/// The Firewall Manager SNS notification channel: the topic Firewall Manager
/// records its activity to, and the role that lets it publish there. ONE per
/// account/Region, which is why it is an `Option` rather than a map.
#[derive(Debug, Clone)]
pub struct NotificationChannelRecord {
    pub sns_topic_arn: String,
    pub sns_role_name: String,
    /// When `PutNotificationChannel` last wrote the channel. Recorded because
    /// the channel is a configuration with a history, though no modelled member
    /// of `GetNotificationChannelResponse` reports it.
    pub configured_at: SystemTime,
}

/// One stored Firewall Manager resource set: the modelled `ResourceSet` exactly
/// as the caller sent it (plus the identifiers the service minted), its ARN, the
/// current update token, its tags, and the resource URIs associated with it.
///
/// `resources` is a `BTreeSet`: associating a URI twice is the same association
/// (the set is the membership, not a log of calls), and the total order makes a
/// later page of `ListResourceSetResources` resumable from a key.
#[derive(Debug, Clone)]
pub struct ResourceSetRecord {
    pub id: String,
    pub arn: String,
    pub update_token: String,
    pub resource_set: ResourceSet,
    pub tags: Vec<Tag>,
    pub resources: BTreeSet<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ResourceSetRecord {
    /// The stored resource set as `PutResourceSet` / `GetResourceSet` report it:
    /// the caller's own members, with the service-owned `Id`, `UpdateToken`,
    /// `LastUpdateTime` and `ResourceSetStatus` filled in from the record, so a
    /// read never reports a stale token or a status the caller invented.
    pub fn resource_set(&self) -> ResourceSet {
        let mut resource_set = self.resource_set.clone();
        resource_set.id = Some(self.id.clone());
        resource_set.update_token = Some(self.update_token.clone());
        resource_set.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        resource_set.resource_set_status = Some(RESOURCE_SET_STATUS_ACTIVE.to_string());
        resource_set
    }
}

/// One stored Firewall Manager applications list, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::apps_lists`] by the `ListId` the
/// service minted.
///
/// `default_list` distinguishes a Firewall Manager MANAGED list from one a
/// customer created: `GetAppsList`'s `DefaultList` member selects between the
/// two namespaces, so a customer list is not reported as a managed one. The
/// emulator seeds no managed lists.
#[derive(Debug, Clone)]
pub struct AppsListRecord {
    pub list_id: String,
    pub list_arn: String,
    pub update_token: String,
    pub default_list: bool,
    pub list: AppsListData,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AppsListRecord {
    /// The stored list as a reader reports it: the caller's own members with the
    /// service-owned `ListId`, `ListUpdateToken` and timestamps overlaid from
    /// the record.
    pub fn list(&self) -> AppsListData {
        let mut list = self.list.clone();
        list.list_id = Some(self.list_id.clone());
        list.list_update_token = Some(self.update_token.clone());
        list.create_time = Some(::aws_smithy_types::DateTime::from(self.created_at));
        list.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        list
    }
}

/// One stored Firewall Manager protocols list. The protocols-list twin of
/// [`AppsListRecord`], with the same `default_list` split between managed and
/// customer lists.
#[derive(Debug, Clone)]
pub struct ProtocolsListRecord {
    pub list_id: String,
    pub list_arn: String,
    pub update_token: String,
    pub default_list: bool,
    pub list: ProtocolsListData,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ProtocolsListRecord {
    /// The stored list as `GetProtocolsList` reports it: the caller's own
    /// members with the service-owned `ListId`, `ListUpdateToken` and timestamps
    /// overlaid from the record.
    pub fn list(&self) -> ProtocolsListData {
        let mut list = self.list.clone();
        list.list_id = Some(self.list_id.clone());
        list.list_update_token = Some(self.update_token.clone());
        list.create_time = Some(::aws_smithy_types::DateTime::from(self.created_at));
        list.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        list
    }
}

/// The association between this organisation and ONE third-party firewall
/// vendor, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewalls`] by the
/// `ThirdPartyFirewall` enum value.
///
/// `settles_at` / `settles_to` carry the transition the CLOCK owes: an
/// association that is onboarding or offboarding reaches its terminal status
/// [`ASSOCIATION_SETTLE`] after the call that started it, whether or not anybody
/// asks. Readers materialise it with [`Self::settle`]; nothing sleeps.
#[derive(Debug, Clone)]
pub struct ThirdPartyFirewallRecord {
    pub status: String,
    pub settles_to: Option<String>,
    pub settles_at: Option<SystemTime>,
    pub marketplace_onboarding_status: String,
}

impl ThirdPartyFirewallRecord {
    /// Apply the clock-due transition: once `settles_at` has passed, the status
    /// IS the terminal one, because the transition was owed at that instant even
    /// if nobody had called. Materialising it is the ONLY write a reporting
    /// operation makes.
    pub fn settle(&mut self, now: SystemTime) {
        let due = self
            .settles_at
            .is_some_and(|at| now >= at)
            .then(|| self.settles_to.clone())
            .flatten();
        if let Some(status) = due {
            self.status = status;
            self.settles_to = None;
            self.settles_at = None;
        }
    }

    /// Start tearing the association down: the status becomes `OFFBOARDING` now
    /// and the clock owes `OFFBOARD_COMPLETE` [`ASSOCIATION_SETTLE`] later.
    ///
    /// Idempotent: an association that is already offboarding or offboarded has
    /// nothing left to start, so its due instant is not pushed back by a second
    /// call.
    pub fn start_offboarding(&mut self, now: SystemTime) {
        if self.status == THIRD_PARTY_FIREWALL_OFFBOARDING
            || self.status == THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE
        {
            return;
        }
        self.status = THIRD_PARTY_FIREWALL_OFFBOARDING.to_string();
        self.settles_to = Some(THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE.to_string());
        self.settles_at = Some(now + ASSOCIATION_SETTLE);
    }
}

/// Per-service state for com-amazonaws-fms-awsfms-20180101. A handler reaches it through [`service_state`].
///
/// `policies` is a `BTreeMap` so a page boundary is a key in a total order:
/// `ListPolicies` can resume from a token without holding a cursor, and a
/// policy added or deleted between pages cannot reshuffle the ones already
/// returned.
#[derive(Debug, Default)]
pub struct ComAmazonawsFmsAwsfms20180101State {
    /// The single Firewall Manager administrator association, if one was made.
    pub admin_account: Option<AdminAccountRecord>,
    /// Stored policies, keyed by the `PolicyId` this service minted.
    pub policies: BTreeMap<String, PolicyRecord>,
    /// Tags of every Firewall Manager resource, keyed by its ARN: the ARN-keyed
    /// INDEX the later `ListTagsForResource` / `TagResource` / `UntagResource`
    /// slice needs (those operations resolve a resource by ARN, not by policy
    /// id), still a 501 tracked by its own gap issues.
    ///
    /// Which copy wins: for a policy, [`PolicyRecord::tags`] is authoritative and
    /// this map is maintained from it — `PutPolicy` writes the record's tags into
    /// it under the minted ARN and `DeletePolicy` removes the entry. A tagging
    /// operation must write the record first and refresh this entry from it, or
    /// (better) the index can be derived by iterating `policies` by `policy_arn`
    /// and this map dropped, once a Firewall Manager resource other than a policy
    /// is stored here too.
    pub tags_by_arn: HashMap<String, Vec<Tag>>,
    /// Stored resource sets, keyed by the `Base62Id` this service minted. A
    /// `BTreeMap` for the same reason as `policies`: a page boundary is a key in
    /// a total order.
    pub resource_sets: BTreeMap<String, ResourceSetRecord>,
    /// Stored applications lists, keyed by `ListId`.
    pub apps_lists: BTreeMap<String, AppsListRecord>,
    /// Stored protocols lists, keyed by `ListId`.
    pub protocols_lists: BTreeMap<String, ProtocolsListRecord>,
    /// The single SNS notification channel, if one was configured.
    pub notification_channel: Option<NotificationChannelRecord>,
    /// The third-party firewall associations, keyed by the `ThirdPartyFirewall`
    /// enum value. An absent entry is `NOT_EXIST`, not an error.
    pub third_party_firewalls: BTreeMap<String, ThirdPartyFirewallRecord>,
    /// The organisation's resources that are available to be associated with a
    /// resource set, keyed by the resource URI (its ARN) so the total order
    /// makes a `ListDiscoveredResources` page resumable from a key.
    ///
    /// This is an INVENTORY, not a cache: Firewall Manager discovers it from the
    /// member accounts of the organisation, and the emulator has no member
    /// accounts to discover from, so it is empty unless something put a resource
    /// in it. `ListDiscoveredResources` reports what is here, filtered by the
    /// request's member accounts and resource type — never a resource it made up.
    pub discovered_resources: BTreeMap<String, DiscoveredResource>,
}

/// The administrator gate every organisation-scoped Firewall Manager operation
/// runs FIRST lives beside the state it reads: see [`require_administrator`].
impl ComAmazonawsFmsAwsfms20180101State {
    /// Materialise, as of `now`, the disassociation the CLOCK has already made
    /// due: once [`AdminAccountRecord::removal_due`] has passed the administrator
    /// role IS `DELETED` ([`AdminAccountRecord::is_active_at`] already reads it
    /// that way), and this writes that status into the record so a reader reports
    /// it.
    ///
    /// It is the one write a reporting operation (`GetAdminAccount`) may make:
    /// the transition was owed at this instant whether or not anybody called, so
    /// performing it here changes nothing a later caller could not have derived
    /// itself. An operation that only needs to know WHETHER an administrator
    /// exists need not call it at all — the predicate is clock-derived, so
    /// [`require_administrator`] answers the same before and after.
    pub fn settle_admin_account_at(&mut self, now: SystemTime) {
        let Some(admin) = self.admin_account.as_mut() else {
            return;
        };
        if !admin.is_active_at(now) && admin.role_status != ROLE_STATUS_DELETED {
            admin.role_status = ROLE_STATUS_DELETED.to_string();
            admin.removal_due = None;
        }
    }

    /// [`Self::settle_admin_account_at`] at the current instant.
    pub fn settle_admin_account(&mut self) {
        self.settle_admin_account_at(SystemTime::now());
    }

    /// The association that currently makes an account the organisation's
    /// default administrator, after settling what the clock owes. `None` when no
    /// association was ever made and when the last one has been torn down.
    pub fn active_admin_account(&mut self) -> Option<&AdminAccountRecord> {
        let now = SystemTime::now();
        self.settle_admin_account_at(now);
        self.admin_account.as_ref().filter(|a| a.is_active_at(now))
    }

    /// [`Self::active_admin_account`] as of `now`, mutably: the association an
    /// operation that CHANGES the administrator slot works on, so the write site
    /// neither re-derives the active-association predicate nor re-reads the clock
    /// after deciding.
    pub fn active_admin_account_mut_at(
        &mut self,
        now: SystemTime,
    ) -> Option<&mut AdminAccountRecord> {
        self.settle_admin_account_at(now);
        self.admin_account.as_mut().filter(|a| a.is_active_at(now))
    }

    /// The `PolicyId` of the stored policy whose ARN is `arn`, or `None` when no
    /// stored Firewall Manager resource carries that ARN.
    ///
    /// This is the ARN -> resource resolution every tagging operation does
    /// before it reads or writes a tag: a policy is the only taggable Firewall
    /// Manager resource this service persists today (applications lists and
    /// protocols lists are still 501s), so an ARN naming anything else resolves
    /// to nothing and the caller gets the modelled not-found.
    pub fn policy_id_for_arn(&self, arn: &str) -> Option<String> {
        self.policies
            .iter()
            .find(|(_, record)| record.policy_arn == arn)
            .map(|(policy_id, _)| policy_id.clone())
    }

    /// The Firewall Manager administrator accounts that manage the calling
    /// account, in account-id order.
    ///
    /// An organisation has ONE default administrator and its `AdminScope` is the
    /// whole organisation, so while an association exists it manages every
    /// account in that organisation — including the caller. With no association
    /// there is no administrator to enumerate at all, which the caller is told
    /// with the modelled `ResourceNotFoundException` rather than an empty page.
    ///
    /// An association `DisassociateAdminAccount` has torn down
    /// ([`AdminAccountRecord::is_active`] is false) manages nothing any more, so
    /// it is left out.
    pub fn managing_admin_accounts(&self) -> Vec<String> {
        let mut accounts: Vec<String> = self
            .admin_account
            .iter()
            .filter(|admin| admin.is_active())
            .map(|admin| admin.account_id.clone())
            .collect();
        accounts.sort();
        accounts
    }

    /// The accounts of the administrator's organization this store records, in
    /// ascending account-id order.
    ///
    /// Firewall Manager learns its organization's accounts from Organizations,
    /// which this emulator does not stand in for; what it DOES hold is every
    /// account id its own control plane was told about:
    ///
    /// * the associated administrator account — `AssociateAdminAccount`
    ///   documents that it "must be a member account of the organization in
    ///   Organizations whose resources you want to protect";
    /// * every account id a stored policy names under the
    ///   [`SCOPE_ID_TYPE_ACCOUNT`] key of its `IncludeMap` or `ExcludeMap` —
    ///   `CustomerPolicyScopeMap`'s accounts are the organization's accounts the
    ///   policy applies to or leaves out.
    ///
    /// Values that are not `AWSAccountId`s (the model's
    /// `com.amazonaws.fms#AWSAccountId`: length 1..1024, `^[0-9]+$`) are left
    /// out: a scope map's `CustomerPolicyScopeId` is a free-form string, and an
    /// account list may only carry account ids.
    ///
    /// A `BTreeSet` so the order is total and an account named by two policies
    /// is reported once: a page boundary is then a key, not a cursor.
    ///
    /// An association `DisassociateAdminAccount` has torn down is not an
    /// administrator of the organization any more, so its account is only
    /// recorded here while the association is active.
    pub fn member_accounts(&self) -> BTreeSet<String> {
        let mut accounts = BTreeSet::new();
        if let Some(admin) = self.admin_account.as_ref().filter(|a| a.is_active()) {
            accounts.insert(admin.account_id.clone());
        }
        for record in self.policies.values() {
            for scope_map in [&record.policy.include_map, &record.policy.exclude_map]
                .into_iter()
                .flatten()
            {
                let Some(scoped) = scope_map.get(SCOPE_ID_TYPE_ACCOUNT) else {
                    continue;
                };
                accounts.extend(
                    scoped
                        .iter()
                        .filter(|id| is_account_id(id))
                        .map(String::clone),
                );
            }
        }
        accounts
    }
}

/// Merge `added` into `existing` the way a tag write does: a key already present
/// takes the new value in place, a key that is new is appended.
///
/// Tags are a key:value MAP (`com.amazonaws.fms#Tag` is one key and one value),
/// so a resource can never hold two values for one key; and "add one or more
/// tags" leaves the keys the request did not mention alone.
pub fn merge_tags(existing: &mut Vec<Tag>, added: &[Tag]) {
    for tag in added {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => existing.push(tag.clone()),
        }
    }
}

/// The reason `arn` is not an Amazon Resource Name at all, or `None` when it is
/// shaped like one.
///
/// Firewall Manager checks the FORM of a `ResourceArn` before it looks anything
/// up, and the modelled `ResourceArn` pattern cannot express it (that pattern
/// accepts any string of letters, digits and punctuation), so this check lives
/// here and answers a 400 `ValidationException` — a malformed ARN is a caller
/// programming error, distinct from an ARN that is well formed but names
/// nothing.
///
/// The form is the documented ARN layout
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`: the literal
/// `arn` prefix, six colon-separated fields, and a non-empty partition, service
/// and resource. Region and account id are deliberately allowed to be empty —
/// ARNs of global and account-less resources leave them so.
pub fn arn_format_error(arn: &str) -> Option<String> {
    if !arn.starts_with("arn:") {
        return Some(format!(
            "Incorrect arn format. error:ARNs must start with 'arn:': {arn}"
        ));
    }
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() < 6 {
        return Some(format!(
            "Incorrect arn format. error:ARNs must have six colon-separated fields, \
             found {}: {arn}",
            fields.len()
        ));
    }
    let empty = match (fields[1], fields[2], fields[5]) {
        ("", _, _) => Some("partition"),
        (_, "", _) => Some("service"),
        (_, _, "") => Some("resource"),
        _ => None,
    };
    empty.map(|field| format!("Incorrect arn format. error:ARNs must name a {field}: {arn}"))
}

/// Whether `value` is an `AWSAccountId` as this service's model declares it
/// (`com.amazonaws.fms#AWSAccountId`: length 1..1024, pattern `^[0-9]+$`).
pub fn is_account_id(value: &str) -> bool {
    (1..=1024).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

/// The `Message` Firewall Manager puts on EVERY `ResourceNotFoundException` it
/// answers, whatever was looked up.
///
/// The model declares the member but no literal, and the recordings of the live
/// service carry this one string on five different operations, across three
/// fixtures and across two kinds of lookup key (a policy UUID and a resource
/// ARN) — never the identifier that was looked up. So the text is a service
/// constant, not a per-call detail, and this is the single place it is written.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The referenced item does not exist.";

/// The modelled `ResourceNotFoundException` for anything this service could not
/// find: the identifier, ARN or name that was looked up never reaches the wire.
///
/// Every not-found site goes through here, so no handler can word its own
/// not-found text and no two of them can disagree. Which resource was missing is
/// the caller's own request — it is not restated in the error, exactly as the
/// live service does not restate it.
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The Firewall Manager administrator the organization has associated, or the
/// service's authorization refusal for a caller that is not entitled to an
/// organization-scoped call.
///
/// Firewall Manager answers an organization-scoped request only for a Firewall
/// Manager administrator or the organization's management account
/// (`ListMemberAccounts`: "Either an Firewall Manager administrator or the
/// organization's management account can make this request"). This emulator
/// presents ONE account, [`::engine::emulator_account_id`], and it is the
/// account that associates the administrator — so what decides the gate is
/// whether an administrator association exists at all. With none, the caller is
/// neither of the two, and the live service refuses the call with the error code
/// `AccessDeniedException`, which this service's model declares for no
/// operation (hence [`crate::scaffolded::Scaffolded::access_denied`], HTTP 400).
///
/// It answers through [`AdminAccountRecord::is_active`], so an association
/// `DisassociateAdminAccount` has torn down is no administrator: the gate closes
/// again once the tear-down the clock owed has completed, whether or not any
/// operation has materialised the `DELETED` status yet. It therefore takes the
/// store immutably and writes nothing.
///
/// The refusal's message is BUILT FROM THE REQUEST — it names the calling
/// account and the Region the request was signed for, the two things that decide
/// which administrator association was looked for — so `region` is the caller's
/// Region ([`request_region`]), never a constant.
///
/// Generic over the operation's modeled error enum, so every administrator-gated
/// handler shares this one gate.
///
/// Called BEFORE any identifier is looked up and before any payload is validated,
/// so a nonexistent id, a malformed ARN and a well-formed request all get the SAME
/// refusal: an authorization decision that leaked whether the resource exists would
/// tell an unauthorized caller what the organization holds. A refused call therefore
/// also writes nothing.
/// The refusal [`require_administrator`] states when the organization has no
/// Firewall Manager administrator: HTTP 400 `AccessDeniedException`, naming the
/// calling account and the Region the request was signed for.
///
/// Separate from the gate so an operation that resolves the association itself —
/// one that must hold it MUTABLY to change it, and so cannot take the gate's
/// shared borrow — states the SAME refusal instead of wording its own.
pub fn no_administrator_refusal<E>(region: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::access_denied(format!(
        "No default admin could be found for account {} in Region {region}",
        ::engine::emulator_account_id()
    ))
}

pub fn require_administrator<'store, E>(
    store: &'store ComAmazonawsFmsAwsfms20180101State,
    region: &str,
) -> Result<&'store AdminAccountRecord, crate::scaffolded::Scaffolded<E>> {
    store
        .admin_account
        .as_ref()
        .filter(|admin| admin.is_active())
        .ok_or_else(|| no_administrator_refusal(region))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFmsAwsfms20180101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFmsAwsfms20180101State>)
}

/// The Region this request was signed for, which is the Region whose resources
/// the request addresses — so it is the Region segment of any ARN the response
/// carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a Firewall Manager policy:
/// `arn:aws:fms:<region>:<account>:policy/<PolicyId>`.
pub fn policy_arn(region: &str, policy_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("policy/{policy_id}"))
}

/// The ARN of a Firewall Manager resource set:
/// `arn:aws:fms:<region>:<account>:resource-set/<Id>`.
pub fn resource_set_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("resource-set/{id}"))
}

/// The ARN of a Firewall Manager applications list:
/// `arn:aws:fms:<region>:<account>:applications-list/<ListId>`.
pub fn apps_list_arn(region: &str, list_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("applications-list/{list_id}"),
    )
}

/// The ARN of a Firewall Manager protocols list:
/// `arn:aws:fms:<region>:<account>:protocols-list/<ListId>`.
pub fn protocols_list_arn(region: &str, list_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("protocols-list/{list_id}"))
}

/// The fields of an AWS ARN, `arn:<partition>:<service>:<region>:<account>:<resource>`,
/// as borrowed slices of the value that was parsed.
///
/// The resource field is everything after the fifth colon, so a resource id that
/// contains colons (`loadbalancer/app/x:1`) stays whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedArn<'a> {
    pub partition: &'a str,
    pub service: &'a str,
    pub region: &'a str,
    pub account: &'a str,
    pub resource: &'a str,
}

/// Parse an ARN, or `None` when `value` is not one at all: six colon-separated
/// fields, the first literally `arn`, and a non-empty partition, service and
/// resource. The region and account fields MAY be empty — a global service's ARN
/// leaves them so.
pub fn parse_arn(value: &str) -> Option<ParsedArn<'_>> {
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let parsed = ParsedArn {
        partition: fields.next()?,
        service: fields.next()?,
        region: fields.next()?,
        account: fields.next()?,
        resource: fields.next()?,
    };
    let complete = !parsed.partition.is_empty()
        && !parsed.service.is_empty()
        && !parsed.resource.is_empty();
    complete.then_some(parsed)
}

/// The ARN partition this emulator's own resources live in, read off an ARN
/// built by [`engine::emulator_arn`] rather than written here as a literal, so
/// the partition has one source of truth.
pub fn emulator_partition() -> String {
    let minted = ::engine::emulator_arn(ARN_NAMESPACE, DEFAULT_REGION, "probe");
    parse_arn(&minted)
        .map(|arn| arn.partition.to_string())
        .unwrap_or_default()
}

/// Encode a page boundary (the key the next page starts at — a `PolicyId` for
/// `ListPolicies`, an account id for `ListMemberAccounts` and
/// `ListAdminsManagingAccount`) as an opaque
/// `PaginationToken`.
///
/// Hex keeps the token inside the modelled `PaginationToken` pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) and its 1..4096 length, and makes it
/// opaque to a caller: nothing in it is meaningful without decoding.
pub fn encode_page_token(policy_id: &str) -> String {
    encode_key_token(policy_id)
}

/// Encode an arbitrary page boundary (the store key the next page starts at) as
/// an opaque `PaginationToken`, for the paginated operations whose key is not a
/// `PolicyId`.
///
/// Hex for the same reasons as [`encode_page_token`]: it keeps the token inside
/// the modelled `PaginationToken` pattern and length whatever the key contains,
/// and nothing in it is meaningful to a caller without decoding.
pub fn encode_key_token(key: &str) -> String {
    ::hex::encode(key)
}

/// Decode a `PaginationToken` produced by [`encode_key_token`]. `None` when the
/// token was not produced by this service: a value that is not hex, not UTF-8,
/// empty, or carries a control character is no store key of ours.
pub fn decode_key_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let is_key = !decoded.is_empty() && !decoded.chars().any(char::is_control);
    is_key.then_some(decoded)
}

/// Decode a `PaginationToken` produced by [`encode_page_token`]. `None` when the
/// token was not produced by this service.
///
/// The decoded value must still be a `PolicyId` (the model's
/// `com.amazonaws.fms#PolicyId`: 36 characters matching `^[a-z0-9A-Z-]{36}$`),
/// so a foreign token that happens to be valid hex is refused rather than
/// resumed from as an arbitrary key.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    let is_policy_id = decoded.len() == 36
        && decoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    is_policy_id.then_some(decoded)
}

/// Decode an account-keyed `PaginationToken` produced by
/// [`encode_page_token`]. `None` when the token was not produced by this
/// service.
///
/// The decoded value must still be an `AWSAccountId` ([`is_account_id`]), so a
/// token minted for a policy page — or any foreign token that happens to be
/// valid hex — is refused rather than resumed from as an arbitrary key.
pub fn decode_account_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    is_account_id(&decoded).then_some(decoded)
}

/// Mint a fresh service-owned identifier: 36 lowercase hex characters, which
/// satisfies the model's `PolicyId` pattern (`^[a-z0-9A-Z-]{36}$`, length 36)
/// and also `PolicyUpdateToken`'s (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, length
/// 1..1024), so one mint serves both.
///
/// The value is the SHA-256 of a process-wide monotonic counter and the current
/// instant, hex-encoded and truncated to the modelled 36 characters: the counter
/// makes two mints in one process distinct whatever the clock does (so a rotated
/// `PolicyUpdateToken` always differs from the one it replaced, even within the
/// same nanosecond), and the instant separates two processes. Hashing and
/// hex-encoding are the `sha2` and `hex` crates' work, not hand-written here,
/// and nothing from the request goes into it.
pub fn mint_identifier() -> String {
    mint_hex(36)
}

/// Mint a fresh service-owned `Base62Id`: 22 characters matching the model's
/// `com.amazonaws.fms#Base62Id` (`^[a-z0-9A-Z]{22}$`, length 22), which is what
/// a resource set's `Id` is.
///
/// The same mint as [`mint_identifier`], cut to 22 characters: hex digits are a
/// subset of base62, so a hex string satisfies the base62 pattern.
pub fn mint_base62_id() -> String {
    mint_hex(22)
}

/// The mint behind [`mint_identifier`] and [`mint_base62_id`]: `len` lowercase
/// hex characters of the SHA-256 of a process-wide counter and the clock.
fn mint_hex(len: usize) -> String {
    use ::sha2::Digest;

    /// Distinguishes mints within one process; `Relaxed` is enough because only
    /// the uniqueness of the returned value matters, not the order.
    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0_u128, |since_epoch| since_epoch.as_nanos());
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let mut id = ::hex::encode(hasher.finalize());
    // SHA-256 renders as 64 hex characters, so every modelled length asked for
    // here is always there.
    id.truncate(len);
    id
}
