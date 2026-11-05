// SPDX-License-Identifier: CC0-1.0

//! The account-level AWS Marketplace subscriptions this service's availability settings
//! are gated behind.
//!
//! Not every deployment type comes with the service: the ones sold as an AWS Marketplace
//! offering are provisioned only for an account that holds an ACTIVE subscription to that
//! offering, and a request for one from an account without it is refused. Nothing in this
//! API subscribes an account — a buyer does that in AWS Marketplace, entirely out of band
//! of these operations — so the set below starts EMPTY, which is exactly what an account
//! that never subscribed looks like, and [`subscribe_account_to_marketplace_offering`] is
//! the out-of-band seam a fixture grants one through.

use super::{
    ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState, CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS,
};

/// The `ValidationExceptionReason` this service reports when the request is well-formed
/// but the account is not entitled to what it asks for: the catch-all member of the
/// modelled enum (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`), because no
/// field of the request failed validation.
pub const VALIDATION_EXCEPTION_REASON_OTHER: &str = "OTHER";

/// An AWS Marketplace offering a deployment type's availability is gated behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceOffering {
    /// The key the account's subscription to this offering is stored under.
    pub key: &'static str,
    /// The message this service refuses a request for the offering with while the account
    /// holds no active subscription to it.
    ///
    /// FIXED text: it names the offering and the remediation and interpolates nothing
    /// from the request or from the store, so every refusal of this class reads the same.
    pub missing_subscription_message: &'static str,
}

/// The Read Replicas offering, which carries the read-replicas cluster deployment type:
/// the availability setting a Timestream for InfluxDB cluster of that type is, sold in
/// AWS Marketplace rather than with the service.
pub const READ_REPLICAS_OFFERING: MarketplaceOffering = MarketplaceOffering {
    key: "READ_REPLICAS",
    missing_subscription_message: "Timestream for InfluxDB didn't find an active AWS \
        Marketplace subscription for Read Replicas associated with the account. You must \
        first create a subscription in Marketplace to access Read replicas availability \
        settings. You might need to wait up to 5 minutes for that subscription to be \
        active.",
};

/// The AWS Marketplace offering `deployment_type` is gated behind, or `None` when this
/// service provisions that deployment type without any subscription.
///
/// The only gated one is the read-replicas cluster deployment type, the sole member of
/// the modelled `ClusterDeploymentType` enum; the DB-instance deployment types
/// (`crate::ops::types::DEPLOYMENT_TYPE_VALUES`) come with the service.
pub fn marketplace_offering_for_deployment_type(
    deployment_type: &str,
) -> Option<MarketplaceOffering> {
    match deployment_type {
        CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS => Some(READ_REPLICAS_OFFERING),
        _ => None,
    }
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Whether the account holds an active AWS Marketplace subscription to `offering`.
    pub fn holds_marketplace_subscription(&self, offering: MarketplaceOffering) -> bool {
        self.marketplace_subscriptions.contains(offering.key)
    }

    /// Record the account's active subscription to `offering`, answering whether it
    /// granted one the account did not already hold.
    ///
    /// The out-of-band grant: AWS Marketplace subscribes an account, no operation of this
    /// service does, so this is reached from the seam below and never from a handler.
    pub fn subscribe_to_marketplace_offering(&mut self, offering: MarketplaceOffering) -> bool {
        self.marketplace_subscriptions.insert(offering.key.to_string())
    }

    /// Drop the account's subscription to `offering` — a subscription cancelled in AWS
    /// Marketplace — answering whether one was held. The counterpart of
    /// [`Self::subscribe_to_marketplace_offering`], so the gate follows the store in both
    /// directions.
    pub fn unsubscribe_from_marketplace_offering(&mut self, offering: MarketplaceOffering) -> bool {
        self.marketplace_subscriptions.remove(offering.key)
    }
}

/// Grant the account an active subscription to `offering` in this service's store.
///
/// The out-of-band creation seam: a buyer subscribes in AWS Marketplace and this API
/// models no operation that does so, so a fixture (or an admin path) hands the
/// subscription to the emulator here instead.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so there
/// is no store to write), otherwise whether the subscription was newly granted.
pub fn subscribe_account_to_marketplace_offering<Eng: Send + Sync + 'static>(
    engine: &Eng,
    offering: MarketplaceOffering,
) -> Option<bool> {
    let state = super::service_state(engine)?;
    let granted = state.lock().subscribe_to_marketplace_offering(offering);
    Some(granted)
}

/// Drop the account's subscription to `offering` from this service's store — the
/// cancellation counterpart of [`subscribe_account_to_marketplace_offering`], answering
/// whether a subscription was held.
pub fn unsubscribe_account_from_marketplace_offering<Eng: Send + Sync + 'static>(
    engine: &Eng,
    offering: MarketplaceOffering,
) -> Option<bool> {
    let state = super::service_state(engine)?;
    let held = state.lock().unsubscribe_from_marketplace_offering(offering);
    Some(held)
}
