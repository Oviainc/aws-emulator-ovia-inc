// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-trustedadvisor-trustedadvisor service.
//!
//! Trusted Advisor is a REPORTING API: it has no create operation, so nothing a
//! caller does brings a check, a recommendation or a recommendation resource
//! into existence. The store is therefore SEEDED deterministically the first
//! time the engine hands this state out (`state::<T>()` builds it from
//! `Default`), and every response is computed from it — never from the request.
//!
//! Three maps, each keyed by the identifier its API looks a resource up by, so
//! `BTreeMap` iteration order is also the stable pagination order:
//!
//! * [`checks`](ComAmazonawsTrustedadvisorTrustedadvisorState::checks) — keyed by check id.
//! * [`recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendations) —
//!   keyed by recommendation ARN, which is what `GetRecommendation`,
//!   `ListRecommendationResources` and `UpdateRecommendationLifecycle` take as
//!   their `@httpLabel`.
//! * [`recommendation_resources`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendation_resources) —
//!   keyed by recommendation-resource ARN, which is what
//!   `BatchUpdateRecommendationResourceExclusion` names its entries by.
//!
//! A recommendation does NOT keep a list of its resource ARNs: a resource
//! carries its owning `recommendation_arn`, so
//! [`resources_of`](ComAmazonawsTrustedadvisorTrustedadvisorState::resources_of)
//! derives the set from the one copy rather than keeping a second one that
//! could disagree. For the same reason `resourcesAggregates` is never stored:
//! [`aggregates_for`](ComAmazonawsTrustedadvisorTrustedadvisorState::aggregates_for)
//! counts the recommendation's own resources on every read.

use std::collections::BTreeMap;

use crate::ops::types::RecommendationResourcesAggregates;

/// The `ExclusionStatus` value a resource carries unless a caller excluded it.
/// `RecommendationResourceSummary$exclusionStatus` declares
/// `@default("included")`.
pub const EXCLUSION_INCLUDED: &str = "included";

/// The other `ExclusionStatus` value (see `EXCLUSION_STATUS_VALUES` in
/// `ops::types::shared_types`).
pub const EXCLUSION_EXCLUDED: &str = "excluded";

/// The `RecommendationType` whose lifecycle `UpdateRecommendationLifecycle` can
/// move: its model doc says "This API only supports prioritized
/// recommendations".
pub const RECOMMENDATION_TYPE_PRIORITY: &str = "priority";

/// The three values `ResourceStatus` models, each with the
/// `RecommendationResourcesAggregates` counter it feeds (see
/// `RESOURCE_STATUS_VALUES` in `ops::types::shared_types`).
pub const RESOURCE_STATUS_OK: &str = "ok";
pub const RESOURCE_STATUS_WARNING: &str = "warning";
pub const RESOURCE_STATUS_ERROR: &str = "error";

/// A check ARN: `arn:aws:trustedadvisor:::check/<id>`.
///
/// Formatted here rather than through `engine::emulator_arn`, which always
/// writes the account segment: `CheckSummary$arn` is modelled
/// `^arn:[\w-]+:trustedadvisor:::check\/[\w-]+$`, i.e. with an EMPTY account.
pub fn check_arn(check_id: &str) -> String {
    format!("arn:aws:trustedadvisor:::check/{check_id}")
}

/// A recommendation ARN: `arn:aws:trustedadvisor::<account>:recommendation/<id>`,
/// matching `AccountRecommendationArn`'s
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`. The account
/// segment is the emulator's canonical account id.
pub fn recommendation_arn(recommendation_id: &str) -> String {
    ::engine::emulator_arn(
        "trustedadvisor",
        "",
        &format!("recommendation/{recommendation_id}"),
    )
}

/// A recommendation-resource ARN:
/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec id>/<resource id>`,
/// matching `RecommendationResourceArn`'s
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`.
pub fn recommendation_resource_arn(recommendation_id: &str, resource_id: &str) -> String {
    ::engine::emulator_arn(
        "trustedadvisor",
        "",
        &format!("recommendation-resource/{recommendation_id}/{resource_id}"),
    )
}

/// The key prefix every resource of `recommendation_arn` shares: that ARN with
/// its `recommendation/` resource part rewritten to `recommendation-resource/`,
/// plus the `/` that separates the resource id. That trailing `/` is what keeps
/// the prefix of `.../foo/` from also matching `.../foo-bar/...`.
///
/// `None` for an ARN that is not a recommendation ARN, which therefore names no
/// stored resource.
fn resource_key_prefix(recommendation_arn: &str) -> Option<String> {
    let (scope, recommendation_id) = recommendation_arn.split_once(":recommendation/")?;
    Some(format!("{scope}:recommendation-resource/{recommendation_id}/"))
}

/// Wall-clock now as milliseconds since the Unix epoch — what an operation
/// stamps on a value it updates. Taken from the clock, never from the request.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Milliseconds since the Unix epoch as the modelled `date-time` timestamp.
///
/// `from_millis` is the exact conversion: going through seconds as an `f64`
/// would divide by 1000 in binary floating point, which is not representable
/// for most millisecond values and can shift the rendered sub-second part.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A Trusted Advisor check: the catalog entry `ListChecks` reports.
///
/// Enum-typed members (`pillars`, `source`) are held as the wire strings the
/// model's enums accept — `RECOMMENDATION_PILLAR_VALUES` /
/// `RECOMMENDATION_SOURCE_VALUES` in `ops::types::shared_types` — which is also
/// how the generated shapes carry them.
#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub pillars: Vec<String>,
    pub aws_services: Vec<String>,
    pub source: String,
    /// Column headings for the metadata a resource of this check carries,
    /// keyed by the column index as a string (`{"0": "Region", ...}`).
    pub metadata: BTreeMap<String, String>,
    pub resource_arn_queryable: bool,
    pub aws_resource_types: Vec<String>,
    pub check_granularity: Option<String>,
    pub recommendation_id: Option<String>,
}

/// A recommendation: one per seeded check, the unit `ListRecommendations`,
/// `GetRecommendation` and `UpdateRecommendationLifecycle` work on.
#[derive(Debug, Clone)]
pub struct RecommendationEntry {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub check_arn: Option<String>,
    pub r#type: String,
    pub status: String,
    pub lifecycle_stage: Option<String>,
    pub pillars: Vec<String>,
    pub source: String,
    pub aws_services: Vec<String>,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub resolved_at_millis: Option<i64>,
    pub update_reason: Option<String>,
    pub update_reason_code: Option<String>,
}

/// One AWS resource a recommendation flagged.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub id: String,
    pub arn: String,
    pub recommendation_arn: String,
    pub aws_resource_id: String,
    pub region_code: String,
    pub status: String,
    /// The values for this resource, positionally aligned with the owning
    /// check's `metadata` column headings.
    pub metadata: BTreeMap<String, String>,
    pub last_updated_at_millis: i64,
    pub exclusion_status: String,
}

/// Per-service state for com-amazonaws-trustedadvisor-trustedadvisor. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// Keyed by check id.
    pub checks: BTreeMap<String, Check>,
    /// Keyed by recommendation ARN.
    pub recommendations: BTreeMap<String, RecommendationEntry>,
    /// Keyed by recommendation-resource ARN.
    pub recommendation_resources: BTreeMap<String, ResourceEntry>,
}

impl ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// The resources of one recommendation, in ARN order.
    ///
    /// A RANGE over the resource map, not a scan of it: every resource of a
    /// recommendation shares the key prefix
    /// [`resource_key_prefix`] derives from that recommendation's ARN, so this
    /// costs O(log n + k) even though `ListRecommendations` calls
    /// [`Self::aggregates_for`] once per row. The stored
    /// `recommendation_arn` is still compared, so the range can never widen the
    /// result.
    pub fn resources_of<'a>(
        &'a self,
        recommendation_arn: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ResourceEntry)> + 'a {
        // A recommendation ARN outside the modelled form has no prefix — and no
        // resource either, since every stored resource ARN is built from one.
        let prefix = resource_key_prefix(recommendation_arn);
        let start = prefix.clone().unwrap_or_default();
        self.recommendation_resources
            .range(start..)
            .take_while(move |(key, _)| {
                prefix.as_ref().is_some_and(|prefix| key.starts_with(prefix))
            })
            .filter(move |(_, resource)| resource.recommendation_arn == recommendation_arn)
    }

    /// `resourcesAggregates` for a recommendation, COUNTED from its resources
    /// on every read: one counter per `ResourceStatus` value, plus the number
    /// of resources a caller excluded. Never a stored constant, so a status or
    /// exclusion change is visible on the next read without a second write.
    pub fn aggregates_for(&self, recommendation_arn: &str) -> RecommendationResourcesAggregates {
        let mut ok_count = 0i64;
        let mut warning_count = 0i64;
        let mut error_count = 0i64;
        let mut excluded_count = 0i64;
        for (_, resource) in self.resources_of(recommendation_arn) {
            match resource.status.as_str() {
                RESOURCE_STATUS_OK => ok_count += 1,
                RESOURCE_STATUS_WARNING => warning_count += 1,
                RESOURCE_STATUS_ERROR => error_count += 1,
                // `ResourceStatus` models exactly those three; a value outside
                // them cannot be stored, so there is nothing to count here.
                _ => {}
            }
            if resource.exclusion_status == EXCLUSION_EXCLUDED {
                excluded_count += 1;
            }
        }
        RecommendationResourcesAggregates {
            ok_count,
            warning_count,
            error_count,
            excluded_count: Some(excluded_count),
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination: an opaque cursor over a map's sorted key order
// ---------------------------------------------------------------------------

/// The page size used when a request names no `maxResults`.
///
/// AWS documents no default for these operations, so this is a choice: 100,
/// which is inside every page-size range the service models (`maxResults` is
/// `1..200` on `ListChecks`/`ListRecommendations` and `1..5000` on
/// `ListRecommendationResources`).
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// One page of a paginated read.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `Some` only when entries remain after this page.
    pub next_token: Option<String>,
}

/// A `nextToken` this service did not issue, or issued for a different list.
#[derive(Debug)]
pub struct MalformedToken;

/// The separator between a cursor's scope and its key. ASCII unit separator: no
/// id, ARN or scope this service builds can contain it, so the split is
/// unambiguous.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The separator between the parts of a scope. ASCII record separator, for the
/// same reason.
const SCOPE_SEPARATOR: char = '\u{1e}';

/// The cursor scope for one read of a filtered list: the list's name (and, for a
/// sub-resource list, the parent it was taken under) plus every filter in force,
/// in the order the handler lists them.
///
/// The filters belong in the scope because they decide the SEQUENCE the cursor
/// points into: resuming a token issued under one filter set against another
/// would skip to a key that sequence orders differently, so a caller following
/// tokens would not see every entry exactly once. Such a token is refused as
/// malformed instead. `maxResults` is deliberately NOT part of the scope: the
/// page size may change between pages without changing the sequence.
pub fn list_scope(list: &str, filters: &[(&str, Option<&str>)]) -> String {
    let mut scope = list.to_string();
    for (name, value) in filters {
        if let Some(value) = value {
            scope.push(SCOPE_SEPARATOR);
            scope.push_str(name);
            scope.push('=');
            scope.push_str(value);
        }
    }
    scope
}

/// The cursor for a page of `scope` that ended on `last_key`: base64 of the
/// scope and that key, so the token is opaque to a caller and carries no state
/// of its own.
///
/// `scope` names the read the cursor belongs to (see [`list_scope`]), so a token
/// cannot be replayed on a different list or filter set, where the same key
/// would mean something else.
pub fn encode_token(scope: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{scope}{TOKEN_SEPARATOR}{last_key}"))
}

/// The key a cursor points at, or [`MalformedToken`] when the token is not one
/// this service issued for `scope`.
pub fn decode_token(scope: &str, token: &str) -> Result<String, MalformedToken> {
    let bytes = ::aws_smithy_types::base64::decode(token).map_err(|_| MalformedToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| MalformedToken)?;
    let (token_scope, key) = decoded.split_once(TOKEN_SEPARATOR).ok_or(MalformedToken)?;
    if token_scope != scope {
        return Err(MalformedToken);
    }
    Ok(key.to_string())
}

/// Cut one page out of `entries`, which must be in key order (a `BTreeMap`
/// walk, filtered).
///
/// `next_token` resumes strictly after the key it names, and the returned token
/// is present only when entries remain — so a caller that follows the token
/// until it is absent sees every entry exactly once. Both the token read and the
/// token written carry `scope`, so a cursor is only accepted by the list that
/// issued it.
pub fn paginate<T>(
    scope: &str,
    entries: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, MalformedToken> {
    let after = match next_token {
        Some(token) => Some(decode_token(scope, token)?),
        None => None,
    };
    // `maxResults` is already inside its modelled range when a handler runs;
    // the floor of 1 only keeps this helper total.
    let limit = max_results
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1);

    let remaining: Vec<(String, T)> = entries
        .into_iter()
        .filter(|(key, _)| after.as_ref().is_none_or(|resume| key > resume))
        .collect();
    let truncated = remaining.len() > limit;
    let page: Vec<(String, T)> = remaining.into_iter().take(limit).collect();
    let next_token = match (truncated, page.last()) {
        (true, Some((last_key, _))) => Some(encode_token(scope, last_key)),
        _ => None,
    };
    Ok(Page {
        items: page.into_iter().map(|(_, value)| value).collect(),
        next_token,
    })
}

// ---------------------------------------------------------------------------
// The seeded catalog
// ---------------------------------------------------------------------------

/// `2024-01-01T00:00:00Z` — when every seeded recommendation was created.
const SEED_CREATED_AT_MILLIS: i64 = 1_704_067_200_000;

/// One day, so each seeded recommendation gets its own `lastUpdatedAt` and the
/// `afterLastUpdatedAt`/`beforeLastUpdatedAt` filters have something to cut on.
const DAY_MILLIS: i64 = 86_400_000;

/// A seeded resource of the row's recommendation.
struct SeedResource {
    id: &'static str,
    aws_resource_id: &'static str,
    region_code: &'static str,
    status: &'static str,
    /// Positionally aligned with the check's `metadata_headings`.
    metadata: &'static [&'static str],
    exclusion_status: &'static str,
}

/// One seeded check, the recommendation it produced, and that recommendation's
/// resources.
struct SeedRow {
    check_id: &'static str,
    name: &'static str,
    description: &'static str,
    pillars: &'static [&'static str],
    aws_services: &'static [&'static str],
    source: &'static str,
    metadata_headings: &'static [&'static str],
    resource_arn_queryable: bool,
    aws_resource_types: &'static [&'static str],
    check_granularity: &'static str,
    recommendation_type: &'static str,
    recommendation_status: &'static str,
    lifecycle_stage: Option<&'static str>,
    resources: &'static [SeedResource],
}

/// The catalog, spanning both `RecommendationType`s, several pillars, two
/// sources and all three `ResourceStatus` values so every modelled filter has
/// matching and non-matching rows. Deterministic: the same bytes on every boot.
///
/// Each `check_id` is kept short enough that its ARN fits the 64-character
/// `CheckIdentifier` bound `ListRecommendations$checkIdentifier` declares —
/// `arn:aws:trustedadvisor:::check/` is already 31 characters, so an id longer
/// than 33 would produce a check nobody could filter on.
const SEED: &[SeedRow] = &[
    SeedRow {
        check_id: "low-utilization-ec2-instances",
        name: "Low Utilization Amazon EC2 Instances",
        description: "Checks the Amazon EC2 instances that were running at any time during the \
                      last 14 days and alerts you if the daily CPU utilization was 10% or less \
                      and network I/O was 5 MB or less on 4 or more days.",
        pillars: &["cost_optimizing"],
        aws_services: &["ec2"],
        source: "ta_check",
        metadata_headings: &[
            "Region",
            "Instance ID",
            "Instance Type",
            "Estimated Monthly Savings",
        ],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::EC2::Instance"],
        check_granularity: "resource",
        recommendation_type: "priority",
        recommendation_status: "warning",
        lifecycle_stage: Some("pending_response"),
        resources: &[
            SeedResource {
                id: "res-ec2-0001",
                aws_resource_id: "i-0abc1de23f4567890",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["us-east-1", "i-0abc1de23f4567890", "m5.xlarge", "72.00"],
                exclusion_status: EXCLUSION_INCLUDED,
            },
            SeedResource {
                id: "res-ec2-0002",
                aws_resource_id: "i-0abc2de23f4567891",
                region_code: "us-west-2",
                status: RESOURCE_STATUS_OK,
                metadata: &["us-west-2", "i-0abc2de23f4567891", "t3.medium", "0.00"],
                exclusion_status: EXCLUSION_INCLUDED,
            },
            SeedResource {
                id: "res-ec2-0003",
                aws_resource_id: "i-0abc3de23f4567892",
                region_code: "eu-west-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["eu-west-1", "i-0abc3de23f4567892", "c5.2xlarge", "148.00"],
                exclusion_status: EXCLUSION_INCLUDED,
            },
        ],
    },
    SeedRow {
        check_id: "amazon-ebs-snapshots",
        name: "Amazon EBS Snapshots",
        description: "Checks the age of the snapshots for your Amazon Elastic Block Store \
                      (Amazon EBS) volumes (available or in-use).",
        pillars: &["fault_tolerance"],
        aws_services: &["ebs"],
        source: "ta_check",
        metadata_headings: &["Region", "Volume ID", "Snapshot ID", "Snapshot Age"],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::EC2::Volume"],
        check_granularity: "resource",
        recommendation_type: "standard",
        recommendation_status: "error",
        lifecycle_stage: None,
        resources: &[
            SeedResource {
                id: "res-ebs-0001",
                aws_resource_id: "vol-0ea1b2c3d4e5f6789",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_ERROR,
                metadata: &[
                    "us-east-1",
                    "vol-0ea1b2c3d4e5f6789",
                    "snap-0ea1b2c3d4e5f6789",
                    "412",
                ],
                exclusion_status: EXCLUSION_INCLUDED,
            },
            SeedResource {
                id: "res-ebs-0002",
                aws_resource_id: "vol-0ea2b2c3d4e5f6789",
                region_code: "ap-southeast-1",
                status: RESOURCE_STATUS_OK,
                metadata: &[
                    "ap-southeast-1",
                    "vol-0ea2b2c3d4e5f6789",
                    "snap-0ea2b2c3d4e5f6789",
                    "11",
                ],
                // Seeded excluded, so the `exclusionStatus` filter and
                // `excludedCount` have a row from the first read.
                exclusion_status: EXCLUSION_EXCLUDED,
            },
        ],
    },
    SeedRow {
        check_id: "iam-access-key-rotation",
        name: "IAM Access Key Rotation",
        description: "Checks for active IAM access keys that have not been rotated in the last \
                      90 days.",
        pillars: &["security"],
        aws_services: &["iam"],
        source: "ta_check",
        metadata_headings: &["Region", "IAM User", "Access Key ID", "Key Age In Days"],
        resource_arn_queryable: false,
        aws_resource_types: &[],
        check_granularity: "account",
        recommendation_type: "standard",
        recommendation_status: "ok",
        lifecycle_stage: None,
        resources: &[SeedResource {
            id: "res-iam-0001",
            aws_resource_id: "AKIAIOSFODNN7EXAMPLE",
            region_code: "us-east-1",
            status: RESOURCE_STATUS_OK,
            metadata: &["us-east-1", "build-agent", "AKIAIOSFODNN7EXAMPLE", "14"],
            exclusion_status: EXCLUSION_INCLUDED,
        }],
    },
    SeedRow {
        check_id: "rds-idle-db-instances",
        name: "Amazon RDS Idle DB Instances",
        description: "Checks the configuration of your Amazon Relational Database Service \
                      (Amazon RDS) for any database (DB) instances that appear to be idle.",
        pillars: &["cost_optimizing", "performance"],
        aws_services: &["rds"],
        source: "rds",
        metadata_headings: &[
            "Region",
            "DB Instance Name",
            "Instance Type",
            "Days Since Last Connection",
            "Estimated Monthly Savings",
        ],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::RDS::DBInstance"],
        check_granularity: "resource",
        recommendation_type: "priority",
        recommendation_status: "warning",
        lifecycle_stage: Some("in_progress"),
        resources: &[
            SeedResource {
                id: "res-rds-0001",
                aws_resource_id: "reporting-prod",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["us-east-1", "reporting-prod", "db.m5.large", "21", "131.40"],
                exclusion_status: EXCLUSION_INCLUDED,
            },
            SeedResource {
                id: "res-rds-0002",
                aws_resource_id: "analytics-staging",
                region_code: "us-west-2",
                status: RESOURCE_STATUS_OK,
                metadata: &["us-west-2", "analytics-staging", "db.t3.medium", "1", "0.00"],
                exclusion_status: EXCLUSION_INCLUDED,
            },
        ],
    },
];

/// `{"0": first, "1": second, ...}` — the positional string map both
/// `CheckSummary$metadata` (column headings) and
/// `RecommendationResourceSummary$metadata` (that resource's values) use.
fn positional_map(values: &[&str]) -> BTreeMap<String, String> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (index.to_string(), (*value).to_string()))
        .collect()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

impl Default for ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// The seeded store. `engine.state::<T>()` builds this once, on the first
    /// request that reaches the service, and every later request sees the same
    /// value — including any lifecycle or exclusion change a caller made.
    fn default() -> Self {
        let mut state = Self {
            checks: BTreeMap::new(),
            recommendations: BTreeMap::new(),
            recommendation_resources: BTreeMap::new(),
        };
        for (index, row) in SEED.iter().enumerate() {
            let recommendation_id = format!("{}-recommendation", row.check_id);
            let check_arn = check_arn(row.check_id);
            let recommendation_arn = recommendation_arn(&recommendation_id);
            // One day apart per row, so each recommendation has its own
            // `lastUpdatedAt`.
            let last_updated_at_millis =
                SEED_CREATED_AT_MILLIS + DAY_MILLIS * (index as i64 + 1);

            state.checks.insert(
                row.check_id.to_string(),
                Check {
                    id: row.check_id.to_string(),
                    arn: check_arn.clone(),
                    name: row.name.to_string(),
                    description: row.description.to_string(),
                    pillars: owned(row.pillars),
                    aws_services: owned(row.aws_services),
                    source: row.source.to_string(),
                    metadata: positional_map(row.metadata_headings),
                    resource_arn_queryable: row.resource_arn_queryable,
                    aws_resource_types: owned(row.aws_resource_types),
                    check_granularity: Some(row.check_granularity.to_string()),
                    recommendation_id: Some(recommendation_id.clone()),
                },
            );

            state.recommendations.insert(
                recommendation_arn.clone(),
                RecommendationEntry {
                    id: recommendation_id.clone(),
                    arn: recommendation_arn.clone(),
                    name: row.name.to_string(),
                    description: row.description.to_string(),
                    check_arn: Some(check_arn),
                    r#type: row.recommendation_type.to_string(),
                    status: row.recommendation_status.to_string(),
                    lifecycle_stage: row.lifecycle_stage.map(str::to_string),
                    pillars: owned(row.pillars),
                    source: row.source.to_string(),
                    aws_services: owned(row.aws_services),
                    created_at_millis: SEED_CREATED_AT_MILLIS,
                    last_updated_at_millis,
                    resolved_at_millis: None,
                    update_reason: None,
                    update_reason_code: None,
                },
            );

            for resource in row.resources {
                let arn = recommendation_resource_arn(&recommendation_id, resource.id);
                state.recommendation_resources.insert(
                    arn.clone(),
                    ResourceEntry {
                        id: resource.id.to_string(),
                        arn,
                        recommendation_arn: recommendation_arn.clone(),
                        aws_resource_id: resource.aws_resource_id.to_string(),
                        region_code: resource.region_code.to_string(),
                        status: resource.status.to_string(),
                        metadata: positional_map(resource.metadata),
                        last_updated_at_millis,
                        exclusion_status: resource.exclusion_status.to_string(),
                    },
                );
            }
        }
        state
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTrustedadvisorTrustedadvisorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTrustedadvisorTrustedadvisorState>)
}
