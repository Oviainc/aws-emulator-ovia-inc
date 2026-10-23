// SPDX-License-Identifier: CC0-1.0

//! Superseded: the rule "every S3 error document carries a `<Message>` beside its
//! `<Code>`" is asserted by [`super::error_message_envelope`], whose table spans the
//! error codes AND re-derives the documented-message catalog's key set from the Smithy
//! model, so a newly modeled code cannot go out message-less.
//!
//! This suite was this fix's own table for the same rule; it was folded into that one
//! rather than kept beside it, because one table for a rule beats two.
