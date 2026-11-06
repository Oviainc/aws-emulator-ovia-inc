// SPDX-License-Identifier: CC0-1.0

//! The groups a directory holds because it IS a directory.
//!
//! Active Directory creates a domain's built-in groups when the domain is created, so a
//! directory's group inventory is never only the groups this API was asked to create:
//! `Domain Users`, `Domain Admins`, `Administrators` and the rest are there as soon as
//! the directory is. They are ordinary group objects — they are reported by the group
//! operations, their names are taken, and they carry the SIDs their domain gives them —
//! so they live in [`super::Directory::groups`] beside the groups `CreateGroup` stores,
//! and every handler that reads that map sees them without knowing they are built in.
//!
//! Each one's SID is the well-known one Microsoft documents for it (see "Security
//! identifiers" / "Well-known SIDs"), which is what makes it recognisable to a client:
//!
//! * a group of the domain carries the domain's own SID plus a reserved relative
//!   identifier (RID) below [`super::FIRST_RID`], so it can never collide with an object
//!   the directory mints;
//! * a group of the local `BUILTIN` authority carries `S-1-5-32-<rid>`, the same SID in
//!   every domain.
//!
//! A built-in group of the domain sits in the domain's `CN=Users` container and one of
//! the `BUILTIN` authority in `CN=Builtin`.

use std::collections::BTreeMap;

use super::{Directory, Group, DEFAULT_GROUP_TYPE};

/// Where a built-in group's SID comes from.
#[derive(Debug, Clone, Copy)]
pub enum Authority {
    /// A group of the domain: the directory's domain SID plus this reserved RID.
    Domain(u32),
    /// A group of the local `BUILTIN` authority: `S-1-5-32-<rid>`.
    Builtin(u32),
}

/// One built-in group: the name it answers to, the scope Active Directory gives it, and
/// where its SID comes from. Its `GroupType` is [`DEFAULT_GROUP_TYPE`] — every built-in
/// group is a security group, because each one exists to carry rights.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    pub name: &'static str,
    /// One of `GroupScope`'s modelled values.
    pub scope: &'static str,
    pub authority: Authority,
}

impl Builtin {
    /// A built-in group of the DOMAIN, carrying the domain's SID plus `rid`.
    const fn of_domain(name: &'static str, scope: &'static str, rid: u32) -> Self {
        Self {
            name,
            scope,
            authority: Authority::Domain(rid),
        }
    }

    /// A built-in group of the local `BUILTIN` authority, carrying `S-1-5-32-<rid>`.
    const fn of_builtin(name: &'static str, rid: u32) -> Self {
        Self {
            name,
            scope: BUILTIN_LOCAL,
            authority: Authority::Builtin(rid),
        }
    }
}

/// The `GroupScope` of a group that spans the whole forest.
const UNIVERSAL: &str = "Universal";
/// The `GroupScope` of a group that spans its own domain.
const GLOBAL: &str = "Global";
/// The `GroupScope` of a group whose rights apply inside its own domain.
const DOMAIN_LOCAL: &str = "DomainLocal";
/// The `GroupScope` of a group of the local `BUILTIN` authority.
const BUILTIN_LOCAL: &str = "BuiltinLocal";

/// The container a built-in group of the DOMAIN sits in.
const DOMAIN_CONTAINER: &str = "Users";
/// The container a built-in group of the `BUILTIN` authority sits in.
const BUILTIN_CONTAINER: &str = "Builtin";

/// Every built-in group of an Active Directory domain, with the well-known SID and the
/// group scope Microsoft documents for each ("Security identifiers": the domain-relative
/// RIDs below, and the `S-1-5-32-*` SIDs of the `BUILTIN` authority).
pub const CATALOG: &[Builtin] = &[
    // Groups of the domain itself: `<domain SID>-<RID>`.
    Builtin::of_domain("Enterprise Read-only Domain Controllers", UNIVERSAL, 498),
    Builtin::of_domain("Domain Admins", GLOBAL, 512),
    Builtin::of_domain("Domain Users", GLOBAL, 513),
    Builtin::of_domain("Domain Guests", GLOBAL, 514),
    Builtin::of_domain("Domain Computers", GLOBAL, 515),
    Builtin::of_domain("Domain Controllers", GLOBAL, 516),
    Builtin::of_domain("Cert Publishers", DOMAIN_LOCAL, 517),
    Builtin::of_domain("Schema Admins", UNIVERSAL, 518),
    Builtin::of_domain("Enterprise Admins", UNIVERSAL, 519),
    Builtin::of_domain("Group Policy Creator Owners", GLOBAL, 520),
    Builtin::of_domain("Read-only Domain Controllers", GLOBAL, 521),
    Builtin::of_domain("Cloneable Domain Controllers", GLOBAL, 522),
    Builtin::of_domain("Protected Users", GLOBAL, 525),
    Builtin::of_domain("Key Admins", GLOBAL, 526),
    Builtin::of_domain("Enterprise Key Admins", UNIVERSAL, 527),
    Builtin::of_domain("RAS and IAS Servers", DOMAIN_LOCAL, 553),
    Builtin::of_domain("Allowed RODC Password Replication Group", DOMAIN_LOCAL, 571),
    Builtin::of_domain("Denied RODC Password Replication Group", DOMAIN_LOCAL, 572),
    // Groups of the local `BUILTIN` authority: `S-1-5-32-<RID>`, all of them
    // `BuiltinLocal` in scope.
    Builtin::of_builtin("Administrators", 544),
    Builtin::of_builtin("Users", 545),
    Builtin::of_builtin("Guests", 546),
    Builtin::of_builtin("Account Operators", 548),
    Builtin::of_builtin("Server Operators", 549),
    Builtin::of_builtin("Print Operators", 550),
    Builtin::of_builtin("Backup Operators", 551),
    Builtin::of_builtin("Replicator", 552),
    Builtin::of_builtin("Pre-Windows 2000 Compatible Access", 554),
    Builtin::of_builtin("Remote Desktop Users", 555),
    Builtin::of_builtin("Network Configuration Operators", 556),
    Builtin::of_builtin("Incoming Forest Trust Builders", 557),
    Builtin::of_builtin("Performance Monitor Users", 558),
    Builtin::of_builtin("Performance Log Users", 559),
    Builtin::of_builtin("Windows Authorization Access Group", 560),
    Builtin::of_builtin("Terminal Server License Servers", 561),
    Builtin::of_builtin("Distributed COM Users", 562),
    Builtin::of_builtin("IIS_IUSRS", 568),
    Builtin::of_builtin("Cryptographic Operators", 569),
    Builtin::of_builtin("Event Log Readers", 573),
    Builtin::of_builtin("Certificate Service DCOM Access", 574),
    Builtin::of_builtin("RDS Remote Access Servers", 575),
    Builtin::of_builtin("RDS Endpoint Servers", 576),
    Builtin::of_builtin("RDS Management Servers", 577),
    Builtin::of_builtin("Hyper-V Administrators", 578),
    Builtin::of_builtin("Access Control Assistance Operators", 579),
    Builtin::of_builtin("Remote Management Users", 580),
];

/// How many group records [`seed`] puts in a directory: the catalogue's length, fixed
/// at compile time, so what a materialised directory costs is a constant and no part of
/// a request chooses it.
const COUNT: usize = CATALOG.len();

/// Put every group of [`CATALOG`] into `directory`, which is what a domain holds the
/// moment it is created. Called once, where the directory record is built, so no later
/// read has to know a group is built in.
pub(super) fn seed(directory: &mut Directory) {
    for builtin in CATALOG {
        let (sid, container) = match builtin.authority {
            Authority::Domain(rid) => (format!("{}-{rid}", directory.domain_sid), DOMAIN_CONTAINER),
            Authority::Builtin(rid) => (format!("S-1-5-32-{rid}"), BUILTIN_CONTAINER),
        };
        let group = Group {
            sam_account_name: builtin.name.to_string(),
            sid,
            distinguished_name: directory.distinguished_name_in(container, builtin.name),
            group_type: DEFAULT_GROUP_TYPE.to_string(),
            group_scope: builtin.scope.to_string(),
            other_attributes: BTreeMap::new(),
            // Which principals a built-in group holds is the domain's business; in this
            // data plane a member reaches one through `AddGroupMember`, or through the
            // primary-group membership a created user gets
            // ([`super::Directory::join_primary_group`]).
            members: BTreeMap::new(),
        };
        directory.groups.insert(super::fold(builtin.name), group);
    }
    // One name, one group: a catalogue holding a name twice would silently seed one
    // group fewer than it lists.
    debug_assert_eq!(
        directory.groups.len(),
        COUNT,
        "every built-in group must answer to its own name"
    );
}
