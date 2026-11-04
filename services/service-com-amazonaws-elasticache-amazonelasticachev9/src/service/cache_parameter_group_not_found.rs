// SPDX-License-Identifier: CC0-1.0

//! The two templates this service answers a `CacheParameterGroupNotFoundFault`
//! with, and which of them each operation takes.
//!
//! ElastiCache words that fault two ways. Each is fixed text followed by the
//! `CacheParameterGroupName` the request named, and neither carries anything this
//! store holds; the two differ only in the separator between the type name and
//! `not found: `:
//!
//! ```text
//! CacheParameterGroupnot found: <name>     <- no separator at all
//! CacheParameterGroup not found: <name>
//! ```
//!
//! WHICH of the two an operation answers with is that operation's own — the two
//! are observable apart on the live service, so one service-wide template cannot
//! serve them all. [`joined_message`] is the first, which
//! `DeleteCacheParameterGroup`, `DescribeCacheParameters` and
//! `ResetCacheParameterGroup` answer; its missing space is the service's and not a
//! slip here, because the type name and `not found: ` arrive joined.
//! [`spaced_message`] is the second, which `ModifyCacheParameterGroup` answers and
//! which the operations resolving a `CacheParameterGroupName` as part of a larger
//! request take too.
//!
//! Both templates in one place, so no operation has to restate one and the
//! handlers answering the same template cannot drift apart about its text. No
//! handler answers the third wording `CacheParameterGroup <name> not found.`
//! that [`crate::state::cache_parameter_group_not_found_message`] carries: it is
//! neither of the two the service emits.

/// The first template: `CacheParameterGroupnot found: <name>`.
///
/// The one variable part is the `CacheParameterGroupName` the request named,
/// reported as the request spelled it, because the message is built from the
/// request and from nothing else.
pub fn joined_message(cache_parameter_group_name: &str) -> String {
    format!("CacheParameterGroupnot found: {cache_parameter_group_name}")
}

/// The second template: `CacheParameterGroup not found: <name>`, with the same
/// one variable part as [`joined_message`].
///
/// The text itself is the one `ModifyCacheParameterGroup` already answers with
/// ([`crate::state::parameter_writer_group_not_found_message`]), reached through
/// here so that every operation choosing between the two templates names them
/// the same way and the two spellings cannot drift apart.
pub fn spaced_message(cache_parameter_group_name: &str) -> String {
    crate::state::parameter_writer_group_not_found_message(cache_parameter_group_name)
}
