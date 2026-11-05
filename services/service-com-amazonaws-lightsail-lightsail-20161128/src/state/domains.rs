// SPDX-License-Identifier: CC0-1.0

//! The stored domain recordset: the DNS zone the account holds, with the entries
//! added to it.

use super::*;

/// One stored domain recordset: the DNS zone `CreateDomain` created, with the
/// entries the account has added to it.
#[derive(Debug, Clone)]
pub struct StoredDomain {
    /// The domain name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The recordsets held in this zone, newest last. A zone is created empty:
    /// `CreateDomainEntry` is what adds an entry to it.
    pub entries: Vec<DomainEntry>,
}

impl StoredDomain {
    /// This record as the modelled `Domain`.
    pub fn view(&self) -> Domain {
        Domain {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DOMAIN.to_string()),
            tags: Some(self.tags.clone()),
            domain_entries: Some(self.entries.clone()),
            // Nothing has delegated a Route 53 registered domain to this zone:
            // that delegation is made in Route 53, which this emulator does not
            // serve, so there is no state to report.
            registered_domain_delegation_info: None,
        }
    }
}
