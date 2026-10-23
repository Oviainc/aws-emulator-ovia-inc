// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafregional-awswaf-regional-20161128 service.
//!
//! AWS WAF Classic's `IPSet`, `RateBasedRule`, `RegexPatternSet`, `Rule`, `RuleGroup`,
//! `SizeConstraintSet`, `SqlInjectionMatchSet`, `WebACL` and `XssMatchSet` records and
//! the change tokens every mutating call consumes live here, so `GetChangeToken`,
//! `GetChangeTokenStatus` and the `Create*`, `Update*`, `Get*`, `List*` and `Delete*`
//! handlers of every resource kind all read and write ONE copy of each resource. The
//! helpers below (id minting, the change-token protocol, pagination, CIDR parsing,
//! metric-name validation, resource-ARN resolution onto the tagged resource's own tag
//! set, filter identity, the `IPSet`/`IPSetSummary`,
//! `RateBasedRule`/`RuleSummary`, `RegexPatternSet`, `Rule`, `RuleGroup`,
//! `SizeConstraintSet`, `SqlInjectionMatchSet`, `WebACL` and
//! `XssMatchSet`/`XssMatchSetSummary` projections) are shared by those handlers rather
//! than duplicated per operation.
//!
//! The `ByteMatchSet` records and bounds live at the END of this file, under "AWS WAF
//! Classic ByteMatchSet records", so this one store carries them beside every other
//! resource kind.
//!
//! The IAM policy `PutPermissionPolicy` attaches to a `RuleGroup` lives on that rule
//! group's own record ([`RuleGroupEntry::permission_policy`]); the restrictions it must
//! satisfy are at the end of this file, under "AWS WAF Classic RuleGroup permission
//! policies".

//! `GeoMatchSet` records live in the same store, under [`GeoMatchSetEntry`] at the end
//! of this file: `CreateGeoMatchSet` stores one and `GetGeoMatchSet` projects it,
//! through that same id minting and change-token protocol.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::ops::types::{
    ActivatedRule, IPSet, IPSetDescriptor, IPSetSummary, Predicate, RateBasedRule,
    RegexPatternSet, Rule, RuleGroup, RuleSummary, SizeConstraint, SizeConstraintSet,
    SqlInjectionMatchSet, SqlInjectionMatchTuple, WafAction, WebACL, XssMatchSet,
    XssMatchSetSummary, XssMatchTuple,
};

// The shapes the ByteMatchSet records at the end of this file project onto. A `use` of
// their own, so the shared list above stays exactly as the other resource kinds leave it.
// `FieldToMatch` comes in here for the whole module: beside the ByteMatchSet projections
// it is what the shared match-filter identity keys the XssMatchSet and
// SqlInjectionMatchSet filters on.
use crate::ops::types::{ByteMatchSet, ByteMatchTuple, FieldToMatch};

// The GeoMatchSet shapes, kept in their own `use` for the same reason: this list stays
// beside the record that projects them.
use crate::ops::types::{GeoMatchConstraint, GeoMatchSet};

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

/// The most `RegexPatternSet`s this emulator stores for the account before
/// `CreateRegexPatternSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota and this file cites none, so this bound is CHOSEN: every
/// created pattern set is a stored record, so the number a caller can create has to be
/// bounded the way [`MAX_IP_SETS`] bounds IPSets. It is deliberately smaller in
/// magnitude than its siblings ([`MAX_IP_SETS`], [`MAX_SQL_INJECTION_MATCH_SETS`]),
/// which carry AWS's published per-account figures: no figure is cited for pattern
/// sets, so the bound is only large enough to be well above the handful of pattern sets
/// a regex match condition refers to, and small enough that a test can reach it.
pub const MAX_REGEX_PATTERN_SETS: usize = 10;

/// The most `RegexPatternString`s one `RegexPatternSet` may hold.
///
/// Source: the model itself — `RegexPatternSet$RegexPatternStrings` is `length 0..10`
/// (`smithy_query(query="shape",shape="RegexPatternSet")`), so a set holding more could
/// not be serialized onto its own shape. `UpdateRegexPatternSet` answers its modelled
/// `WAFLimitsExceededException` for an insert that would pass this bound.
pub const MAX_REGEX_PATTERN_STRINGS: usize = 10;

/// The most `RegexPatternSetUpdate`s one `UpdateRegexPatternSet` request may carry.
///
/// The model bounds `RegexPatternSetUpdates` only below (`length 1..`), so this bound is
/// CHOSEN, not cited, on the same reasoning as
/// [`MAX_SQL_INJECTION_MATCH_SET_UPDATES_PER_REQUEST`]: a request can at most empty a set
/// ([`MAX_REGEX_PATTERN_STRINGS`] deletes) and then fill it again
/// ([`MAX_REGEX_PATTERN_STRINGS`] inserts), since a delete must name a pattern the set
/// holds and an insert one it does not, so no larger request can leave the set in a state
/// a request of this size cannot. It caps the work one caller can ask for before the
/// handler takes the shared store's lock.
pub const MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST: usize = 2 * MAX_REGEX_PATTERN_STRINGS;

/// The most `SqlInjectionMatchSet`s this emulator stores for the account before
/// `CreateSqlInjectionMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 conditions of each type, a `SqlInjectionMatchSet` being one condition),
/// which keeps the match-set state a caller can grow bounded.
pub const MAX_SQL_INJECTION_MATCH_SETS: usize = 100;

/// The most `SqlInjectionMatchTuple`s one `SqlInjectionMatchSet` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes per SQL
/// injection match condition (10 filters). Together with
/// [`MAX_SQL_INJECTION_MATCH_SETS`] it bounds the filter state a caller can grow over
/// many `UpdateSqlInjectionMatchSet` requests, not just the number of match sets.
pub const MAX_SQL_INJECTION_MATCH_TUPLES: usize = 10;

/// The most `SqlInjectionMatchSetUpdate`s one `UpdateSqlInjectionMatchSet` request may
/// carry.
///
/// The model states no per-request quota for this operation, so this bound is CHOSEN,
/// not cited, on the same reasoning as [`MAX_XSS_MATCH_SET_UPDATES_PER_REQUEST`]: a
/// request can at most empty a set ([`MAX_SQL_INJECTION_MATCH_TUPLES`] deletes) and then
/// fill it again ([`MAX_SQL_INJECTION_MATCH_TUPLES`] inserts), since a delete must name a
/// filter the set holds and an insert one it does not, so no larger request can leave the
/// set in a state a request of this size cannot. It caps the work one caller can ask for
/// before the handler takes the shared store's lock.
pub const MAX_SQL_INJECTION_MATCH_SET_UPDATES_PER_REQUEST: usize =
    2 * MAX_SQL_INJECTION_MATCH_TUPLES;

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

/// The most `SizeConstraintSet`s this emulator stores for the account before
/// `CreateSizeConstraintSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 conditions of each type, a `SizeConstraintSet` being one condition —
/// the same source as [`MAX_SQL_INJECTION_MATCH_SETS`]), which keeps the
/// SizeConstraintSet state a caller can grow bounded.
pub const MAX_SIZE_CONSTRAINT_SETS: usize = 100;

/// The most `SizeConstraint`s one `SizeConstraintSet` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes per
/// size constraint condition (10 filters — the same source as
/// [`MAX_SQL_INJECTION_MATCH_TUPLES`]). Together with [`MAX_SIZE_CONSTRAINT_SETS`] it
/// bounds the constraint state a caller can grow over many `UpdateSizeConstraintSet`
/// requests, not just the number of sets.
pub const MAX_SIZE_CONSTRAINTS: usize = 10;

/// The most `SizeConstraintSetUpdate`s one `UpdateSizeConstraintSet` request may carry.
///
/// The model states no per-request quota for this operation, so this bound is CHOSEN,
/// not cited, on the same reasoning as
/// [`MAX_SQL_INJECTION_MATCH_SET_UPDATES_PER_REQUEST`]: a request can at most empty a
/// set ([`MAX_SIZE_CONSTRAINTS`] deletes) and then fill it again
/// ([`MAX_SIZE_CONSTRAINTS`] inserts), since a delete must name a constraint the set
/// holds and an insert one it does not, so no larger request can leave the set in a
/// state a request of this size cannot. It caps the work one caller can ask for before
/// the handler takes the shared store's lock.
pub const MAX_SIZE_CONSTRAINT_SET_UPDATES_PER_REQUEST: usize = 2 * MAX_SIZE_CONSTRAINTS;

/// The largest `Size` a `SizeConstraint` may compare against, in bytes.
///
/// Source: `Size`'s modelled `@range` of 0..21474836480, which its own documentation
/// repeats — "Valid values for size are 0 - 21474836480 bytes (0 - 20 GB)".
pub const MAX_SIZE_CONSTRAINT_SIZE: i64 = 21_474_836_480;

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

/// The largest page a paginated `List*` returns (`ListIPSets`, `ListRules`,
/// `ListRateBasedRules`, …), and the page size a `Limit` of 0 (the model's declared
/// default) asks for.
///
/// Source: `PaginationLimit`'s modelled `@range` of 0..100.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most `RateBasedRule`s this emulator stores for the account before
/// `CreateRateBasedRule` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is NOT modelled and NOT cited: it is the
/// per-account rate-based-rule quota AWS WAF Classic publishes in its Limits page (10),
/// chosen because every rate-based rule is a stored record, so the number a caller can
/// create has to be bounded somewhere. Raising it changes only how many rules one
/// account may hold, never how a stored rule behaves.
pub const MAX_RATE_BASED_RULES: usize = 10;

/// The most `Rule`s this emulator stores for the account before `CreateRule` answers
/// its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an account (100 rules); every `Rule` is a stored record, so the number a caller can
/// create has to be bounded somewhere.
pub const MAX_RULES: usize = 100;

/// The most `Predicate`s one `Rule` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes per
/// rule (10 conditions per rule); with [`MAX_RULES`] bounding the number of rules it
/// keeps the predicate state a caller can grow bounded.
pub const MAX_RULE_PREDICATES: usize = 10;

/// The most `RuleUpdate`s one `UpdateRule` request may carry.
///
/// The model states no per-request quota for this operation, so this bound is CHOSEN,
/// not cited, on the same reasoning as [`MAX_XSS_MATCH_SET_UPDATES_PER_REQUEST`]: a
/// request can at most empty a rule ([`MAX_RULE_PREDICATES`] deletes) and then fill it
/// again ([`MAX_RULE_PREDICATES`] inserts), since a delete must name a predicate the
/// rule holds and an insert one it does not, so no larger request can leave the rule in
/// a state a request of this size cannot. It caps the work one caller can ask for
/// before the handler takes the shared store's lock.
pub const MAX_RULE_UPDATES_PER_REQUEST: usize = 2 * MAX_RULE_PREDICATES;

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
/// Shared by every handler whose input carries a `MetricName` (`CreateRule`,
/// `CreateRateBasedRule`, and the `CreateRuleGroup` / `CreateWebACL` handlers once they
/// are implemented), so one rule is enforced in one place.
pub fn metric_name_is_allowed(metric_name: &str) -> bool {
    !metric_name.is_empty()
        && metric_name.chars().all(|c| c.is_ascii_alphanumeric())
        && !RESERVED_METRIC_NAMES.contains(&metric_name)
}

/// One page of a `List*` operation over records keyed by their resource id, plus the
/// `NextMarker` that resumes the walk, or `None` when nothing remains past the page.
///
/// This is the pagination rule every AWS WAF Classic `List*` operation follows, held
/// once rather than per operation: `Limit` bounds the page (0 — the model's declared
/// default — asks for the service's own page size, [`MAX_PAGE_SIZE`]), `NextMarker`
/// names the id the page resumes at, and the returned marker is the id of the first
/// record AFTER the page, so it is present exactly when a record remains.
pub fn page<'records, Record>(
    records: &'records BTreeMap<String, Record>,
    limit: i32,
    next_marker: Option<&str>,
) -> (Vec<&'records Record>, Option<String>) {
    // `Limit`'s `@range` of 0..100 is enforced at the boundary, so a larger value
    // cannot arrive; it is bounded here anyway.
    let page_size = match usize::try_from(limit) {
        Ok(0) => MAX_PAGE_SIZE,
        Ok(limit) => limit.min(MAX_PAGE_SIZE),
        Err(_) => MAX_PAGE_SIZE,
    };
    // Borrowed bounds, so resuming copies no id.
    let start = match next_marker {
        Some(marker) => ::std::ops::Bound::Included(marker),
        None => ::std::ops::Bound::Unbounded,
    };
    let mut remaining = records.range::<str, _>((start, ::std::ops::Bound::Unbounded));
    let page: Vec<&Record> = remaining
        .by_ref()
        .take(page_size)
        .map(|(_, record)| record)
        .collect();
    let next_marker = remaining.next().map(|(id, _)| id.clone());
    (page, next_marker)
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
/// The fixed sentence for `WAFReferencedItemException`, the refusal of a delete of a
/// resource another resource still points at, from the shape's own documentation ("you
/// tried to delete an object that is still in use") — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_REFERENCED_ITEM: &str = "The referenced item is still in use.";
/// The fixed sentence for `WAFInvalidOperationException`, the refusal of an update that
/// would leave the resource as it was — see [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_INVALID_OPERATION: &str = "The operation is not valid.";
/// The fixed sentence for `WAFInvalidPermissionPolicyException`, the refusal of a
/// permission policy outside the restrictions `PutPermissionPolicy` documents — see
/// [`MESSAGE_DISALLOWED_NAME`].
pub const MESSAGE_INVALID_PERMISSION_POLICY: &str =
    "The specific permission policy is not valid.";

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

/// The ARN resource type of a `Rule`: the segment before the `RuleId` in
/// `arn:aws:waf-regional:<region>:<account-id>:rule/<RuleId>`.
pub const ARN_TYPE_RULE: &str = "rule";

/// The ARN resource type of a `WebACL`, as [`web_acl_arn`] mints it.
pub const ARN_TYPE_WEB_ACL: &str = "webacl";

/// The ARN resource type of a `RuleGroup`.
pub const ARN_TYPE_RULE_GROUP: &str = "rulegroup";

/// The ARN resource type of a `RateBasedRule`, which AWS WAF Classic names apart from
/// a plain `Rule` even though the two are the same kind of resource.
pub const ARN_TYPE_RATE_BASED_RULE: &str = "ratebasedrule";

/// The resource kinds AWS WAF Classic carries tags for, by their ARN resource type.
///
/// Source: `TagResource`'s own documentation — "You can use this action to tag the AWS
/// resources that you manage through AWS WAF Classic: web ACLs, rule groups, and
/// rules." Every other kind this service holds (a condition such as an `IPSet` or an
/// `XssMatchSet`) carries no tags, so an ARN naming one names no tag set.
pub const TAGGABLE_ARN_TYPES: &[&str] = &[
    ARN_TYPE_WEB_ACL,
    ARN_TYPE_RULE_GROUP,
    ARN_TYPE_RULE,
    ARN_TYPE_RATE_BASED_RULE,
];

/// The `(resource type, resource id)` an AWS WAF Classic regional resource ARN names,
/// or `None` when `arn` is not one.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account-id>:<resource-type>/<id>`, so
/// this takes six colon-separated fields whose first is `arn`, whose service segment is
/// [`ARN_SERVICE`], and whose resource field is `<type>/<id>` with a non-empty type and
/// a non-empty id.
///
/// The partition, region and account fields are required to be PRESENT but their values
/// are not matched: this emulator serves a single account and stores no region on a
/// resource, so the resource id is the only field of an ARN that can identify a record
/// it holds.
pub fn parse_resource_arn(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.split(':');
    let prefix = fields.next()?;
    let _partition = fields.next()?;
    let service = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    if fields.next().is_some() || prefix != "arn" || service != ARN_SERVICE {
        return None;
    }
    let (resource_type, resource_id) = resource.split_once('/')?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some((resource_type, resource_id))
}

/// The `(resource type, resource id)` a taggable AWS WAF Classic regional resource ARN
/// names, or `None` when `arn` names no taggable kind.
///
/// The ARN itself is parsed by [`parse_resource_arn`]; this adds the one restriction
/// the tagging operations carry, that the kind it names is in [`TAGGABLE_ARN_TYPES`].
pub fn parse_taggable_resource_arn(arn: &str) -> Option<(&str, &str)> {
    parse_resource_arn(arn).filter(|(resource_type, _)| TAGGABLE_ARN_TYPES.contains(resource_type))
}

/// The `RuleGroupId` the `ResourceArn` of a permission-policy call names, or `None`
/// when `arn` is not a WAF `RuleGroup` ARN.
///
/// Source: `PutPermissionPolicy`'s own documentation — "The ARN in the request must be a
/// valid WAF RuleGroup ARN"; the policy operations are documented for that one kind
/// ("The only supported use for this action is to share a RuleGroup across accounts").
pub fn parse_rule_group_arn(arn: &str) -> Option<&str> {
    let (resource_type, resource_id) = parse_resource_arn(arn)?;
    (resource_type == ARN_TYPE_RULE_GROUP).then_some(resource_id)
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

/// One stored `RegexPatternSet`.
#[derive(Debug, Clone)]
pub struct RegexPatternSetEntry {
    pub regex_pattern_set_id: String,
    pub name: String,
    /// The `RegexPatternString`s this set holds, in insertion order.
    ///
    /// Empty on creation: `CreateRegexPatternSet` documents that it answers "a
    /// RegexPatternSet that contains no objects", and directs the caller to
    /// `UpdateRegexPatternSet` "to specify the regular expression (regex) pattern that
    /// you want AWS WAF to search for".
    pub patterns: Vec<String>,
}

impl RegexPatternSetEntry {
    /// A new, empty RegexPatternSet record.
    pub fn new(regex_pattern_set_id: String, name: String) -> Self {
        Self {
            regex_pattern_set_id,
            name,
            patterns: Vec::new(),
        }
    }

    /// This record projected onto the modelled `RegexPatternSet` shape the Get and
    /// Create responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> RegexPatternSet {
        RegexPatternSet {
            regex_pattern_set_id: self.regex_pattern_set_id.clone(),
            name: Some(self.name.clone()),
            regex_pattern_strings: self.patterns.clone(),
        }
    }

    /// Add `pattern`, unless the set already holds it. Answers whether the set changed.
    ///
    /// A `RegexPatternSet` is a SET of pattern strings, so one stored twice would be one
    /// member of it: `UpdateRegexPatternSet` answers the second insert with its modelled
    /// `WAFInvalidOperationException` ("there was nothing to do") instead.
    pub fn insert_pattern(&mut self, pattern: &str) -> bool {
        if self.patterns.iter().any(|stored| stored == pattern) {
            return false;
        }
        self.patterns.push(pattern.to_string());
        true
    }

    /// Remove `pattern`, if the set holds it. Answers whether the set changed.
    pub fn remove_pattern(&mut self, pattern: &str) -> bool {
        let before = self.patterns.len();
        self.patterns.retain(|stored| stored != pattern);
        self.patterns.len() != before
    }
}

/// One `SizeConstraint`'s identity: the part of the request it measures and the
/// transformation applied first (the shared [`MatchFilterKey`]), plus the comparison it
/// makes against a size.
///
/// `UpdateSizeConstraintSet` documents that "if you want to change a
/// SizeConstraintSetUpdate object, you delete the existing object and add a new one", so
/// two constraints are the same constraint exactly when every member of the modelled
/// `SizeConstraint` agrees — which is what the derived `Eq` here means, with the `Data`
/// name normalised the one way [`match_filter_key`] normalises it (it is "not case
/// sensitive", and omitted for the types that name no header or query argument).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SizeConstraintKey {
    /// What part of the request is measured, and the text transformation applied before
    /// measuring it.
    pub filter: MatchFilterKey,
    /// A `ComparisonOperator` value.
    pub comparison_operator: String,
    /// The `Size` the comparison is against, in bytes.
    pub size: i64,
}

/// The identity of `constraint` — see [`SizeConstraintKey`].
pub fn size_constraint_key(constraint: &SizeConstraint) -> SizeConstraintKey {
    SizeConstraintKey {
        filter: match_filter_key(&constraint.field_to_match, &constraint.text_transformation),
        comparison_operator: constraint.comparison_operator.clone(),
        size: constraint.size,
    }
}

/// One stored `SizeConstraintSet`.
///
/// A set is created empty — `CreateSizeConstraintSet` documents that it answers "a
/// `SizeConstraintSet` that contains no `SizeConstraint` objects" — and its
/// constraints arrive through `UpdateSizeConstraintSet`.
#[derive(Debug, Clone)]
pub struct SizeConstraintSetEntry {
    pub size_constraint_set_id: String,
    pub name: String,
    /// The constraints in insertion order, deduplicated by [`SizeConstraintKey`]. The
    /// modelled shape is stored as it was accepted, so a read reports the stored record
    /// rather than a re-rendering of it. No separate membership index is kept beside it
    /// (as `ByteMatchSetEntry` keeps one): one set holds at most
    /// [`MAX_SIZE_CONSTRAINTS`] constraints, so a lookup over this list is a walk of a
    /// bounded handful, and the list stays the single copy of the record.
    pub size_constraints: Vec<SizeConstraint>,
}

impl SizeConstraintSetEntry {
    /// A new, empty `SizeConstraintSet` record.
    pub fn new(size_constraint_set_id: String, name: String) -> Self {
        Self {
            size_constraint_set_id,
            name,
            size_constraints: Vec::new(),
        }
    }

    /// This record projected onto the modelled `SizeConstraintSet` shape the Get and
    /// Create responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> SizeConstraintSet {
        SizeConstraintSet {
            size_constraint_set_id: self.size_constraint_set_id.clone(),
            name: Some(self.name.clone()),
            size_constraints: self.size_constraints.clone(),
        }
    }

    /// Whether this set holds a constraint whose identity equals `key`'s.
    fn holds(&self, key: &SizeConstraintKey) -> bool {
        self.size_constraints
            .iter()
            .any(|stored| &size_constraint_key(stored) == key)
    }

    /// Add a constraint, unless one with the same identity is already stored. Answers
    /// whether the set changed.
    pub fn insert_constraint(&mut self, constraint: &SizeConstraint) -> bool {
        if self.holds(&size_constraint_key(constraint)) {
            return false;
        }
        self.size_constraints.push(constraint.clone());
        true
    }

    /// Remove the constraint whose identity equals `constraint`'s, if the set holds one.
    /// Answers whether the set changed.
    pub fn remove_constraint(&mut self, constraint: &SizeConstraint) -> bool {
        let key = size_constraint_key(constraint);
        if !self.holds(&key) {
            return false;
        }
        self.size_constraints
            .retain(|stored| size_constraint_key(stored) != key);
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
    /// a request none of the web ACL's rules match. Never
    /// [`crate::ops::create_web_acl::DEFAULT_ACTION_EXCLUDED`], the one value that
    /// catalog excludes for a default action.
    pub default_action: String,
    /// The rules activated in this web ACL, in priority order. A newly created web ACL
    /// holds none: they arrive through `UpdateWebACL`.
    pub rules: Vec<ActivatedRule>,
    /// The resource's tags, as a key→value map — a repeated key is one tag, the last
    /// value winning, as an AWS tag set is. At most [`MAX_TAGS_PER_RESOURCE`] entries,
    /// so one record's tag state is bounded; the tagging operations
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) project and update it
    /// through [`ComAmazonawsWafregionalAwswafRegional20161128State::tags_for_resource`].
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

/// One match filter's identity: the part of the request it inspects and the text
/// transformation applied before inspecting it.
///
/// `FieldToMatch`'s `Data` documentation states that the header name and the
/// query-parameter name are "not case sensitive", so two spellings of one name are one
/// filter; and that `Data` is omitted for every other `Type`, so a type that takes no
/// `Data` has none in its identity.
///
/// Shared by the `XssMatchSet` and `SqlInjectionMatchSet` filters, whose modelled shapes
/// are both a `FieldToMatch` plus a `TextTransformation` — one rule of identity, in one
/// place, for both.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchFilterKey {
    pub field_type: String,
    pub data: Option<String>,
    pub text_transformation: String,
}

/// The identity of the filter `field_to_match` and `text_transformation` name — see
/// [`MatchFilterKey`].
pub fn match_filter_key(
    field_to_match: &FieldToMatch,
    text_transformation: &str,
) -> MatchFilterKey {
    let field_type = field_to_match.r#type.clone();
    let data = field_to_match
        .data
        .as_deref()
        .filter(|_| match_field_takes_data(&field_type))
        .map(str::to_lowercase);
    MatchFilterKey {
        field_type,
        data,
        text_transformation: text_transformation.to_string(),
    }
}

/// The identity of `tuple` — see [`MatchFilterKey`].
pub fn xss_match_tuple_key(tuple: &XssMatchTuple) -> MatchFilterKey {
    match_filter_key(&tuple.field_to_match, &tuple.text_transformation)
}

/// The identity of `tuple` — see [`MatchFilterKey`].
pub fn sql_injection_match_tuple_key(tuple: &SqlInjectionMatchTuple) -> MatchFilterKey {
    match_filter_key(&tuple.field_to_match, &tuple.text_transformation)
}

/// One stored `XssMatchSet`.
#[derive(Debug, Clone)]
pub struct XssMatchSetEntry {
    pub xss_match_set_id: String,
    pub name: String,
    /// The filters in insertion order, deduplicated by [`MatchFilterKey`]. The
    /// modelled shape is stored as it was accepted, so a read reports the stored record
    /// rather than a re-rendering of it.
    pub tuples: Vec<XssMatchTuple>,
    /// Membership index over the [`MatchFilterKey`]s of `tuples`, kept in step with it
    /// by [`XssMatchSetEntry::insert_tuple`] and [`XssMatchSetEntry::remove_tuple`], so
    /// deduplicating one update costs a lookup rather than a scan of the whole set.
    keys: BTreeSet<MatchFilterKey>,
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

/// The `PredicateType` whose `DataId` names an `IPSet`, from the model's
/// `PREDICATE_TYPE_VALUES`.
pub const PREDICATE_TYPE_IP_MATCH: &str = "IPMatch";
/// The `PredicateType` whose `DataId` names a `SqlInjectionMatchSet`.
pub const PREDICATE_TYPE_SQL_INJECTION_MATCH: &str = "SqlInjectionMatch";
/// The `PredicateType` whose `DataId` names an `XssMatchSet`.
pub const PREDICATE_TYPE_XSS_MATCH: &str = "XssMatch";
/// The `PredicateType` whose `DataId` names a `SizeConstraintSet`.
pub const PREDICATE_TYPE_SIZE_CONSTRAINT: &str = "SizeConstraint";
/// The `PredicateType` whose `DataId` names a `ByteMatchSet`.
pub const PREDICATE_TYPE_BYTE_MATCH: &str = "ByteMatch";
/// The `PredicateType` whose `DataId` names a `GeoMatchSet`.
pub const PREDICATE_TYPE_GEO_MATCH: &str = "GeoMatch";

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
    /// The record for a modelled `Predicate` an `UpdateRule` request carried.
    pub fn from_modelled(predicate: &Predicate) -> Self {
        Self {
            negated: predicate.negated,
            kind: predicate.r#type.clone(),
            data_id: predicate.data_id.clone(),
        }
    }

    /// Whether this record is the predicate `predicate` names.
    ///
    /// A predicate's identity is the whole modelled `Predicate` — `Negated`, `Type` and
    /// `DataId`, all three of which `RuleUpdate` requires — so a `DELETE` naming any one
    /// of them differently names a predicate the rule does not hold.
    pub fn matches(&self, predicate: &Predicate) -> bool {
        self.negated == predicate.negated
            && self.kind == predicate.r#type
            && self.data_id == predicate.data_id
    }

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

    /// Add `predicate`, unless the rule already holds one with the same identity (see
    /// [`PredicateEntry::matches`]). Answers whether the rule changed.
    pub fn insert_predicate(&mut self, predicate: &Predicate) -> bool {
        if self.predicates.iter().any(|stored| stored.matches(predicate)) {
            return false;
        }
        self.predicates
            .push(PredicateEntry::from_modelled(predicate));
        true
    }

    /// Remove the predicate `predicate` names, if the rule holds it. Answers whether the
    /// rule changed.
    pub fn remove_predicate(&mut self, predicate: &Predicate) -> bool {
        let before = self.predicates.len();
        self.predicates.retain(|stored| !stored.matches(predicate));
        self.predicates.len() != before
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

/// One stored `RateBasedRule`.
///
/// A rate-based rule is a `Rule` with a rate limit: the same id, name, metric name and
/// predicates, plus the `RateKey`/`RateLimit` pair, so it is stored as a [`RuleEntry`]
/// beside them.
#[derive(Debug, Clone)]
pub struct RateBasedRuleEntry {
    /// The rule's own record: id, name, metric name, predicates and tags.
    pub rule: RuleEntry,
    /// `RateKey` as it goes on the wire; the model admits only `IP`.
    pub rate_key: String,
    /// The requests allowed in a five-minute period before the rule's action applies.
    pub rate_limit: i64,
}

impl RateBasedRuleEntry {
    /// A new rate-based rule record holding no predicates and no tags.
    pub fn new(
        rule_id: String,
        name: String,
        metric_name: String,
        rate_key: String,
        rate_limit: i64,
    ) -> Self {
        Self {
            rule: RuleEntry::new(rule_id, name, metric_name),
            rate_key,
            rate_limit,
        }
    }

    /// This record projected onto the modelled `RateBasedRule` shape the Create and
    /// Get responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> RateBasedRule {
        RateBasedRule {
            rule_id: self.rule.rule_id.clone(),
            name: Some(self.rule.name.clone()),
            metric_name: Some(self.rule.metric_name.clone()),
            match_predicates: self
                .rule
                .predicates
                .iter()
                .map(PredicateEntry::to_modelled)
                .collect(),
            rate_key: self.rate_key.clone(),
            rate_limit: self.rate_limit,
        }
    }

    /// This record projected onto the `RuleSummary` shape `ListRateBasedRules`
    /// returns.
    pub fn summary(&self) -> RuleSummary {
        RuleSummary {
            rule_id: self.rule.rule_id.clone(),
            name: self.rule.name.clone(),
        }
    }
}

/// One stored `SqlInjectionMatchSet`.
#[derive(Debug, Clone)]
pub struct SqlInjectionMatchSetEntry {
    pub sql_injection_match_set_id: String,
    pub name: String,
    /// The `SqlInjectionMatchTuple`s in insertion order, deduplicated by
    /// [`MatchFilterKey`]. A newly created set holds none:
    /// `CreateSqlInjectionMatchSet`'s documentation sends the caller to
    /// `UpdateSqlInjectionMatchSet` "to specify the parts of web requests in which you
    /// want to allow, block, or count malicious SQL code". The modelled shape is stored
    /// as it was accepted, so a read reports the stored record rather than a
    /// re-rendering of it.
    pub tuples: Vec<SqlInjectionMatchTuple>,
    /// Membership index over the [`MatchFilterKey`]s of `tuples`, kept in step with it
    /// by [`SqlInjectionMatchSetEntry::insert_tuple`] and
    /// [`SqlInjectionMatchSetEntry::remove_tuple`], so deduplicating one update costs a
    /// lookup rather than a scan of the whole set.
    keys: BTreeSet<MatchFilterKey>,
}

impl SqlInjectionMatchSetEntry {
    /// A new, empty `SqlInjectionMatchSet` record.
    pub fn new(sql_injection_match_set_id: String, name: String) -> Self {
        Self {
            sql_injection_match_set_id,
            name,
            tuples: Vec::new(),
            keys: BTreeSet::new(),
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

    /// Add a filter, unless one with the same identity is already stored. Answers
    /// whether the set changed.
    pub fn insert_tuple(&mut self, tuple: &SqlInjectionMatchTuple) -> bool {
        if !self.keys.insert(sql_injection_match_tuple_key(tuple)) {
            return false;
        }
        self.tuples.push(tuple.clone());
        true
    }

    /// Remove the filter whose identity equals `tuple`'s, if the set holds one. Answers
    /// whether the set changed.
    pub fn remove_tuple(&mut self, tuple: &SqlInjectionMatchTuple) -> bool {
        let key = sql_injection_match_tuple_key(tuple);
        if !self.keys.remove(&key) {
            return false;
        }
        self.tuples
            .retain(|stored| sql_injection_match_tuple_key(stored) != key);
        true
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
    /// a key repeated in one request keeps the last value given for it. The tagging
    /// operations (`ListTagsForResource`, `TagResource`, `UntagResource`) read and write
    /// this same set, through
    /// [`ComAmazonawsWafregionalAwswafRegional20161128State::tags_for_resource`].
    pub tags: BTreeMap<String, String>,
    /// The IAM policy `PutPermissionPolicy` attached to this rule group, exactly as the
    /// request gave it, or `None` while none is attached.
    ///
    /// It lives on the rule group's own record because the policy operations address it
    /// by the rule group's ARN ("Attaches an IAM policy to the specified resource. The
    /// only supported use for this action is to share a RuleGroup across accounts"), so
    /// `PutPermissionPolicy`, `GetPermissionPolicy` and `DeletePermissionPolicy` read and
    /// write ONE copy of it, and deleting the rule group takes its policy with it.
    pub permission_policy: Option<String>,
}

impl RuleGroupEntry {
    /// A new rule group holding no rules and no tags.
    pub fn new(rule_group_id: String, name: String, metric_name: String) -> Self {
        Self {
            rule_group_id,
            name,
            metric_name,
            tags: BTreeMap::new(),
            permission_policy: None,
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
    /// Every stored `RegexPatternSet`, keyed by its `RegexPatternSetId`. A `BTreeMap`
    /// for the same reason as [`Self::ip_sets`]: one stable order for the list and
    /// pagination of these records.
    pub regex_pattern_sets: BTreeMap<String, RegexPatternSetEntry>,
    /// Every stored `WebACL`, keyed by its `WebACLId`. A `BTreeMap` so `ListWebACLs`
    /// walks them in one stable order and a `NextMarker` taken on one page resumes at
    /// the right place on the next.
    pub web_acls: BTreeMap<String, WebACLEntry>,
    /// Every stored `Rule`, keyed by its `RuleId`. A `BTreeMap` for the same reason as
    /// [`Self::ip_sets`]: one stable order for the paginated `ListRules`.
    pub rules: BTreeMap<String, RuleEntry>,
    /// Every stored `RateBasedRule`, keyed by its `RuleId`. Rate-based rules are a
    /// resource of their own — `ListRateBasedRules` reports these and `ListRules` the
    /// regular ones — so they live in their own map, walked in the same stable order.
    pub rate_based_rules: BTreeMap<String, RateBasedRuleEntry>,
    /// Every stored `SizeConstraintSet`, keyed by its `SizeConstraintSetId`. A
    /// `BTreeMap` so `ListSizeConstraintSets` walks them in one stable order and a
    /// `NextMarker` taken on one page resumes at the right place on the next.
    pub size_constraint_sets: BTreeMap<String, SizeConstraintSetEntry>,
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
    /// Every stored `ByteMatchSet`, keyed by its `ByteMatchSetId`. A `BTreeMap` for the
    /// same reason as `ip_sets`: one stable order for `ListByteMatchSets` to walk. The
    /// records themselves are at the end of this file.
    pub byte_match_sets: BTreeMap<String, ByteMatchSetEntry>,
    /// Every stored `GeoMatchSet`, keyed by its `GeoMatchSetId`. A `BTreeMap` for the
    /// same reason as [`Self::ip_sets`]: one stable order for `ListGeoMatchSets` to page
    /// over. The records themselves are at the end of this file.
    pub geo_match_sets: BTreeMap<String, GeoMatchSetEntry>,
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

    /// Whether this service stores the condition set a `Predicate` of type `kind`
    /// points at with `data_id`.
    ///
    /// Each `PredicateType` resolves its `DataId` against the store of that condition
    /// kind — `IPMatch` against the `IPSet`s, `SqlInjectionMatch` against the
    /// `SqlInjectionMatchSet`s, `XssMatch` against the `XssMatchSet`s, `SizeConstraint`
    /// against the `SizeConstraintSet`s, `ByteMatch` against the `ByteMatchSet`s and
    /// `GeoMatch` against the `GeoMatchSet`s. `RegexMatch` is the one `PredicateType`
    /// left whose `Create*` is still a scaffold, so this service holds no
    /// `RegexMatchSet` under any id and no `DataId` of that type can resolve.
    pub fn condition_set_exists(&self, kind: &str, data_id: &str) -> bool {
        match kind {
            PREDICATE_TYPE_IP_MATCH => self.ip_sets.contains_key(data_id),
            PREDICATE_TYPE_SQL_INJECTION_MATCH => {
                self.sql_injection_match_sets.contains_key(data_id)
            }
            PREDICATE_TYPE_XSS_MATCH => self.xss_match_sets.contains_key(data_id),
            PREDICATE_TYPE_SIZE_CONSTRAINT => self.size_constraint_sets.contains_key(data_id),
            PREDICATE_TYPE_BYTE_MATCH => self.byte_match_sets.contains_key(data_id),
            PREDICATE_TYPE_GEO_MATCH => self.geo_match_sets.contains_key(data_id),
            _ => false,
        }
    }

    /// Whether any stored rule — a `Rule` or a `RateBasedRule`, both of which hold a
    /// predicate list — points at `data_id`.
    ///
    /// Every resource id this service mints comes from one sequence ([`Self::mint_id`]),
    /// so an id identifies one condition set whatever its kind, and a `DataId` match is
    /// a reference to that set. A `Delete*` of a referenced condition set is refused:
    /// "You can't delete an IPSet if it's still used in any Rules", and the same
    /// sentence appears on every condition kind's delete.
    pub fn is_referenced_by_a_rule(&self, data_id: &str) -> bool {
        let points_at = |rule: &RuleEntry| {
            rule.predicates
                .iter()
                .any(|predicate| predicate.data_id == data_id)
        };
        self.rules.values().any(points_at)
            || self
                .rate_based_rules
                .values()
                .any(|rate_based| points_at(&rate_based.rule))
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

    /// The tag set of the resource `arn` names, or `None` when this store holds no
    /// taggable resource under that ARN.
    ///
    /// ONE copy of each resource's tag set: a resource's tags live on its own record —
    /// [`RuleEntry`], [`WebACLEntry`] or [`RuleGroupEntry`], which is also where that
    /// kind's create operation stored the `Tags` its request carried — so `TagResource`,
    /// `UntagResource` and `ListTagsForResource` read and write what the create wrote.
    pub fn tags_for_resource(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        let (resource_type, resource_id) = parse_taggable_resource_arn(arn)?;
        match resource_type {
            ARN_TYPE_RULE => Some(&self.rules.get(resource_id)?.tags),
            ARN_TYPE_WEB_ACL => Some(&self.web_acls.get(resource_id)?.tags),
            ARN_TYPE_RULE_GROUP => Some(&self.rule_groups.get(resource_id)?.tags),
            // A rate-based rule is a `Rule` with a rate limit, so its tags are the tags
            // of the `RuleEntry` it holds.
            ARN_TYPE_RATE_BASED_RULE => Some(&self.rate_based_rules.get(resource_id)?.rule.tags),
            // Unreachable while every type in `TAGGABLE_ARN_TYPES` has an arm above;
            // a kind added there without a store answers "no such resource".
            _ => None,
        }
    }

    /// The tag set of the resource `arn` names, for a call that changes it. `None`
    /// exactly where [`Self::tags_for_resource`] answers `None`.
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        let (resource_type, resource_id) = parse_taggable_resource_arn(arn)?;
        match resource_type {
            ARN_TYPE_RULE => Some(&mut self.rules.get_mut(resource_id)?.tags),
            ARN_TYPE_WEB_ACL => Some(&mut self.web_acls.get_mut(resource_id)?.tags),
            ARN_TYPE_RULE_GROUP => Some(&mut self.rule_groups.get_mut(resource_id)?.tags),
            ARN_TYPE_RATE_BASED_RULE => {
                Some(&mut self.rate_based_rules.get_mut(resource_id)?.rule.tags)
            }
            _ => None,
        }
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

// ---------------------------------------------------------------------------
// AWS WAF Classic GeoMatchSet records
//
// `CreateGeoMatchSet` and `GetGeoMatchSet` read and write the `geo_match_sets` map of
// the one state struct above through the record and bound below.
// ---------------------------------------------------------------------------

/// The most `GeoMatchSet`s this emulator stores for the account before
/// `CreateGeoMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an account (50 GeoMatchSets), the same shape of bound as [`MAX_IP_SETS`].
pub const MAX_GEO_MATCH_SETS: usize = 50;

/// One stored `GeoMatchConstraint`.
///
/// A `GeoMatchSet` created by `CreateGeoMatchSet` holds none of these — countries
/// arrive through `UpdateGeoMatchSet` — but the record carries the member so every
/// reader projects the same shape.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GeoMatchConstraintEntry {
    /// `GeoMatchConstraintType` as it goes on the wire; the model's
    /// `GEO_MATCH_CONSTRAINT_TYPE_VALUES` declares only `Country`.
    pub kind: String,
    /// A `GeoMatchConstraintValue`: the two-letter country code.
    pub value: String,
}

/// One stored `GeoMatchSet`.
#[derive(Debug, Clone)]
pub struct GeoMatchSetEntry {
    pub geo_match_set_id: String,
    pub name: String,
    /// The constraints in insertion order; a `GeoMatchSet` is a SET of countries, so
    /// one country appears at most once.
    pub constraints: Vec<GeoMatchConstraintEntry>,
}

impl GeoMatchSetEntry {
    /// A new, empty GeoMatchSet record. `CreateGeoMatchSet` documents the emptiness:
    /// "The GeoMatchSet contains no GeoMatchConstraints."
    pub fn new(geo_match_set_id: String, name: String) -> Self {
        Self {
            geo_match_set_id,
            name,
            constraints: Vec::new(),
        }
    }

    /// This record projected onto the modelled `GeoMatchSet` shape the Get and Create
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> GeoMatchSet {
        GeoMatchSet {
            geo_match_set_id: self.geo_match_set_id.clone(),
            name: Some(self.name.clone()),
            geo_match_constraints: self
                .constraints
                .iter()
                .map(|constraint| GeoMatchConstraint {
                    r#type: constraint.kind.clone(),
                    value: constraint.value.clone(),
                })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// AWS WAF Classic ByteMatchSet records
//
// `CreateByteMatchSet`, `UpdateByteMatchSet` and `GetByteMatchSet` read and write the
// `byte_match_sets` map of the one state struct above through the records and bounds
// below.
// ---------------------------------------------------------------------------

/// The most `ByteMatchSet`s this emulator stores for the account before
/// `CreateByteMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 conditions of each condition type); with [`MAX_BYTE_MATCH_TUPLES`]
/// bounding each one's contents, it keeps the ByteMatchSet state a caller can grow
/// bounded.
pub const MAX_BYTE_MATCH_SETS: usize = 100;

/// The most `ByteMatchTuple`s one `ByteMatchSet` may hold.
///
/// The model states no quota for this and no AWS sentence is cited for it, so the bound
/// is CHOSEN: far above the number of filters any one condition carries, and low enough
/// that the tuple state a caller can accumulate over many `UpdateByteMatchSet` requests
/// stays bounded.
pub const MAX_BYTE_MATCH_TUPLES: usize = 1_000;

/// The most `ByteMatchSetUpdate`s one `UpdateByteMatchSet` request may carry.
///
/// `ByteMatchSetUpdates` is modelled `len 1..`, bounded only from below, and no AWS
/// sentence is cited for a maximum, so this per-request batch cap is CHOSEN: it is the
/// same bound the sibling `UpdateIPSet` applies to its own updates
/// ([`MAX_UPDATES_PER_REQUEST`]), it is far above the number of filters one condition
/// carries, and with [`MAX_BYTE_MATCH_TUPLES`] it bounds the work one request can do
/// while holding this service's state.
pub const MAX_BYTE_MATCH_UPDATES_PER_REQUEST: usize = 1_000;

/// The most bytes a `ByteMatchTuple`'s `TargetString` may carry.
///
/// Source: `ByteMatchTuple`'s own `TargetString` documentation — "The maximum length of
/// the value is 50 bytes."
pub const MAX_TARGET_STRING_BYTES: usize = 50;

/// One stored `ByteMatchTuple`, and its own identity.
///
/// `ByteMatchSetUpdate` documents the identity: "If you specify `DELETE` for the value of
/// `Action`, the `ByteMatchTuple` values must exactly match the values in the
/// `ByteMatchTuple` that you want to delete from the `ByteMatchSet`" — so two tuples are
/// the same tuple exactly when all four of their members are equal, which is what the
/// derived `Eq`/`Ord` here mean. The tuple is stored DECOMPOSED (rather than as the
/// modelled shape with a separate key type, as `XssMatchSetEntry` does) because that
/// whole-tuple identity covers every member, including the `TargetString` blob's bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteMatchTupleEntry {
    /// `FieldToMatch.Type` as it goes on the wire: a `MatchFieldType` value.
    pub field_type: String,
    /// `FieldToMatch.Data` — the header or query-argument name, for the types that name
    /// one (see [`match_field_takes_data`]).
    pub field_data: Option<String>,
    /// The bytes AWS WAF would search for. A blob, so it is stored as bytes rather than
    /// as the base64 the wire carries.
    pub target_string: Vec<u8>,
    /// A `TextTransformation` value.
    pub text_transformation: String,
    /// A `PositionalConstraint` value.
    pub positional_constraint: String,
}

impl ByteMatchTupleEntry {
    /// The record a modelled `ByteMatchTuple` from an `UpdateByteMatchSet` request
    /// denotes.
    pub fn from_modelled(tuple: &ByteMatchTuple) -> Self {
        Self {
            field_type: tuple.field_to_match.r#type.clone(),
            field_data: tuple.field_to_match.data.clone(),
            target_string: tuple.target_string.clone(),
            text_transformation: tuple.text_transformation.clone(),
            positional_constraint: tuple.positional_constraint.clone(),
        }
    }

    /// This record projected back onto the modelled `ByteMatchTuple` shape.
    pub fn to_modelled(&self) -> ByteMatchTuple {
        ByteMatchTuple {
            field_to_match: FieldToMatch {
                r#type: self.field_type.clone(),
                data: self.field_data.clone(),
            },
            target_string: self.target_string.clone(),
            text_transformation: self.text_transformation.clone(),
            positional_constraint: self.positional_constraint.clone(),
        }
    }
}

/// One stored `ByteMatchSet`.
#[derive(Debug, Clone)]
pub struct ByteMatchSetEntry {
    pub byte_match_set_id: String,
    pub name: String,
    /// The tuples in insertion order, deduplicated by whole-tuple equality.
    pub tuples: Vec<ByteMatchTupleEntry>,
    /// Membership index over `tuples`, kept in step with it by
    /// [`ByteMatchSetEntry::insert_tuple`] and [`ByteMatchSetEntry::remove_tuple`], so
    /// deduplicating one update costs a lookup rather than a scan of the whole set.
    keys: BTreeSet<ByteMatchTupleEntry>,
}

impl ByteMatchSetEntry {
    /// A new `ByteMatchSet` record, which holds no tuples: `CreateByteMatchSet`
    /// documents that it answers "A ByteMatchSet that contains no ByteMatchTuple
    /// objects", and the tuples arrive through `UpdateByteMatchSet`.
    pub fn new(byte_match_set_id: String, name: String) -> Self {
        Self {
            byte_match_set_id,
            name,
            tuples: Vec::new(),
            keys: BTreeSet::new(),
        }
    }

    /// This record projected onto the modelled `ByteMatchSet` shape the Get and Create
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> ByteMatchSet {
        ByteMatchSet {
            byte_match_set_id: self.byte_match_set_id.clone(),
            name: Some(self.name.clone()),
            byte_match_tuples: self
                .tuples
                .iter()
                .map(ByteMatchTupleEntry::to_modelled)
                .collect(),
        }
    }

    /// Add a tuple, unless an equal one is already stored. Answers whether the set
    /// changed.
    pub fn insert_tuple(&mut self, tuple: ByteMatchTupleEntry) -> bool {
        if !self.keys.insert(tuple.clone()) {
            return false;
        }
        self.tuples.push(tuple);
        true
    }

    /// Remove the tuple equal to `tuple`, if the set holds one. Answers whether the set
    /// changed.
    pub fn remove_tuple(&mut self, tuple: &ByteMatchTupleEntry) -> bool {
        if !self.keys.remove(tuple) {
            return false;
        }
        self.tuples.retain(|stored| stored != tuple);
        true
    }
}

// ---------------------------------------------------------------------------
// AWS WAF Classic RuleGroup permission policies
//
// `PutPermissionPolicy` attaches an IAM policy to the rule group its `ResourceArn`
// names and `GetPermissionPolicy` reports it; the attached document itself lives on
// [`RuleGroupEntry::permission_policy`], so one copy serves every reader. The
// restrictions below are the ones `PutPermissionPolicy` and
// `WAFInvalidPermissionPolicyException` document, nothing wider.
// ---------------------------------------------------------------------------

/// The IAM policy language version a permission policy must declare.
///
/// Source: `PutPermissionPolicy`'s own documentation — "Your policy must be composed
/// using IAM Policy version 2012-10-17."
pub const PERMISSION_POLICY_VERSION: &str = "2012-10-17";

/// The one `Effect` a permission policy statement may carry.
///
/// Source: `PutPermissionPolicy`'s own documentation — "`Effect` must specify `Allow`."
pub const PERMISSION_POLICY_EFFECT: &str = "Allow";

/// Every `Action` a permission policy may grant, and the only ones.
///
/// Source: `PutPermissionPolicy`'s own documentation — "The `Action` in the policy must
/// be `waf:UpdateWebACL`, `waf-regional:UpdateWebACL`, `waf:GetRuleGroup` and
/// `waf-regional:GetRuleGroup`. Any extra or wildcard actions in the policy will be
/// rejected." An action outside this list — including any form carrying a `*` — is
/// therefore refused.
pub const PERMISSION_POLICY_ACTIONS: &[&str] = &[
    "waf:UpdateWebACL",
    "waf-regional:UpdateWebACL",
    "waf:GetRuleGroup",
    "waf-regional:GetRuleGroup",
];

/// Whether `policy` is a permission policy AWS WAF Classic accepts: a JSON object
/// declaring IAM policy version [`PERMISSION_POLICY_VERSION`] whose `Statement` — one
/// statement object, or a non-empty list of them — every entry of satisfies
/// [`permission_policy_statement_is_valid`].
///
/// Source: the restrictions `PutPermissionPolicy` and
/// `WAFInvalidPermissionPolicyException` document. The document is only CHECKED here:
/// what gets stored is the caller's own text, byte for byte.
pub fn permission_policy_is_valid(policy: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(document) = document.as_object() else {
        return false;
    };
    if document
        .get("Version")
        .and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_VERSION)
    {
        return false;
    }
    let statements: Vec<&::serde_json::Value> = match document.get("Statement") {
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(::serde_json::Value::Array(list)) => list.iter().collect(),
        // A policy with no `Statement`, or one that is neither a statement nor a list
        // of statements, carries no `Effect`, `Action` or `Principal` at all.
        _ => return false,
    };
    !statements.is_empty()
        && statements
            .into_iter()
            .all(permission_policy_statement_is_valid)
}

/// Whether one `Statement` of a permission policy is within the documented
/// restrictions: it carries an `Effect` of [`PERMISSION_POLICY_EFFECT`], a non-empty
/// `Principal`, an `Action` naming only [`PERMISSION_POLICY_ACTIONS`], and no
/// `Resource`.
///
/// `NotAction`, `NotPrincipal` and `NotResource` are refused with the member they
/// negate: a statement carrying one names no `Action` or `Principal`, or names a
/// resource the policy "cannot include".
pub fn permission_policy_statement_is_valid(statement: &::serde_json::Value) -> bool {
    let Some(statement) = statement.as_object() else {
        return false;
    };
    if statement.contains_key("Resource") || statement.contains_key("NotResource") {
        return false;
    }
    if statement.get("Effect").and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_EFFECT)
    {
        return false;
    }
    if statement.contains_key("NotPrincipal") || statement.contains_key("NotAction") {
        return false;
    }
    permission_policy_principal_is_present(statement.get("Principal"))
        && permission_policy_actions_are_allowed(statement.get("Action"))
}

/// Whether a statement names a `Principal`: a non-blank string, or a map of principal
/// kinds (`AWS`, `Service`, …) whose every entry carries a non-blank value or a
/// non-empty list of them.
fn permission_policy_principal_is_present(principal: Option<&::serde_json::Value>) -> bool {
    match principal {
        Some(::serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(::serde_json::Value::Object(kinds)) => {
            !kinds.is_empty() && kinds.values().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// Whether one entry of a `Principal` map names a principal.
fn permission_policy_principal_value(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(value) => !value.trim().is_empty(),
        ::serde_json::Value::Array(values) => {
            !values.is_empty() && values.iter().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// Whether a statement's `Action` — one action, or a non-empty list of them — names
/// only actions in [`PERMISSION_POLICY_ACTIONS`].
fn permission_policy_actions_are_allowed(action: Option<&::serde_json::Value>) -> bool {
    fn allowed(action: &::serde_json::Value) -> bool {
        action
            .as_str()
            .is_some_and(|action| PERMISSION_POLICY_ACTIONS.contains(&action))
    }
    match action {
        Some(action @ ::serde_json::Value::String(_)) => allowed(action),
        Some(::serde_json::Value::Array(actions)) => {
            !actions.is_empty() && actions.iter().all(allowed)
        }
        _ => false,
    }
}
