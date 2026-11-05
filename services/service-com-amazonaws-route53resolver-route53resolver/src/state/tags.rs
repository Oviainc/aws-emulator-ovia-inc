// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The most tags one resource carries.
///
/// The model's `TagList` is length 0..200 — it is both the list `TagResource`
/// takes and the list `ListTagsForResource` reports — so 200 tags is the most
/// one call can hand over and the most a resource can report back. A write that
/// would take a resource past it is refused with the `LimitExceededException`
/// `TagResource` models, and the stored set is left exactly as it was.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The resource-type segment of each taggable resource's ARN, with the modelled
/// shape name of that resource.
///
/// The segments are the ones the create operations mint an ARN out of
/// (`resolver-endpoint/<id>`, `resolver-rule/<id>`, `outpost-resolver/<id>`,
/// `firewall-domain-list/<id>`, `resolver-query-log-config/<id>`); the shape
/// names are what every other refusal of this service reports in
/// `ResourceNotFoundException$ResourceType`.
pub(super) const TAGGABLE_ARN_SEGMENTS: [(&str, &str); 7] = [
    ("resolver-endpoint", RESOURCE_TYPE_RESOLVER_ENDPOINT),
    ("resolver-rule", RESOURCE_TYPE_RESOLVER_RULE),
    ("outpost-resolver", RESOURCE_TYPE_OUTPOST_RESOLVER),
    ("firewall-domain-list", RESOURCE_TYPE_FIREWALL_DOMAIN_LIST),
    ("firewall-rule-group", RESOURCE_TYPE_FIREWALL_RULE_GROUP),
    (
        "firewall-rule-group-association",
        RESOURCE_TYPE_FIREWALL_RULE_GROUP_ASSOCIATION,
    ),
    (
        "resolver-query-log-config",
        RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG,
    ),
];

/// The resource type an ARN states, when the ARN names one of this service's
/// taggable resources: the segment that follows the account id, up to the `/`
/// that separates it from the resource id
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// `None` for anything else, so a tag operation's refusal names a resource type
/// only when the ARN it was handed actually states one.
pub fn arn_resource_type(resource_arn: &str) -> Option<&'static str> {
    let (segment, _) = resource_arn.split(':').nth(5)?.split_once('/')?;
    TAGGABLE_ARN_SEGMENTS
        .iter()
        .find(|(name, _)| *name == segment)
        .map(|(_, resource_type)| *resource_type)
}

/// A resource of this service that carries tags, as a tag operation's
/// `ResourceArn` resolves it: which collection holds it, and its key there.
///
/// `TagResource$ResourceArn`'s doc names the Get and List commands an ARN comes
/// from, so the resources whose records carry an `Arn` are the ones the tag
/// operations address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    ResolverEndpoint(String),
    ResolverRule(String),
    OutpostResolver(String),
    FirewallDomainList(String),
    FirewallRuleGroup(String),
    FirewallRuleGroupAssociation(String),
    ResolverQueryLogConfig(String),
}

/// The id an ARN names the resource by: everything after the last `/`, the
/// separator between an ARN's resource type and its resource id
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// A string carrying no `/` is taken as naming the resource by itself, so the
/// refusal still states what was asked for.
pub fn arn_resource_id(resource_arn: &str) -> &str {
    match resource_arn.rsplit_once('/') {
        Some((_, id)) => id,
        None => resource_arn,
    }
}

/// The `ResourceNotFoundException` the tag operations answer for an ARN this
/// store holds no resource under.
///
/// Built here so `TagResource`, `UntagResource` and `ListTagsForResource`
/// cannot drift apart: all of them report the RSLVR-coded message, which names
/// the resource by the id its ARN carries ([`resource_not_found_message`]), and
/// the resource type the model asks for.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(resource_arn)),
        resource_type: arn_resource_type(resource_arn).map(str::to_string),
    }
}

/// The `LimitExceededException` `TagResource` answers for a write that would
/// take a resource past [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// The sentence states the limit the model's `TagList` length fixes and nothing
/// from the request, so the rejected tags are not echoed back. No recorded
/// response available here states the wording Route 53 Resolver uses for this
/// fault, and the model states no message text, so — as for
/// [`outpost_resolver_not_found`] — the HTTP status (400) and the error shape
/// are what the contract fixes and the wording is an example.
pub fn tag_limit_exceeded(resource_arn: &str) -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "A resource can have at most {MAX_TAGS_PER_RESOURCE} tags"
        )),
        resource_type: arn_resource_type(resource_arn).map(str::to_string),
    }
}

/// Add `requested` to `stored`, one resource's tag set.
///
/// A tag write is an upsert keyed on the tag key: a key the resource already
/// carries keeps its position and takes the requested value, and a key new to
/// the resource is appended — so the order a resource reports its tags in is the
/// order its keys were first written in, and no key is ever reported twice.
/// Keys are compared exactly, as tag keys are case-sensitive. `requested` is
/// applied in order, so a request that names one key twice leaves the value it
/// named last.
///
/// Answers `false`, leaving `stored` EXACTLY as it was, when the result would
/// hold more than [`MAX_TAGS_PER_RESOURCE`] tags.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) -> bool {
    let projected = {
        let mut keys: HashSet<&str> = stored.iter().map(|tag| tag.key.as_str()).collect();
        keys.extend(requested.iter().map(|tag| tag.key.as_str()));
        keys.len()
    };
    if projected > MAX_TAGS_PER_RESOURCE {
        return false;
    }

    for tag in requested {
        match stored.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => stored.push(tag.clone()),
        }
    }
    true
}

/// Whether every tag of `tags` meets the criteria Route 53 Resolver accepts a
/// tag by: neither its `Key` nor its `Value` is an empty string.
///
/// The model's `TagValue` is length 0..256, so an empty value satisfies the
/// modelled constraint and the boundary validator serves it: the rule that a
/// stored tag names a value is the SERVICE's, which is why it is decided here.
/// An empty `Key` is already outside `TagKey`'s length 1..128 and so never
/// reaches a handler, and it is tested here anyway so one function states the
/// whole criterion for both members. The lengths themselves are the modelled
/// constraints and stay the boundary validator's.
///
/// Asked by every operation that WRITES tags — the tag operation and each
/// create that takes a `Tags` list — before it touches the store, so a refused
/// write persists nothing.
pub fn tags_are_acceptable(tags: &[Tag]) -> bool {
    tags.iter()
        .all(|tag| !tag.key.is_empty() && !tag.value.is_empty())
}

/// The message Route 53 Resolver answers for a tag that does not meet its
/// acceptance criteria.
///
/// The whole sentence pair is FIXED text for this fault, from the recorded
/// responses of the tag write: it states that one of the tag parameters is not
/// acceptable and what the caller has to do, and it interpolates nothing from
/// the request — so the rejected key and value are not echoed back.
pub(super) fn unacceptable_tag_message() -> String {
    rslvr_message_sentences(
        RSLVR_CODE_TAG_NOT_ACCEPTABLE,
        &[
            "One of the input parameters of the tags does not meet the acceptance criteria specified in the restrictions",
            "Please fix the invalid parameter before retrying",
        ],
    )
}

/// The `InvalidParameterException` a tag write answers for a `Tags` list
/// carrying a tag outside [`tags_are_acceptable`], or `None` when every tag is
/// acceptable.
///
/// The parameter at fault is the tag list the sentence already names, so no
/// `FieldName` is added beside it, exactly as for [`vpc_not_found`].
pub fn unacceptable_tag(tags: &[Tag]) -> Option<InvalidParameterException> {
    (!tags_are_acceptable(tags)).then(|| InvalidParameterException {
        message: unacceptable_tag_message(),
        field_name: None,
    })
}

/// The same refusal as [`unacceptable_tag`] in the `ValidationException` shape,
/// for an operation whose model gives it that error and no
/// `InvalidParameterException` to report a parameter it will not accept —
/// exactly as [`vpc_not_found_validation`] is the other shape of
/// [`vpc_not_found`].
pub fn unacceptable_tag_validation(tags: &[Tag]) -> Option<ValidationException> {
    (!tags_are_acceptable(tags)).then(|| ValidationException {
        message: Some(unacceptable_tag_message()),
    })
}

/// Drop every tag of `stored` whose key is named in `keys`, and leave the rest
/// in the order they were written in.
///
/// Idempotent: a key the resource does not carry removes nothing, so removing
/// the same keys twice leaves the same tag set.
pub fn remove_tags(stored: &mut Vec<Tag>, keys: &[String]) {
    stored.retain(|tag| !keys.contains(&tag.key));
}

/// The cursor scope of one `ListTagsForResource` query.
///
/// The offset a cursor carries is an index into ONE resource's tags, so — for
/// the same reason as [`endpoint_list_scope`] — a cursor minted while paging one
/// resource is not read as an offset into another resource's tags, but refused
/// as a cursor this query did not issue.
pub fn tag_list_scope(resource_arn: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListTagsForResource/resource-arn/v1");
    feed_field(&mut hasher, resource_arn.as_bytes());
    format!("ListTagsForResource/{}", hex::encode(&hasher.finalize()[..8]))
}
