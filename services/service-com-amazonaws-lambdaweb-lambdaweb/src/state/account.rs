// SPDX-License-Identifier: CC0-1.0

//! The account-settings slice of this service's state: the quotas that apply to
//! web functions in the account, and the usage the stored resources add up to.
//!
//! The usage is DERIVED from the same records every other handler reads
//! (`ComAmazonawsLambdawebLambdawebState::functions`), never counted into a
//! separate field — so a create or a delete moves it with no counter to keep in
//! step. The per-function revision quota it reports is the very constant
//! `CreateWebFunctionRevision` refuses at; the remaining three are allowances the
//! model publishes no value for, documented one by one below.

use crate::ops::types::{AccountQuotas, AccountUsage};

use super::{ComAmazonawsLambdawebLambdawebState, MAX_REVISIONS_PER_FUNCTION};

/// The account-level maximum total Arm vCPUs across every web function.
///
/// CHOSEN, not cited: the model bounds `AccountQuotas$maxTotalArmVCpus` at
/// `@range(min: 0)` and publishes no value for it (source:
/// `smithy_query(query="shape", shape="AccountQuotas")`), so this emulator
/// reports a fixed, non-zero allowance.
pub const MAX_TOTAL_ARM_V_CPUS: i32 = 100;

/// The account-level maximum total requests per second across every endpoint.
///
/// `ThrottleConfig$rateLimit`'s documentation lists `10000` as the largest
/// supported per-endpoint rate and says "The maximum effective value is also
/// bounded by your account-level maximum total rate limit" (source:
/// `smithy_query(query="shape", shape="ThrottleConfig")`), so the account total
/// this emulator reports is that largest supported per-endpoint rate: every rate
/// the model lets an endpoint ask for stays reachable.
pub const MAX_TOTAL_RATE_LIMIT: i32 = 10_000;

/// The most endpoints a single web function may have.
///
/// CHOSEN, not cited: the model bounds
/// `AccountQuotas$maxEndpointsPerFunction` at `@range(min: 0)` and publishes no
/// value for it, so this emulator reports a fixed, non-zero allowance — far
/// above the one endpoint a create can add today.
pub const MAX_ENDPOINTS_PER_FUNCTION: i32 = 10;

/// An `i32` member's value for a stored count, saturating rather than wrapping:
/// every `AccountQuotas` / `AccountUsage` member is an `Integer` with
/// `@range(min: 0)`, so a count that cannot be represented reports the largest
/// value the member can carry instead of a negative one.
fn as_count_member(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// The quotas that apply to web functions in this account and Region.
///
/// `maxRevisionsPerFunction` is the very bound the revision create path refuses
/// at, so a caller that reads that quota and a caller that trips it see the same
/// number; the other three are the allowances this emulator reports for the
/// account, each documented above.
pub fn account_quotas() -> AccountQuotas {
    AccountQuotas {
        max_total_arm_v_cpus: MAX_TOTAL_ARM_V_CPUS,
        max_total_rate_limit: MAX_TOTAL_RATE_LIMIT,
        max_revisions_per_function: as_count_member(MAX_REVISIONS_PER_FUNCTION),
        max_endpoints_per_function: MAX_ENDPOINTS_PER_FUNCTION,
    }
}

impl ComAmazonawsLambdawebLambdawebState {
    /// The current web function usage: how many web functions are stored.
    ///
    /// "This count includes web functions in all states, not only functions in
    /// the `Active` state" (source: `smithy_query(query="shape",
    /// shape="AccountUsage")`), so every stored record counts whatever its
    /// [`super::FunctionState`] is — which also makes the count exactly the
    /// number of functions `ListWebFunctions` reports with no filter.
    pub fn account_usage(&self) -> AccountUsage {
        AccountUsage {
            function_count: as_count_member(self.functions.len()),
        }
    }
}
