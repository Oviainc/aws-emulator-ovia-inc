// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-inspector2-inspector2 service.
//!
//! Three resources live here, and every implemented operation computes its answer from
//! them:
//!
//! * `accounts` — per-account Amazon Inspector enablement, written by `Enable` and read
//!   back by `BatchGetAccountStatus`.
//! * `filters` — the suppression/other findings filters, keyed by their own ARN so a
//!   record and its ARN can never diverge. `BTreeMap` iteration is stable, which is what
//!   makes a `ListFilters` page token meaningful across calls.
//! * `encryption_keys` — the customer managed KMS key in force per resource type and scan
//!   type, written by `UpdateEncryptionKey` and read back by `GetEncryptionKey`.

use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::FilterCriteria;

/// The region a request is scoped to when it carries no SigV4 credential scope to read
/// one from: the region every generated emulator defaults to (see `emulator_arn` in
/// `vendor/server/src/lib.rs`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's SigV4 / ARN service namespace.
pub const SERVICE_NAMESPACE: &str = "inspector2";

/// Every `ResourceScanType` token, in model order.
///
/// Source: `smithy_query(query="shape", shape="ResourceScanType")` —
/// `EC2 | ECR | LAMBDA | LAMBDA_CODE | CODE_REPOSITORY`.
pub const RESOURCE_SCAN_TYPES: [&str; 5] = [
    "EC2",
    "ECR",
    "LAMBDA",
    "LAMBDA_CODE",
    "CODE_REPOSITORY",
];

/// The `Status` token for a scan that is on.
/// Source: `smithy_query(query="shape", shape="Status")`.
pub const STATUS_ENABLED: &str = "ENABLED";

/// The `Status` token for a scan that is off — the state of an account Amazon Inspector
/// has never been enabled for.
/// Source: `smithy_query(query="shape", shape="Status")`.
pub const STATUS_DISABLED: &str = "DISABLED";

/// The most filters this emulator stores before refusing to mint another, so a caller
/// looping `CreateFilter` cannot grow the process heap without bound. `CreateFilter`
/// models `ServiceQuotaExceededException` precisely because the real service caps filters
/// per account too; this is the emulator's own bound, of the same order.
pub const MAX_STORED_FILTERS: usize = 1_000;

/// The most accounts this emulator tracks enablement for. `Enable` refuses to add one
/// past this, so a caller looping `Enable` with fresh account ids cannot grow the account
/// map without bound.
pub const MAX_ENABLED_ACCOUNTS: usize = 1_000;

/// The most `Enable` idempotency tokens remembered per account. Tokens are caller-chosen
/// strings, so the record keeps only the most recent ones (oldest dropped first): a
/// retry, which is what the token exists for, follows its original call closely, and
/// nothing can accumulate tokens without bound.
pub const MAX_CLIENT_TOKENS_PER_ACCOUNT: usize = 64;

/// One stored findings filter: exactly what `CreateFilter`/`UpdateFilter` were given,
/// plus the ARN the service minted and the two timestamps it maintains.
#[derive(Debug, Clone)]
pub struct StoredFilter {
    /// The minted ARN; also this record's key in [`ComAmazonawsInspector2Inspector2State::filters`].
    pub arn: String,
    /// The 12-digit account that created the filter (`Filter$ownerId`).
    pub owner_id: String,
    pub name: String,
    pub criteria: FilterCriteria,
    /// A `FilterAction` token: `NONE` or `SUPPRESS`.
    pub action: String,
    pub description: Option<String>,
    pub reason: Option<String>,
    /// The tags as sent: `None` when the request carried no `tags` member, and an empty
    /// map when it carried an empty one, so a read renders back exactly what was stored.
    /// Sorted so the rendering order is stable.
    pub tags: Option<BTreeMap<String, String>>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// Amazon Inspector enablement for one account.
#[derive(Debug, Clone, Default)]
pub struct AccountEnablement {
    /// The account-level `Status` token, or `None` for an account Amazon Inspector has
    /// never been enabled for: `Status` has no token for "no record", so the absence is
    /// kept out of the token space instead of being spelled with an illegal value.
    pub status: Option<String>,
    /// The per-`ResourceScanType` `Status` map both `Enable`'s `Account$resourceStatus`
    /// and `BatchGetAccountStatus`'s `AccountState$resourceState` are rendered from.
    pub resource_status: BTreeMap<String, String>,
    /// When the account was first enabled.
    pub enabled_at: Option<SystemTime>,
    /// `Enable` idempotency tokens already honoured for this account, so a retry of the
    /// same request is a replay rather than a second write. A BOUNDED queue, oldest
    /// first: at most [`MAX_CLIENT_TOKENS_PER_ACCOUNT`] are kept, so caller-chosen
    /// strings cannot accumulate here. Add through [`AccountEnablement::remember_token`].
    pub client_tokens: VecDeque<String>,
}

impl AccountEnablement {
    /// The account-level `Status`. An account with no enablement record reports
    /// [`STATUS_DISABLED`], which is what the real service reports for an account Amazon
    /// Inspector is not enabled for.
    pub fn account_status(&self) -> &str {
        self.status.as_deref().unwrap_or(STATUS_DISABLED)
    }

    /// The `Status` of one resource scan type. An account that was never enabled for a
    /// type reports [`STATUS_DISABLED`], which is what the real service reports for a
    /// scan that is off.
    pub fn status_of(&self, resource_type: &str) -> &str {
        self.resource_status
            .get(resource_type)
            .map_or(STATUS_DISABLED, String::as_str)
    }

    /// Whether this account was already enabled under `token`.
    pub fn has_token(&self, token: &str) -> bool {
        self.client_tokens.iter().any(|seen| seen == token)
    }

    /// Remember `token`, dropping the oldest once
    /// [`MAX_CLIENT_TOKENS_PER_ACCOUNT`] are held.
    pub fn remember_token(&mut self, token: String) {
        if self.has_token(&token) {
            return;
        }
        self.client_tokens.push_back(token);
        while self.client_tokens.len() > MAX_CLIENT_TOKENS_PER_ACCOUNT {
            self.client_tokens.pop_front();
        }
    }
}

/// Per-service state for com-amazonaws-inspector2-inspector2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsInspector2Inspector2State {
    /// Stored filters, keyed by ARN. Bounded by [`MAX_STORED_FILTERS`]: `CreateFilter`
    /// refuses once the map holds that many.
    pub filters: BTreeMap<String, StoredFilter>,
    /// Account enablement, keyed by 12-digit account id. Bounded by
    /// [`MAX_ENABLED_ACCOUNTS`]: `Enable` refuses to add an account past that.
    pub accounts: BTreeMap<String, AccountEnablement>,
    /// Monotonic source for the id segment of a minted filter ARN.
    pub next_filter_id: u64,
    /// The customer managed KMS key in force for one `ResourceType` / `ScanType` pair —
    /// the scope Amazon Inspector configures an encryption key at, for the single account
    /// this emulator presents (neither encryption-key operation carries an account
    /// member). An absent entry means that pair is encrypted with an Amazon Web Services
    /// owned key, which is a different answer from a configured key rather than a missing
    /// value, so the absence is kept out of the `KmsKeyArn` value space.
    ///
    /// BOUNDED without a cap of its own: both halves of the key are modelled enum tokens,
    /// and a token outside those enums is refused at the boundary before any handler runs,
    /// so the map holds at most `ResourceType` × `ScanType` entries however many requests
    /// arrive. No request-supplied string ever becomes a key here.
    pub encryption_keys: BTreeMap<EncryptionKeyScope, String>,
}

/// The scope one encryption-key setting belongs to: a `ResourceType` token and a
/// `ScanType` token, in that order, so iteration groups a resource type's settings
/// together.
pub type EncryptionKeyScope = (String, String);

impl ComAmazonawsInspector2Inspector2State {
    /// The customer managed KMS key ARN configured for the `resource_type` /
    /// `scan_type` pair, or `None` when that pair is encrypted with an Amazon Web
    /// Services owned key.
    pub fn encryption_key(&self, resource_type: &str, scan_type: &str) -> Option<&str> {
        self.encryption_keys
            .get(&(resource_type.to_string(), scan_type.to_string()))
            .map(String::as_str)
    }

    /// Put `kms_key_id` in force for the `resource_type` / `scan_type` pair, replacing
    /// whatever key that pair used before. Writes at most one entry per modelled pair, so
    /// repeating the call cannot grow the map.
    pub fn set_encryption_key(
        &mut self,
        resource_type: &str,
        scan_type: &str,
        kms_key_id: String,
    ) {
        self.encryption_keys.insert(
            (resource_type.to_string(), scan_type.to_string()),
            kms_key_id,
        );
    }

    /// Mint the ARN for a new filter:
    /// `arn:aws:inspector2:<region>:<account>:owner/<owner>/filter/<id>`.
    ///
    /// The id segment is a zero-padded 16-hex-digit counter, so the whole ARN is well
    /// inside `FilterArn`'s modelled 1..128 length and no two filters of one emulator
    /// run ever collide.
    pub fn mint_filter_arn(&mut self, region: &str, owner_id: &str) -> String {
        self.next_filter_id += 1;
        let id = format!("{:016x}", self.next_filter_id);
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            region,
            &format!("owner/{owner_id}/filter/{id}"),
        )
    }

    /// The aggregation rows the findings this service has recorded roll up to, for one
    /// `AggregationType` token.
    ///
    /// Amazon Inspector counts an aggregation over the findings its own scans produced:
    /// one row per key of the requested aggregation — an account, a repository, an image
    /// layer — and a key no finding falls under is left OUT of the answer rather than
    /// reported with zero counts. This store records account enablement and suppression
    /// filters, and no finding: Inspector mints a finding from a scan it runs itself and
    /// no operation of this API submits one, so nothing has recorded a finding for any
    /// account the store knows. The roll-up of that finding set is no rows at all — for
    /// every aggregation type, and whatever `accountIds` or aggregation-request filter a
    /// call carries, since an empty set has nothing for a filter to keep or drop. Rows
    /// appear here as soon as there are recorded findings to count.
    pub fn finding_aggregation(
        &self,
        _aggregation_type: &str,
    ) -> Vec<crate::ops::types::AggregationResponse> {
        Vec::new()
    }
}

/// The region this request is signed for, or [`DEFAULT_REGION`] when it is unsigned.
///
/// The SigV4 credential scope (`<akid>/<date>/<region>/<service>/aws4_request`) is the
/// only place an AWS request states its region, so every regional ARN this service mints
/// is built from it. The parsing itself is the vendored
/// `emulator_server::sigv4_credential_scope`, so no second scope parser exists; this
/// wrapper only supplies the default, and it lives per service because `vendor/` is
/// read-only here and a shared crate for four lines would be reached by one caller.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Render a stored instant as the modelled `DateTimeTimestamp`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInspector2Inspector2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInspector2Inspector2State>)
}
