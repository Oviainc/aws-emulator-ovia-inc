// SPDX-License-Identifier: CC0-1.0

//! Both experiment slices of this service's state under one name: experiment MANAGEMENT
//! ([`super::experiment`]: the experiment record, its bound and its ARN kind) and
//! experiment TRACKING ([`super::trial_component`]: the trial component, its bounds and
//! the name-or-ARN readings every trial and component operation shares).
//!
//! Each slice owns its own file so it reads as one unit; everything they declare is
//! re-exported here, so a path naming this module resolves to the same items. Nothing is
//! declared twice.

pub use super::experiment::*;
pub use super::trial_component::*;
