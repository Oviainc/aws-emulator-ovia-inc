// SPDX-License-Identifier: CC0-1.0

//! The form a service reads a CALLER-SUPPLIED email address against.
//!
//! Several services bind a member their model DOCUMENTS as an email address — the
//! address a budget notification is sent to, the endpoint a topic subscription
//! delivers to, the identity a mail service sends from — and the models target those
//! members at a bare string, or at a pattern far too wide to state the form, so the
//! boundary validator admits any text at all and reading the form is the handler's
//! job. It has to be read: a value that is not an address names no mailbox, so such a
//! request is a parameter violation (attributed to the member) rather than a resource
//! that happens to be missing. The one reading lives here so no two services disagree
//! about what an email address is.
//!
//! The form is the one those members document,
//! `UserName@[SubDomain.]Domain.TopLevelDomain`: a local part, a single `@`, and a
//! domain of at least two labels ([`is_domain_name`]). The local part is a dot-atom —
//! one or more atoms of the RFC 5322 atom characters separated by single `.`s — so a
//! leading, trailing or doubled dot makes an EMPTY atom and names no mailbox, exactly
//! as an empty label is not a domain label.
//!
//! The WORDING of a refusal stays with the service, which owns its error envelope:
//! this module answers only whether the value is an address.
//!
//! `service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs` and
//! `service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs` still carry copies
//! of this form, each inlined before this module existed and pinned by that service's own
//! refusal suite; they are to be migrated onto this one rather than copied again, as
//! `service-com-amazonaws-ses-simpleemailservice/src/state.rs` already has been.

/// The punctuation an ATOM of a local part may carry, beside the ASCII letters and
/// digits: the RFC 5322 atom characters. The `.` that SEPARATES atoms is not one of
/// them — it is what [`is_email_address`] splits a local part on.
const ATOM_PUNCTUATION: &str = "!#$%&'*+-/=?^_`{|}~";

/// Whether `value` is an email address: a dot-atom local part, a single `@`, and a
/// domain name ([`is_domain_name`]).
///
/// A value carrying a second `@` is not an address either: everything after the first
/// one is read as the domain, and `@` is not a character a domain label may carry.
/// Nothing in an address is whitespace, since neither an atom nor a label admits any.
#[must_use]
pub fn is_email_address(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    is_dot_atom(local) && is_domain_name(domain)
}

/// Whether `value` is a domain name of at least two non-empty labels — the
/// `[SubDomain.]Domain.TopLevelDomain` half of the form.
///
/// A label is ASCII letters, digits and hyphens, and the top-level label carries at
/// least one letter, which admits an internationalised (`xn--…`) top-level domain and
/// refuses an all-numeric one (an address's domain is a name, not an address literal).
#[must_use]
pub fn is_domain_name(value: &str) -> bool {
    // A name with no `.` is a single label, which is no domain under this form.
    let Some((host, top_level)) = value.rsplit_once('.') else {
        return false;
    };
    host.split('.').all(is_domain_label)
        && is_domain_label(top_level)
        && top_level.chars().any(|character| character.is_ascii_alphabetic())
}

/// Whether `value` is ONE label of a domain name: non-empty, ASCII letters, digits and
/// hyphens.
fn is_domain_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// Whether `value` is a dot-atom: one or more non-empty atoms of the ASCII letters,
/// digits and [`ATOM_PUNCTUATION`], separated by single `.`s.
///
/// The empty string is not one: splitting it yields one empty atom, so a member that is
/// present but empty names no mailbox.
fn is_dot_atom(value: &str) -> bool {
    value.split('.').all(|atom| {
        !atom.is_empty()
            && atom.chars().all(|character| {
                character.is_ascii_alphanumeric() || ATOM_PUNCTUATION.contains(character)
            })
    })
}
