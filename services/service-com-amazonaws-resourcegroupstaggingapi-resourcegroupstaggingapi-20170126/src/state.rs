// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126 service, and the pure
//! projections over it that the tag read/write loop is built from.
//!
//! The service is a single store — ARN -> tag map — that `TagResources` and
//! `UntagResources` write and `GetResources`, `GetTagKeys` and `GetTagValues`
//! read. Everything the readers need (ARN parsing, filter matching, paging) lives
//! here so the five handlers share ONE implementation of each rule rather than
//! five copies.

use std::collections::BTreeMap;

use crate::ops::types::{FailureInfo, ResourceTagMapping, Tag, TagFilter};

/// Per-service state for com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State {
    /// Every resource this account has tagged through the API, keyed by ARN.
    ///
    /// A `BTreeMap` rather than a `HashMap` so ARN order — and therefore every
    /// page boundary and every resumed cursor — is the same on every call.
    ///
    /// Bounded by [`MAX_TAGGED_RESOURCES`]: an entry is deliberately KEPT after
    /// its last tag is removed (the service reports a resource that ever had a
    /// tag with an empty tag set), so nothing else would ever shrink this map.
    pub resources: BTreeMap<String, ResourceEntry>,
}

/// The most tagged resources this emulator holds at once.
///
/// NOT an AWS quota: the real service has no such limit. This is the emulator's
/// bound on in-memory fixture state, so a client looping on `TagResources` with
/// fresh ARNs is refused rather than growing the store without end. Reaching it
/// refuses only the ARNs that would be new, as that ARN's own
/// `FailedResourcesMap` entry; re-tagging a resource already in the store always
/// succeeds.
pub const MAX_TAGGED_RESOURCES: usize = 1_000;

/// The most tags one resource holds at once.
///
/// An AWS limit, not an emulator one: "Each resource can have up to 50 tags"
/// (`TagResources` documentation), which is also the modelled `TagMap` bound on a
/// single request. The modelled bound caps ONE request; this caps what repeated
/// requests can accumulate on one resource.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

impl ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State {
    /// How many tags `arn` would carry after `keys` were merged into it, so a
    /// write can be refused BEFORE it grows the store past
    /// [`MAX_TAGS_PER_RESOURCE`].
    pub fn tag_count_after_merging<'k>(
        &self,
        arn: &str,
        keys: impl IntoIterator<Item = &'k String>,
    ) -> usize {
        let mut merged: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if let Some(entry) = self.resources.get(arn) {
            merged.extend(entry.tags.keys().cloned());
        }
        merged.extend(keys.into_iter().cloned());
        merged.len()
    }

    /// The entry for `arn`, created with an empty tag map when this is the first
    /// tag ever written to it, or `None` when the ARN is new and the store is
    /// already holding [`MAX_TAGGED_RESOURCES`].
    ///
    /// A resource stays in the store once tagged, even after its last tag is
    /// removed: the service reports a resource that "ever had a tag" with an
    /// empty tag set (see the `TagFilters` documentation on `GetResources`).
    pub fn entry_mut(&mut self, arn: &ParsedArn) -> Option<&mut ResourceEntry> {
        if !self.resources.contains_key(&arn.arn) && self.resources.len() >= MAX_TAGGED_RESOURCES {
            return None;
        }
        Some(
            self.resources
                .entry(arn.arn.clone())
                .or_insert_with(|| ResourceEntry {
                    arn: arn.arn.clone(),
                    tags: BTreeMap::new(),
                    resource_type: arn.resource_type(),
                }),
        )
    }
}

/// One tagged resource: its ARN, its current tags, and the
/// `service[:resource-type]` key that `ResourceTypeFilters` matches against.
///
/// `resource_type` is derived from the ARN when the entry is created and never
/// changes afterwards, because the ARN it is derived from never changes.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// The resource's ARN, exactly as the caller wrote it.
    pub arn: String,
    /// The resource's tags, key -> value. Ordered, so `Tags` is stable per read.
    pub tags: BTreeMap<String, String>,
    /// `service` for an ARN with no resource-type segment, else
    /// `service:resource-type`.
    pub resource_type: String,
}

impl ResourceEntry {
    /// This resource's tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// How many tag pairs this resource costs against a `TagsPerPage` budget.
    ///
    /// "A resource with no tags is counted as having one tag (one key and value
    /// pair)" — `GetResources`' `TagsPerPage` documentation.
    pub fn tags_per_page_cost(&self) -> usize {
        self.tags.len().max(1)
    }

    /// [`Self::tags_per_page_cost`] as the cost function [`page`] takes over
    /// BORROWED rows, so paging never has to clone a row to weigh it.
    pub fn tags_per_page_cost_of(entry: &&Self) -> usize {
        entry.tags_per_page_cost()
    }

    /// This resource as a `GetResources` row.
    ///
    /// `ComplianceDetails` is always absent: it reports a resource's compliance
    /// with the effective tag policy, and this emulator models no tag policy at
    /// all, so there is nothing to judge a resource against. `GetResources`
    /// refuses `IncludeComplianceDetails` as not implemented rather than filling
    /// the member with a value nothing computed.
    pub fn mapping(&self) -> ResourceTagMapping {
        ResourceTagMapping {
            resource_arn: Some(self.arn.clone()),
            tags: Some(self.tag_list()),
            compliance_details: None,
        }
    }

    /// Whether this resource satisfies every entry of `filters`.
    ///
    /// Filters are AND-ed; within one filter the `Values` are OR-ed, and a filter
    /// with no values matches the key with any value (`GetResources`' `TagFilters`
    /// documentation). An empty filter list matches every resource.
    pub fn matches_tag_filters(&self, filters: &[TagFilter]) -> bool {
        filters.iter().all(|filter| {
            let Some(key) = filter.key.as_deref() else {
                // A keyless filter is refused before this runs; treat it as
                // matching nothing rather than matching everything.
                return false;
            };
            match self.tags.get(key) {
                None => false,
                Some(value) => match filter.values.as_deref() {
                    None | Some([]) => true,
                    Some(values) => values.iter().any(|wanted| wanted == value),
                },
            }
        })
    }

    /// Whether this resource's type is selected by `filters`.
    ///
    /// Each filter is `service[:resourceType]`: a bare service selects every
    /// resource of that service, and `service:resourceType` only that type. An
    /// empty filter list selects every resource.
    pub fn matches_resource_type_filters(&self, filters: &[String]) -> bool {
        filters.is_empty()
            || filters.iter().any(|filter| {
                filter == &self.resource_type
                    || self
                        .resource_type
                        .split_once(':')
                        .is_some_and(|(service, _)| filter == service)
            })
    }
}

/// The six fields of an ARN: `arn:partition:service:region:account-id:resource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArn {
    /// The ARN as written, so a handler stores exactly what the caller sent.
    pub arn: String,
    /// Field 2 — `aws`, `aws-cn`, `aws-us-gov`, …
    pub partition: String,
    /// Field 3 — the service namespace (`s3`, `sqs`, …).
    pub service: String,
    /// Field 4 — the region, empty for a global resource.
    pub region: String,
    /// Field 5 — the account id, empty where the service omits it.
    pub account: String,
    /// Field 6 and everything after it: `resource-id`, `resource-type/id` or
    /// `resource-type:id`.
    pub resource: String,
}

impl ParsedArn {
    /// The `service[:resource-type]` key `ResourceTypeFilters` matches against.
    ///
    /// "The format of each resource type is `service[:resourceType]` … The string
    /// for each service name and resource type is the same as that embedded in a
    /// resource's ARN" (`GetResources`' `ResourceTypeFilters` documentation). An
    /// ARN whose resource field carries no type segment (`arn:aws:s3:::bucket`)
    /// therefore has only a service key.
    pub fn resource_type(&self) -> String {
        match self.resource.split_once(['/', ':']) {
            Some((kind, _)) if !kind.is_empty() => format!("{}:{}", self.service, kind),
            _ => self.service.clone(),
        }
    }
}

/// Parse an ARN, or `None` when the string is not one.
///
/// An ARN is the literal `arn`, then five `:`-separated fields, of which the
/// partition, the service and the resource must be non-empty; the region and the
/// account are legitimately empty for global and account-less resources
/// (`arn:aws:s3:::bucket-a`). The resource field is everything after the fifth
/// `:`, because it may itself contain `:`.
pub fn parse_arn(arn: &str) -> Option<ParsedArn> {
    let mut fields = arn.splitn(6, ':');
    let scheme = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if scheme != "arn" || partition.is_empty() || service.is_empty() || resource.is_empty() {
        return None;
    }
    Some(ParsedArn {
        arn: arn.to_string(),
        partition: partition.to_string(),
        service: service.to_string(),
        region: region.to_string(),
        account: account.to_string(),
        resource: resource.to_string(),
    })
}

/// The per-resource failure `TagResources`/`UntagResources` report for an ARN
/// they could not parse.
///
/// A resource-level failure is reported in `FailedResourcesMap` with HTTP 200
/// overall, never as a top-level error: the other ARNs in the same request were
/// tagged. `ErrorCode` is one of the values the `ErrorCode` enum lists, and the
/// message names the member and what was wrong with it.
pub fn malformed_arn_failure(arn: &str) -> FailureInfo {
    FailureInfo {
        status_code: 400,
        error_code: Some("InvalidParameterException".to_string()),
        error_message: Some(format!(
            "ResourceARNList contains a value that is not an Amazon Resource Name: {arn}"
        )),
    }
}

/// The per-resource failure `TagResources` reports for a write that would push
/// one resource past [`MAX_TAGS_PER_RESOURCE`].
///
/// A documented service limit, so the message states the limit and the count the
/// request would have reached.
pub fn too_many_tags_failure(arn: &str, would_hold: usize) -> FailureInfo {
    FailureInfo {
        status_code: 400,
        error_code: Some("InvalidParameterException".to_string()),
        error_message: Some(format!(
            "{arn} would hold {would_hold} tags; each resource can have up to \
             {MAX_TAGS_PER_RESOURCE} tags"
        )),
    }
}

/// The per-resource failure `TagResources` reports for a NEW ARN once the store
/// already holds [`MAX_TAGGED_RESOURCES`].
///
/// The bound is the emulator's, not the service's, so the message says so rather
/// than quoting an AWS quota that does not exist.
pub fn store_at_capacity_failure(arn: &str) -> FailureInfo {
    FailureInfo {
        status_code: 400,
        error_code: Some("InvalidParameterException".to_string()),
        error_message: Some(format!(
            "this emulator holds at most {MAX_TAGGED_RESOURCES} tagged resources and is already \
             at that limit, so {arn} was not tagged"
        )),
    }
}

/// The documented minimum of `GetResources`' `ResourcesPerPage`.
pub const MIN_RESOURCES_PER_PAGE: i32 = 1;

/// The documented maximum of `GetResources`' `ResourcesPerPage`: "You can specify
/// a minimum of 1 and a maximum value of 100."
pub const MAX_RESOURCES_PER_PAGE: i32 = 100;

/// The documented minimum of `GetResources`' `TagsPerPage`: "You can set
/// `TagsPerPage` to a minimum of 100 items up to a maximum of 500 items."
pub const MIN_TAGS_PER_PAGE: i32 = 100;

/// The documented maximum of `GetResources`' `TagsPerPage`.
pub const MAX_TAGS_PER_PAGE: i32 = 500;

/// The page size `GetTagKeys` and `GetTagValues` serve.
///
/// Neither operation exposes a page-size member, so the page size is the
/// server's to choose; this emulator serves the same 100 items a page that
/// `GetResources` documents as its `ResourcesPerPage` maximum.
pub const KEYS_PER_PAGE: usize = 100;

/// A stable, short digest of the request members a page is scoped to.
///
/// It goes into the pagination token so a token minted for one projection —
/// another operation, or another filter set — is refused instead of resuming
/// against a list it was never cut from. Lengths are hashed alongside the parts
/// so two different scopes cannot collide by re-splitting the same bytes.
pub fn page_fingerprint(scope: &[&str]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in scope {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..8])
}

/// The opaque token that resumes a page after the item keyed `after`.
///
/// `fingerprint` is hex, so the first `:` always separates the two halves even
/// when `after` is an ARN full of them. The longest token this can build is 16
/// hex digits, a separator and a 1011-character `ResourceARN`, well inside the
/// modelled 2048-character `PaginationToken` bound.
pub fn encode_pagination_token(fingerprint: &str, after: &str) -> String {
    format!("{fingerprint}:{after}")
}

/// The resume point a token carries, or `None` when the token was not minted for
/// this projection (which the caller answers with
/// `PaginationTokenExpiredException`).
pub fn decode_pagination_token(token: &str, fingerprint: &str) -> Option<String> {
    let (minted_for, after) = token.split_once(':')?;
    (minted_for == fingerprint && !after.is_empty()).then(|| after.to_string())
}

/// One page of a key-ordered projection, plus the token that resumes after it.
///
/// `items` must be ascending and unique by key — the key is the cursor, so the
/// resumed call returns exactly the items the page did not reach, with no
/// repeats. `max_items` caps the page by count. `max_cost` caps it by a second,
/// item-weighted budget (`GetResources`' `TagsPerPage`), and is never allowed to
/// empty a page: an item costing more than the whole budget is served alone
/// rather than stalling pagination, because the service "does not split a
/// resource and its associated tags across pages".
pub fn page<T>(
    items: Vec<(String, T)>,
    after: Option<&str>,
    max_items: usize,
    max_cost: Option<usize>,
    cost: impl Fn(&T) -> usize,
    fingerprint: &str,
) -> (Vec<T>, Option<String>) {
    let max_items = max_items.max(1);
    let mut rest: Vec<(String, T)> = items
        .into_iter()
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
        .collect();

    let mut taken = 0usize;
    let mut spent = 0usize;
    for (_, item) in &rest {
        if taken == max_items {
            break;
        }
        let item_cost = cost(item);
        if max_cost.is_some_and(|max| taken > 0 && spent + item_cost > max) {
            break;
        }
        spent += item_cost;
        taken += 1;
    }

    let truncated = rest.len() > taken;
    rest.truncate(taken);
    let next = truncated
        .then(|| {
            rest.last()
                .map(|(key, _)| encode_pagination_token(fingerprint, key))
        })
        .flatten();

    (rest.into_iter().map(|(_, item)| item).collect(), next)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State>)
}
