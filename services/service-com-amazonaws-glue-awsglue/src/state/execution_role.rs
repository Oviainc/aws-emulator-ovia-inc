// SPDX-License-Identifier: CC0-1.0

//! Nothing lives here any more: the reading of the caller-supplied IAM role a Glue resource
//! RUNS AS lives in [`super::crawler_roles_and_connections`], and the module root re-exports
//! it from there. Kept as an empty module, declared nowhere, so no path in the tree resolves
//! to two copies of that one reading.
