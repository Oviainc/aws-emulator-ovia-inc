// SPDX-License-Identifier: CC0-1.0

//! Staged, not yet wired: the parts of an AWS validation-exception message that are the
//! same for EVERY service.
//!
//! A service that refuses a request against its model's own constraints states, in the
//! `message` of its 400, one clause per failing member:
//! `<subject> '<member path>' failed to satisfy constraint: <requirement>`. How the
//! clauses are counted, ordered and joined into an envelope is a service DIALECT
//! (`N validation error(s) detected: …,…` for Amazon EVS, `N validation errors detected:
//! …; …` elsewhere, and `artifact` prints a `@pattern` WITH its anchors), but three
//! things inside a clause are not a dialect at all and belong in ONE place:
//!
//! * the requirement half, read off the framework's own wording from a violation with NO
//!   value attached and split at the LAST clause marker, so neither the rejected value
//!   nor a caller-chosen map key in the member path can reach it;
//! * the subject half: a present-but-invalid value is reported as `Value at '<path>' …`
//!   and never quoted back, while an ABSENT member keeps `Value null at '<path>' …`;
//! * a `@pattern` printed as the expression a value has to satisfy, without the `^`/`$`
//!   anchors a model writes to demand a whole-string match.
//!
//! This module is deliberately EMPTY and is declared nowhere, so it compiles nowhere: the
//! tree carries those three rules inlined in ~24 service crates (`grep -rln 'fn
//! unanchored' services/*/src`), each pinned by that service's own recordings and tests,
//! and migrating them is one change of its own rather than a side effect of a single
//! service's issue. The first fix to own that migration declares this module, moves the
//! rules here and calls it from the service crates; until then nothing new should inline
//! them again without leaving the same note.
