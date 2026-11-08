// SPDX-License-Identifier: CC0-1.0

//! One gateway's rate limits: the record a rate limit is stored as, the
//! clock-owed status it reports, the response envelopes every operation that
//! reports one renders, and the one check a submitted set has to pass.
//!
//! One concern, kept beside the gateway record that holds the limits rather than
//! in any one `ops/` file: a rate limit is addressed only THROUGH its gateway
//! (`/gateways/{gatewayIdentifier}/rate-limits/{rateLimitId}`), so the batch
//! write and every later read answer from this one shape and cannot drift.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BatchPutLimitEntry, GatewayRateLimitDetail, GetGatewayRateLimitOutput, LimitEntry,
    ValidationException,
};

use super::{field_validation_failed, timestamp};

/// `GatewayRateLimitStatus` members this service drives. The enum's full value
/// list is `CREATING | ACTIVE | UPDATING | DELETING`
/// (`smithy_query shape=GatewayRateLimitStatus`).
pub const GATEWAY_RATE_LIMIT_STATUS_CREATING: &str = "CREATING";
pub const GATEWAY_RATE_LIMIT_STATUS_UPDATING: &str = "UPDATING";
pub const GATEWAY_RATE_LIMIT_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a freshly written rate limit stays in its transient status
/// (`CREATING` for one this write added, `UPDATING` for one it replaced) before
/// the clock owes it `ACTIVE`.
///
/// A rate limit is a child of the gateway that enforces it, so it settles on the
/// same span the gateway family uses ([`super::GATEWAY_SETTLE_DELAY`]): one
/// chosen span for the whole family rather than a second one only this resource
/// would wait out.
pub const GATEWAY_RATE_LIMIT_SETTLE_DELAY: Duration = super::GATEWAY_SETTLE_DELAY;

/// The resource noun [`super::resource_not_found`] names for one rate limit of a
/// gateway.
pub const RESOURCE_GATEWAY_RATE_LIMIT: &str = "rate limit";

/// How many rate limits one gateway will hold.
///
/// `BatchPutLimitEntries` is `@length 1..50` and the batch write REPLACES the
/// gateway's whole set, so a submitted set cannot exceed this on its own; the
/// ceiling is named here so every writer of the set is bounded by one number
/// rather than by the shape of its own request.
pub const MAX_RATE_LIMITS_PER_GATEWAY: usize = 50;

/// The request member a refused batch names.
const RATE_LIMITS_MEMBER: &str = "rateLimits";

/// One rate limit configured on a gateway: what the caller submitted for it, and
/// the transition to `ACTIVE` the clock owes it.
///
/// Every operation that reports a rate limit renders it from this record, so a
/// write and a later read cannot disagree about its entries or its status.
#[derive(Debug, Clone)]
pub struct GatewayRateLimitRecord {
    pub rate_limit_id: String,
    pub description: Option<String>,
    pub dimension_keys: Vec<String>,
    pub entries: Vec<LimitEntry>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
}

impl GatewayRateLimitRecord {
    /// The record a write stores for one submitted entry, under the id the write
    /// resolved for it.
    ///
    /// `replacing` is the rate limit that id already named, if any: an upsert
    /// keeps the instant that limit was first created and reports `UPDATING`,
    /// while an id nothing held is created at `now` and reports `CREATING`.
    pub fn stored(
        rate_limit_id: String,
        submitted: BatchPutLimitEntry,
        replacing: Option<&Self>,
        now: SystemTime,
    ) -> Self {
        Self {
            rate_limit_id,
            description: submitted.description,
            dimension_keys: submitted.dimension_keys,
            entries: submitted.entries,
            created_at: replacing.map_or(now, |previous| previous.created_at),
            updated_at: now,
            transient_status: if replacing.is_some() {
                GATEWAY_RATE_LIMIT_STATUS_UPDATING.to_string()
            } else {
                GATEWAY_RATE_LIMIT_STATUS_CREATING.to_string()
            },
            settled_status: GATEWAY_RATE_LIMIT_STATUS_ACTIVE.to_string(),
            settles_at: now + GATEWAY_RATE_LIMIT_SETTLE_DELAY,
        }
    }

    /// The status this rate limit reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// This rate limit as the `GatewayRateLimitDetail` every list- and
    /// batch-shaped response reports, under the gateway identifier the request
    /// addressed it through.
    pub fn detail(&self, gateway_identifier: &str, now: SystemTime) -> GatewayRateLimitDetail {
        GatewayRateLimitDetail {
            rate_limit_id: self.rate_limit_id.clone(),
            gateway_identifier: gateway_identifier.to_string(),
            description: self.description.clone(),
            dimension_keys: self.dimension_keys.clone(),
            entries: self.entries.clone(),
            status: self.status_at(now).to_string(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
        }
    }

    /// This rate limit as `GetGatewayRateLimit`'s response envelope — the same
    /// members [`Self::detail`] renders, read off the same record, so a read
    /// cannot differ from the write that stored it.
    pub fn get_output(
        &self,
        gateway_identifier: &str,
        now: SystemTime,
    ) -> GetGatewayRateLimitOutput {
        let detail = self.detail(gateway_identifier, now);
        GetGatewayRateLimitOutput {
            rate_limit_id: detail.rate_limit_id,
            gateway_identifier: detail.gateway_identifier,
            description: detail.description,
            dimension_keys: detail.dimension_keys,
            entries: detail.entries,
            status: detail.status,
            created_at: detail.created_at,
            updated_at: detail.updated_at,
        }
    }
}

/// Refuse a submitted set that cannot BE one gateway's rate-limit set, with the
/// modelled `ValidationException` naming the member and why.
///
/// Three rules the model states in prose, so the boundary constraint walk cannot
/// refuse them: an id is what the set is keyed by, so one id cannot appear twice
/// in the same submission; "no two rate limits can share the same dimension
/// keys" (`smithy_query doc shape=CreateGatewayRateLimitRequest
/// member=dimensionKeys`); and a rule entry's dimension names are the parent
/// limit's — "Keys must match the parent rate limit's dimension keys"
/// (`smithy_query doc shape=LimitEntry member=dimensions`).
pub fn refusal_for_batch(submitted: &[BatchPutLimitEntry]) -> Option<ValidationException> {
    let mut ids = BTreeSet::new();
    let mut dimension_key_sets = BTreeSet::new();
    for entry in submitted {
        if let Some(rate_limit_id) = &entry.rate_limit_id {
            if !ids.insert(rate_limit_id.clone()) {
                return Some(field_validation_failed(
                    RATE_LIMITS_MEMBER,
                    &format!(
                        "carries the rate limit {rate_limit_id} more than once; a rateLimitId \
                         identifies one entry of the set"
                    ),
                ));
            }
        }
        let keys: Vec<String> = entry.dimension_keys.clone();
        // The dimension keys are compared as a SET: the list's order decides how
        // a wildcard may be used inside one rate limit, but two rate limits
        // scoped by the same keys in a different order still share those keys,
        // which is what the rule forbids.
        let declared: BTreeSet<&str> = entry.dimension_keys.iter().map(String::as_str).collect();
        if !dimension_key_sets.insert(declared.clone()) {
            return Some(field_validation_failed(
                RATE_LIMITS_MEMBER,
                &format!(
                    "carries two rate limits with the dimension keys {}; no two rate limits of a \
                     gateway can share the same dimension keys",
                    keys.join(", ")
                ),
            ));
        }
        for rule in &entry.entries {
            if let Some(unknown) = rule
                .dimensions
                .keys()
                .find(|dimension| !declared.contains(dimension.as_str()))
            {
                return Some(field_validation_failed(
                    RATE_LIMITS_MEMBER,
                    &format!(
                        "names the dimension {unknown} in a rule entry of the rate limit with the \
                         dimension keys {}; a rule entry's dimensions are the rate limit's own \
                         dimension keys",
                        keys.join(", ")
                    ),
                ));
            }
        }
    }
    None
}
