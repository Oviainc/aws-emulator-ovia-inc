// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and not declared as a module anywhere.
//!
//! This file held an endpoint-host reader (`addresses_endpoint`) that no claim rule in
//! this crate called: a client reaches this emulator through ONE local `endpoint_url`, so
//! every member of a shared-signing-name family sees the same `Host` and a host test
//! decides nothing — which is why [`crate::lex`] splits the one route shape its members
//! share by the shape of the label in the path and [`crate::bedrock`] hands such a segment
//! to a named owner. Its only consumer was one precondition assertion in a test, which now
//! states that fact directly, so the reader is gone rather than shipped unused beside the
//! three private copies of host parsing the service crates already carry (`sso`,
//! `artifact`, `sustainability`) and the framework's own `service_from_host_authority`.
//!
//! The file itself is left in place because the issue-fixer tooling can write a file but
//! cannot remove one; nothing compiles it (no module declares it), and it may be deleted
//! outright by anyone touching this crate.
