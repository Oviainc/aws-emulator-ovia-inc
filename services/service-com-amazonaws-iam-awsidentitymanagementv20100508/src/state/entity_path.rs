// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a `Path` whose form is wrong.
//!
//! WHAT the published form is — and why the model-level constraint walk cannot decide it — is
//! the shared reading [`::service_common::iam_entity_path::is_published_form`], which every
//! operation of this crate that takes a `Path` calls directly. What belongs to this service
//! is the SENTENCE it answers a value outside that form with, which is this module.

#[allow(unused_imports)]
use super::*;

/// How this service refuses a `Path` whose form is wrong: AWS's one sentence for that member,
/// under the code IAM answers a value it cannot accept with — the same code and sentence the
/// refusal seam answers when the same member breaks the pattern the walk DOES catch
/// ([`super::member_forms`]), so one logical failure has one answer however it was reached.
pub fn invalid_entity_path<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation_error(
        super::member_forms::ENTITY_PATH_REFUSAL.replace("{member}", "path"),
    )
}
