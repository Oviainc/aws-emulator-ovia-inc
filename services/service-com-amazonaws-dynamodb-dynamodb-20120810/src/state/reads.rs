// SPDX-License-Identifier: CC0-1.0

//! What a SINGLE-ITEM read reports: `GetItem`, and the per-element reads of
//! `BatchGetItem` and `TransactGetItems`.
//!
//! Those three differ from `Query` and `Scan` in having no page, no index and no filter:
//! one key addresses one item, so all that is left of what the read REPORTS is the
//! projection the request carried. The projection itself is read by the one reader every
//! read of this service reads it with ([`super::requested_projection`]) and applied by
//! the one that applies it ([`super::project_paths`]); this file is only where the two
//! meet for a single item, so the three handlers state that once rather than three times.
//!
//! A file of its own beside the item store rather than inside it, for the reason the
//! other read modules are: this is about what one REQUEST reports, not about what the
//! store holds. It is re-exported up to `crate::state`, so every caller names it
//! `crate::state::…`.

use super::{project_paths, DocumentPath, Item};

/// The `Item` a single-item read reports: what `projection` names of the item the key
/// addressed, or the whole item when the request named no projection — "If no attribute
/// names are specified, then all attributes are returned"
/// (`Get$ProjectionExpression`). A key that holds no item reports none, whichever
/// projection the request carried.
pub fn reported_item(item: Option<Item>, projection: Option<&[DocumentPath]>) -> Option<Item> {
    match (item, projection) {
        (Some(item), Some(paths)) => Some(project_paths(&item, paths)),
        (item, _) => item,
    }
}
