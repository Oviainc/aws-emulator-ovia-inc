// SPDX-License-Identifier: CC0-1.0

//! The header this service reports a response's correlation id in.
//!
//! Every answer AWS sends names the request it answered, in the `x-amzn-requestid` response
//! header — a success exactly as a failure — and that header is where an SDK reads the
//! `RequestId` of its response metadata from. A client that cannot read it has no
//! correlation handle on the call at all: an SDK's own error reporting and retry logging
//! both quote it, and a client that logs a failed call has nothing to log without it.
//!
//! What it is NOT is the id of the EXECUTION an invocation started. That one is the
//! invocation's own (`crate::state::mint_request_id`, reported in the run's `START`/`END`/
//! `REPORT` lines, in its log stream name and in the error document a failed handler
//! answers with); this header is a property of the HTTP response, minted for every answer
//! of every operation alike, and the two are deliberately separate ids until an invocation
//! can carry its own onto its response.
//!
//! The framework's restJson1 renderers write the document and no such header (`vendor/` is
//! read-only), so this service reports it — through the TREE's statement of that rule
//! ([`service_common::query_error_envelope::with_request_id_header`]), which already words
//! it for a protocol with no envelope element for the id: one freshly minted id per
//! response, and a response that already names its request keeps the id it has. Nothing is
//! restated here; this module only says WHERE this service applies it, which is the one
//! wrapper every modelled operation is served behind ([`super::StatedPerOperation`]) and the
//! answer to a request that named no modelled operation.

pub use service_common::query_error_envelope::with_request_id_header;
