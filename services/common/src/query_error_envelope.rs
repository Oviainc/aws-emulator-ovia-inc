// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED — this file is intentionally empty. It carries no code and is declared
//! nowhere, so it compiles to nothing.
//!
//! It was written as the shared home for the `<RequestId>` element an awsQuery
//! `<ErrorResponse>` carries beside its `<Error>`, for the three services in this tree
//! whose models speak that protocol. While it was in flight, the RDS front door landed
//! its own writer at
//! `services/service-com-amazonaws-rds-amazonrdsv19/src/service/query_error_envelope.rs`,
//! beside the two that predate it (`cloudformation` and `sns`), and that module's own
//! documentation records the consolidation as the job of those services' issues. So this
//! module had no user and states no rule twice.
//!
//! The content is gone rather than the file because this fixer's tool set can write a
//! file but not remove one; the next change to touch this directory should delete it, and
//! the consolidation it was meant for should land here under the issue that owns it.
