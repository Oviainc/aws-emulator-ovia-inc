// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafv2-awswaf-20190729 service.
//!
//! WAFv2 namespaces every resource by `Scope` (`CLOUDFRONT` | `REGIONAL`), so a
//! CLOUDFRONT web ACL named `x` and a REGIONAL one named `x` are two different
//! resources. Every map below is therefore keyed by `(Scope, …)`.
//!
//! An entry stores the MODELLED shape (`WebACL` / `IPSet`) the create call built,
//! not a flattened copy of its members, so a `Get` hands back exactly what was
//! stored — including the rule tree — and no member can be lost in translation
//! when the model grows one. The API bookkeeping that is not part of the shape
//! (its scope, its optimistic-lock token and its tags) sits beside it.

use std::collections::HashMap;
use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    validate, ElementConstraintsTrait, LengthTrait, PatternTrait, Violation,
};

use crate::ops::types::{
    IPSet, LabelSummary, OnSourceDDoSProtectionConfig, Rule, RuleGroup, Statement, Tag,
    WAFAssociatedItemException, WAFDuplicateItemException, WAFInvalidParameterException,
    WAFLimitsExceededException, WAFNonexistentItemException, WAFOptimisticLockException, WebACL,
};

/// The `Scope` values the model declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`).
pub const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";
/// See [`SCOPE_CLOUDFRONT`].
pub const SCOPE_REGIONAL: &str = "REGIONAL";

/// The region an ARN carries when the request names none: the emulator's default
/// endpoint region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `true` when `scope` is one of the two values the `Scope` enum declares.
pub fn is_known_scope(scope: &str) -> bool {
    matches!(scope, SCOPE_CLOUDFRONT | SCOPE_REGIONAL)
}

// ---------------------------------------------------------------------------
// The exception messages: ONE constant sentence per exception type.
// ---------------------------------------------------------------------------
//
// Every WAFv2 exception below models its detail as a lone `Message` (the limits
// error adds a `SourceType`), and the model documents ONE sentence for the SHAPE
// itself — `smithy_query(query="shape",shape="WAFNonexistentItemException")` is
// "WAF couldn’t perform the operation because your resource doesn't exist …", not
// a template with a slot for the resource that was named. The message is therefore
// a property of the exception TYPE, not of the call: WAF answers the same sentence
// for every request that trips that exception, and the body carries no name, id,
// ARN or scope taken from the request. A handler that needs to record WHICH
// resource tripped it writes that to the audit log ([`audit`]), never to the
// response.
//
// `WAFInvalidParameterException` is the one exception whose body does describe the
// call: the model gives it `Field`, `Parameter` and `Reason` members for exactly
// that ("The invalid parameter that resulted in the exception"), so its message
// stays request-derived. `WAFInternalErrorException` already carries a constant,
// request-independent sentence naming the internal fault it reports.

/// The sentence `WAFDuplicateItemException` carries, whatever the request named.
///
/// The wire wording WAF answers with, as the recorded wafv2 responses show it; the
/// model documents the same single sentence for the shape (see the note above).
pub const DUPLICATE_ITEM_MESSAGE: &str = "AWS WAF couldn’t perform the operation \
     because some resource in your request is a duplicate of an existing one.";

/// The sentence `WAFOptimisticLockException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`] for where the wording comes from.
pub const OPTIMISTIC_LOCK_MESSAGE: &str = "AWS WAF couldn’t save your changes \
     because someone changed the resource after you started to edit it. Reapply \
     your changes.";

/// The sentence `WAFNonexistentItemException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`].
pub const NONEXISTENT_ITEM_MESSAGE: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// The sentence `WAFAssociatedItemException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFAssociatedItemException")`, its
/// wrapping whitespace flattened), which is what pins it as constant per exception
/// type.
pub const ASSOCIATED_ITEM_MESSAGE: &str = "WAF couldn’t perform the operation \
     because your resource is being used by another resource or it’s associated \
     with another resource.";

/// The sentence `WAFLimitsExceededException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFLimitsExceededException")`),
/// link text flattened, which names no particular quota or request.
pub const LIMITS_EXCEEDED_MESSAGE: &str = "WAF couldn’t perform the operation \
     because you exceeded your resource limit. For example, the maximum number of \
     WebACL objects that you can create for an Amazon Web Services account. For \
     more information, see WAF quotas in the WAF Developer Guide.";

/// `WAFDuplicateItemException` as WAF returns it: nothing but
/// [`DUPLICATE_ITEM_MESSAGE`].
pub fn duplicate_item_exception() -> WAFDuplicateItemException {
    WAFDuplicateItemException {
        message: Some(DUPLICATE_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFOptimisticLockException` as WAF returns it: nothing but
/// [`OPTIMISTIC_LOCK_MESSAGE`].
pub fn optimistic_lock_exception() -> WAFOptimisticLockException {
    WAFOptimisticLockException {
        message: Some(OPTIMISTIC_LOCK_MESSAGE.to_string()),
    }
}

/// `WAFNonexistentItemException` as WAF returns it: nothing but
/// [`NONEXISTENT_ITEM_MESSAGE`].
pub fn nonexistent_item_exception() -> WAFNonexistentItemException {
    WAFNonexistentItemException {
        message: Some(NONEXISTENT_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFAssociatedItemException` as WAF returns it: nothing but
/// [`ASSOCIATED_ITEM_MESSAGE`].
pub fn associated_item_exception() -> WAFAssociatedItemException {
    WAFAssociatedItemException {
        message: Some(ASSOCIATED_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFLimitsExceededException` as WAF returns it: [`LIMITS_EXCEEDED_MESSAGE`] and
/// no `SourceType`, which the model leaves optional and which this emulator has no
/// source value to report.
pub fn limits_exceeded_exception() -> WAFLimitsExceededException {
    WAFLimitsExceededException {
        message: Some(LIMITS_EXCEEDED_MESSAGE.to_string()),
        source_type: None,
    }
}

/// A WebACL as stored: the modelled shape plus the bookkeeping WAFv2 keeps beside
/// it.
#[derive(Debug, Clone)]
pub struct WebAclEntry {
    /// The scope the web ACL was created in.
    pub scope: String,
    /// Exactly what `GetWebACL` answers with.
    pub web_acl: WebACL,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// An IPSet as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct IpSetEntry {
    /// The scope the IP set was created in.
    pub scope: String,
    /// Exactly what `GetIPSet` answers with.
    pub ip_set: IPSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// A RuleGroup as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct RuleGroupEntry {
    /// The scope the rule group was created in.
    pub scope: String,
    /// Exactly what `GetRuleGroup` answers with.
    pub rule_group: RuleGroup,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// What an ARN in [`ComAmazonawsWafv2Awswaf20190729State::arn_index`] resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A web ACL, by `(Scope, Id)`.
    WebAcl {
        /// The scope the web ACL lives in.
        scope: String,
        /// The web ACL's id.
        id: String,
    },
    /// An IP set, by `(Scope, Id)`.
    IpSet {
        /// The scope the IP set lives in.
        scope: String,
        /// The IP set's id.
        id: String,
    },
    /// A rule group, by `(Scope, Id)`.
    ///
    /// `CreateRuleGroup` indexes every rule group it mints under this variant, so a
    /// `rulegroup` ARN resolves to the rule group it names and to nothing else: the
    /// index resolves an ARN to its resource KIND as well as to its identity, so a
    /// reference of one kind is never mistaken for a resource of another. See
    /// [`ComAmazonawsWafv2Awswaf20190729State::holds_rule_group`].
    RuleGroup {
        /// The scope the rule group lives in.
        scope: String,
        /// The rule group's id.
        id: String,
    },
}

/// Per-service state for com-amazonaws-wafv2-awswaf-20190729. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafv2Awswaf20190729State {
    /// Web ACLs by `(Scope, Id)`.
    pub web_acls: HashMap<(String, String), WebAclEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub web_acl_names: HashMap<(String, String), String>,
    /// IP sets by `(Scope, Id)`.
    pub ip_sets: HashMap<(String, String), IpSetEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub ip_set_names: HashMap<(String, String), String>,
    /// Rule groups by `(Scope, Id)`.
    pub rule_groups: HashMap<(String, String), RuleGroupEntry>,
    /// `(Scope, Name)` to `Id`, for rule groups: duplicate-name detection and
    /// Name+Id lookup.
    pub rule_group_names: HashMap<(String, String), String>,
    /// Every resource ARN this service minted, so a by-ARN lookup resolves from
    /// state instead of parsing the ARN back apart.
    pub arn_index: HashMap<String, ResourceRef>,
}

impl ComAmazonawsWafv2Awswaf20190729State {
    /// The web ACL stored under `(scope, id)`, but only when it is also the one
    /// called `name`. WAFv2's Get/Update/Delete take Name, Scope AND Id together,
    /// and a triple that does not describe one stored resource names no resource
    /// at all.
    pub fn web_acl(&self, scope: &str, name: &str, id: &str) -> Option<&WebAclEntry> {
        let entry = self.web_acls.get(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// [`Self::web_acl`], mutably.
    pub fn web_acl_mut(&mut self, scope: &str, name: &str, id: &str) -> Option<&mut WebAclEntry> {
        let entry = self.web_acls.get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// The IP set stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn ip_set(&self, scope: &str, name: &str, id: &str) -> Option<&IpSetEntry> {
        let entry = self.ip_sets.get(&(scope.to_string(), id.to_string()))?;
        (entry.ip_set.name == name).then_some(entry)
    }

    /// Drop a web ACL and every index entry that pointed at it.
    pub fn remove_web_acl(&mut self, scope: &str, id: &str) -> Option<WebAclEntry> {
        let entry = self.web_acls.remove(&(scope.to_string(), id.to_string()))?;
        self.web_acl_names
            .remove(&(entry.scope.clone(), entry.web_acl.name.clone()));
        self.arn_index.remove(&entry.web_acl.arn);
        Some(entry)
    }

    /// Drop an IP set and every index entry that pointed at it.
    pub fn remove_ip_set(&mut self, scope: &str, id: &str) -> Option<IpSetEntry> {
        let entry = self.ip_sets.remove(&(scope.to_string(), id.to_string()))?;
        self.ip_set_names
            .remove(&(entry.scope.clone(), entry.ip_set.name.clone()));
        self.arn_index.remove(&entry.ip_set.arn);
        Some(entry)
    }

    /// How many web ACLs this scope already holds. See [`MAX_WEB_ACLS_PER_SCOPE`].
    pub fn web_acls_in_scope(&self, scope: &str) -> usize {
        self.web_acls
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// How many IP sets this scope already holds. See [`MAX_IP_SETS_PER_SCOPE`].
    pub fn ip_sets_in_scope(&self, scope: &str) -> usize {
        self.ip_sets
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// The rule group stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn rule_group(&self, scope: &str, name: &str, id: &str) -> Option<&RuleGroupEntry> {
        let entry = self.rule_groups.get(&(scope.to_string(), id.to_string()))?;
        (entry.rule_group.name == name).then_some(entry)
    }

    /// How many rule groups this scope already holds. See
    /// [`MAX_RULE_GROUPS_PER_SCOPE`].
    pub fn rule_groups_in_scope(&self, scope: &str) -> usize {
        self.rule_groups
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// Every rule group ARN this scope holds, with the group's declared
    /// `Capacity`.
    ///
    /// This is what prices a `RuleGroupReferenceStatement` in
    /// [`web_acl_capacity`]: `RuleGroup$Capacity` is "the web ACL capacity units
    /// (WCUs) required for this rule group", fixed at creation, so a web ACL rule
    /// that runs a rule group consumes exactly that group's declared capacity. It
    /// is read out of state BEFORE a save writes through it, so a handler can hold
    /// it while it borrows the entry it is about to update.
    pub fn rule_group_capacities(&self, scope: &str) -> HashMap<String, i64> {
        self.rule_groups
            .values()
            .filter(|entry| entry.scope == scope)
            .map(|entry| (entry.rule_group.arn.clone(), entry.rule_group.capacity))
            .collect()
    }

    /// Why a reference in `rules` cannot be resolved, if any cannot.
    ///
    /// WAF resolves a rule statement's reference when the rule is SAVED: a
    /// reference statement names another entity ("the Amazon Resource Name (ARN) of
    /// the IPSet that this statement references", `IPSetReferenceStatement$ARN`),
    /// and a save naming an entity that does not exist is refused with the modelled
    /// `WAFNonexistentItemException` ("WAF couldn’t perform the operation because
    /// your resource doesn't exist") rather than stored for a later `Get` to hand
    /// back as though the reference were live. This is the ingress counterpart of
    /// [`Self::web_acl_referencing_ip_set`]: the same reference keeps an entity from
    /// being deleted and keeps a rule that names a missing one from being stored.
    ///
    /// Each of the model's four reference-bearing statements is resolved against
    /// this service's own stores. An `IPSetReferenceStatement` and a
    /// `RuleGroupReferenceStatement` resolve through [`Self::arn_index`], each only
    /// to a resource of its own kind in the SAME scope, because a scope is a
    /// namespace. The other two — `ManagedRuleGroupStatement` and
    /// `RegexPatternSetReferenceStatement` — name entity kinds this emulator has no
    /// store for yet (`CreateRegexPatternSet` and the managed-rule-group catalogue
    /// are still scaffolded 501s, tracked as separate gap issues), so every
    /// reference to one names nothing that exists and is refused here. The `Some` it
    /// answers is the DIAGNOSTIC for the refusal's audit record — the response
    /// itself carries [`NONEXISTENT_ITEM_MESSAGE`], the constant sentence that
    /// exception type always carries. That is also what keeps
    /// [`web_acl_capacity`] from pricing a statement whose cost it cannot know: a
    /// stored rule list can only carry a reference to a rule group this service
    /// holds, whose declared `Capacity` is exactly that statement's cost, and never
    /// a managed-rule-group or regex-pattern-set reference.
    ///
    /// Walked with an explicit worklist, like [`validate_statement_nesting`]: the
    /// tree is supplied by the request.
    pub fn unresolvable_rule_reference(&self, scope: &str, rules: &[Rule]) -> Option<String> {
        let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
        while let Some(statement) = pending.pop() {
            if let Some(reference) = statement.ip_set_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::IpSet {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No IPSet with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::RuleGroup {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No RuleGroup with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.regex_pattern_set_reference_statement.as_ref() {
                return Some(format!(
                    "No RegexPatternSet with the ARN {} exists in scope {scope}",
                    reference.arn
                ));
            }
            if let Some(managed) = statement.managed_rule_group_statement.as_ref() {
                return Some(format!(
                    "No managed rule group named {:?} from vendor {:?} is available",
                    managed.name, managed.vendor_name
                ));
            }
            pending.extend(nested_statements(statement));
        }
        None
    }

    /// `true` when `arn` names a rule group this service holds.
    ///
    /// WAF resolves a published version's `AssociatedRuleGroupArn` against the
    /// vendor's own rule groups, so this is what
    /// `PutManagedRuleSetVersions` consults before it judges anything else about
    /// the request. The ARN must be a well-formed wafv2 ARN whose resource kind is
    /// `rulegroup` AND be indexed as one: an ARN of another kind, or one naming a
    /// rule group that was never created, resolves to nothing.
    ///
    /// A rule group is indexed under [`ResourceRef::RuleGroup`] by
    /// `CreateRuleGroup`, so this answers `true` for a rule group this service
    /// holds and `false` for an ARN of another kind or one naming a rule group that
    /// was never created. That is the same stance
    /// [`Self::unresolvable_rule_reference`] takes for a
    /// `RuleGroupReferenceStatement`: a reference WAF cannot resolve is refused
    /// rather than stored as though it were live.
    pub fn holds_rule_group(&self, arn: &str) -> bool {
        arn_resource_kind(arn) == Some(RULE_GROUP_ARN_KIND)
            && matches!(self.arn_index.get(arn), Some(ResourceRef::RuleGroup { .. }))
    }

    /// The live web ACL whose rule tree references `ip_set_arn`, if any.
    ///
    /// An IP set that a web ACL still references cannot be deleted, so this is
    /// what `DeleteIPSet` consults. It walks the whole tree, because an
    /// `IPSetReferenceStatement` can sit inside `AndStatement`, `OrStatement`,
    /// `NotStatement` or a rate-based/managed-rule-group scope-down statement.
    pub fn web_acl_referencing_ip_set(&self, ip_set_arn: &str) -> Option<&WebAclEntry> {
        self.web_acls.values().find(|entry| {
            entry
                .web_acl
                .rules
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|rule| {
                    statement_references_ip_set(
                        &rule.statement,
                        ip_set_arn,
                        MAX_STATEMENT_NESTING_DEPTH,
                    )
                })
        })
    }
}

/// The deepest a rule's statement tree may nest.
///
/// Statements nest: `AndStatement` and `OrStatement` over a list, `NotStatement`
/// over one, and the scope-down statement of a rate-based or managed-rule-group
/// statement. WAF publishes a maximum nesting depth for rule statements in its
/// quota list, and the tree comes straight off the wire, so the bound is enforced
/// ON INGRESS by [`validate_statement_nesting`] — before a tree is stored, before
/// its capacity is computed, and so before any later walk sees it.
pub const MAX_STATEMENT_NESTING_DEPTH: u32 = 10;

/// The most web ACLs this service stores per scope: WAF's published quota is 100
/// web ACLs per Region per scope, and reaching it answers the modelled
/// `WAFLimitsExceededException` the way the real service does — which is also what
/// keeps the store from growing without limit.
pub const MAX_WEB_ACLS_PER_SCOPE: usize = 100;

/// The most IP sets this service stores per scope; see [`MAX_WEB_ACLS_PER_SCOPE`].
/// WAF's published quota is 100 IP sets per Region per scope.
pub const MAX_IP_SETS_PER_SCOPE: usize = 100;

/// The most rule groups this service stores per scope; see
/// [`MAX_WEB_ACLS_PER_SCOPE`]. WAF's published quota is 100 rule groups per Region
/// per scope.
pub const MAX_RULE_GROUPS_PER_SCOPE: usize = 100;

/// The most addresses one IP set holds: WAF's published quota is 10,000 IP
/// addresses or ranges per IP set. It bounds how large a single stored entry can
/// be, beside the serve path's own 16 MiB request-body cap.
pub const MAX_IP_SET_ADDRESSES: usize = 10_000;

/// The statements nested directly inside `statement`, in no particular order.
///
/// One place names every nesting the model has, so the depth validator and both
/// tree walks below agree on what "nested" means.
///
/// [`nested_statements_mut`] is its mutable twin, arm for arm: a nesting the model
/// grows has to be added to BOTH, or a default would be applied on one walk and
/// not the other.
fn nested_statements(statement: &Statement) -> Vec<&Statement> {
    let mut nested: Vec<&Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_ref() {
        nested.extend(and.statements.iter());
    }
    if let Some(or) = statement.or_statement.as_ref() {
        nested.extend(or.statements.iter());
    }
    if let Some(not) = statement.not_statement.as_ref() {
        nested.push(&not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_ref()
        .and_then(|rate| rate.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_ref()
        .and_then(|managed| managed.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    nested
}

/// [`nested_statements`], mutably: the statements nested directly inside
/// `statement`, in no particular order.
///
/// Used by [`apply_rule_defaults`] to reach every statement of a tree in place.
/// Each arm borrows a DIFFERENT field of `statement`, so the borrows are disjoint.
/// Kept arm for arm in step with [`nested_statements`]: a nesting the model grows
/// belongs in both.
fn nested_statements_mut(statement: &mut Statement) -> Vec<&mut Statement> {
    let mut nested: Vec<&mut Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_mut() {
        nested.extend(and.statements.iter_mut());
    }
    if let Some(or) = statement.or_statement.as_mut() {
        nested.extend(or.statements.iter_mut());
    }
    if let Some(not) = statement.not_statement.as_mut() {
        nested.push(&mut not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_mut()
        .and_then(|rate| rate.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_mut()
        .and_then(|managed| managed.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    nested
}

/// Refuse a rule list whose statements nest deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`]. `Err` carries the reason, for the refusal's
/// message.
///
/// Walked with an explicit worklist rather than by recursion: the tree is supplied
/// by the request, so the check that bounds its depth must not itself consume
/// stack in proportion to that depth.
pub fn validate_statement_nesting(rules: &[Rule]) -> Result<(), String> {
    let mut pending: Vec<(&Statement, u32)> = rules
        .iter()
        .map(|rule| (&rule.statement, 1_u32))
        .collect();
    while let Some((statement, depth)) = pending.pop() {
        if depth > MAX_STATEMENT_NESTING_DEPTH {
            return Err(format!(
                "a rule statement nests {depth} levels deep; WAF allows at most \
                 {MAX_STATEMENT_NESTING_DEPTH}"
            ));
        }
        pending.extend(
            nested_statements(statement)
                .into_iter()
                .map(|nested| (nested, depth + 1)),
        );
    }
    Ok(())
}

/// Where a `RuleGroupReferenceStatement` may appear, if `rules` puts one
/// somewhere it may not. `Err` carries the reason, for the refusal's message.
///
/// The model states the placement rule on the statement itself
/// (`smithy_query(query="doc",shape="RuleGroupReferenceStatement")`): "You cannot
/// nest a `RuleGroupReferenceStatement`, for example for use inside a
/// `NotStatement` or `OrStatement`. You cannot use a rule group reference
/// statement inside another rule group. You can only reference a rule group as a
/// top-level statement within a rule that you define in a web ACL." So a reference
/// is well placed ONLY as a rule's own top-level statement, and only in a web ACL:
/// `top_level_allowed` is `true` for a web ACL save and `false` for a rule group
/// save. A misplacement is a parameter WAF does not accept rather than a dangling
/// reference, so it is answered with `WAFInvalidParameterException` ("Your nested
/// statement isn't valid. You might have tried to nest a statement that can’t be
/// nested", from that error's own doc) — and it is judged BEFORE the reference is
/// resolved, because a well-placed reference is what resolution is defined over.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request.
pub fn misplaced_rule_group_reference(rules: &[Rule], top_level_allowed: bool) -> Option<String> {
    for rule in rules {
        if !top_level_allowed && rule.statement.rule_group_reference_statement.is_some() {
            return Some(format!(
                "the rule {:?} carries a RuleGroupReferenceStatement; a rule group \
                 can only be referenced from a rule of a web ACL, not from a rule \
                 of another rule group",
                rule.name
            ));
        }
        let mut pending: Vec<&Statement> = nested_statements(&rule.statement);
        while let Some(statement) = pending.pop() {
            if statement.rule_group_reference_statement.is_some() {
                return Some(format!(
                    "the rule {:?} nests a RuleGroupReferenceStatement inside another \
                     statement; a rule group can only be referenced by a rule's \
                     top-level statement",
                    rule.name
                ));
            }
            pending.extend(nested_statements(statement));
        }
    }
    None
}

/// `true` when `statement`, or anything nested inside it, is an
/// `IPSetReferenceStatement` for `arn`.
///
/// `depth_budget` is the number of levels left to descend, so this cannot recurse
/// further than [`MAX_STATEMENT_NESTING_DEPTH`] even if a tree somehow reached the
/// store without passing [`validate_statement_nesting`].
fn statement_references_ip_set(statement: &Statement, arn: &str, depth_budget: u32) -> bool {
    if depth_budget == 0 {
        return false;
    }
    if statement
        .ip_set_reference_statement
        .as_ref()
        .is_some_and(|reference| reference.arn == arn)
    {
        return true;
    }
    nested_statements(statement)
        .into_iter()
        .any(|nested| statement_references_ip_set(nested, arn, depth_budget - 1))
}

/// A fresh lowercase, hyphenated UUIDv4 — the form every `EntityId` and
/// `LockToken` in this model takes
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`, from
/// `smithy_query(query="shape",shape="LockToken")`).
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2`
/// crate) and hex-encoded (the `hex` crate), the way this repo's other services
/// mint a UUID-shaped id (see
/// `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs`): the
/// OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds,
/// and a process-wide counter, so two calls in the same nanosecond cannot collide.
/// The version nibble and the variant bits RFC 4122 §4.4 fixes are then set, so
/// the value is a well-formed version-4 UUID. An id or a lock token names a
/// resource; a credential would come from `ctx.credential_store()`, never here.
pub fn new_uuid() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    static SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are
    // `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// The AWS region the request was signed for, or [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope to read one from.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record what a state-changing WAFv2 call did, as the audit trail a mutation
/// leaves behind.
///
/// `outcome` is what the service decided — `"created"`, `"updated"`, `"deleted"`,
/// or the modelled error it answered — so a refused mutation is as visible as an
/// applied one. The record carries only "what was called": the operation and the
/// resource it names. Nothing from the caller's credential is recorded, not even
/// the access key id the request presented, and the request body never enters the
/// record.
pub fn audit(operation: &str, scope: &str, name: &str, id: &str, outcome: &str) {
    ::tracing::info!(
        target: "wafv2::audit",
        operation,
        scope,
        name,
        id,
        outcome,
        "wafv2 state-changing request"
    );
}

/// `true` when `value` has the shape of a wafv2 resource ARN:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`.
///
/// `GetWebACL` separates the two ARN failures the model names: an ARN that is
/// malformed is `WAFInvalidParameterException` ("Your request references an ARN
/// that is malformed", from that error's own doc), while a well-formed ARN naming
/// nothing stored is `WAFNonexistentItemException`.
pub fn is_wafv2_resource_arn(value: &str) -> bool {
    let mut segments = value.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("wafv2"),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return false;
    };
    let resource_parts: Vec<&str> = resource.split('/').collect();
    !partition.is_empty()
        && resource_parts.len() == 4
        && matches!(resource_parts[0], "global" | "regional")
        && resource_parts[1..].iter().all(|part| !part.is_empty())
}

/// The region segment of a scope's ARNs.
///
/// A CLOUDFRONT-scoped resource is always reached through one endpoint — "API and
/// SDKs - For all calls, use the Region endpoint us-east-1" (the `Scope` member
/// doc) — so its ARN carries that region whatever the caller signed for.
fn arn_region<'a>(scope: &str, request_region: &'a str) -> &'a str {
    if scope == SCOPE_CLOUDFRONT {
        DEFAULT_REGION
    } else {
        request_region
    }
}

/// The first resource segment of a wafv2 ARN: `global` for a CLOUDFRONT-scoped
/// resource, `regional` for a REGIONAL one.
fn scope_path(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        "global"
    } else {
        "regional"
    }
}

/// `arn:aws:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`, where
/// `kind` is the resource type segment (`ipset`, `webacl`, …).
pub fn resource_arn(
    scope: &str,
    request_region: &str,
    kind: &str,
    name: &str,
    id: &str,
) -> String {
    ::engine::emulator_arn(
        "wafv2",
        arn_region(scope, request_region),
        &format!("{}/{kind}/{name}/{id}", scope_path(scope)),
    )
}

/// `awswaf:<account ID>:<kind>:<entity name>:` — the prefix the `LabelNamespace`
/// member of a label-bearing entity specifies for every label a rule in it adds.
///
/// One helper for every entity kind, because both docs give the same shape with
/// only the kind segment differing: `WebACL$LabelNamespace` is
/// "awswaf:<account ID>:webacl:<web ACL name>:" and `RuleGroup$LabelNamespace` is
/// "awswaf:<account ID>:rulegroup:<rule group name>:". `kind` is the same resource
/// segment an ARN carries, so pass the `…_ARN_KIND` constant for the entity.
pub fn label_namespace(kind: &str, name: &str) -> String {
    format!(
        "awswaf:{}:{kind}:{name}:",
        ::engine::emulator_account_id()
    )
}

/// Check one `Addresses` entry against the set's declared `IPAddressVersion`.
///
/// `IPSet$Addresses` documents the rule: "All addresses must be specified using
/// Classless Inter-Domain Routing (CIDR) notation. WAF supports all IPv4 and IPv6
/// CIDR ranges except for `/0`", and `IPAddressVersion` fixes which family the
/// entries belong to. `Err` carries the reason, for the refusal's message.
///
/// What the doc does NOT say is anything about the host bits below the prefix
/// length, so `10.0.0.1/8` is deliberately accepted and stored verbatim: the
/// documented rule is the notation, the family and the `/0` exclusion, and this
/// check goes no further than the rule it can cite.
pub fn validate_cidr(address: &str, ip_address_version: &str) -> Result<(), String> {
    let Some((literal, prefix)) = address.split_once('/') else {
        return Err(format!(
            "{address:?} is not in CIDR notation; expected <address>/<prefix length>"
        ));
    };
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{address:?} has a non-numeric CIDR prefix length"));
    }
    let Ok(prefix_len) = prefix.parse::<u32>() else {
        return Err(format!("{address:?} has an out-of-range CIDR prefix length"));
    };
    // `/0` is the one range WAF documents as unsupported, for either family.
    let (parsed, max_prefix) = match ip_address_version {
        "IPV4" => (
            literal.parse::<::std::net::Ipv4Addr>().is_ok(),
            32_u32,
        ),
        "IPV6" => (
            literal.parse::<::std::net::Ipv6Addr>().is_ok(),
            128_u32,
        ),
        other => {
            return Err(format!("{other:?} is not a known IPAddressVersion"));
        }
    };
    if !parsed {
        return Err(format!(
            "{address:?} is not an {ip_address_version} address in CIDR notation"
        ));
    }
    if prefix_len == 0 || prefix_len > max_prefix {
        return Err(format!(
            "{address:?} has prefix length /{prefix_len}; {ip_address_version} \
             CIDR ranges must be between /1 and /{max_prefix}"
        ));
    }
    Ok(())
}

/// The web ACL capacity units (WCUs) a rule list consumes.
///
/// `WebACL$Capacity` is "the web ACL capacity units (WCUs) currently being used by
/// this web ACL", so it is DERIVED from the stored rules rather than carried in
/// the request. The constants below are the base WCU costs the WAF Developer
/// Guide publishes for each rule statement in its rule-statement list; a logical
/// statement costs the sum of what it nests.
///
/// One cost is not a constant: a `RuleGroupReferenceStatement` costs the
/// referenced rule group's own `Capacity` — "the web ACL capacity units (WCUs)
/// required for this rule group", fixed when the group is created — so
/// `rule_group_capacities` carries `ARN` → `Capacity` for the scope being saved
/// into, as [`ComAmazonawsWafv2Awswaf20190729State::rule_group_capacities`] reads
/// it out of state. A reference naming a rule group this service does not hold is
/// refused on ingress by
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`], so every
/// reference a stored rule list carries is in that map.
///
/// The one cost that is still never reached rather than guessed is a
/// `ManagedRuleGroupStatement`'s: it costs the managed group's capacity and the
/// managed catalogue (`ListAvailableManagedRuleGroups`) is not implemented yet — a
/// separate gap issue, as is `CheckCapacity`. Such a statement names nothing this
/// service holds and is refused on ingress too, so this function never has to
/// price a reference whose cost it cannot know, and no caller is handed an
/// understated `Capacity`.
pub fn web_acl_capacity(rules: &[Rule], rule_group_capacities: &HashMap<String, i64>) -> i64 {
    rules
        .iter()
        .map(|rule| {
            statement_capacity(
                &rule.statement,
                MAX_STATEMENT_NESTING_DEPTH,
                rule_group_capacities,
            )
        })
        .sum()
}

/// The WCUs one `ByteMatchStatement` consumes.
///
/// The WCU table lives in the WAF Developer Guide, not in the Smithy model
/// (`WebACL$Capacity` only points at that guide), so where the recorded wire corpus
/// prices a statement it is the authority here: a recorded web ACL whose three
/// rules each hold ONE `ByteMatchStatement` — over a single field, with one `NONE`
/// text transformation and a `CONTAINS` positional constraint — reports `Capacity`
/// 30, so one such statement costs 10 rather than the 1 a plain single-field match
/// is often quoted at. Nothing available here prices the other positional
/// constraints separately, so every byte match costs this.
pub const BYTE_MATCH_WCU: i64 = 10;

/// The WCUs one `RateBasedStatement` consumes before anything it nests.
///
/// Pinned by the recorded corpus: a web ACL holding exactly one rate-based rule
/// that aggregates on `IP`, with no scope-down statement and no custom keys,
/// reports `Capacity` 2.
pub const RATE_BASED_WCU: i64 = 2;

/// The WCUs a `RateBasedStatement` adds when it aggregates on CUSTOM KEYS.
///
/// Aggregating on request components costs more than aggregating on the source IP,
/// because WAF has to extract and hash the components of every request:
/// `RateBasedStatement$AggregateKeyType` documents `CUSTOM_KEYS` as "aggregate the
/// request counts using one or more web request components as the aggregate keys",
/// specified in `CustomKeys`. Pinned by the recorded corpus: the same web ACL,
/// updated so its one rate-based rule aggregates on a custom key, reports
/// `Capacity` 32 — [`RATE_BASED_WCU`] plus this. The recording pins the cost of
/// aggregating on custom keys, not a price per key (`RateBasedStatementCustomKeys`
/// is `length 0..5`), so it is charged once for the list.
pub const RATE_BASED_CUSTOM_KEYS_WCU: i64 = 30;

/// The WCU cost of one statement. See [`web_acl_capacity`].
///
/// `depth_budget` bounds the descent exactly as in
/// [`statement_references_ip_set`]: a caller-supplied tree deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`] is refused on ingress, and this budget is the
/// second line of defence so no request can drive the recursion past it.
fn statement_capacity(
    statement: &Statement,
    depth_budget: u32,
    rule_group_capacities: &HashMap<String, i64>,
) -> i64 {
    if depth_budget == 0 {
        return 0;
    }
    let mut capacity = 0_i64;
    if statement.byte_match_statement.is_some() {
        capacity += BYTE_MATCH_WCU;
    }
    // Base cost 1: size constraint, geo match, IP set reference, label match and
    // ASN match statements.
    for present in [
        statement.size_constraint_statement.is_some(),
        statement.geo_match_statement.is_some(),
        statement.ip_set_reference_statement.is_some(),
        statement.label_match_statement.is_some(),
        statement.asn_match_statement.is_some(),
    ] {
        if present {
            capacity += 1;
        }
    }
    if statement.sqli_match_statement.is_some() {
        capacity += 20;
    }
    if statement.xss_match_statement.is_some() {
        capacity += 40;
    }
    if statement.regex_match_statement.is_some() {
        capacity += 3;
    }
    if statement.regex_pattern_set_reference_statement.is_some() {
        capacity += 25;
    }
    // A rate-based statement costs `RATE_BASED_WCU` on its own, plus
    // `RATE_BASED_CUSTOM_KEYS_WCU` when it aggregates on custom keys; every nesting
    // (logical operator or scope-down) costs what it nests, which
    // `nested_statements` enumerates.
    if let Some(rate) = statement.rate_based_statement.as_ref() {
        capacity += RATE_BASED_WCU;
        if !rate.custom_keys.as_deref().unwrap_or_default().is_empty() {
            capacity += RATE_BASED_CUSTOM_KEYS_WCU;
        }
    }
    // A rule group reference costs the referenced group's declared capacity, which
    // `rule_group_capacities` carries for every rule group of this scope. A
    // reference naming one this service does not hold never reaches here.
    if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
        capacity += rule_group_capacities
            .get(&reference.arn)
            .copied()
            .unwrap_or_default();
    }
    capacity += nested_statements(statement)
        .into_iter()
        .map(|nested| statement_capacity(nested, depth_budget - 1, rule_group_capacities))
        .sum::<i64>();
    capacity
}

/// The `Description` a WAFv2 entity reports when the save that created it supplied
/// none.
///
/// Every WAFv2 response that carries an entity, or one of its `…Summary`
/// projections, carries the `Description` member: the recorded wire responses show
/// `""` for an entity saved without a description, rather than the member being
/// omitted. So the empty string is what gets STORED, and `Get`, `Create` and
/// `List` all report it from state instead of each deciding whether to emit the
/// member. (`EntityDescription` is modelled `length 1..256`, which bounds what a
/// REQUEST may carry; the empty description a RESPONSE reports is recorded
/// behaviour.)
pub fn stored_description(requested: Option<String>) -> Option<String> {
    Some(requested.unwrap_or_default())
}

/// The DDoS-protection mode a web ACL carries when the save names none.
///
/// `OnSourceDDoSProtectionConfig$ALBLowReputationMode` documents
/// `ACTIVE_UNDER_DDOS` as "enabled by default whenever a web ACL is associated with
/// an Application Load Balancer", and the member is `required` inside the
/// structure, so the account default is a real stored value — not an absent
/// structure — and a read reports it.
pub const DEFAULT_ALB_LOW_REPUTATION_MODE: &str = "ACTIVE_UNDER_DDOS";

/// The evaluation window a rate-based statement carries when the save names none.
///
/// `RateBasedStatement$EvaluationWindowSec` documents "Valid settings are 60, 120,
/// 300, and 600" and "Default: `300` (5 minutes)". The member's modelled
/// zero-value default is `0`, which is not one of the valid settings, so a request
/// that omits it arrives here as `0` and WAF's documented default is what gets
/// stored.
pub const DEFAULT_EVALUATION_WINDOW_SEC: i64 = 300;

/// Materialise, in place, every default WAF applies inside a saved rule tree.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request, so nothing here may consume stack in proportion to
/// its depth. A default is applied wherever the statement that carries it sits,
/// top level or nested.
pub fn apply_rule_defaults(rules: &mut [Rule]) {
    let mut pending: Vec<&mut Statement> = rules.iter_mut().map(|rule| &mut rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(rate) = statement
            .rate_based_statement
            .as_mut()
            .filter(|rate| rate.evaluation_window_sec == 0)
        {
            rate.evaluation_window_sec = DEFAULT_EVALUATION_WINDOW_SEC;
        }
        pending.extend(nested_statements_mut(statement));
    }
}

/// Materialise, on a web ACL about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// One place, called by both `CreateWebACL` and `UpdateWebACL`, so a read cannot
/// see a web ACL that was created one way and updated another:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — always a list, so a read of a rule-less web ACL answers `[]` rather
///   than omitting the member; `Rules` is "the Rule statements used to identify the
///   web requests that you want to manage", and a web ACL with none has an empty
///   collection of them, not an unknown one.
/// * `OnSourceDDoSProtectionConfig` — [`DEFAULT_ALB_LOW_REPUTATION_MODE`].
/// * `Capacity` — [`web_acl_capacity`] of the stored rules, which is why it is
///   computed AFTER the rule defaults are applied. `rule_group_capacities` prices
///   the rule-group references the tree carries; see [`web_acl_capacity`].
pub fn apply_web_acl_defaults(
    web_acl: &mut WebACL,
    rule_group_capacities: &HashMap<String, i64>,
) {
    web_acl.description = stored_description(web_acl.description.take());
    let mut rules = web_acl.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    web_acl.capacity = web_acl_capacity(&rules, rule_group_capacities);
    web_acl.rules = Some(rules);
    web_acl.on_source_d_do_s_protection_config = Some(
        web_acl
            .on_source_d_do_s_protection_config
            .take()
            .unwrap_or_else(|| OnSourceDDoSProtectionConfig {
                alb_low_reputation_mode: DEFAULT_ALB_LOW_REPUTATION_MODE.to_string(),
            }),
    );
}

/// The labels a rule group's rules ADD to matching requests, deduplicated and in
/// a stable order.
///
/// `RuleGroup$AvailableLabels` is "the labels that one or more rules in this rule
/// group add to matching web requests. These labels are defined in the
/// `RuleLabels` for a Rule", so it is DERIVED from the stored rules rather than
/// carried in the request. Each entry is the label as the rule specified it
/// (`LabelSummary$Name` is "an individual label specification"); the prefix every
/// one of them acquires at match time is reported separately, as
/// [`label_namespace`].
pub fn rule_group_available_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    let names: ::std::collections::BTreeSet<&str> = rules
        .iter()
        .flat_map(|rule| rule.rule_labels.as_deref().unwrap_or_default())
        .map(|label| label.name.as_str())
        .collect();
    names
        .into_iter()
        .map(|name| LabelSummary {
            name: Some(name.to_string()),
        })
        .collect()
}

/// The labels a rule group's rules MATCH AGAINST, deduplicated and in a stable
/// order.
///
/// `RuleGroup$ConsumedLabels` is "the labels that one or more rules in this rule
/// group match against in label match statements. These labels are defined in a
/// `LabelMatchStatement` specification, in the Statement definition of a rule", so
/// it is derived from the stored rules the way [`rule_group_available_labels`] is —
/// and from the whole statement tree, because a `LabelMatchStatement` can sit
/// inside any nesting [`nested_statements`] enumerates.
pub fn rule_group_consumed_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    let mut keys: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(label_match) = statement.label_match_statement.as_ref() {
            keys.insert(label_match.key.clone());
        }
        pending.extend(nested_statements(statement));
    }
    keys.into_iter()
        .map(|key| LabelSummary { name: Some(key) })
        .collect()
}

/// Materialise, on a rule group about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// The rule-group counterpart of [`apply_web_acl_defaults`], so a rule group reads
/// back the same way however it was last saved:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — always a list, for the same reason the web ACL's is: a rule group
///   with no rules has an empty collection of them, not an unknown one.
/// * `AvailableLabels` / `ConsumedLabels` — derived from the stored rules by
///   [`rule_group_available_labels`] and [`rule_group_consumed_labels`], which is
///   why they are computed AFTER the rule defaults are applied.
///
/// `Capacity` is NOT derived here: unlike `WebACL$Capacity`, a rule group's is
/// "the web ACL capacity units (WCUs) required for this rule group", which the
/// create call declares and "you cannot change it after creation" — so the
/// declared value is what is stored and reported.
pub fn apply_rule_group_defaults(rule_group: &mut RuleGroup) {
    rule_group.description = stored_description(rule_group.description.take());
    let mut rules = rule_group.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    rule_group.available_labels = Some(rule_group_available_labels(&rules));
    rule_group.consumed_labels = Some(rule_group_consumed_labels(&rules));
    rule_group.rules = Some(rules);
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafv2Awswaf20190729State>)
}

// ===========================================================================
// The WAFv2 request-validation layer
// ===========================================================================
//
// Two facts about how WAF refuses a bad parameter live here, and both are about
// the ANSWER rather than about the checking:
//
//   * WAF answers with its OWN modelled `WAFInvalidParameterException`, which the
//     model gives `Field`, `Parameter` and `Reason` members beside the message —
//     so a client can read WHICH input to correct instead of parsing prose.
//     [`invalid_parameter_exception`] builds it, and composes the message from
//     exactly those three the way WAF does.
//   * WAF resolves what a request REFERENCES before it judges the syntax of the
//     request's other members, and it reports a collection whose ELEMENTS are at
//     fault against the collection member's own path. Neither is something the
//     request boundary can do for a handler: the boundary runs every modelled
//     constraint first, and words an element failure at the element's indexed
//     path. An operation therefore hands the members concerned over with
//     `skip_validation` on its input struct (see `src/ops/types/`) and checks them
//     here, at the point in its own sequence where WAF checks them.
//
// NOTHING about constraint CHECKING is re-implemented. The constraints a member's
// `skip_validation` strips off its generated member schema are RE-DECLARED below
// in the framework's own representation — a [`TraitMap`] of
// `LengthTrait`/`PatternTrait`/`ElementConstraintsTrait`, the exact form
// `vendor/smithy-macros` would have attached — hung on a member schema of the same
// name, and the check itself is
// [`::aws_smithy_schema_server::constraints::validate`], the SAME walk the
// boundary runs. So the clause, the elision of a long value, the violation cap and
// the lowerCamel path stay the framework's, and a member refused here reads
// identically to one refused at the boundary.

/// The `ParameterExceptionField` value naming a resource ARN
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `RESOURCE_ARN` among its values).
pub const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `ParameterExceptionField` value naming a web ACL. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_WEB_ACL: &str = "WEB_ACL";

/// The `ParameterExceptionField` value naming a rule group. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP: &str = "RULE_GROUP";

/// The `ParameterExceptionField` value naming a `RuleGroupReferenceStatement`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP_REFERENCE_STATEMENT: &str = "RULE_GROUP_REFERENCE_STATEMENT";

/// The `ParameterExceptionField` value naming a `Scope`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `ParameterExceptionField` value naming a rule statement. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_STATEMENT: &str = "STATEMENT";

/// The `ParameterExceptionField` value naming an IP address. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_IP_ADDRESS: &str = "IP_ADDRESS";

/// The `ParameterExceptionField` value naming a per-call entity limit. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_ENTITY_LIMIT: &str = "ENTITY_LIMIT";

/// The `ErrorReason` WAF carries for an ARN it will not accept, as the recorded
/// WAF responses show it.
///
/// One sentence for the whole class — an ARN that is malformed, and an ARN that is
/// well formed but names nothing WAF can use for this parameter — because that is
/// what WAF answers for both: the `Parameter` member carries the rejected value,
/// so the reason does not have to distinguish them.
pub const ARN_NOT_VALID_REASON: &str = "The ARN isn't valid. A valid ARN begins with arn: \
     and includes other information separated by colons or slashes.";

/// The resource-kind segment a rule group's ARN carries
/// (`arn:…:wafv2:…:<global|regional>/rulegroup/<name>/<id>`, the form
/// [`resource_arn`] mints).
pub const RULE_GROUP_ARN_KIND: &str = "rulegroup";

/// The resource-kind segment a web ACL's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const WEB_ACL_ARN_KIND: &str = "webacl";

/// The resource-kind segment an IP set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const IP_SET_ARN_KIND: &str = "ipset";

/// `WAFInvalidParameterException` as WAF returns it: the structured `Field`,
/// `Parameter` and `Reason` the model gives this one exception, and the message
/// WAF composes from exactly those three.
///
/// This is the one WAFv2 exception whose body describes the CALL (see the note
/// above the constant sentences at the top of this module): `Field` is "the
/// settings where the invalid parameter was found", `Parameter` is "the invalid
/// parameter that resulted in the exception" and `Reason` is "additional
/// information about the exception" (each member's own doc). The message is built
/// from the response's own three members and carries nothing else from the
/// request, so every refusal of this type reads alike whatever tripped it.
pub fn invalid_parameter_exception(
    field: &str,
    parameter: &str,
    reason: &str,
) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "Error reason: {reason}, field: {field}, parameter: {parameter}"
        )),
        field: Some(field.to_string()),
        parameter: Some(parameter.to_string()),
        reason: Some(reason.to_string()),
    }
}

/// [`invalid_parameter_exception`] for an ARN WAF will not accept: `Field`
/// [`FIELD_RESOURCE_ARN`], `Parameter` the rejected ARN and `Reason`
/// [`ARN_NOT_VALID_REASON`].
pub fn invalid_arn_exception(arn: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID_REASON)
}

/// The resource-kind segment of a wafv2 resource ARN (`webacl`, `ipset`,
/// `rulegroup`, …), or `None` when `value` is not one at all.
///
/// The ARN layout is the one [`is_wafv2_resource_arn`] pins:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`,
/// so the kind is the second slash-separated segment of the resource part.
pub fn arn_resource_kind(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(5)?.split('/').nth(1)
}

/// This service's Smithy namespace, for the shadow member schemas below.
const NAMESPACE: &str = "com.amazonaws.wafv2";

/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$` — the `@pattern` shared by
/// `LockToken` and `EntityId`
/// (`smithy_query(query="shape",shape="LockToken,EntityId")`), the form
/// [`new_uuid`] mints.
const UUID_PATTERN: &str = r"^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$";

/// `^[\w\.\-/]+$` — `TokenDomain`'s `@pattern`
/// (`smithy_query(query="shape",shape="TokenDomain")`).
const TOKEN_DOMAIN_PATTERN: &str = r"^[\w\.\-/]+$";

/// `LockToken`: `@length(1..36)` plus [`UUID_PATTERN`].
static LOCK_TOKEN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(36))));
    traits.insert(Box::new(PatternTrait::new(UUID_PATTERN)));
    traits
});

/// `TokenDomain`: `@length(1..253)` plus [`TOKEN_DOMAIN_PATTERN`].
static TOKEN_DOMAIN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(253))));
    traits.insert(Box::new(PatternTrait::new(TOKEN_DOMAIN_PATTERN)));
    traits
});

/// `APIKeyTokenDomains`: `@length(min 1)` ELEMENTS, with `TokenDomain`'s own
/// constraints carried for each element exactly as codegen carries them
/// (`smithy_query(query="shape",shape="APIKeyTokenDomains")`).
static TOKEN_DOMAINS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), None)));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TOKEN_DOMAIN_TRAITS)));
    traits
});

/// `LockToken`, as every operation that takes an optimistic-lock token declares
/// it. The walk reads only a member schema's NAME (for the violation path) and its
/// trait map, so one declaration serves every such operation.
pub static M_LOCK_TOKEN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$LockToken",
        NAMESPACE,
        "LockToken",
    ),
    ShapeType::String,
    "LockToken",
    0,
)
.with_traits(&LOCK_TOKEN_TRAITS);

/// `TokenDomains`, which targets `APIKeyTokenDomains`.
pub static M_TOKEN_DOMAINS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$TokenDomains",
        NAMESPACE,
        "TokenDomains",
    ),
    ShapeType::List,
    "TokenDomains",
    1,
)
.with_traits(&TOKEN_DOMAINS_TRAITS);

/// `TokenDomain`'s constraints as the aggregated clause lists them — every
/// constraint of the element shape, in one set, whichever of them an element
/// failed.
///
/// Spelled out because the framework's `Violation::requirement` wording is not
/// public: each sentence here is that function's own
/// (`Member must have length less than or equal to …`,
/// `Member must satisfy regular expression pattern: …`) and each bound is
/// `TokenDomain`'s, declared once in [`TOKEN_DOMAIN_TRAITS`] above. The ORDER is
/// the order WAF's request validator lists them: the length ceiling, the length
/// floor, then the pattern.
pub const TOKEN_DOMAIN_CONSTRAINTS: &[&str] = &[
    "Member must have length less than or equal to 253",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ^[\w\.\-/]+$",
];

/// The root of the shadow input the walk is driven over. Its member list is empty
/// on purpose: the walk visits what [`ConstrainedMembers`] WRITES, which is
/// whichever members the calling operation handed over, and a root struct schema
/// carries no member name so it contributes no path segment.
static CONSTRAINED_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput",
        NAMESPACE,
        "ConstrainedInput",
    ),
    ShapeType::Structure,
    &[],
);

/// One value handed to the validating walk, written the way the codec writes that
/// shape so the walk builds the same path and runs the same checks.
pub enum ConstrainedValue<'a> {
    /// A string leaf.
    Text(&'a str),
    /// A list of strings; the walk names each element `<member>.<n>.member`.
    Texts(&'a [String]),
}

/// The members of ONE request whose constraints their operation handed over,
/// written for [`::aws_smithy_schema_server::constraints::validate`].
pub struct ConstrainedMembers<'a> {
    members: &'a [(&'static Schema, ConstrainedValue<'a>)],
}

impl SerializableStruct for ConstrainedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        for (schema, value) in self.members {
            match value {
                // A scalar list element is written against the COLLECTION member
                // schema, which is what `vendor/smithy-macros` emits and what lets
                // the walk tell the collection's element-count bound from the
                // elements' own bounds (the latter travel in
                // `ElementConstraintsTrait`).
                ConstrainedValue::Text(text) => s.write_string(schema, text)?,
                ConstrainedValue::Texts(texts) => s.write_list(schema, &|sl| {
                    for text in texts.iter() {
                        sl.write_string(schema, text)?;
                    }
                    Ok(())
                })?,
            }
        }
        Ok(())
    }
}

/// Run the framework's own constraint walk over the members an operation handed
/// over, and report every violation it found.
pub fn validate_members(members: &[(&'static Schema, ConstrainedValue<'_>)]) -> Vec<Violation> {
    validate(&ConstrainedMembers { members }, &CONSTRAINED_INPUT_SCHEMA)
}

/// The framework's own `ValidationException` envelope for `violations`, so a
/// member refused by a handler reads exactly like one refused at the boundary.
pub fn constraint_envelope(violations: &[Violation]) -> String {
    ::aws_smithy_schema_server::envelope(violations)
        .unwrap_or_else(|| "invalid request".to_string())
}

/// The envelope WAF's request validator answers for a collection of scalars,
/// given the violations the framework walk found in it — or `None` when it found
/// none.
///
/// WAF reports an ELEMENT that fails the element shape's own constraints against
/// the COLLECTION member's path, with the value rendered as the whole list and
/// every constraint of the element shape listed in one bracketed set: ONE clause
/// for the member, however many elements failed and however many constraints each
/// failed. A violation of the collection ITSELF — its element-count `@length` — is
/// a different constraint about a different value, so it keeps the framework's own
/// clause.
///
/// `member_path` is the collection member's path as the envelope names it (use
/// [`member_path`]), `elements` is the list as the request sent it, and
/// `element_constraints` is the element shape's constraint set (e.g.
/// [`TOKEN_DOMAIN_CONSTRAINTS`]).
pub fn collection_constraint_envelope(
    member_path: &str,
    elements: &[String],
    element_constraints: &[&str],
    violations: &[Violation],
) -> Option<String> {
    // The value every clause about this member echoes: the list, the way WAF
    // renders one.
    let rendered = format!("[{}]", echoed(&elements.join(", ")));
    let element_prefix = format!("{member_path}.");
    let mut clauses: Vec<String> = Vec::new();
    let mut element_clause_emitted = false;
    for violation in violations {
        // The collection ITSELF (its element-count `@length`): the framework's own
        // requirement wording, restated against the list as the value — the walk
        // renders a count there (`3 items`), and WAF renders the list.
        if violation.path == member_path {
            clauses.push(
                Violation {
                    path: violation.path.clone(),
                    kind: violation.kind.clone(),
                    value: Some(rendered.clone()),
                }
                .message(),
            );
            continue;
        }
        if !violation.path.starts_with(&element_prefix) {
            clauses.push(violation.message());
            continue;
        }
        // Every element failure of this member collapses into the one clause, so
        // it is emitted at the FIRST of them and skipped thereafter.
        if element_clause_emitted {
            continue;
        }
        element_clause_emitted = true;
        clauses.push(format!(
            "Value '{rendered}' at '{member_path}' failed to satisfy constraint: \
             Member must satisfy constraint: [{}]",
            element_constraints.join(", ")
        ));
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// A member's path as a validation clause names it: the Smithy member name in
/// lowerCamel.
///
/// Delegates to the framework, which owns that casing rule, so a clause a handler
/// writes cannot spell a position differently from one the boundary writes.
pub fn member_path(smithy_name: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(smithy_name)
}

/// The rejected value as it appears in a clause: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// The cap and its rendering are the framework's own (its `MAX_ECHOED_VALUE`, 256
/// bytes), re-stated here only because that helper is private. A caller controls
/// the length of what a clause echoes, so an unelided echo turns a large request
/// into an arbitrarily large response. It is applied to the RENDERING only: every
/// check runs on the value the caller actually sent.
fn echoed(value: &str) -> String {
    const MAX_ECHOED_VALUE: usize = 256;
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}
