// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCE a tagging request addresses, and the refusals for one this store holds no
//! record of.
//!
//! `ListTags`, `TagResource` and `UntagResource` all address a resource by the ARN in their
//! `/2017-03-31/tags/{Resource}` path, so the whole of that reading lives here, beside the
//! records it resolves: what a `TaggableResource` ARN names ([`TaggableResourceRef`]), the
//! quotas a tag set is held to, the tag set the ARN resolves to and the sentence a resource
//! of a type this store holds no record of is answered with. It moved out of the state
//! module's root with no change: every name is re-exported there, so a handler still reaches
//! it as `crate::state::…`.

use super::*;

/// What a `TaggableResource` ARN — the `Resource` label of the
/// `/2017-03-31/tags/{Resource}` operations — names.
///
/// The member's own pattern admits exactly six Lambda resource types
/// (`smithy_query(query="operation", operation="ListTags")`): a function, optionally
/// qualified by a published version or an alias, a layer, a code-signing configuration, an
/// event source mapping, a capacity provider and a network connector. A function, a
/// capacity provider, an event source mapping and a code-signing configuration are the
/// ones this store holds records of; the rest are reported by their resource type and
/// answered as what is true of them here — a resource of that type this account does not
/// have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResourceRef {
    /// `…:function:<name>` — one function, whose tag set is
    /// [`FunctionRecord::tags`].
    Function(String),
    /// `…:function:<name>:<qualifier>` — a published version or an alias of a function.
    /// The `Resource` member's own documentation says "Lambda does not support adding
    /// tags to function aliases or versions", so a qualified ARN names no tag set.
    FunctionQualifier {
        /// The bare function name the ARN carries.
        name: String,
        /// The version label or alias name qualifying it.
        qualifier: String,
    },
    /// `…:capacity-provider:<name>` — one capacity provider, whose tag set is
    /// [`CapacityProviderRecord::tags`].
    CapacityProvider(String),
    /// `…:event-source-mapping:<uuid>` — one event source mapping, whose tag set is
    /// [`EventSourceMappingRecord::tags`].
    EventSourceMapping(String),
    /// `…:code-signing-config:<id>` — one code-signing configuration, whose tag set is
    /// [`CodeSigningConfigRecord::tags`]. The store keys a configuration by its ARN, so
    /// this carries the whole ARN rather than the bare id.
    CodeSigningConfig(String),
    /// Another Lambda resource type the pattern admits, by its ARN resource type.
    Other(String),
}

/// Split a `TaggableResource` into the parts that identify what it names, or `None`
/// when the value is not a `lambda` resource ARN at all.
///
/// The ARN is read through `service_common::arn` — the one parse this repository states
/// for a caller-supplied ARN — so the `<type>:<id>` and `<type>/<id>` conventions are
/// handled in one place, and the function form is then split by
/// [`parse_function_reference`] so that every tag operation resolves a function exactly
/// the way every other operation on this service does.
///
/// The ARN's region and account fields are read by that parse and deliberately NOT
/// reported: this store keys a function by its name alone, exactly as
/// [`resolve_function`] resolves every other `FunctionName`, so reporting them here
/// would invite a caller to refuse a cross-region or cross-account ARN that every
/// other operation on this service accepts. A tag operation that needs to refuse one
/// has to make that rule hold for the whole service, and should call
/// `service_common::arn::parse_of` for the fields when it does.
///
/// [`TaggableResourceRef::Other`] is only a resource TYPE because this store holds no
/// records of those types yet — whoever implements the network-connector operations has
/// to carry the type's id through here and turn the tag handlers' `Other` branch into a
/// real lookup against the new records, the way a capacity provider, an event source
/// mapping and a code signing configuration are resolved here.
#[must_use]
pub fn parse_taggable_resource(value: &str) -> Option<TaggableResourceRef> {
    let arn = service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    let resource_type = arn.resource_type();
    if resource_type.is_empty() {
        return None;
    }
    // A capacity provider is named by `…:capacity-provider:<name>`, and this store holds
    // one record per provider, so the ARN resolves to the name that record is keyed by.
    if resource_type == "capacity-provider" {
        return Some(TaggableResourceRef::CapacityProvider(
            parse_capacity_provider_name(value)?,
        ));
    }
    // An event source mapping is named by `…:event-source-mapping:<uuid>`, and this store
    // holds one record per mapping, so the ARN resolves to the UUID that record is keyed
    // by.
    if resource_type == "event-source-mapping" {
        let uuid = service_common::arn::resource_id_of_type(value, resource_type)?;
        return (!uuid.is_empty())
            .then(|| TaggableResourceRef::EventSourceMapping(uuid.to_string()));
    }
    // A code-signing configuration is named by `…:code-signing-config:<id>`, and this
    // store keys one record per configuration BY THAT ARN (the identifier every modelled
    // operation on one binds), so the whole value is what names the record.
    if resource_type == CODE_SIGNING_CONFIG_RESOURCE_TYPE {
        let id = service_common::arn::resource_id_of_type(value, resource_type)?;
        return (!id.is_empty()).then(|| TaggableResourceRef::CodeSigningConfig(value.to_string()));
    }
    if resource_type != "function" {
        return Some(TaggableResourceRef::Other(resource_type.to_string()));
    }
    let reference = parse_function_reference(value)?;
    Some(match reference.qualifier {
        Some(qualifier) => TaggableResourceRef::FunctionQualifier {
            name: reference.name,
            qualifier,
        },
        None => TaggableResourceRef::Function(reference.name),
    })
}

/// The most tags one Lambda resource may carry.
///
/// AWS publishes it as a tagging restriction rather than in the model: the Lambda
/// Developer Guide's "Tag Lambda resources" states "Maximum number of tags per resource:
/// 50". A tag write that would leave a resource holding more than this is refused whole,
/// so the resource keeps the tag set it had.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Lambda's sentence for a tag write that would push a resource past
/// [`MAX_TAGS_PER_RESOURCE`].
///
/// It states the quota that was hit and nothing of the request: not the resource, not the
/// count the merge would have left behind. The restriction is a published quota, so the
/// sentence is the same for every resource that hits it.
pub const TAG_LIMIT_EXCEEDED_MESSAGE: &str = "Number of tags exceeds resource tag limit.";

/// The tag-key namespace AWS keeps for the tags it applies itself.
///
/// The Lambda Developer Guide's "Tag Lambda resources" states the restriction on a tag
/// KEY: "Don't use the `aws:` prefix in tag keys, because it is reserved for Amazon Web
/// Services use. You can't edit or delete tag keys with this prefix."
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Lambda's sentence for a request carrying a tag key it will not store.
///
/// It names the three conditions a key must meet and NOTHING of the request — not the
/// resource, not the offending key — so the sentence is the same for every request that
/// fails any of them.
pub const INVALID_TAGS_MESSAGE: &str = "One or more of your tags had a problem, please \
     ensure none of your tag names start with \"aws:\", none are empty, and there are no \
     duplicate tags";

/// Lambda's sentence for a tag ARN that qualifies a function with a version or an alias.
///
/// `Resource`'s own documentation states the rule — "Lambda does not support adding tags
/// to function aliases or versions" — and the sentence says that and what to send instead,
/// carrying nothing of the request: the caller already named the ARN it sent.
pub const TAGS_ON_QUALIFIED_FUNCTION_MESSAGE: &str =
    "Tags on function aliases and versions are not supported. Please specify a function \
     ARN.";

/// Lambda's sentence for a tag write that names no tag at all.
///
/// `TagResource`'s `Tags` is `@required` ("A list of tags to apply to the resource"), and
/// a map with no entry applies none: there is nothing to store, so the request is refused
/// rather than answered as a write that changed nothing. The sentence states the refusal
/// and nothing about the request — it is the one AWS answers for this condition.
pub const EMPTY_TAGS_MESSAGE: &str = "An error occurred and the request cannot be processed.";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Compared without regard to case, and by BYTES rather than by characters, so a key
/// whose first bytes are a multi-byte character is simply not reserved instead of
/// panicking on a split character boundary.
#[must_use]
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Whether every key in `keys` is one Lambda will store: not empty, not in the reserved
/// namespace, and named at most once.
///
/// This is the whole of what [`INVALID_TAGS_MESSAGE`] states, asked of the keys a single
/// request carries, so every operation that STORES tags asks it the same way and refuses
/// with the same sentence.
#[must_use]
pub fn tag_keys_are_storable<'a, I>(keys: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    keys.into_iter()
        .all(|key| !key.is_empty() && !is_reserved_tag_key(key) && seen.insert(key))
}

/// Why a `TaggableResource` — the `Resource` label of the `/2017-03-31/tags/{Resource}`
/// operations — names no tag set.
///
/// The two variants are the two answers AWS gives, and every tag operation maps them to
/// its own modelled errors the same way: [`Self::Invalid`] is the caller naming something
/// that carries no tag set at all (an `InvalidParameterValueException`, HTTP 400) and
/// [`Self::NotFound`] is a well-formed ARN for a resource this account does not hold (a
/// `ResourceNotFoundException`, HTTP 404).
#[derive(Debug, Clone)]
pub enum TagResolveFailure {
    /// The value is no Lambda resource ARN, or it qualifies a function with a version or
    /// an alias, which carries no tag set of its own.
    Invalid(String),
    /// The ARN is well formed but names a resource this account does not hold.
    NotFound(String),
}

/// Resolve a `TaggableResource` to the ONE tag map the store holds for the resource it
/// names, so `ListTags`, `TagResource` and `UntagResource` all read and write the same
/// record rather than three copies of it.
///
/// A function's tags live on [`FunctionRecord::tags`] (where `CreateFunction` persists
/// the `Tags` of its request and `GetFunction` reads the `Tags` it reports), a capacity
/// provider's on [`CapacityProviderRecord::tags`], an event source mapping's on
/// [`EventSourceMappingRecord::tags`] and a code signing configuration's on
/// [`CodeSigningConfigRecord::tags`]. A function ARN carrying a qualifier names no tag
/// set — `Resource`'s own documentation says "Lambda does not support adding tags to
/// function aliases or versions" — and a resource type this store holds no records of (a
/// network connector) names no resource at all until whoever implements its create
/// carries its records through [`parse_taggable_resource`].
///
/// The borrow is mutable because that is what a tag write needs; a reporting caller takes
/// it too and simply does not write. The function branch goes through
/// [`resolve_function`], so a function is resolved exactly as every other operation on
/// this service resolves one, and the lifecycle transition the clock already owed is
/// materialised on the way.
pub fn resolve_resource_tags<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    resource: &str,
    region: &str,
    now: SystemTime,
) -> Result<&'a mut BTreeMap<String, String>, TagResolveFailure> {
    // The `Resource` pattern has already been enforced at the boundary, so the value here
    // is one of the resource ARNs it admits; the parse says WHICH.
    let named = parse_taggable_resource(resource).ok_or_else(|| {
        TagResolveFailure::Invalid(format!(
            "1 validation error detected: Value '{resource}' at 'resource' failed to satisfy \
             constraint: Member must be the Amazon Resource Name of a Lambda resource"
        ))
    })?;

    match named {
        TaggableResourceRef::Function(name) => {
            let (record, _version) = resolve_function(store, &name, None, region, now).map_err(
                |failure| match failure {
                    ResolveFailure::Malformed(message) => TagResolveFailure::Invalid(message),
                    ResolveFailure::NotFound(message) => TagResolveFailure::NotFound(message),
                },
            )?;
            Ok(&mut record.tags)
        }
        TaggableResourceRef::FunctionQualifier { .. } => Err(TagResolveFailure::Invalid(
            TAGS_ON_QUALIFIED_FUNCTION_MESSAGE.to_string(),
        )),
        TaggableResourceRef::CapacityProvider(provider) => {
            let record = store
                .capacity_providers
                .get_mut(&provider)
                .ok_or_else(no_such_taggable_resource)?;
            record.materialize(now);
            Ok(&mut record.tags)
        }
        TaggableResourceRef::EventSourceMapping(uuid) => {
            // A mapping whose removal the clock already made due is gone, so it holds no
            // tag set either: what the clock owes is applied in ONE place.
            store.materialize_all(now);
            let record = store
                .event_source_mappings
                .get_mut(&uuid)
                .ok_or_else(no_such_taggable_resource)?;
            Ok(&mut record.tags)
        }
        TaggableResourceRef::CodeSigningConfig(arn) => {
            let record = store
                .code_signing_configs
                .get_mut(&arn)
                .ok_or_else(no_such_taggable_resource)?;
            Ok(&mut record.tags)
        }
        TaggableResourceRef::Other(_) => Err(no_such_taggable_resource()),
    }
}

/// The refusal a well-formed `TaggableResource` ARN this account holds no resource for
/// gets: the canonical [`RESOURCE_NOT_FOUND_MESSAGE`], naming neither the resource kind
/// nor the ARN the request addressed.
///
/// A FUNCTION is the exception, because AWS gives it its own sentence; that branch above
/// carries [`resolve_function`]'s refusal through unchanged.
fn no_such_taggable_resource() -> TagResolveFailure {
    TagResolveFailure::NotFound(RESOURCE_NOT_FOUND_MESSAGE.to_string())
}
