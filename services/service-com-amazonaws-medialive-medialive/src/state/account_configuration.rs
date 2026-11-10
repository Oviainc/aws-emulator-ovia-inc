// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT CONFIGURATION slice of this service's state.
//!
//! One file for the whole slice — the record, its bound, the KMS key ARN shape
//! it accepts and the store's methods — so the slice can be read, and merged, on
//! its own, exactly as the [`super::alarm_template_groups`] slice is. The store
//! itself is the `account_configurations` member of
//! [`ComAmazonawsMedialiveMedialiveStore`] in the parent module.
//!
//! There is no resource to mint here and none to delete: an account always HAS a
//! configuration, so `DescribeAccountConfiguration` answers for an account that
//! has configured nothing yet with the configuration it has rather than a 404,
//! and `UpdateAccountConfiguration` — whose request is "List of account
//! configuration parameters to update" (`smithy_query(query="shape",
//! shape="UpdateAccountConfigurationRequest")`) — changes the parameters the
//! request carries and leaves the ones it omits exactly as they stood.
//!
//! `KmsKeyId` carries the CALLER'S OWN key — "Specify the ARN of a KMS key that
//! you have created. Or leave blank to use the key that MediaLive creates and
//! manages for you" — so it is reported only when an update has set one: the
//! member is BLANK for an account that configured nothing, which is what asks
//! for the key the service manages. The store therefore keeps a record only for
//! a region where a customer-managed key was configured, and the projection
//! leaves the member absent everywhere else rather than substituting a key of
//! the service's own, which is not a value a caller could configure back.
//!
//! The configuration is held PER REGION, keyed by the region the request was
//! signed for ([`super::request_region`]): MediaLive is a regional service, so
//! each regional endpoint answers for the account's configuration in its own
//! region, and configuring a key through one endpoint is not visible through
//! another. It is keyed on the STORE rather than held inside a region's
//! resource partition because it is the account's settings and not a resource,
//! so the bound below is on the configurations the whole store keeps.

use crate::ops::types::AccountConfiguration;

use super::ComAmazonawsMedialiveMedialiveStore;

/// The most regions this emulator keeps an account configuration for.
///
/// A ceiling on the whole store, not a quota of AWS's: the key is the region a
/// request was signed for, which the caller chooses, so the bound is what stops
/// an update loop over made-up region names from growing the emulator's heap
/// without limit. `UpdateAccountConfiguration` models
/// `TooManyRequestsException`, which is that refusal once the store is full. It
/// is set well above the number of regions MediaLive is offered in, so no
/// sequence of real regional endpoints can reach it.
pub const MAX_ACCOUNT_CONFIGURATION_REGIONS: usize = 64;

/// The account's MediaLive configuration in one region, as
/// `UpdateAccountConfiguration` left it.
///
/// A record exists only once an update has set something in that region; a
/// region without one reads as [`AccountConfigurationRecord::default`], which is
/// the configuration of an account that has configured nothing of its own.
#[derive(Debug, Clone, Default)]
pub struct AccountConfigurationRecord {
    /// The customer-managed KMS key this account's MediaLive features encrypt
    /// with, as `AccountConfiguration$KmsKeyId` carries it.
    ///
    /// `None` is "no CUSTOMER-managed key", which is what the member documents as
    /// the meaning of a BLANK value: "Specify the ARN of a KMS key that you have
    /// created. Or leave blank to use the key that MediaLive creates and manages
    /// for you". So a blank update is stored as `None` rather than as an empty
    /// string, and a read of a region with no customer key reports no `KmsKeyId`
    /// at all ([`ComAmazonawsMedialiveMedialiveStore::account_configuration`]).
    pub kms_key_id: Option<String>,
}

impl AccountConfigurationRecord {
    /// Whether this record holds no setting at all, which is indistinguishable
    /// from the account holding no record for the region: both read as the
    /// configuration of an account with no customer-managed key, so a record in
    /// this state is dropped rather than kept (and so it holds none of the
    /// store's bounded room). `KmsKeyId` is the whole of the configuration the
    /// model declares today; a member added to `AccountConfiguration` later joins
    /// this check.
    fn is_unset(&self) -> bool {
        self.kms_key_id.is_none()
    }
}

impl ComAmazonawsMedialiveMedialiveStore {
    /// The account's configuration in `region`, as the modelled shape every
    /// operation that reports one answers with.
    ///
    /// The ONE projection, so `DescribeAccountConfiguration` and the
    /// `UpdateAccountConfiguration` that preceded it cannot report the same
    /// stored configuration differently.
    ///
    /// `KmsKeyId` is reported exactly as the last update that set one left it,
    /// byte for byte, and is ABSENT where no update has set one: the member
    /// carries the key the CALLER created ("Specify the ARN of a KMS key that you
    /// have created. Or leave blank to use the key that MediaLive creates and
    /// manages for you"), so a blank member is itself the account's state — it
    /// says the account pinned no key of its own — and no value of this service's
    /// own is substituted for it. Substituting one would report a key the account
    /// never configured and that a caller cannot configure back.
    pub fn account_configuration(&self, region: &str) -> AccountConfiguration {
        AccountConfiguration {
            kms_key_id: self
                .account_configurations
                .get(region)
                .and_then(|record| record.kms_key_id.clone()),
        }
    }

    /// Whether `region` has no configuration record yet AND the store is already
    /// holding [`MAX_ACCOUNT_CONFIGURATION_REGIONS`] of them, so an update that
    /// would have to add one must refuse instead.
    ///
    /// A region the store already holds a record for is never refused: changing
    /// a configuration that exists adds nothing to the store.
    pub fn account_configurations_full(&self, region: &str) -> bool {
        !self.account_configurations.contains_key(region)
            && self.account_configurations.len() >= MAX_ACCOUNT_CONFIGURATION_REGIONS
    }

    /// Apply the parameters `requested` carries to the account's configuration in
    /// `region` and answer the configuration that results.
    ///
    /// PARTIAL, because the request is "List of account configuration parameters
    /// to update": a parameter the request supplies replaces the stored one, a
    /// parameter it omits — and a request that carries no `AccountConfiguration`
    /// at all — leaves the stored value alone. A supplied BLANK `KmsKeyId` is a
    /// value and not an omission: it clears the customer-managed key, which is
    /// the member's documented way of asking for the key MediaLive manages, and a
    /// read then reports no `KmsKeyId` at all.
    ///
    /// The write and the read-back are one step in the store, so what the update
    /// answers is what the next read answers.
    pub fn update_account_configuration(
        &mut self,
        region: &str,
        requested: Option<&AccountConfiguration>,
    ) -> AccountConfiguration {
        let requested_kms_key_id = requested.and_then(|config| config.kms_key_id.as_deref());
        if let Some(kms_key_id) = requested_kms_key_id {
            let record = self
                .account_configurations
                .entry(region.to_string())
                .or_default();
            record.kms_key_id = if kms_key_id.is_empty() {
                None
            } else {
                Some(kms_key_id.to_string())
            };
            // A record with nothing set reads exactly as no record, so it is
            // dropped instead of kept: clearing the customer-managed key gives
            // the region's slot in the bounded store back rather than holding it
            // for a configuration that pins nothing.
            if record.is_unset() {
                self.account_configurations.remove(region);
            }
        }
        self.account_configuration(region)
    }
}

/// The AWS service segment of a KMS key's ARN.
const KMS_SERVICE: &str = "kms";

/// The resource prefixes of the KMS ARNs that name a key: the key itself, and an
/// alias for one.
///
/// `arn:<partition>:kms:<region>:<account>:key/<key-id>` is a key's own ARN and
/// `arn:<partition>:kms:<region>:<account>:alias/<name>` is the ARN of an alias
/// that resolves to one; every other KMS ARN (a grant, say) names something that
/// is not a key.
const KMS_KEY_RESOURCE_PREFIXES: [&str; 2] = ["key/", "alias/"];

/// Whether `kms_key_id` is the ARN of a KMS key, which is what
/// `AccountConfiguration$KmsKeyId` is documented to carry: "Specify the ARN of a
/// KMS key that you have created" (`smithy_query(query="doc",
/// shape="AccountConfiguration", member="KmsKeyId")`).
///
/// The SHAPE is all that is checked — an ARN in the form every AWS ARN takes
/// ([`super::is_resource_arn`], one definition of that form for the whole
/// service), naming the KMS service, whose resource names a key. Whether the key
/// exists and whether this account may use it are not: no KMS runs behind this
/// emulator, so a key that is syntactically a key is taken at its word rather
/// than refused for a reason the emulator cannot establish.
pub fn is_kms_key_arn(kms_key_id: &str) -> bool {
    if !super::is_resource_arn(kms_key_id) {
        return false;
    }
    // `is_resource_arn` has already established the six segments `arn`,
    // partition, service, region, account and resource. `splitn` rather than
    // `split` so a resource carrying colons of its own stays whole: the service
    // is the segment two along, and the resource is the one two along from
    // there.
    let mut segments = kms_key_id.splitn(6, ':');
    let service = segments.nth(2);
    let resource = segments.nth(2).unwrap_or_default();
    service == Some(KMS_SERVICE)
        && KMS_KEY_RESOURCE_PREFIXES
            .iter()
            .any(|prefix| resource.starts_with(prefix) && resource.len() > prefix.len())
}
