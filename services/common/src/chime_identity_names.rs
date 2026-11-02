// SPDX-License-Identifier: CC0-1.0

//! The IDENTITY NAME directory the Amazon Chime SDK family shares: the name an app
//! instance's identity — an `AppInstanceUser` or an `AppInstanceBot` — is registered
//! under, keyed by that identity's ARN.
//!
//! An identity is a resource of ONE service — Chime SDK Identity registers it, names it,
//! renames it and deletes it — while its name is REPORTED by another: every participant
//! Chime SDK Messaging puts on the wire (a channel's creator, a message's sender, a
//! channel member, a banned member, a moderator, the member that invited another) is an
//! `Identity` of an ARN *and* the name the identity it names carries. The messaging
//! service cannot read the identity service's own store, because the engine keys state by
//! TYPE, so the identity service PUBLISHES the name here and the messaging service reads
//! it. Without that hand-off a participant's name is unobservable through the messaging
//! API, even though the caller registered it through the same emulator.
//!
//! One directory under ONE shared type, for the same reason as the family's other two
//! ([`crate::chime_app_instance_admins`], [`crate::chime_tags`]): a copy inside either
//! service's own state struct would be invisible to the other, and the two would disagree
//! about what an identity is called.
//!
//! Nothing here names a wire shape: the directory holds an identity ARN and the name it
//! was registered under, and each reader projects that pair onto the shape ITS model
//! declares. The identity's own record stays the authority — this is the index the owning
//! service keeps in step with it on every write, so a rename is visible to the reader on
//! the next read and a deleted identity leaves no name behind.
//!
//! The directory is a record of what this emulator REGISTERED, so it is partial by
//! construction: an identity it holds no entry for simply has no name to report, and the
//! member is modelled optional. It is also BOUNDED — see [`MAX_NAMED_IDENTITIES`] — so a
//! caller looping on create cannot grow it without end; the oldest entry is dropped
//! first, which keeps the identities a live workflow just registered reportable.

use std::collections::{BTreeMap, VecDeque};

/// How many identity names the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so without a bound a caller registering identities in a loop would grow
/// it without end. Past it the OLDEST entry is dropped — never a newer one — so an
/// identity a running workflow just registered still reports its name, and the worst
/// consequence of an eviction is a very old identity reported with no name, which the
/// shape models as allowed.
pub const MAX_NAMED_IDENTITIES: usize = 10_000;

/// The directory itself: the name of every registered identity, keyed by its ARN.
///
/// `BTreeMap` (not `HashMap`) so a reader that walks the directory — dropping an app
/// instance's identities with it — walks them in a stable key order.
#[derive(Debug, Default)]
pub struct ChimeIdentityNameDirectory {
    names: BTreeMap<String, String>,
    /// The order the entries were first written in, so the oldest can be dropped first.
    order: VecDeque<String>,
}

impl ChimeIdentityNameDirectory {
    /// Publish `name` as the name of `identity_arn`, replacing the name the directory held
    /// for it.
    ///
    /// A rename is a REPLACEMENT, because the owning service's own record holds exactly one
    /// name at a time: re-registering an ARN keeps its position in the eviction order, so a
    /// rename cannot promote an old identity past a newer one. Past
    /// [`MAX_NAMED_IDENTITIES`] the oldest entry is dropped.
    pub fn register(&mut self, identity_arn: &str, name: &str) {
        if self
            .names
            .insert(identity_arn.to_string(), name.to_string())
            .is_none()
        {
            self.order.push_back(identity_arn.to_string());
            while self.order.len() > MAX_NAMED_IDENTITIES {
                if let Some(oldest) = self.order.pop_front() {
                    self.names.remove(&oldest);
                }
            }
        }
    }

    /// The name `identity_arn` is registered under, or `None` for an identity the
    /// directory holds no entry for.
    pub fn name_of(&self, identity_arn: &str) -> Option<&str> {
        self.names.get(identity_arn).map(String::as_str)
    }

    /// Drop `identity_arn`'s name — what the service that owns the identity calls when the
    /// identity itself is deleted, so a deleted identity leaves no name behind. Answers
    /// whether the directory held an entry for it.
    pub fn forget(&mut self, identity_arn: &str) -> bool {
        self.order.retain(|held| held != identity_arn);
        self.names.remove(identity_arn).is_some()
    }

    /// Drop the name of every identity that hangs off `app_instance_arn`, and answer how
    /// many were dropped.
    ///
    /// An identity's ARN is its app instance's ARN extended by a child segment, so deleting
    /// an app instance cascades to its identities here the way it cascades in the store that
    /// owns them.
    pub fn forget_identities_of(&mut self, app_instance_arn: &str) -> usize {
        let prefix = format!("{app_instance_arn}/");
        let dropped: Vec<String> = self
            .names
            .keys()
            .filter(|arn| arn.starts_with(&prefix))
            .cloned()
            .collect();
        for arn in &dropped {
            self.forget(arn);
        }
        dropped.len()
    }

    /// How many identities the directory currently holds a name for.
    pub fn named_identities(&self) -> usize {
        self.names.len()
    }
}

#[cfg(test)]
mod tests {
    //! The directory over what the hand-off generalises over: a name published and read
    //! back, a rename, an identity the directory never held, a deletion, an app instance's
    //! cascade, and the capacity bound.

    use super::*;

    const INSTANCE: &str = "arn:aws:chime:us-east-1:000000000000:app-instance/ai1";

    fn user(id: &str) -> String {
        format!("{INSTANCE}/user/{id}")
    }

    #[test]
    fn a_published_name_is_read_back_and_a_rename_replaces_it() {
        let mut directory = ChimeIdentityNameDirectory::default();

        // Rows: an ordinary name, the empty name the shape's `min 0` length allows, and a
        // name of non-ASCII text.
        for (case, id, name) in [
            ("an ordinary name", "u1", "Reporting Bot"),
            ("the empty name the shape allows", "u2", ""),
            ("a name of non-ASCII text", "u3", "Ría Ünicode"),
        ] {
            directory.register(&user(id), name);
            let actual = directory.name_of(&user(id));
            assert_eq!(
                actual,
                Some(name),
                "case {case:?}: expected {name:?}, got {actual:?}"
            );
        }

        directory.register(&user("u1"), "Renamed");
        let actual = directory.name_of(&user("u1"));
        assert_eq!(
            actual,
            Some("Renamed"),
            "case {:?}: expected the new name, got {actual:?}",
            "a rename"
        );

        let actual = directory.name_of(&user("never-registered"));
        assert_eq!(
            actual, None,
            "case {:?}: expected no name, got {actual:?}",
            "an identity the directory never held"
        );
    }

    #[test]
    fn deleting_an_identity_or_its_app_instance_leaves_no_name_behind() {
        let mut directory = ChimeIdentityNameDirectory::default();
        for id in ["u1", "u2"] {
            directory.register(&user(id), id);
        }
        let other_instance = "arn:aws:chime:us-east-1:000000000000:app-instance/ai2";
        directory.register(&format!("{other_instance}/user/u3"), "u3");

        assert!(
            directory.forget(&user("u1")),
            "case {:?}: expected the directory to have held an entry",
            "deleting a registered identity"
        );
        assert_eq!(
            directory.name_of(&user("u1")),
            None,
            "case {:?}: expected no name after the delete, got {:?}",
            "deleting a registered identity",
            directory.name_of(&user("u1"))
        );
        assert!(
            !directory.forget(&user("u1")),
            "case {:?}: expected no entry the second time",
            "deleting the same identity twice"
        );

        let dropped = directory.forget_identities_of(INSTANCE);
        assert_eq!(
            dropped, 1,
            "case {:?}: expected the one remaining identity of that instance dropped, got {dropped}",
            "deleting an app instance"
        );
        assert_eq!(
            directory.name_of(&format!("{other_instance}/user/u3")),
            Some("u3"),
            "case {:?}: expected another instance's identity kept, got {:?}",
            "deleting an app instance",
            directory.name_of(&format!("{other_instance}/user/u3"))
        );
    }

    #[test]
    fn the_directory_stays_inside_its_capacity_bound() {
        let mut directory = ChimeIdentityNameDirectory::default();
        for index in 0..=MAX_NAMED_IDENTITIES {
            directory.register(&user(&format!("u{index:06}")), "name");
        }

        assert_eq!(
            directory.named_identities(),
            MAX_NAMED_IDENTITIES,
            "case {:?}: expected the directory to stay at its bound, got {}",
            "one identity past capacity",
            directory.named_identities()
        );
        assert_eq!(
            directory.name_of(&user("u000000")),
            None,
            "case {:?}: expected the oldest entry dropped, got {:?}",
            "one identity past capacity",
            directory.name_of(&user("u000000"))
        );
        let newest = user(&format!("u{MAX_NAMED_IDENTITIES:06}"));
        assert_eq!(
            directory.name_of(&newest),
            Some("name"),
            "case {:?}: expected the newest entry kept, got {:?}",
            "one identity past capacity",
            directory.name_of(&newest)
        );
    }
}
