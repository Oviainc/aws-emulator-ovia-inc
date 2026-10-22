// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafregional-awswaf-regional-20161128 service.
//!
//! AWS WAF Classic's `IPSet`, `Rule`, `SqlInjectionMatchSet`, `WebACL`, `XssMatchSet`
//! and `RuleGroup` records and the change tokens every mutating call consumes live
//! here, so `GetChangeToken`, `GetChangeTokenStatus` and the `Create*`, `Update*`,
//! `Get*`, `List*` and `Delete*` handlers of every resource kind all read and write ONE
//! copy of each resource. The helpers below (id minting, the change-token protocol,
//! CIDR parsing, metric-name validation, filter identity, the `IPSet`/`IPSetSummary`,
//! `Rule`, `SqlInjectionMatchSet`, `WebACL`, `XssMatchSet`/`XssMatchSetSummary` and
//! `RuleGroup`
//! projections) are shared by those handlers rather than duplicated per operation.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::ops::types::{
    ActivatedRule, IPSet, IPSetDescriptor, IPSetSummary, Predicate, Rule, RuleGroup, RuleSummary,
    SqlInjectionMatchSet, SqlInjectionMatchTuple, WafAction, WebACL, XssMatchSet,
    XssMatchSetSummary, XssMatchTuple,
};

/// `ChangeTokenStatus` wire values, from the model's `CHANGE_TOKEN_STATUS_VALUES`
/// (`INSYNC`, `PENDING`, `PROVISIONED`).
///
/// A token starts `PROVISIONED` when `GetChangeToken` mints it. `GetChangeToken`'s own
/// documentation states the rest: "When you use a change token in a create, update, or
/// delete request, the status of the change token changes to `PENDING`, which indicates
/// that AWS WAF is propagating the change to all AWS WAF servers."
pub const CHANGE_TOKEN_PROVISIONED: &str = "PROVISIONED";
/// A token a mutating call has consumed, whose change is still propagating.
pub const CHANGE_TOKEN_PENDING: &str = "PENDING";
/// A token whose change has finished propagating.
pub const CHANGE_TOKEN_INSYNC: &str = "INSYNC";

/// `IPSetDescriptorType` wire values, from the model's
/// `I_P_SET_DESCRIPTOR_TYPE_VALUES`.
pub const IP_SET_TYPE_IPV4: &str = "IPV4";
/// The IPv6 `IPSetDescriptorType`.
pub const IP_SET_TYPE_IPV6: &str = "IPV6";

/// `ChangeAction` wire values, from the model's `CHANGE_ACTION_VALUES`.
pub const CHANGE_ACTION_INSERT: &str = "INSERT";
/// The `DELETE` `ChangeAction`.
pub const CHANGE_ACTION_DELETE: &str = "DELETE";

/// How long a consumed change token stays `PENDING` before the clock owes it
/// `INSYNC`.
///
/// The model documents the `PENDING` phase ("AWS WAF is propagating the change to all
/// AWS WAF servers") but not how long it lasts, so this span is CHOSEN, not cited: it
/// is long enough that a caller polling `GetChangeTokenStatus` straight after a
/// mutating call still observes `PENDING`, and short enough that the propagation is
/// over by the time that caller polls again. Nothing ever sleeps for it — the due
/// instant is stored and compared on read.
pub const CHANGE_TOKEN_PROPAGATION: Duration = Duration::from_millis(400);

/// The IPv6 prefix lengths AWS WAF accepts.
///
/// Source: `UpdateIPSet`'s own documentation — "AWS WAF supports IPv6 address ranges:
/// /24, /32, /48, /56, /64, and /128."
pub const IPV6_PREFIX_LENGTHS: &[u8] = &[24, 32, 48, 56, 64, 128];

/// The most `IPSetDescriptor`s one `UpdateIPSet` request may carry.
///
/// Source: `UpdateIPSet`'s own documentation — "You can insert a maximum of 1000
/// addresses in a single request."
pub const MAX_UPDATES_PER_REQUEST: usize = 1_000;

/// The most `IPSet`s this emulator stores for the account before `CreateIPSet`
/// answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an account (50 IPSets); with [`MAX_IP_SET_DESCRIPTORS`] bounding each one's
/// contents, it keeps the IPSet state a caller can grow bounded.
pub const MAX_IP_SETS: usize = 50;

/// The most `IPSetDescriptor`s one `IPSet` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an IPSet (10,000 IP address ranges). Together with [`MAX_IP_SETS`] it bounds the
/// total descriptor state a caller can grow, not just the number of IPSets.
pub const MAX_IP_SET_DESCRIPTORS: usize = 10_000;

/// The most `SqlInjectionMatchSet`s this emulator stores for the account before
/// `CreateSqlInjectionMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 conditions of each type, a `SqlInjectionMatchSet` being one condition),
/// which keeps the match-set state a caller can grow bounded.
pub const MAX_SQL_INJECTION_MATCH_SETS: usize = 100;

/// The most `RuleGroup`s this emulator stores for the account before
/// `CreateRuleGroup` answers its modelled `WAFLimitsExceededException`.
///
/// Source: `CreateRuleGroup`'s own documentation — "Rule groups are subject to the
/// following limits: Three rule groups per account. You can request an increase to
/// this limit by contacting customer support."
pub const MAX_RULE_GROUPS: usize = 3;

/// The most change-token records kept before the oldest settled ones are dropped.
///
/// A change token is a short-lived record on the real service too — a status a client
/// polls for and then forgets — so keeping every token a long-running emulator ever
/// minted would grow without end. Chosen far above the number of tokens any one
/// client has in flight.
pub const MAX_CHANGE_TOKENS: usize = 1_000;

/// The most `WebACL`s this emulator stores for the account before `CreateWebACL`
/// answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota — only that `WAFLimitsExceededException` covers "the
/// maximum number of `WebACL` objects that you can create for an AWS account" — so
/// this bound is the one AWS WAF Classic publishes for an account (50 web ACLs). It
/// keeps the web ACL state a caller can grow bounded.
pub const MAX_WEB_ACLS: usize = 50;

/// The ARN service segment of every ARN this service mints: its SigV4 signing name,
/// as `WebACLArn` carries it.
pub const ARN_SERVICE: &str = "waf-regional";

/// The region an ARN names when the request carries no SigV4 credential scope to read
/// one from — `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region ARNs minted for this request name: the one in the caller's SigV4
/// credential scope, else [`DEFAULT_REGION`]. A scope region that is empty, or that is
/// not shaped like an AWS region name, falls back too: on the real service the region
/// in an ARN is a property of the endpoint the request reached, so a caller cannot put
/// arbitrary text there.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_is_well_formed(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS region name: two or more non-empty groups of
/// lowercase letters and digits joined by single hyphens (`us-east-1`,
/// `ap-southeast-4`), bounded in length.
fn region_is_well_formed(region: &str) -> bool {
    (1..=32).contains(&region.len())
        && region.split('-').count() >= 2
        && region.split('-').all(|group| {
            !group.is_empty()
                && group
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// The `WebACLArn` of the web ACL `web_acl_id` in `region`, built through
/// `engine::emulator_arn` so the account segment is the one canonical account id this
/// emulator presents.
pub fn web_acl_arn(region: &str, web_acl_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("webacl/{web_acl_id}"))
}

/// The largest page a paginated `List*` returns (`ListIPSets`, `ListRules`, …), and
/// the page size a `Limit` of 0 (the model's declared default) asks for.
///
/// Source: `PaginationLimit`'s modelled `@range` of 0..100.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most `Rule`s this emulator stores for the account before `CreateRule` answers
/// its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an account (100 rules); every `Rule` is a stored record, so the number a caller can
/// create has to be bounded somewhere.
pub const MAX_RULES: usize = 100;

/// The most tags one resource may carry, and so the most `Tags` entries one create
/// request may bring.
///
/// The model bounds `TagList` at `len 1..` — a minimum only — so without this the
/// number of tag entries one request persists would be the caller's choice. This bound
/// is the per-resource tag quota AWS publishes (50 tags per resource).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The metric names AWS WAF reserves, which no resource may take.
///
/// Source: the `MetricName` member's own documentation on every create operation that
/// carries one — "It can't contain whitespace or metric names reserved for AWS WAF,
/// including \"All\" and \"Default_Action.\""
///
/// Only `All` is reachable through [`metric_name_is_allowed`]: `Default_Action` carries
/// an underscore, so the character-set rule refuses it first. It is listed anyway
/// because the documentation names it, and because the list is what a reader looks at
/// to see which names are reserved.
pub const RESERVED_METRIC_NAMES: &[&str] = &["All", "Default_Action"];

/// Whether `metric_name` is a metric name AWS WAF accepts.
///
/// The rule is the `MetricName` member's own documentation: "The name can contain only
/// alphanumeric characters (A-Z, a-z, 0-9), with maximum length 128 and minimum length
/// one. It can't contain whitespace or metric names reserved for AWS WAF, including
/// "All" and "Default_Action."" The length bounds are modelled (`len 1..128`) and so
/// already refused at the boundary; what is left to enforce here is the character set
/// (which excludes whitespace, punctuation and non-ASCII letters alike) and the
/// reserved names in [`RESERVED_METRIC_NAMES`].
///
/// Shared by every handler whose input carries a `MetricName` (`CreateRule`, and the
/// `CreateRateBasedRule` / `CreateRuleGroup` / `CreateWebACL` handlers once they are
/// implemented), so one rule is enforced in one place.
pub fn metric_name_is_allowed(metric_name: &str) -> bool {
    !metric_name.is_empty()
        && metric_name.chars().all(|c| c.is_ascii_alphanumeric())
        && !RESERVED_METRIC_NAMES.contains(&metric_name)
}

/// AWS WAF Classic answers each refusal class with ONE fixed sentence that carries no
/// request-derived content: the recordings show the identical `message` for a
/// never-created id and for a deleted one. These constants are that per-exception
/// sentence, so a handler never builds an error message out of the request.
///
/// The model declares a bare `message` member on each error shape and no wire text, so
/// each constant below is the sentence the recordings carry for its exception type.
pub const MESSAGE_DISALLOWED_NAME: &str = "The specified name is not permitted.";
/// The fixed sentence for `WAFStaleDataException` — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_STALE_DATA: &str = "The input token is no longer current.";
/// The fixed sentence for `WAFNonexistentItemException` — see
/// [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_NONEXISTENT_ITEM: &str = "The referenced item does not exist.";
/// The fixed sentence for `WAFLimitsExceededException`, from the shape's own
/// documentation ("The operation exceeds a resource limit") — see
/// [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_LIMITS_EXCEEDED: &str = "The operation exceeds a resource limit.";
/// The fixed sentence for `WAFInternalErrorException`, from the shape's own
/// documentation — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_INTERNAL_ERROR: &str =
    "The operation failed because of a system problem, even though the request was \
     valid. Retry your request.";
/// The fixed sentence for `WAFNonexistentContainerException`, the refusal of an update
/// whose container resource does not exist — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_NONEXISTENT_CONTAINER: &str = "The referenced item does not exist.";
/// The fixed sentence for `WAFNonEmptyEntityException`, the refusal of a delete of a
/// resource that still holds filters or addresses — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_NON_EMPTY_ENTITY: &str = "The referenced entity is not empty.";
/// The fixed sentence for `WAFInvalidOperationException`, the refusal of an update that
/// would leave the resource as it was — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_INVALID_OPERATION: &str = "The operation is not valid.";

/// The `MatchFieldType` whose `FieldToMatch.Data` names a request header.
pub const MATCH_FIELD_TYPE_HEADER: &str = "HEADER";
/// The `MatchFieldType` whose `FieldToMatch.Data` names a query-string parameter.
pub const MATCH_FIELD_TYPE_SINGLE_QUERY_ARG: &str = "SINGLE_QUERY_ARG";

/// Whether `FieldToMatch.Data` carries a name for match-field type `field_type`.
///
/// Source: `FieldToMatch`'s own `Data` documentation — "When the value of `Type` is
/// `HEADER`, enter the name of the header that you want AWS WAF to search
/// \[…\] When the value of `Type` is `SINGLE_QUERY_ARG`, enter the name of the parameter
/// that you want AWS WAF to search \[…\] If the value of `Type` is any other value, omit
/// `Data`."
pub fn match_field_takes_data(field_type: &str) -> bool {
    field_type == MATCH_FIELD_TYPE_HEADER || field_type == MATCH_FIELD_TYPE_SINGLE_QUERY_ARG
}

/// The most `XssMatchSet`s this emulator stores for the account before
/// `CreateXssMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 cross-site scripting match conditions); with
/// [`MAX_XSS_MATCH_TUPLES`] bounding each one's contents it keeps the XssMatchSet state a
/// caller can grow bounded.
pub const MAX_XSS_MATCH_SETS: usize = 100;

/// The most `XssMatchTuple`s one `XssMatchSet` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes per
/// cross-site scripting match condition (10 filters).
pub const MAX_XSS_MATCH_TUPLES: usize = 10;

/// The most `XssMatchSetUpdate`s one `UpdateXssMatchSet` request may carry.
///
/// The model states no per-request quota for this operation, so this bound is CHOSEN,
/// not cited: a request can at most empty a set ([`MAX_XSS_MATCH_TUPLES`] deletes) and
/// then fill it again ([`MAX_XSS_MATCH_TUPLES`] inserts), since a delete must name a
/// filter the set holds and an insert one it does not, so no larger request can leave the
/// set in a state a request of this size cannot. It caps the work one caller can ask for
/// before the handler takes the shared store's lock.
pub const MAX_XSS_MATCH_SET_UPDATES_PER_REQUEST: usize = 2 * MAX_XSS_MATCH_TUPLES;

/// A descriptor's identity: the parsed IP address and the prefix length.
///
/// `UpdateIPSet` documents that one IPv6 address has several spellings
/// ("1111:0000:0000:0000:0000:0000:0000:0111/128", "1111:0:0:0:0:0:0:0111/128",
/// "1111::0111/128", "1111::111/128" are listed as formats of the same address), so
/// two descriptors are the same descriptor when their addresses and prefix lengths
/// are equal — not when their text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix_len: u8,
}

/// The CIDR `value` denotes for descriptor type `kind`, or `None` when it is not an
/// address of that family with a prefix length AWS WAF supports.
///
/// The address itself is parsed by `std::net::IpAddr`, so every spelling std accepts
/// is accepted here and normalises to the same [`Cidr`]. The supported prefix lengths
/// come from `UpdateIPSet`'s documentation: "AWS WAF supports IPv4 address ranges: /8
/// and any range between /16 through /32. AWS WAF supports IPv6 address ranges: /24,
/// /32, /48, /56, /64, and /128."
pub fn parse_cidr(kind: &str, value: &str) -> Option<Cidr> {
    let (addr, prefix_len) = value.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    let prefix_len: u8 = prefix_len.parse().ok()?;
    let supported = match (kind, addr) {
        (IP_SET_TYPE_IPV4, IpAddr::V4(_)) => prefix_len == 8 || (16..=32).contains(&prefix_len),
        (IP_SET_TYPE_IPV6, IpAddr::V6(_)) => IPV6_PREFIX_LENGTHS.contains(&prefix_len),
        // A family that does not match the address (an IPv4 address declared IPV6, or
        // the reverse) is not a descriptor AWS WAF can hold.
        _ => false,
    };
    supported.then_some(Cidr { addr, prefix_len })
}

/// One stored `IPSetDescriptor`.
#[derive(Debug, Clone)]
pub struct IPSetDescriptorEntry {
    /// `IPSetDescriptorType` as it goes on the wire: [`IP_SET_TYPE_IPV4`] or
    /// [`IP_SET_TYPE_IPV6`].
    pub kind: String,
    /// The CIDR as the caller wrote it, so `GetIPSet` reads back what was stored
    /// rather than a re-rendering of it.
    pub value: String,
    /// The descriptor's identity — see [`Cidr`]. Two inserts of one address under
    /// different spellings are one descriptor.
    pub cidr: Cidr,
}

/// One stored `IPSet`.
#[derive(Debug, Clone)]
pub struct IPSetEntry {
    pub ip_set_id: String,
    pub name: String,
    /// The descriptors in insertion order, deduplicated by [`Cidr`].
    pub descriptors: Vec<IPSetDescriptorEntry>,
    /// Membership index over the [`Cidr`] keys of `descriptors`, kept in step with it
    /// by [`IPSetEntry::insert_descriptor`] and [`IPSetEntry::remove_descriptor`], so
    /// deduplicating one update costs a lookup rather than a scan of the whole set.
    cidrs: BTreeSet<Cidr>,
}

impl IPSetEntry {
    /// A new, empty IPSet record.
    pub fn new(ip_set_id: String, name: String) -> Self {
        Self {
            ip_set_id,
            name,
            descriptors: Vec::new(),
            cidrs: BTreeSet::new(),
        }
    }
    /// This record projected onto the modelled `IPSet` shape the Get and Create
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> IPSet {
        IPSet {
            ip_set_id: self.ip_set_id.clone(),
            name: Some(self.name.clone()),
            ip_set_descriptors: self
                .descriptors
                .iter()
                .map(|descriptor| IPSetDescriptor {
                    r#type: descriptor.kind.clone(),
                    value: descriptor.value.clone(),
                })
                .collect(),
        }
    }

    /// This record projected onto the `IPSetSummary` shape `ListIPSets` returns.
    pub fn summary(&self) -> IPSetSummary {
        IPSetSummary {
            ip_set_id: self.ip_set_id.clone(),
            name: self.name.clone(),
        }
    }

    /// Add a descriptor, unless an equal one (same address, same prefix length) is
    /// already stored. Answers whether the set changed.
    pub fn insert_descriptor(&mut self, kind: &str, value: &str, cidr: Cidr) -> bool {
        if !self.cidrs.insert(cidr) {
            return false;
        }
        self.descriptors.push(IPSetDescriptorEntry {
            kind: kind.to_string(),
            value: value.to_string(),
            cidr,
        });
        true
    }

    /// Remove the descriptor equal to `cidr`, if the set holds one. Answers whether
    /// the set changed.
    pub fn remove_descriptor(&mut self, cidr: &Cidr) -> bool {
        if !self.cidrs.remove(cidr) {
            return false;
        }
        self.descriptors.retain(|stored| &stored.cidr != cidr);
        true
    }
}

/// One stored `WebACL`.
#[derive(Debug, Clone)]
pub struct WebACLEntry {
    pub web_acl_id: String,
    /// `Name`, which "you can't change ... after you create it".
    pub name: String,
    /// `MetricName`, which "you can't change ... after you create the `WebACL`".
    pub metric_name: String,
    /// The `WafActionType` wire value of `DefaultAction`: the action AWS WAF takes for
    /// a request none of the web ACL's rules match.
    pub default_action: String,
    /// The rules activated in this web ACL, in priority order. A newly created web ACL
    /// holds none: they arrive through `UpdateWebACL`.
    pub rules: Vec<ActivatedRule>,
    /// The resource's tags, as a key→value map — a repeated key is one tag, the last
    /// value winning, as an AWS tag set is. At most [`MAX_TAGS_PER_RESOURCE`] entries,
    /// so one record's tag state is bounded; the tagging operations
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) will project and update
    /// it when their own stub issues are implemented.
    pub tags: BTreeMap<String, String>,
    /// `WebACLArn`, minted once at create time so every later read reports the same
    /// ARN rather than one rebuilt from the reading request's region.
    pub web_acl_arn: String,
}

impl WebACLEntry {
    /// A new web ACL record, holding no rules.
    pub fn new(
        web_acl_id: String,
        name: String,
        metric_name: String,
        default_action: String,
        web_acl_arn: String,
        tags: BTreeMap<String, String>,
    ) -> Self {
        Self {
            web_acl_id,
            name,
            metric_name,
            default_action,
            rules: Vec::new(),
            tags,
            web_acl_arn,
        }
    }

    /// This record projected onto the modelled `WebACL` shape the Create and Get
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> WebACL {
        WebACL {
            web_acl_id: self.web_acl_id.clone(),
            name: Some(self.name.clone()),
            metric_name: Some(self.metric_name.clone()),
            default_action: WafAction {
                r#type: self.default_action.clone(),
            },
            rules: self.rules.clone(),
            web_acl_arn: Some(self.web_acl_arn.clone()),
        }
    }
}

/// One `XssMatchTuple`'s identity: the part of the request it inspects and the text
/// transformation applied before inspecting it.
///
/// `FieldToMatch`'s `Data` documentation states that the header name and the
/// query-parameter name are "not case sensitive", so two spellings of one name are one
/// filter; and that `Data` is omitted for every other `Type`, so a type that takes no
/// `Data` has none in its identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct XssMatchTupleKey {
    pub field_type: String,
    pub data: Option<String>,
    pub text_transformation: String,
}

/// The identity of `tuple` — see [`XssMatchTupleKey`].
pub fn xss_match_tuple_key(tuple: &XssMatchTuple) -> XssMatchTupleKey {
    let field_type = tuple.field_to_match.r#type.clone();
    let data = tuple
        .field_to_match
        .data
        .as_deref()
        .filter(|_| match_field_takes_data(&field_type))
        .map(str::to_lowercase);
    XssMatchTupleKey {
        field_type,
        data,
        text_transformation: tuple.text_transformation.clone(),
    }
}

/// One stored `XssMatchSet`.
#[derive(Debug, Clone)]
pub struct XssMatchSetEntry {
    pub xss_match_set_id: String,
    pub name: String,
    /// The filters in insertion order, deduplicated by [`XssMatchTupleKey`]. The
    /// modelled shape is stored as it was accepted, so a read reports the stored record
    /// rather than a re-rendering of it.
    pub tuples: Vec<XssMatchTuple>,
    /// Membership index over the [`XssMatchTupleKey`]s of `tuples`, kept in step with it
    /// by [`XssMatchSetEntry::insert_tuple`] and [`XssMatchSetEntry::remove_tuple`], so
    /// deduplicating one update costs a lookup rather than a scan of the whole set.
    keys: BTreeSet<XssMatchTupleKey>,
}

impl XssMatchSetEntry {
    /// A new, empty XssMatchSet record.
    pub fn new(xss_match_set_id: String, name: String) -> Self {
        Self {
            xss_match_set_id,
            name,
            tuples: Vec::new(),
            keys: BTreeSet::new(),
        }
    }

    /// This record projected onto the modelled `XssMatchSet` shape the Get and Create
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> XssMatchSet {
        XssMatchSet {
            xss_match_set_id: self.xss_match_set_id.clone(),
            name: Some(self.name.clone()),
            xss_match_tuples: self.tuples.clone(),
        }
    }

    /// This record projected onto the `XssMatchSetSummary` shape `ListXssMatchSets`
    /// returns.
    pub fn summary(&self) -> XssMatchSetSummary {
        XssMatchSetSummary {
            xss_match_set_id: self.xss_match_set_id.clone(),
            name: self.name.clone(),
        }
    }

    /// Add a filter, unless one with the same identity is already stored. Answers
    /// whether the set changed.
    pub fn insert_tuple(&mut self, tuple: &XssMatchTuple) -> bool {
        if !self.keys.insert(xss_match_tuple_key(tuple)) {
            return false;
        }
        self.tuples.push(tuple.clone());
        true
    }

    /// Remove the filter whose identity equals `tuple`'s, if the set holds one. Answers
    /// whether the set changed.
    pub fn remove_tuple(&mut self, tuple: &XssMatchTuple) -> bool {
        let key = xss_match_tuple_key(tuple);
        if !self.keys.remove(&key) {
            return false;
        }
        self.tuples.retain(|stored| xss_match_tuple_key(stored) != key);
        true
    }
}

/// One `Predicate` stored on a `Rule`.
///
/// The predicates themselves arrive through `UpdateRule`; a rule `CreateRule` just
/// made holds none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateEntry {
    /// Whether the predicate is negated.
    pub negated: bool,
    /// `PredicateType` as it goes on the wire (`IPMatch`, `ByteMatch`, …).
    pub kind: String,
    /// The id of the condition set this predicate points at.
    pub data_id: String,
}

impl PredicateEntry {
    /// This record projected onto the modelled `Predicate` shape.
    pub fn to_modelled(&self) -> Predicate {
        Predicate {
            negated: self.negated,
            r#type: self.kind.clone(),
            data_id: self.data_id.clone(),
        }
    }
}

/// One stored `Rule`.
#[derive(Debug, Clone)]
pub struct RuleEntry {
    pub rule_id: String,
    pub name: String,
    pub metric_name: String,
    /// The predicates in insertion order; empty until `UpdateRule` adds any.
    pub predicates: Vec<PredicateEntry>,
    /// The resource's tag set. A tag set is keyed by `Key`, so the last value given
    /// for a key is the value the resource carries.
    pub tags: BTreeMap<String, String>,
}

impl RuleEntry {
    /// A new rule holding no predicates and no tags.
    pub fn new(rule_id: String, name: String, metric_name: String) -> Self {
        Self {
            rule_id,
            name,
            metric_name,
            predicates: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    /// This record projected onto the modelled `Rule` shape the Create and Get
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> Rule {
        Rule {
            rule_id: self.rule_id.clone(),
            name: Some(self.name.clone()),
            metric_name: Some(self.metric_name.clone()),
            predicates: self
                .predicates
                .iter()
                .map(PredicateEntry::to_modelled)
                .collect(),
        }
    }

    /// This record projected onto the `RuleSummary` shape `ListRules` returns: the
    /// identifier and the friendly name, taken from the stored record.
    pub fn summary(&self) -> RuleSummary {
        RuleSummary {
            rule_id: self.rule_id.clone(),
            name: self.name.clone(),
        }
    }
}

/// One stored `SqlInjectionMatchSet`.
#[derive(Debug, Clone)]
pub struct SqlInjectionMatchSetEntry {
    pub sql_injection_match_set_id: String,
    pub name: String,
    /// The `SqlInjectionMatchTuple`s in insertion order. A newly created set holds
    /// none: `CreateSqlInjectionMatchSet`'s documentation sends the caller to
    /// `UpdateSqlInjectionMatchSet` "to specify the parts of web requests in which you
    /// want to allow, block, or count malicious SQL code".
    pub tuples: Vec<SqlInjectionMatchTuple>,
}

impl SqlInjectionMatchSetEntry {
    /// A new, empty `SqlInjectionMatchSet` record.
    pub fn new(sql_injection_match_set_id: String, name: String) -> Self {
        Self {
            sql_injection_match_set_id,
            name,
            tuples: Vec::new(),
        }
    }

    /// This record projected onto the modelled `SqlInjectionMatchSet` shape the Create
    /// and Get responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> SqlInjectionMatchSet {
        SqlInjectionMatchSet {
            sql_injection_match_set_id: self.sql_injection_match_set_id.clone(),
            name: Some(self.name.clone()),
            sql_injection_match_tuples: self.tuples.clone(),
        }
    }
}

/// One stored `RuleGroup`.
///
/// A rule group's activated rules are not held here: `CreateRuleGroup` documents that
/// it creates "An empty RuleGroup" and that "You use UpdateRuleGroup to add rules to
/// the rule group", and the modelled `RuleGroup` shape carries only the id, the name
/// and the metric name.
#[derive(Debug, Clone)]
pub struct RuleGroupEntry {
    pub rule_group_id: String,
    pub name: String,
    /// The metric name, fixed at creation: "You can't change the name of the metric
    /// after you create the RuleGroup."
    pub metric_name: String,
    /// The tags the create request carried, keyed by `Key` as `RuleEntry::tags` is, so
    /// a key repeated in one request keeps the last value given for it. NOTHING READS
    /// THIS YET: `TagResource`, `UntagResource` and `ListTagsForResource` are still
    /// scaffold stubs, so no test exercises it — whoever implements them reads the tags
    /// from here.
    pub tags: BTreeMap<String, String>,
}

impl RuleGroupEntry {
    /// A new rule group holding no rules and no tags.
    pub fn new(rule_group_id: String, name: String, metric_name: String) -> Self {
        Self {
            rule_group_id,
            name,
            metric_name,
            tags: BTreeMap::new(),
        }
    }

    /// This record projected onto the modelled `RuleGroup` shape the Create and Get
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> RuleGroup {
        RuleGroup {
            rule_group_id: self.rule_group_id.clone(),
            name: Some(self.name.clone()),
            metric_name: Some(self.metric_name.clone()),
        }
    }
}

/// One minted change token.
#[derive(Debug, Clone)]
pub struct ChangeTokenRecord {
    /// A `ChangeTokenStatus` wire value: [`CHANGE_TOKEN_PROVISIONED`],
    /// [`CHANGE_TOKEN_PENDING`] or [`CHANGE_TOKEN_INSYNC`].
    pub status: String,
    /// When the clock owes this token `INSYNC`, set when a mutating call consumed it.
    pub insync_at: Option<Instant>,
    /// Mint order, so the oldest settled records are the ones dropped once
    /// [`MAX_CHANGE_TOKENS`] is reached.
    pub seq: u64,
}

/// Per-service state for com-amazonaws-wafregional-awswaf-regional-20161128. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafregionalAwswafRegional20161128State {
    /// Every stored `IPSet`, keyed by its `IPSetId`. A `BTreeMap` so `ListIPSets`
    /// walks them in one stable order and a `NextMarker` taken on one page resumes at
    /// the right place on the next.
    pub ip_sets: BTreeMap<String, IPSetEntry>,
    /// Every stored `WebACL`, keyed by its `WebACLId`. A `BTreeMap` so `ListWebACLs`
    /// walks them in one stable order and a `NextMarker` taken on one page resumes at
    /// the right place on the next.
    pub web_acls: BTreeMap<String, WebACLEntry>,
    /// Every stored `Rule`, keyed by its `RuleId`. A `BTreeMap` for the same reason as
    /// [`Self::ip_sets`]: one stable order for the paginated `ListRules`.
    pub rules: BTreeMap<String, RuleEntry>,
    /// Every stored `SqlInjectionMatchSet`, keyed by its `SqlInjectionMatchSetId`. A
    /// `BTreeMap` for the same reason as `ip_sets`: one stable walk order for the
    /// paginated list.
    pub sql_injection_match_sets: BTreeMap<String, SqlInjectionMatchSetEntry>,
    /// Every stored `XssMatchSet`, keyed by its `XssMatchSetId`. A `BTreeMap` so
    /// `ListXssMatchSets` walks them in one stable order and a `NextMarker` taken on one
    /// page resumes at the right place on the next.
    pub xss_match_sets: BTreeMap<String, XssMatchSetEntry>,
    /// Every stored `RuleGroup`, keyed by its `RuleGroupId`. A `BTreeMap` for the same
    /// reason as [`Self::ip_sets`]: one stable order for a paginated listing.
    pub rule_groups: BTreeMap<String, RuleGroupEntry>,
    /// Every change token this service minted and has not yet dropped, keyed by the
    /// token value, so `GetChangeTokenStatus` can report one and a mutating call can
    /// tell a fresh token from a used one.
    pub change_tokens: BTreeMap<String, ChangeTokenRecord>,
    /// The token `GetChangeToken` hands out until a mutating call consumes it.
    ///
    /// `GetChangeToken` documents this: "If your application submits a
    /// `GetChangeToken` request and then submits a second `GetChangeToken` request
    /// before submitting a create, update, or delete request, the second
    /// `GetChangeToken` request returns the same value as the first."
    pub outstanding_change_token: Option<String>,
    /// Monotonic counter behind every minted id and change token.
    next_seq: u64,
}

impl ComAmazonawsWafregionalAwswafRegional20161128State {
    /// Mint a fresh, unique 36-character UUID-shaped id, as AWS WAF Classic's
    /// `ResourceId` and `ChangeToken` values are.
    ///
    /// The monotonic sequence number occupies the last group, so every value this
    /// method returns differs from every other one it has returned.
    pub fn mint_id(&mut self) -> String {
        self.next_seq = self.next_seq.wrapping_add(1);
        let seq = self.next_seq;
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            (seq >> 16) as u32,
            (seq >> 48) as u16,
            (seq >> 36) & 0xfff,
            (seq >> 24) & 0xfff,
            seq & 0xffff_ffff_ffff,
        )
    }

    /// The change token `GetChangeToken` answers with: the outstanding one when a
    /// minted token has not been consumed yet, else a freshly minted `PROVISIONED`
    /// one.
    pub fn change_token(&mut self) -> String {
        if let Some(outstanding) = &self.outstanding_change_token {
            if self.change_tokens.contains_key(outstanding) {
                return outstanding.clone();
            }
        }
        let token = self.mint_id();
        let seq = self.next_seq;
        self.change_tokens.insert(
            token.clone(),
            ChangeTokenRecord {
                status: CHANGE_TOKEN_PROVISIONED.to_string(),
                insync_at: None,
                seq,
            },
        );
        self.outstanding_change_token = Some(token.clone());
        self.prune_change_tokens();
        token
    }

    /// Consume `token` for a mutating call: a `PROVISIONED` token becomes `PENDING`
    /// and the clock owes it `INSYNC` [`CHANGE_TOKEN_PROPAGATION`] later.
    ///
    /// Answers `false` for a token this service never minted, and for one a mutating
    /// call already used — the caller answers both with the modelled
    /// `WAFStaleDataException`, since `GetChangeToken` requires that "each create,
    /// update, or delete request must use a unique change token".
    pub fn consume_change_token(&mut self, token: &str) -> bool {
        let Some(record) = self.change_tokens.get_mut(token) else {
            return false;
        };
        if record.status != CHANGE_TOKEN_PROVISIONED {
            return false;
        }
        record.status = CHANGE_TOKEN_PENDING.to_string();
        record.insync_at = Some(Instant::now() + CHANGE_TOKEN_PROPAGATION);
        if self.outstanding_change_token.as_deref() == Some(token) {
            self.outstanding_change_token = None;
        }
        true
    }

    /// The status of `token`, or `None` when this service holds no such token.
    ///
    /// This materialises the one transition the CLOCK owes — a consumed token whose
    /// propagation span has elapsed becomes `INSYNC` — which was due at that instant
    /// whether or not anybody asked. Nothing else about the token changes on a read.
    pub fn change_token_status(&mut self, token: &str) -> Option<String> {
        let now = Instant::now();
        let record = self.change_tokens.get_mut(token)?;
        if record.insync_at.is_some_and(|due| now >= due) {
            record.status = CHANGE_TOKEN_INSYNC.to_string();
            record.insync_at = None;
        }
        Some(record.status.clone())
    }

    /// Drop the oldest tokens no mutating call can still consume, until at most
    /// [`MAX_CHANGE_TOKENS`] records remain. The outstanding token is never dropped.
    fn prune_change_tokens(&mut self) {
        while self.change_tokens.len() > MAX_CHANGE_TOKENS {
            let oldest = self
                .change_tokens
                .iter()
                .filter(|(token, _)| {
                    self.outstanding_change_token.as_deref() != Some(token.as_str())
                })
                .min_by_key(|(_, record)| record.seq)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.change_tokens.remove(&token);
                }
                None => break,
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafregionalAwswafRegional20161128State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafregionalAwswafRegional20161128State>)
}
