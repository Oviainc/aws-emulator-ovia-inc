Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ae9fe3612b346aad8c571a3d654fb37d429bcc9f

# Fold materialise_due's pending-guard recomputation onto OwedTransition instead of restating it per record kind

_Filed: 2026-10-29T15:58:19Z_
_Operation: AssociateUser | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ae9fe3612b346aad8c571a3d654fb37d429bcc9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "which instant does this stored record owe the clock, given its status" is implemented TWICE in this crate's own state module, once per record kind in each copy:

1. in the `OwedTransition` impls (`src/state.rs:683-727`), which the crate declares to be the single source of that fact — every creating and teardown writer reaches it through `schedule_record_transition` (`src/state.rs:1077-1081`), and the wire test `tests/all/pending_transition_comes_with_the_record.rs` imports `OwedTransition` and asserts the store's guard against it;
2. inline in `materialise_due` (`src/state.rs:935-1012`), which re-enumerates, for all four kinds, which statuses are non-terminal and which field carries the due instant, in order to recompute `pending_transition_due_at` via its local `still_pending` closure.

Both copies answer the same question with the same branches (`status == STATUS_REGISTERING -> registered_due_at`, `deregistration_started(status) -> deregistered_due_at`, and so on), so they can drift. The drift is silent and the test above names its consequence exactly: a non-terminal status (or a new due field) added to the trait impl and not to the sweep leaves `pending_transition_due_at` recomputed as if that record owed nothing, so "a transition that falls due and is never materialised until some unrelated record happens to fall due too" (`tests/all/pending_transition_comes_with_the_record.rs:20-22`).

The sweep's `still_pending(...)` calls can read the instant off `record.owed_transition_due_at()` after it has applied whatever status change is due — for a record it just advanced the trait answers `None`, and for one not yet due it answers the same instant the inline branch names — so the whole per-kind enumeration survives in exactly one place. Every response and every existing test stays unchanged; the sweep keeps its own branches only for APPLYING a transition, which is a different thing from reporting when one is owed.

## Evidence

Copy 1 — `services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs:667-692`, which declares itself the one place:

    /// The status transition a stored record owes the clock, read off the record itself.
    ///
    /// A record is in a non-terminal status exactly when its own status says so, and the
    /// instant that status falls due is a field of the same record — so the record, not its
    /// writer, is what knows whether anything is owed and when. Every writer that stores a
    /// record of one of these kinds asks it through
    /// [`schedule_record_transition`](...),
    /// which is the ONE place "store a record in a non-terminal status" is paired with
    /// "schedule the transition it owes"
    pub trait OwedTransition { ... }

    impl OwedTransition for IdentityProviderRecord {
        fn owed_transition_due_at(&self) -> Option<SystemTime> {
            if self.status == STATUS_REGISTERING {
                Some(self.registered_due_at)
            } else if deregistration_started(&self.status) {
                self.deregistered_due_at
            } else {
                None
            }
        }
    }

Copy 2 — the same per-kind rule restated inline in the sweep, `src/state.rs:926-952`:

        // The sweep also recomputes the guard, from the records that are still pending.
        let mut next: Option<SystemTime> = None;
        let still_pending = |due_at: SystemTime, next: &mut Option<SystemTime>| { ... };
        ...
        for record in self.identity_providers.values_mut() {
            if record.status == STATUS_REGISTERING {
                if now >= record.registered_due_at {
                    record.status = STATUS_REGISTERED.to_string();
                } else {
                    still_pending(record.registered_due_at, &mut next);
                }
            } else if record.status == STATUS_DEREGISTERING {
                if let Some(due_at) = record.deregistered_due_at {
                    if now >= due_at { deregistered.push(record.arn.clone()); }
                    else { still_pending(due_at, &mut next); }
                }
            }
        }

and the same pairing again for `InstanceUserRecord` (`src/state.rs:957-976` vs. impl at `695-705`), `ProductUserRecord` (`src/state.rs:980-993` vs. impl at `707-715`) and `LicenseServerEndpointRecord` (`src/state.rs:994-1012` vs. impl at `717-727`).

Test coverage that must stay green and that pins the guard against the trait, not against the sweep: `tests/all/pending_transition_comes_with_the_record.rs:28-30` (`use ...state::{seed_account_instances, OwedTransition};`) and its stated rule at lines 8-22 — "the store's pending guard is exactly the earliest transition the records it holds owe, whichever operation did the storing"; plus the teardown suites that drive the sweep end to end (`tests/all/product_subscription_teardown.rs`, `tests/all/user_disassociation.rs`, `tests/all/identity_provider_deregistration.rs`, `tests/all/license_server_endpoint_deletion.rs`, `tests/all/state_roundtrip.rs`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs and read the `OwedTransition` trait doc and its four impls (lines 667-727).
2. Read `materialise_due` (lines 918-1049) and compare its per-kind branches against those impls: `identity_providers` 935-952 vs. 683-692, `instance_users` 957-976 vs. 695-705, `product_users` 980-993 vs. 707-715, `license_server_endpoints` 994-1012 vs. 717-727.
3. Observe that each pair enumerates the same non-terminal statuses and the same due-instant fields, and that only the trait copy is reached by the writers (`schedule_record_transition`, lines 1077-1081) and by the test that asserts the guard (tests/all/pending_transition_comes_with_the_record.rs:28-30).
4. Confirm the drift is unobservable from the wire until it bites: tests/all/pending_transition_comes_with_the_record.rs:18-22 states that the only wire-visible consequence of a wrong guard is a due transition never materialised until an unrelated record falls due.
