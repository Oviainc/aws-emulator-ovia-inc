// SPDX-License-Identifier: CC0-1.0

//! The AD DIRECTORY INVENTORY: which Active Directory directories the account holds, so
//! the directory DATA plane can tell a directory of this account from one that is not.
//!
//! A directory is created by the AWS Directory Service CONTROL plane (`CreateDirectory`,
//! `CreateMicrosoftAD`) and deleted by it (`DeleteDirectory`); the Directory Service DATA
//! plane — which reads and writes the users, groups and memberships INSIDE a directory —
//! creates none and so cannot know from its own records whether a `DirectoryId` a request
//! names is one the account holds at all. It has to know, because every one of its
//! operations answers a directory the caller cannot reach by refusing the call rather than
//! by reporting anything about the directory.
//!
//! This is that record: ONE inventory in the shared engine, written by the control plane
//! when it creates a directory and cleared when it deletes one, read by the data plane.
//! Nothing here names a wire shape — the inventory holds directory ids and, per id, the
//! one cross-plane fact the data plane cannot read anywhere else: WHETHER the control plane
//! has turned the directory's data API on, and from WHEN. Nothing here decides what a
//! reader does with the answer: WHICH error a data-plane request gets for a directory the
//! inventory does not hold, or holds shut, is that service's own.
//!
//! The toggle is recorded as the CHANGE the control plane accepted and the instant it
//! takes effect, because both toggles answer before the directory's data API has moved:
//! the clock applies it, at the same instant the control plane's own transient status
//! settles, so the two planes never disagree about whether the data API is in service. A
//! directory the control plane has just created is out of service, which is the state every
//! create leaves it in.
//!
//! The inventory is a record of what this emulator CREATED, so a reader asking about an id
//! gets a yes or a no about this account's own directories and nothing else. It is BOUNDED
//! — see [`MAX_RECORDED_DIRECTORIES`], far above the control plane's own directory quota —
//! so nothing a caller loops on can grow it without end; the oldest registration is
//! dropped first, which keeps the directory a live workflow just created resolvable.
//!
//! LOCK ORDER: a service takes its OWN store first and this inventory second, and nothing
//! holds this inventory while taking another store — so two services cannot invert the
//! order between them.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// How many directories the inventory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this inventory lives in memory for the lifetime
/// of the process, so without a bound a caller creating directories in a loop would grow
/// it without end. It sits far above the control plane's own per-account directory quota,
/// and past it the OLDEST registration is dropped — never a newer one — so a directory a
/// running workflow just created is always still resolvable.
pub const MAX_RECORDED_DIRECTORIES: usize = 1_000;

/// The inventory itself: every `DirectoryId` the control plane created and has not deleted.
///
/// Registered in the shared engine under THIS type, so the control plane that creates a
/// directory and the data plane that serves the objects inside it read one record instead
/// of each keeping its own copy.
#[derive(Debug, Default)]
pub struct AdDirectoryInventory {
    /// The ids held, each with its data-access toggle: one lookup answers both questions a
    /// reader asks — does the account hold this directory, and does its data API answer.
    held: HashMap<String, DataAccess>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

/// Whether one directory's data API is in service, and the change the clock still owes it.
///
/// The control plane's enable and disable both answer BEFORE the directory's data API has
/// moved, so what a toggle records here is the change and the instant it takes effect; the
/// clock — not a later call — is what applies it, at the same instant the control plane's
/// own transient status settles. A directory a create has just registered is out of
/// service: a create opens nothing.
#[derive(Debug, Clone, Copy, Default)]
struct DataAccess {
    /// Whether the data API answers for the directory, before any change still owed.
    in_service: bool,
    /// The change the clock owes — whether the data API will answer, and the instant that
    /// takes effect — or `None` when the toggle has nothing outstanding.
    owed: Option<(bool, SystemTime)>,
}

impl DataAccess {
    /// Whether the data API answers for the directory at `now`: the change the clock owes
    /// once its instant has arrived, and what the toggle last settled on until then.
    fn in_service_at(self, now: SystemTime) -> bool {
        match self.owed {
            Some((in_service, effective_at)) if now >= effective_at => in_service,
            _ => self.in_service,
        }
    }

    /// This toggle with a change the control plane just accepted recorded on it: the data
    /// API is `in_service` from `effective_at` on, and until then stands as it stands at
    /// `now` — so a change recorded while an earlier one is still outstanding does not
    /// apply that earlier one early.
    fn changing_to(self, in_service: bool, effective_at: SystemTime, now: SystemTime) -> Self {
        Self {
            in_service: self.in_service_at(now),
            owed: Some((in_service, effective_at)),
        }
    }
}

impl AdDirectoryInventory {
    /// Record that the account holds the directory `directory_id`, its data API out of
    /// service as a create leaves it.
    ///
    /// Recording an id the inventory already holds changes nothing — neither its place in
    /// the eviction order nor its data-access toggle — so a control plane that re-registers
    /// a directory it already told the inventory about cannot age another one out, and
    /// cannot withdraw a directory whose data access it opened.
    pub fn register(&mut self, directory_id: &str) {
        if !self.held.contains_key(directory_id) {
            self.held
                .insert(directory_id.to_string(), DataAccess::default());
            self.order.push_back(directory_id.to_string());
            while self.order.len() > MAX_RECORDED_DIRECTORIES {
                if let Some(oldest) = self.order.pop_front() {
                    self.held.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of `directory_id`: the directory is gone, so the inventory no longer
    /// claims the account holds it.
    ///
    /// A no-op for an id the inventory holds no record of, so a repeated delete is harmless.
    pub fn forget(&mut self, directory_id: &str) {
        if self.held.remove(directory_id).is_some() {
            self.order.retain(|recorded| recorded != directory_id);
        }
    }

    /// Whether the account holds the directory `directory_id`.
    #[must_use]
    pub fn holds(&self, directory_id: &str) -> bool {
        self.held.contains_key(directory_id)
    }

    /// Record the control plane's ENABLE of the directory's data API: it is in service from
    /// `effective_at` on, which is when the transition that enable owes falls due.
    ///
    /// A no-op for an id the inventory holds no record of: a directory the account does not
    /// hold has no data API to put in service, and recording one would claim the account
    /// holds it.
    pub fn enable_data_access(
        &mut self,
        directory_id: &str,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        self.change_data_access(directory_id, true, now, effective_at);
    }

    /// Record the control plane's DISABLE of it: the data API is out of service from
    /// `effective_at` on, which is when the transition that disable owes falls due, and
    /// serves nothing for the directory again until another enable.
    ///
    /// A no-op for an id the inventory holds no record of, as [`Self::enable_data_access`]
    /// is.
    pub fn disable_data_access(
        &mut self,
        directory_id: &str,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        self.change_data_access(directory_id, false, now, effective_at);
    }

    /// The one writer behind both toggles, so an enable and a disable record a change the
    /// same way and differ only in what they change the data API to.
    fn change_data_access(
        &mut self,
        directory_id: &str,
        in_service: bool,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        if let Some(toggle) = self.held.get_mut(directory_id) {
            *toggle = toggle.changing_to(in_service, effective_at, now);
        }
    }

    /// Whether the directory's data API answers at `now`: the account holds the directory
    /// and the control plane's toggle has it in service at that instant.
    ///
    /// `false` while an enable's transition is still outstanding and `true` while a
    /// disable's is, so the data API is in service at exactly the instants the control
    /// plane reports its data access as settled on — nothing is served for a directory the
    /// account does not hold at all.
    #[must_use]
    pub fn data_access_is_in_service(&self, directory_id: &str, now: SystemTime) -> bool {
        self.held
            .get(directory_id)
            .is_some_and(|toggle| toggle.in_service_at(now))
    }

    /// How many directories the inventory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether the inventory holds no directory at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{AdDirectoryInventory, MAX_RECORDED_DIRECTORIES};

    /// A `DirectoryId` of the form the models declare (`^d-[0-9a-f]{10}$`), one per rank.
    fn directory(rank: usize) -> String {
        format!("d-{rank:010x}")
    }

    /// RULE: the inventory answers for exactly the directories registered and not forgotten,
    /// whatever order the two happen in.
    #[test]
    fn the_inventory_holds_what_was_registered_and_not_forgotten() {
        let mut inventory = AdDirectoryInventory::default();
        inventory.register(&directory(1));
        inventory.register(&directory(2));
        // Registering twice is the same fact, not a second record.
        inventory.register(&directory(2));

        for (case, id, expected) in [
            ("a registered directory", directory(1), true),
            ("a directory registered twice", directory(2), true),
            ("a directory never registered", directory(3), false),
        ] {
            assert_eq!(
                inventory.holds(&id),
                expected,
                "case {case:?}: expected holds({id:?}) to be {expected}, got {}",
                inventory.holds(&id)
            );
        }
        assert_eq!(
            inventory.len(),
            2,
            "case two distinct directories: expected 2 records, got {}",
            inventory.len()
        );

        inventory.forget(&directory(1));
        // Forgetting an id the inventory never held changes nothing.
        inventory.forget(&directory(9));
        assert!(
            !inventory.holds(&directory(1)) && inventory.holds(&directory(2)),
            "case one directory forgotten: expected only the other to be held, got {} record(s)",
            inventory.len()
        );

        inventory.forget(&directory(2));
        assert!(
            inventory.is_empty(),
            "case every directory forgotten: expected an empty inventory, got {} record(s)",
            inventory.len()
        );
    }

    /// RULE: the inventory is bounded, and past the ceiling the OLDEST registration is the
    /// one dropped, so the directory just created is still resolvable.
    #[test]
    fn the_inventory_is_bounded_and_drops_its_oldest_registration() {
        let mut inventory = AdDirectoryInventory::default();
        for rank in 0..MAX_RECORDED_DIRECTORIES {
            inventory.register(&directory(rank));
        }
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case an inventory filled to the ceiling: expected {MAX_RECORDED_DIRECTORIES} \
             records, got {}",
            inventory.len()
        );

        inventory.register(&directory(MAX_RECORDED_DIRECTORIES));
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case one past the ceiling: expected the ceiling to hold, got {}",
            inventory.len()
        );
        assert!(
            !inventory.holds(&directory(0)),
            "case one past the ceiling: expected the oldest registration to be dropped"
        );
        assert!(
            inventory.holds(&directory(MAX_RECORDED_DIRECTORIES))
                && inventory.holds(&directory(1)),
            "case one past the ceiling: expected the newest and the next-oldest to be held"
        );
    }

    /// RULE: a directory's data API is in service exactly while the control plane's toggle
    /// has it so — from the instant an enable's transition falls due until a disable's does
    /// — and a registration alone puts nothing in service.
    #[test]
    fn data_access_is_in_service_exactly_while_the_control_planes_toggle_has_it_so() {
        let span = Duration::from_millis(500);
        let enabled_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let enabled_from = enabled_at + span;
        let hour = Duration::from_secs(3_600);

        let mut inventory = AdDirectoryInventory::default();
        inventory.register(&directory(1));
        inventory.register(&directory(2));

        // (case, the instant asked about, whether the data API answers)
        let before_any_enable: [(&str, SystemTime, bool); 3] = [
            ("the instant of the create", enabled_at, false),
            ("one span later", enabled_from, false),
            ("an hour later", enabled_from + hour, false),
        ];
        for (case, now, expected) in before_any_enable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on a registered directory nothing enabled: expected \
                 {expected}, got {in_service}"
            );
        }

        inventory.enable_data_access(&directory(1), enabled_at, enabled_from);
        // (case, the instant asked about, whether the data API answers)
        let after_enable: [(&str, SystemTime, bool); 4] = [
            ("the instant of the enable", enabled_at, false),
            (
                "a moment before the enable's transition falls due",
                enabled_from - Duration::from_millis(1),
                false,
            ),
            ("the instant it falls due", enabled_from, true),
            ("well past it", enabled_from + hour, true),
        ];
        for (case, now, expected) in after_enable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on an enabled directory: expected {expected}, got {in_service}"
            );
        }

        // The toggle is per directory: the other registration is untouched.
        assert!(
            !inventory.data_access_is_in_service(&directory(2), enabled_from),
            "case another directory of the same account: expected its data API to stay out \
             of service"
        );

        // Re-registering a directory whose data access is on — which a control plane does
        // for a directory it already recorded — must not withdraw it.
        inventory.register(&directory(1));
        assert!(
            inventory.data_access_is_in_service(&directory(1), enabled_from),
            "case a re-registered enabled directory: expected its data API to stay in service"
        );

        // A disable takes effect when ITS transition falls due, and not before: the data
        // API is still in service the instant the disable is recorded.
        let disabled_at = enabled_from + hour;
        let disabled_from = disabled_at + span;
        inventory.disable_data_access(&directory(1), disabled_at, disabled_from);
        // (case, the instant asked about, whether the data API answers)
        let after_disable: [(&str, SystemTime, bool); 4] = [
            ("the instant of the disable", disabled_at, true),
            (
                "a moment before the disable's transition falls due",
                disabled_from - Duration::from_millis(1),
                true,
            ),
            ("the instant it falls due", disabled_from, false),
            ("well past it", disabled_from + hour, false),
        ];
        for (case, now, expected) in after_disable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on a disabled directory: expected {expected}, got {in_service}"
            );
        }

        // A disable recorded while an ENABLE is still outstanding does not apply that
        // enable early: the data API stays out of service throughout.
        inventory.register(&directory(3));
        inventory.enable_data_access(&directory(3), enabled_at, enabled_from);
        let cancelled_from = enabled_at + Duration::from_millis(100) + span;
        inventory.disable_data_access(
            &directory(3),
            enabled_at + Duration::from_millis(100),
            cancelled_from,
        );
        for (case, now) in [
            ("the instant the first enable would have fallen due", enabled_from),
            ("the instant the disable falls due", cancelled_from),
            ("an hour later", cancelled_from + hour),
        ] {
            assert!(
                !inventory.data_access_is_in_service(&directory(3), now),
                "case {case:?} on a directory disabled while its enable was outstanding: \
                 expected the data API to stay out of service"
            );
        }

        // A re-enable puts it back in service on its own transition, and forgetting the
        // directory closes every question about it.
        let re_enabled_at = disabled_from + hour;
        let re_enabled_from = re_enabled_at + span;
        inventory.enable_data_access(&directory(1), re_enabled_at, re_enabled_from);
        assert!(
            inventory.data_access_is_in_service(&directory(1), re_enabled_from)
                && !inventory.data_access_is_in_service(&directory(1), re_enabled_at),
            "case a re-enabled directory: expected the second transition to decide, not the \
             first"
        );
        inventory.forget(&directory(1));
        assert!(
            !inventory.data_access_is_in_service(&directory(1), re_enabled_from),
            "case a forgotten directory: expected no data API at all"
        );

        // A directory the inventory never held gains no record from either toggle.
        inventory.enable_data_access(&directory(7), re_enabled_at, re_enabled_from);
        inventory.disable_data_access(&directory(8), re_enabled_at, re_enabled_from);
        assert!(
            !inventory.holds(&directory(7)) && !inventory.holds(&directory(8)),
            "case the toggle on an unheld directory: expected no registration, got {} record(s)",
            inventory.len()
        );
    }
}
