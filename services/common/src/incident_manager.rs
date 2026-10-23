// SPDX-License-Identifier: CC0-1.0

//! NOT PART OF THIS CRATE: no `mod` in `src/lib.rs` declares this file, so nothing
//! compiles it and no service's behaviour depends on it. It is a leftover of an earlier
//! round of the `ssm-contacts GetRotation` fix, which the fixer session that committed it
//! cannot delete — DELETE IT rather than wiring anything to it.
//!
//! The convention it recorded lives where it is applied: Amazon SSM Contacts is fronted by
//! a per-account Incident Manager replication set, and
//! `ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set` /
//! `require_active_replication_set_for` in
//! `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/state.rs` are the gate every
//! one of that service's data-plane handlers consults, documented there. The replication
//! set itself is owned by `ssm-incidents`' own `replication_sets` store.
