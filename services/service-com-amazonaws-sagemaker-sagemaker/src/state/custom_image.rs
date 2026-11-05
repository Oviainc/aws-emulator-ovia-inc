// SPDX-License-Identifier: CC0-1.0

//! The custom-image slice of this service's state, under the name this module gathers it
//! for: the records, bounds, statuses and settle live in [`super::image`], and everything
//! they declare is re-exported here, so a path naming either module resolves to the one
//! slice. Nothing is declared twice.

pub use super::image::*;
