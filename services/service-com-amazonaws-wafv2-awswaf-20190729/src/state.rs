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

use crate::ops::types::{
    IPSet, Rule, Statement, Tag, WAFAssociatedItemException, WAFDuplicateItemException,
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
    /// this service's own stores. An `IPSetReferenceStatement` resolves through
    /// [`Self::arn_index`], and only to an IP set in the SAME scope, because a scope
    /// is a namespace. The other three — `RuleGroupReferenceStatement`,
    /// `ManagedRuleGroupStatement` and `RegexPatternSetReferenceStatement` — name
    /// entity kinds this emulator has no store for yet (`CreateRuleGroup`,
    /// `CreateRegexPatternSet` and the managed-rule-group catalogue are still
    /// scaffolded 501s, tracked as separate gap issues), so every reference to one
    /// names nothing that exists and is refused here. The `Some` it answers is the
    /// DIAGNOSTIC for the refusal's audit record — the response itself carries
    /// [`NONEXISTENT_ITEM_MESSAGE`], the constant sentence that exception type always
    /// carries. That is also what keeps
    /// [`web_acl_capacity`] from pricing a statement whose cost is the referenced
    /// group's own capacity: no stored rule list can carry one.
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
                return Some(format!(
                    "No RuleGroup with the ARN {} exists in scope {scope}",
                    reference.arn
                ));
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

/// The most addresses one IP set holds: WAF's published quota is 10,000 IP
/// addresses or ranges per IP set. It bounds how large a single stored entry can
/// be, beside the serve path's own 16 MiB request-body cap.
pub const MAX_IP_SET_ADDRESSES: usize = 10_000;

/// The statements nested directly inside `statement`, in no particular order.
///
/// One place names every nesting the model has, so the depth validator and both
/// tree walks below agree on what "nested" means.
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

/// `awswaf:<account ID>:webacl:<web ACL name>:` — the prefix the
/// `WebACL$LabelNamespace` doc specifies for every label a rule in this web ACL
/// adds.
pub fn web_acl_label_namespace(name: &str) -> String {
    format!("awswaf:{}:webacl:{name}:", ::engine::emulator_account_id())
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
/// Two costs this slice cannot know are never reached rather than guessed: a
/// `RuleGroupReferenceStatement` costs the referenced rule group's own capacity and
/// a `ManagedRuleGroupStatement` costs the managed group's, and neither rule groups
/// (`CreateRuleGroup`) nor the managed catalogue
/// (`ListAvailableManagedRuleGroups`) is implemented yet — both are separate gap
/// issues, as is `CheckCapacity`. A statement referencing either names nothing this
/// service holds, so it is refused on ingress by
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`] and no
/// stored rule list can carry one: this function never has to price a reference
/// whose cost it cannot know, and no caller is handed an understated `Capacity`.
pub fn web_acl_capacity(rules: &[Rule]) -> i64 {
    rules
        .iter()
        .map(|rule| statement_capacity(&rule.statement, MAX_STATEMENT_NESTING_DEPTH))
        .sum()
}

/// The WCU cost of one statement. See [`web_acl_capacity`].
///
/// `depth_budget` bounds the descent exactly as in
/// [`statement_references_ip_set`]: a caller-supplied tree deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`] is refused on ingress, and this budget is the
/// second line of defence so no request can drive the recursion past it.
fn statement_capacity(statement: &Statement, depth_budget: u32) -> i64 {
    if depth_budget == 0 {
        return 0;
    }
    let mut capacity = 0_i64;
    // Base cost 1: byte match, size constraint, geo match, IP set reference,
    // label match and ASN match statements.
    for present in [
        statement.byte_match_statement.is_some(),
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
    // A rate-based statement costs 2 on its own; every nesting (logical operator
    // or scope-down) costs what it nests, which `nested_statements` enumerates.
    if statement.rate_based_statement.is_some() {
        capacity += 2;
    }
    capacity += nested_statements(statement)
        .into_iter()
        .map(|nested| statement_capacity(nested, depth_budget - 1))
        .sum::<i64>();
    capacity
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
