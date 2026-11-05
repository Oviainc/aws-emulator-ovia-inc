// SPDX-License-Identifier: CC0-1.0

//! Every lineage slice of this service's state under one name: the action
//! ([`super::lineage_action`]), the context ([`super::lineage_context`]), the lineage
//! group ([`super::lineage_group`]) and the associations that join those entities
//! ([`super::lineage_association`], which also holds the entity kinds an association end
//! can name).
//!
//! Each slice owns its own file so it reads as one unit; everything they declare is
//! re-exported here, so a path naming this module resolves to the same items. Nothing is
//! declared twice.

pub use super::lineage_action::*;
pub use super::lineage_association::*;
pub use super::lineage_context::*;
pub use super::lineage_group::*;
