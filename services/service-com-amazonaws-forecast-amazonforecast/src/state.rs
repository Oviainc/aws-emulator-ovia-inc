// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-forecast-amazonforecast service.
//!
//! The dataset / dataset-group control plane is stored here, plus the handful of
//! helpers every handler that touches it needs: ARN minting, the SigV4-scope region,
//! and the opaque `NextToken` cursor codec the `@paginated` List operations share.
//! Keeping them in one place is what makes two operations read ONE copy of a record.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::SystemTime;

use crate::ops::types::{EncryptionConfig, Schema, Tag};

/// The `Status` a dataset or dataset group carries. Forecast creates both
/// synchronously and `DescribeDataset`/`DescribeDatasetGroup` report them `ACTIVE`
/// from the first read ("The `Status` of a dataset must be `ACTIVE` before you can
/// import training data" — CreateDataset's own documentation), so there is no
/// clock-owed transition to materialise here.
pub const ACTIVE: &str = "ACTIVE";

/// This service's ARN namespace / SigV4 signing name
/// (`smithy_query(query="service_info")`: arnNamespace `forecast`).
pub const ARN_SERVICE: &str = "forecast";

/// The region an ARN is minted in when the request carries no SigV4 credential
/// scope to read one from (an anonymous call routed by endpoint).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Page size for a List call that names no `MaxResults`: the modelled maximum.
/// `MaxResults` is `range 1..100` on both `ListDatasets` and `ListDatasetGroups`
/// (`smithy_query(query="operation",operation="ListDatasets")`), and the model
/// declares no default, so the largest page the member admits is used.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most datasets this emulator retains for one account.
///
/// Every `CreateDataset` is a growth site a caller drives — a fresh `DatasetName`
/// inserts a record holding that caller's `Schema`, `EncryptionConfig` and up to the
/// modelled 200 `Tags` — so the store carries an explicit ceiling: past it the create
/// is REFUSED with the modelled `LimitExceededException` ("The limit on the number of
/// resources per account has been exceeded", HTTP 409) rather than inserting, so
/// emulator memory cannot be grown without bound. The number is CHOSEN for the
/// emulator: Forecast publishes its own per-account quota, and the point of the
/// constant is that the growth site has a ceiling at all, answered with the error AWS
/// answers a quota with.
pub const MAX_DATASETS: usize = 500;

/// The most dataset groups this emulator retains for one account, for the reason
/// [`MAX_DATASETS`] gives and refused the same way.
pub const MAX_DATASET_GROUPS: usize = 200;

/// The most `DatasetArns` one dataset group's membership may name.
///
/// `ArnList` carries no modelled `@length`, so nothing upstream clamps the element
/// count of a caller-supplied membership before the handler walks it under the shared
/// state lock. This ceiling is checked BEFORE that walk, on both
/// `CreateDatasetGroup` and `UpdateDatasetGroup`. Also chosen for the emulator; it is
/// far above any real membership, which names at most one dataset per `DatasetType`.
pub const MAX_DATASET_ARNS_PER_GROUP: usize = 50;

/// Cursor kind tag for `ListDatasets`, so a dataset-group token cannot be replayed
/// against the dataset listing.
pub const DATASET_CURSOR: &str = "dataset";

/// Cursor kind tag for `ListDatasetGroups`.
pub const DATASET_GROUP_CURSOR: &str = "dataset-group";

/// A dataset exactly as `CreateDataset` wrote it: every member
/// `DescribeDataset` / `ListDatasets` reports is read back from here.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub arn: String,
    pub name: String,
    pub domain: String,
    pub dataset_type: String,
    pub data_frequency: Option<String>,
    pub schema: Schema,
    pub encryption_config: Option<EncryptionConfig>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modification_time: SystemTime,
    /// The tags `CreateDataset` carried. Nothing reads them yet: `DescribeDataset` and
    /// `DatasetSummary` model no `Tags` member and `ListTagsForResource` is still a 501
    /// stub with its own gap issue, so they are stored for that operation to report.
    pub tags: Vec<Tag>,
}

/// A dataset group. `dataset_arns` is the membership last written by
/// `CreateDatasetGroup` or `UpdateDatasetGroup`, and is what `DescribeDatasetGroup`
/// reports verbatim — including an ARN whose dataset has since been deleted, because
/// "Forecast does not automatically update any dataset groups that contain the deleted
/// dataset" (`DeleteDataset`'s modelled documentation).
#[derive(Debug, Clone)]
pub struct DatasetGroup {
    pub arn: String,
    pub name: String,
    pub domain: String,
    pub dataset_arns: Vec<String>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modification_time: SystemTime,
    /// As on [`Dataset`]: stored for `ListTagsForResource`, read by nothing yet.
    pub tags: Vec<Tag>,
}

/// Per-service state for com-amazonaws-forecast-amazonforecast. A handler reaches it through [`service_state`].
///
/// Both maps are keyed by ARN and ordered, so the `@paginated` List operations walk a
/// stable sequence and a `NextToken` names a position in it rather than an index that
/// a concurrent create would shift. The ARN carries the resource's region and name, so
/// the key is also the uniqueness constraint a repeated Create trips over: Forecast
/// resources are regional, and the same name signed for another region is another
/// resource with another ARN.
///
/// Both maps are BOUNDED: a create past [`MAX_DATASETS`] / [`MAX_DATASET_GROUPS`] is
/// refused with the modelled `LimitExceededException` instead of inserting, and a
/// group's membership is refused past [`MAX_DATASET_ARNS_PER_GROUP`], so no sequence
/// of requests grows this state without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsForecastAmazonforecastState {
    /// Datasets by `DatasetArn`.
    pub datasets: BTreeMap<String, Dataset>,
    /// Dataset groups by `DatasetGroupArn`.
    pub dataset_groups: BTreeMap<String, DatasetGroup>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsForecastAmazonforecastState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsForecastAmazonforecastState>)
}

/// The ARN of the dataset named `name` in `region`.
pub fn dataset_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("dataset/{name}"))
}

/// The ARN of the dataset group named `name` in `region`.
pub fn dataset_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("dataset-group/{name}"))
}

/// The region this request was signed for, from its SigV4 credential scope —
/// the same signal the router uses to pick the service — falling back to
/// [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The page size a List call serves: its `MaxResults` when it names one (the model
/// already refused anything outside 1..100), else [`DEFAULT_PAGE_SIZE`].
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .filter(|requested| *requested > 0)
        .map_or(DEFAULT_PAGE_SIZE, |requested| requested as usize)
}

/// Encode an opaque `NextToken` naming the last key served on this page.
///
/// Hex rather than the raw key so the token is opaque to a client — it carries a
/// version and the listing it belongs to, which is what lets [`decode_next_token`]
/// reject a token minted for the other listing.
pub fn encode_next_token(cursor: &str, last_key: &str) -> String {
    ::hex::encode(format!("forecast:v1:{cursor}:{last_key}"))
}

/// The key a `NextToken` resumes after, or `None` when the token was not minted by
/// [`encode_next_token`] for this listing — which the caller answers with
/// `InvalidNextTokenException`.
pub fn decode_next_token(cursor: &str, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("forecast:v1:{cursor}:"))
        .map(str::to_string)
}

/// One page of an ARN-ordered map: the entries after `after` (exclusive), at most
/// `limit` of them, plus the key to resume from when more entries remain.
///
/// The cursor is a KEY, not an offset, so a create or delete between two pages
/// neither skips nor repeats an entry the caller has not seen.
pub fn page<V>(
    items: &BTreeMap<String, V>,
    after: Option<String>,
    limit: usize,
) -> (Vec<&V>, Option<String>) {
    let lower = match after {
        Some(key) => Bound::Excluded(key),
        None => Bound::Unbounded,
    };
    let mut remaining = items.range((lower, Bound::Unbounded));
    let mut served = Vec::new();
    let mut last_key = None;
    for (key, value) in remaining.by_ref().take(limit) {
        served.push(value);
        last_key = Some(key.clone());
    }
    // A token is emitted only when there IS a further entry, so the last page omits
    // `NextToken` instead of handing back a cursor onto an empty page.
    let next = remaining.next().and(last_key);
    (served, next)
}
