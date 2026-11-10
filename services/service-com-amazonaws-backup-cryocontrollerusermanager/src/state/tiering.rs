// SPDX-License-Identifier: CC0-1.0

//! The TIERING side of this service's state: the configuration
//! `CreateTieringConfiguration` stores, and what every call that names a
//! `TieringConfigurationName` resolves against.
//!
//! # The rules this module holds
//!
//! * a tiering configuration is addressed by NAME and the name is unique to the
//!   account: the first create of a name stores the one configuration that name
//!   has, and a create naming one the account already holds is refused as
//!   already existing ([`super::tiering_configuration_already_exists`]) — so
//!   however many creates of a name race, exactly one of them stores a
//!   configuration and every other is refused;
//! * a name the account holds no configuration under is the not-found condition
//!   ([`super::tiering_configuration_not_found`]), which the read and the delete
//!   both answer, so a delete-then-read reports the same refusal as a read of a
//!   name nothing ever created;
//! * the selections a create carries are stored verbatim and reported back by
//!   both reads through the two projections below, so the configuration a
//!   create stored and the one a read reports can never differ.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::ComAmazonawsBackupCryocontrollerusermanagerState;
use crate::ops::types::{
    ResourceSelection, TieringConfiguration, TieringConfigurationsListMember,
};

/// The most tiering configurations this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota, for the reason given on
/// [`super::MAX_VAULTS`]: the store is in memory and every request can add to
/// it. AWS caps the tiering configurations an account may hold too, and answers
/// the same modelled `LimitExceededException` for a create past it.
pub const MAX_TIERING_CONFIGURATIONS: usize = 500;

/// A tiering configuration: what `CreateTieringConfiguration` stores,
/// `GetTieringConfiguration` and `ListTieringConfigurations` report, and
/// `DeleteTieringConfiguration` removes.
#[derive(Debug, Clone)]
pub struct TieringConfigurationRecord {
    pub name: String,
    pub arn: String,
    /// The vault the configuration applies to, or
    /// [`super::VAULT_NAMES_WILDCARD`] for every vault of the account — "use
    /// `*` to apply to all backup vaults" (model doc for the member).
    pub backup_vault_name: String,
    /// The resource selections the create carried, stored as submitted: the
    /// resources, their type and the day count after which each transitions.
    pub resource_selection: Vec<ResourceSelection>,
    pub creator_request_id: Option<String>,
    pub creation_time: SystemTime,
    /// When an update last rewrote this configuration; `None` until one has.
    pub last_updated_time: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl TieringConfigurationRecord {
    /// The configuration as the single-resource READ reports it — the ONE
    /// projection of a stored configuration, so `GetTieringConfiguration` can
    /// report nothing the create did not store.
    pub fn described(&self) -> TieringConfiguration {
        TieringConfiguration {
            tiering_configuration_name: self.name.clone(),
            tiering_configuration_arn: Some(self.arn.clone()),
            backup_vault_name: self.backup_vault_name.clone(),
            resource_selection: self.resource_selection.clone(),
            creator_request_id: self.creator_request_id.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_updated_time: self
                .last_updated_time
                .map(::aws_smithy_types::DateTime::from),
        }
    }

    /// The same configuration as the LISTING reports it: the summary members
    /// the list shape declares, taken from the same stored record the read
    /// projects, so a listed configuration and the read of it agree.
    pub fn listed(&self) -> TieringConfigurationsListMember {
        TieringConfigurationsListMember {
            tiering_configuration_arn: Some(self.arn.clone()),
            tiering_configuration_name: Some(self.name.clone()),
            backup_vault_name: Some(self.backup_vault_name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_updated_time: self
                .last_updated_time
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Resolve the tiering configuration a `TieringConfigurationName`
    /// addresses, or the modelled refusal for one this account does not hold.
    ///
    /// The ONE resolution step the read and the delete share, so neither can
    /// answer a different class or a different sentence for the same condition.
    pub fn stored_tiering_configuration(
        &self,
        name: &str,
    ) -> Result<&TieringConfigurationRecord, crate::ops::types::ResourceNotFoundException> {
        self.tiering_configurations
            .get(name)
            .ok_or_else(super::tiering_configuration_not_found)
    }

    /// The configuration a repeated `CreatorRequestId` replays: "a unique
    /// string that identifies the request and allows failed requests to be
    /// retried without the risk of running the operation twice" (model doc for
    /// the member), so a retry carrying the token of a create that already
    /// stored a configuration answers with that configuration rather than
    /// storing a second one.
    pub fn tiering_configuration_by_creator_request_id(
        &self,
        creator_request_id: &str,
    ) -> Option<&TieringConfigurationRecord> {
        self.tiering_configurations.values().find(|configuration| {
            configuration.creator_request_id.as_deref() == Some(creator_request_id)
        })
    }
}
