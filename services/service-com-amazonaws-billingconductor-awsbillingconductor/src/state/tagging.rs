// SPDX-License-Identifier: CC0-1.0

//! The tagging plane's shared vocabulary: how a tagging `ResourceArn` is read into the
//! resource it names, the bounds a resource's tag map is held to, and the not-found this
//! service's tagging surface answers.

use super::*;

/// How many tags one resource holds.
///
/// The modelled `TagMap`'s own `@length` maximum
/// (`smithy_query(query="shape",shape="TagMap")`: len 1..200) — the same bound the
/// boundary already enforces on ONE request's map, applied to what a resource
/// ACCUMULATES over several `TagResource` calls. A merge that would take a resource
/// past it is REFUSED with the modelled `ValidationException` rather than stored, so a
/// caller cannot grow one resource's tag map without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix Amazon Web Services keeps for the tags it generates itself.
///
/// Amazon Web Services' tagging rules reserve this prefix service-wide: a caller can
/// neither create nor edit nor delete a tag key carrying it, and a request that tries is
/// refused. The model cannot express the rule — `TagKey` carries only
/// `@length(1..128)` and no pattern (`smithy_query(query="shape",shape="TagKey")`), so
/// the boundary walk admits such a key — which is why every path taking a
/// caller-supplied tag key enforces it through [`check_tag_keys_not_reserved`].
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The comparison is case-insensitive, because the namespace is reserved however it is
/// cased. It is a byte-slice compare rather than a lowercasing one so no allocation
/// happens per key; `get` answers `None` for a key shorter than the prefix, or one whose
/// fourth byte falls inside a multi-byte character, and both of those are exactly "not
/// reserved" — a `TagKey`'s only modelled constraint is its length in characters, so any
/// character at all reaches here.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The input member a caller-supplied TAG MAP travels in, as the model declares it —
/// what a refusal about that map names in `Fields`, and the spelling the clause it builds
/// interpolates.
///
/// `TagResource` and the four creates that persist tags all declare it under this one
/// name (`smithy_query(query="operation",operation="TagResource")`). It is a constant
/// rather than a literal per call site so the several refusals ABOUT that member — the
/// reserved namespace, the per-resource cap — cannot spell it two different ways.
pub const TAGS_MEMBER: &str = "Tags";

/// The input member the tag KEYS of a removal travel in, as the model declares it
/// (`smithy_query(query="operation",operation="UntagResource")`), beside
/// [`TAGS_MEMBER`] for the map a tagging call carries: a refusal names the member the
/// request itself carried the offending value in.
pub const TAG_KEYS_MEMBER: &str = "TagKeys";

/// `Ok(())` when no key in `keys` is reserved, or the modelled `ValidationException`
/// that refuses the request.
///
/// One helper, so every operation that takes caller-supplied tag keys — `TagResource`
/// and `UntagResource`, and the `Tags` of `CreateBillingGroup`, `CreatePricingPlan`,
/// `CreatePricingRule` and `CreateCustomLineItem` — refuses that namespace the same way
/// and cannot drift apart. `member` names the input member that carried the keys, which
/// is what travels in `Fields` — [`TAGS_MEMBER`] or [`TAG_KEYS_MEMBER`], as the model
/// declares it — and the message is built from the request: the offenders
/// are sorted and deduplicated, so a map's iteration order never shows through the text.
pub fn check_tag_keys_not_reserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), crate::ops::types::ValidationException> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    Err(validation_exception(
        "FIELD_VALIDATION_FAILED",
        member,
        format!(
            "Value at '{member}' failed to satisfy constraint: a tag key must not begin \
             with '{RESERVED_TAG_KEY_PREFIX}', the prefix reserved for the tags Amazon Web \
             Services generates; this request names {}",
            reserved.join(", ")
        ),
    ))
}

/// The wire error code a tagging operation answers when its `ResourceArn` names no
/// resource of this service.
///
/// NOT a shape this service's model declares — the model lists
/// `ResourceNotFoundException` (404) for these operations — but the code the real
/// service answers the tagging surface with, as recorded by the behavioral corpus:
/// HTTP 404, error code `NotFoundException`, message "Resource was not found" (see
/// `issues/.../20261022T200837Z-billingconductor-tagresource-stub.md`, whose check
/// asserts `tag_code == "NotFoundException"`, and cause 6 of
/// `...-billingconductor-payer-account-authorization-gate-absent-across-the.md`, which
/// records the same code, status and message). The code and the status are what a
/// client classifies on, so they follow the recording rather than the model.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The status [`TAGGING_NOT_FOUND_CODE`] is served on (recorded: HTTP 404, the same
/// status the model gives `ResourceNotFoundException`).
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The message the real service's tagging not-found carries. It names nothing from the
/// request, so it is the same sentence for every resource.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The tagging not-found failure, as a tagging handler returns it: the recorded code
/// and status under the operation's own error channel.
pub fn tagging_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGING_NOT_FOUND_MESSAGE,
    )
}

/// Whether a tagging `ResourceArn` addresses a resource AMAZON WEB SERVICES owns rather
/// than one of this account's own.
///
/// The decision the tagging surface makes BEFORE it reads a missing record as a
/// not-found: a resource of Amazon Web Services' own is present in every account without
/// any create, so "this account stores no such record" does not mean it does not exist,
/// and the tags of a resource the caller does not own are not the caller's to read or to
/// write. The whole ARN decides it — the account segment the service's pricing-plan ARN
/// form spells `aws` for the plans Amazon Web Services owns
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`:
/// `arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`)
/// — so it is the same recognition the pricing-plan control plane makes
/// ([`is_aws_managed_pricing_plan`]) and the two doors onto those plans agree: an ARN of
/// a 12-digit account, in another partition, or of another resource kind names an
/// ordinary resource that has to be stored to exist.
pub fn is_aws_owned_taggable_resource(resource_arn: &str) -> bool {
    parse_taggable_arn(resource_arn).is_some_and(|resource| {
        resource.kind == TaggableResourceKind::PricingPlan
            && is_aws_managed_pricing_plan(&resource.arn)
    })
}

/// The modelled refusal a tagging call addressing a resource Amazon Web Services owns
/// earns: `AccessDeniedException`, which every tagging operation declares
/// (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
/// `AccessDeniedException` @httpError 403, documented "You do not have sufficient access
/// to perform this action.").
///
/// It is the classification, not the not-found, that is the requirement here: the resource
/// IS there, so [`tagging_not_found`] would tell a client the plan does not exist. The
/// message is the sentence the model documents for this error class, which names nothing
/// from the request — the same way this surface's not-found sentence does.
pub fn tagging_access_denied() -> crate::ops::types::AccessDeniedException {
    crate::ops::types::AccessDeniedException {
        message: AWS_OWNED_RESOURCE_ACCESS_DENIED_MESSAGE.to_string(),
    }
}

/// The message [`tagging_access_denied`] carries: the text the model documents for this
/// service's `AccessDeniedException`.
pub const AWS_OWNED_RESOURCE_ACCESS_DENIED_MESSAGE: &str =
    "You do not have sufficient access to perform this action.";

/// The resource kinds this service's tagging surface addresses, as the resource
/// segment of the modelled tagging `Arn` spells them
/// (`smithy_query(query="operation",operation="TagResource")`:
/// `billinggroup/?[0-9]{12}`, `pricingplan/[a-zA-Z0-9]{10}`,
/// `pricingrule/[a-zA-Z0-9]{10}` and `(customlineitem/)?[a-zA-Z0-9]{10}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableResourceKind {
    BillingGroup,
    PricingPlan,
    PricingRule,
    CustomLineItem,
}

impl TaggableResourceKind {
    /// Every kind, in the order the `Arn` pattern enumerates them.
    pub const ALL: [Self; 4] = [
        Self::BillingGroup,
        Self::PricingPlan,
        Self::PricingRule,
        Self::CustomLineItem,
    ];

    /// The resource segment's prefix, which is also how this service's errors name
    /// the kind (`ConflictException.ResourceType` spells a plan `pricingplan`).
    pub fn segment(self) -> &'static str {
        match self {
            Self::BillingGroup => "billinggroup",
            Self::PricingPlan => "pricingplan",
            Self::PricingRule => "pricingrule",
            Self::CustomLineItem => "customlineitem",
        }
    }
}

/// A resource a tagging operation addressed: which kind its ARN names, that ARN in the
/// ONE spelling this service's maps are keyed by (see the module header), and the bare
/// resource id.
#[derive(Debug, Clone)]
pub struct TaggableResource {
    pub kind: TaggableResourceKind,
    pub id: String,
    pub arn: String,
}

/// Read a tagging `ResourceArn` into the resource it names, `None` when it names no
/// resource of this service at all.
///
/// The modelled `Arn` accepts the full ARN of any of the four taggable kinds — with the
/// `billinggroup` separator OPTIONAL — or a bare custom-line-item id, so one value has
/// several spellings and they must all resolve to the one key the maps use. The
/// partition and account segments are KEPT: another account's (or another partition's)
/// ARN names a resource this emulator does not hold, which is a not-found rather than a
/// hit on the same-id local resource. A region segment is kept the same way: this
/// service is global, so every ARN it mints carries the empty region [`ARN_REGION`] and
/// an ARN naming a region names no resource of this service.
pub fn parse_taggable_arn(value: &str) -> Option<TaggableResource> {
    let (partition, account, resource) = match value.strip_prefix("arn:") {
        Some(rest) => {
            let mut segments = rest.splitn(5, ':');
            let partition = segments.next()?;
            let service = segments.next()?;
            let region = segments.next()?;
            let account = segments.next()?;
            let resource = segments.next()?;
            if service != ARN_SERVICE || region != ARN_REGION {
                return None;
            }
            (
                partition.to_string(),
                account.to_string(),
                resource.to_string(),
            )
        }
        // The only form the pattern admits with no ARN prefix is a custom line item id
        // (`^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$`),
        // so a bare value names a custom line item of this emulator's own account.
        None => (
            "aws".to_string(),
            ::engine::emulator_account_id().to_string(),
            format!("customlineitem/{value}"),
        ),
    };

    let kind = TaggableResourceKind::ALL
        .into_iter()
        .find(|kind| resource.starts_with(kind.segment()))?;
    // `billinggroup/?` — the separator the pattern makes optional, so both spellings of
    // one resource resolve to the same id.
    let rest = &resource[kind.segment().len()..];
    let id = rest.strip_prefix('/').unwrap_or(rest);
    if id.is_empty() {
        return None;
    }
    Some(TaggableResource {
        kind,
        id: id.to_string(),
        arn: format!(
            "arn:{partition}:{ARN_SERVICE}::{account}:{}/{id}",
            kind.segment()
        ),
    })
}
