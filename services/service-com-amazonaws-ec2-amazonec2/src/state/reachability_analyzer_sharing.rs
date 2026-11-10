// SPDX-License-Identifier: CC0-1.0

//! The TRUST RELATIONSHIP between Reachability Analyzer and Organizations: the one fact
//! `EnableReachabilityAnalyzerOrganizationSharing` establishes for the organization.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `serial_console_access.rs` gives: one subject in one file, and out of the end of
//! `state.rs`, where every concurrently landing EC2 fix inserts. Everything here is an
//! inherent method of [`ComAmazonawsEc2Amazonec2State`], so a handler reaches it on the store
//! itself.
//!
//! The flag itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`reachability_analyzer_organization_sharing`), because the state struct is one struct;
//! only the code that reads and writes it lives here. It is ONE value rather than a map
//! keyed by Region: the relationship is between Reachability Analyzer and the ORGANIZATION —
//! "establishes a trust relationship between Reachability Analyzer and Organizations. This
//! operation must be performed by the management account for the organization"
//! (`EnableReachabilityAnalyzerOrganizationSharing`) — and the operation that establishes it
//! names no Region at all.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether Reachability Analyzer has a trust relationship with Organizations: whether "a
    /// user in the management account or a delegated administrator account can run a
    /// cross-account analysis using resources from the member accounts"
    /// (`EnableReachabilityAnalyzerOrganizationSharing`).
    ///
    /// A pure read: the relationship has no clock-owed move — the enable's own answer reports
    /// it as established — so nothing about it changes with time. An organization that has
    /// never established one reads `false`, there being no trust relationship until one is
    /// established.
    pub fn reachability_analyzer_organization_sharing(&self) -> bool {
        self.reachability_analyzer_organization_sharing
    }

    /// Establish the trust relationship between Reachability Analyzer and Organizations, and
    /// answer it as it now stands.
    ///
    /// The answer is read back off the field rather than echoed from the argument, so the
    /// operation reports exactly what the store holds. Idempotent: establishing one for an
    /// organization that already has it changes nothing and answers the same value, a trust
    /// relationship being a fact that either holds or does not.
    pub fn set_reachability_analyzer_organization_sharing(&mut self, enabled: bool) -> bool {
        self.reachability_analyzer_organization_sharing = enabled;
        self.reachability_analyzer_organization_sharing
    }
}
