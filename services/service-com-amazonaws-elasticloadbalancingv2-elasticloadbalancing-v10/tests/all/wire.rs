// SPDX-License-Identifier: CC0-1.0

//! The one piece of wire plumbing a suite needs beyond the harness every suite shares: the
//! emulator under test as a router.
//!
//! The shared harness itself — the request builder, the XML envelope reader, the [`Reply`]
//! it answers with and the API version every request carries — is
//! [`crate::provisioning_loop`], and is re-exported here so a suite reads ONE definition of
//! it rather than a second copy.

use smithy_server_runtime::MultiServiceRouter;

// The shared harness. Re-exported rather than copied: one request builder and one envelope
// reader for the crate, so no two suites can drift on encoding or on how an empty element
// is reported — and ONE definition of the API version, the one the requests themselves
// carry.
pub(crate) use crate::provisioning_loop::{
    action, call, first_text, members, p, Reply, VERSION,
};

/// A router carrying only this service, over one shared engine — what the production
/// router mounts, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    crate::provisioning_loop::emulator().0
}
