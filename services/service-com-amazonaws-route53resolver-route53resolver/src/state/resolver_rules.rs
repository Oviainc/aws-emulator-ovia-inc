// SPDX-License-Identifier: CC0-1.0

use super::*;

/// What one `CreateResolverRule` request asked for, apart from the token it
/// carried: the members that DEFINE the rule it would make.
///
/// It exists so the token a create carries can be compared against the request
/// the stored rule was made from, in [`rule_content_digest`], without a handler
/// listing those members a second time.
#[derive(Debug, Clone, Copy)]
pub struct RuleContent<'a> {
    pub name: Option<&'a str>,
    pub rule_type: &'a str,
    /// The domain name in the CANONICAL form a rule stores it in
    /// ([`canonical_domain_name`]), so two requests that differ only in how
    /// they wrote the root label are one request.
    pub domain_name: Option<&'a str>,
    /// The target addresses in the NORMALIZED form a rule stores them in
    /// ([`normalize_target_addresses`]), for the same reason.
    pub target_ips: &'a [TargetAddress],
    pub resolver_endpoint_id: Option<&'a str>,
    pub delegation_record: Option<&'a str>,
    pub tags: &'a [Tag],
}

/// A digest of everything a `CreateResolverRule` request asked for beside its
/// `CreatorRequestId`.
///
/// `CreatorRequestId` "identifies the request", so deciding whether a replayed
/// token is a RETRY of the request that minted the rule, or a different request
/// wearing the same token, is a comparison of the rest of the request against
/// the rule that token made. Every member is fed length-prefixed
/// ([`feed_field`]), so no two different requests collide by running their
/// values together, and the tag list is folded order-insensitively because a
/// tag set is not ordered.
pub fn rule_content_digest(content: &RuleContent<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/resolver-rule/content/v1");
    feed_field(&mut hasher, content.name.unwrap_or_default().as_bytes());
    feed_field(&mut hasher, content.rule_type.as_bytes());
    feed_field(
        &mut hasher,
        content.domain_name.unwrap_or_default().as_bytes(),
    );
    feed_field(
        &mut hasher,
        content.resolver_endpoint_id.unwrap_or_default().as_bytes(),
    );
    feed_field(
        &mut hasher,
        content.delegation_record.unwrap_or_default().as_bytes(),
    );
    for target in content.target_ips {
        feed_field(&mut hasher, target.ip.clone().unwrap_or_default().as_bytes());
        feed_field(
            &mut hasher,
            target.ipv6.clone().unwrap_or_default().as_bytes(),
        );
        feed_field(
            &mut hasher,
            target.port.unwrap_or_default().to_string().as_bytes(),
        );
        feed_field(
            &mut hasher,
            target.protocol.clone().unwrap_or_default().as_bytes(),
        );
        feed_field(
            &mut hasher,
            target
                .server_name_indication
                .clone()
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    let mut tags: Vec<String> = content
        .tags
        .iter()
        .map(|tag| format!("{}={}", tag.key, tag.value))
        .collect();
    tags.sort();
    for tag in &tags {
        feed_field(&mut hasher, tag.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

/// A Resolver rule, as stored.
#[derive(Debug, Clone)]
pub struct ResolverRuleRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`].
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub creator_request_id: String,
    pub name: Option<String>,
    pub domain_name: Option<String>,
    pub rule_type: String,
    pub target_ips: Vec<TargetAddress>,
    pub resolver_endpoint_id: Option<String>,
    pub delegation_record: Option<String>,
    pub owner_id: String,
    pub share_status: String,
    /// The `ResolverRuleStatus` this rule reports; see [`RULE_STATUS_COMPLETE`]
    /// on why a stored rule is already settled.
    pub status: String,
    pub status_message: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The resource policy attached to this rule, as the last
    /// `PutResolverRulePolicy` for it wrote it, and `None` while no policy has
    /// been attached.
    ///
    /// Held on the rule's own record, not in a table beside it, because a
    /// resource policy belongs to the resource it shares: a read of the rule's
    /// policy resolves through the same record every other rule operation
    /// reads, and a rule the store no longer holds has no policy to report.
    pub resolver_rule_policy: Option<String>,
}

impl ResolverRuleRecord {
    /// What the request that made this rule asked for, for the replay
    /// comparison [`rule_content_digest`] describes.
    pub fn content(&self) -> RuleContent<'_> {
        RuleContent {
            name: self.name.as_deref(),
            rule_type: &self.rule_type,
            domain_name: self.domain_name.as_deref(),
            target_ips: &self.target_ips,
            resolver_endpoint_id: self.resolver_endpoint_id.as_deref(),
            delegation_record: self.delegation_record.as_deref(),
            tags: &self.tags,
        }
    }

    /// Project the record onto the modelled `ResolverRule`.
    pub fn to_rule(&self) -> ResolverRule {
        ResolverRule {
            id: Some(self.id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            arn: Some(self.arn.clone()),
            domain_name: self.domain_name.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            rule_type: Some(self.rule_type.clone()),
            name: self.name.clone(),
            // `TargetIps` is modelled with a minimum length of 1, so an empty
            // list is left unset rather than serialized as `[]`.
            target_ips: if self.target_ips.is_empty() {
                None
            } else {
                Some(self.target_ips.clone())
            },
            resolver_endpoint_id: self.resolver_endpoint_id.clone(),
            owner_id: Some(self.owner_id.clone()),
            share_status: Some(self.share_status.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            delegation_record: self.delegation_record.clone(),
        }
    }

    /// Whether this rule satisfies one `ListResolverRules` filter: whether the
    /// value it presents for that member is among the filter's values.
    ///
    /// A set lookup per record and per filter key, as
    /// [`ResolverEndpointRecord::matches_filter`] is, so a filtered page costs
    /// O(stored rules x filter keys) and not O(values the request carried). A
    /// member the rule does not carry matches nothing.
    pub fn matches_filter(&self, key: RuleFilterKey, values: &HashSet<String>) -> bool {
        match key {
            RuleFilterKey::CreatorRequestId => values.contains(self.creator_request_id.as_str()),
            RuleFilterKey::DomainName => self
                .domain_name
                .as_deref()
                .is_some_and(|domain| values.contains(domain)),
            RuleFilterKey::Name => self.name.as_deref().is_some_and(|name| values.contains(name)),
            RuleFilterKey::ResolverEndpointId => self
                .resolver_endpoint_id
                .as_deref()
                .is_some_and(|id| values.contains(id)),
            RuleFilterKey::Status => values.contains(self.status.as_str()),
            RuleFilterKey::Type => values.contains(self.rule_type.as_str()),
        }
    }
}

/// The member a `ListResolverRules` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRules**): `CreatorRequestId`, `DomainName`, `Name`,
/// `ResolverEndpointId`, `Status`, `Type`. It is a different catalog from
/// [`FilterKey`]'s, because the valid filter names "depend on the action that
/// you're including the filter in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFilterKey {
    CreatorRequestId,
    DomainName,
    Name,
    ResolverEndpointId,
    Status,
    Type,
}

impl RuleFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreatorRequestId => "CreatorRequestId",
            Self::DomainName => "DomainName",
            Self::Name => "Name",
            Self::ResolverEndpointId => "ResolverEndpointId",
            Self::Status => "Status",
            Self::Type => "Type",
        }
    }

    /// Whether the values of this filter name a rule domain, which is matched in
    /// the canonical fully-qualified form a rule is stored and reported in (see
    /// [`canonical_domain_name`]): the doc asks a caller to "include a trailing
    /// dot (.) after the domain name", and canonicalising the value makes both
    /// spellings of one domain select the same rules.
    pub fn selects_on_domain_name(self) -> bool {
        matches!(self, Self::DomainName)
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`FilterKey::parse`] folds the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DOMAINNAME" => Some(Self::DomainName),
            "NAME" => Some(Self::Name),
            "RESOLVERENDPOINTID" => Some(Self::ResolverEndpointId),
            "STATUS" => Some(Self::Status),
            "TYPE" => Some(Self::Type),
            _ => None,
        }
    }
}

/// One association between a Resolver rule and a VPC, as stored.
#[derive(Debug, Clone)]
pub struct ResolverRuleAssociationRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`].
    pub seq: u64,
    pub id: String,
    pub resolver_rule_id: String,
    pub vpc_id: String,
    pub name: Option<String>,
    /// The transient `ResolverRuleAssociationStatus` reported until
    /// [`Self::complete_at`] falls due.
    pub status: String,
    pub status_message: Option<String>,
    /// The instant at which the move to `COMPLETE` is owed.
    pub complete_at: Instant,
}

impl ResolverRuleAssociationRecord {
    /// This association's `ResolverRuleAssociationStatus` as of `now`: the
    /// transient status until the transition the clock owes falls due,
    /// `COMPLETE` after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.complete_at {
            &self.status
        } else {
            RULE_ASSOCIATION_STATUS_COMPLETE
        }
    }

    /// Project the record onto the modelled `ResolverRuleAssociation`.
    pub fn to_association(&self, now: Instant) -> ResolverRuleAssociation {
        ResolverRuleAssociation {
            id: Some(self.id.clone()),
            resolver_rule_id: Some(self.resolver_rule_id.clone()),
            name: self.name.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
        }
    }

    /// Whether this association satisfies one `ListResolverRuleAssociations`
    /// filter: whether the value it presents for that member is among the
    /// filter's values, as [`ResolverRuleRecord::matches_filter`] decides it for
    /// a rule. A member the association does not carry matches nothing, and the
    /// status it is matched on is the one it reports as of `now`.
    pub fn matches_filter(
        &self,
        key: RuleAssociationFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            RuleAssociationFilterKey::Name => {
                self.name.as_deref().is_some_and(|name| values.contains(name))
            }
            RuleAssociationFilterKey::ResolverRuleId => {
                values.contains(self.resolver_rule_id.as_str())
            }
            RuleAssociationFilterKey::Status => values.contains(self.status_at(now)),
            RuleAssociationFilterKey::VpcId => values.contains(self.vpc_id.as_str()),
        }
    }
}

/// The member a `ListResolverRuleAssociations` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRuleAssociations**): `Name`, `ResolverRuleId`, `Status`,
/// `VPCId`. It is a different catalog from [`RuleFilterKey`]'s, because the
/// valid filter names "depend on the action that you're including the filter
/// in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAssociationFilterKey {
    Name,
    ResolverRuleId,
    Status,
    VpcId,
}

impl RuleAssociationFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::ResolverRuleId => "ResolverRuleId",
            Self::Status => "Status",
            Self::VpcId => "VPCId",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`RuleFilterKey::parse`] folds the rule catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "NAME" => Some(Self::Name),
            "RESOLVERRULEID" => Some(Self::ResolverRuleId),
            "STATUS" => Some(Self::Status),
            "VPCID" => Some(Self::VpcId),
            _ => None,
        }
    }
}
