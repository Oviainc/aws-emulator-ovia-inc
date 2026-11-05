// SPDX-License-Identifier: CC0-1.0

//! The lookups the RESOURCE-POLICY surface addresses a table by.
//!
//! The same lookup [`super::ComAmazonawsDynamodbDynamodb20120810State::table_by_arn`]
//! makes, with the refusal that surface words: it reports a missing resource as the
//! TABLE it is, by the bare name inside the `ResourceArn` the request carried, while the
//! tagging surface names the member it was addressed by. One sentence per surface rather
//! than one shared one: a message is part of each surface's contract.

use super::*;

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// The table a RESOURCE-POLICY operation names, or the not-found that surface words.
    pub fn table_for_policy(&self, resource: &TaggedResource) -> Result<&Table, RequestError> {
        self.tables_in(&resource.region)
            .and_then(|tables| tables.get(&resource.table))
            .ok_or_else(|| not_found(&resource.table))
    }

    /// The same, mutably, for the policy operations that attach or remove a document.
    pub fn table_for_policy_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut Table, RequestError> {
        self.table_mut(&resource.region, &resource.table)
            .map_err(|_| not_found(&resource.table))
    }
}
