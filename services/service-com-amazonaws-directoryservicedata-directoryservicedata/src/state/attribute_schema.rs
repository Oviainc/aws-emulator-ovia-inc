// SPDX-License-Identifier: CC0-1.0

//! The ATTRIBUTE NAMES a directory holds an object's `OtherAttributes` under.
//!
//! An attribute name is a name of the directory's SCHEMA, not a string a request chose: a
//! caller may write it in any case ("Attribute names are case insensitive" —
//! `smithy_query(query="doc", shape="CreateUserRequest", member="OtherAttributes")`), and
//! the directory resolves it against the schema and holds the value under the SCHEMA's own
//! spelling. So two callers writing one attribute in two cases write ONE attribute, and
//! every read of it reports the one name the schema gives it rather than whichever case the
//! last writer happened to send.
//!
//! [`CATALOG`] is that schema, as far as this plane needs it: the `lDAPDisplayName` of each
//! attribute of an Active Directory user or group object that this API carries in
//! `OtherAttributes` — the attributes its own documented attribute list covers (the list
//! `CreateUserRequest$OtherAttributes` links), minus the ones the model carries as members of
//! their own (`SAMAccountName`, `GivenName`, `Surname`, `EmailAddress`, `DistinguishedName`,
//! `UserPrincipalName`, `SID`, `GroupType`, `GroupScope`), which are reported through those
//! members and never through the attribute map.
//!
//! The name this API REPORTS an attribute under is that schema name with its leading letter
//! in upper case, which is how this API spells every attribute name it states: the members it
//! models for an attribute of its own are `GivenName` for `givenName`, `Surname` for `sn` and
//! `EmailAddress` for `mail`, and an attribute it carries in the map is spelled the same way.
//!
//! An attribute OUTSIDE the catalogue is reported exactly as it was written: this plane has
//! no schema entry to resolve it against, and inventing a spelling for a name the schema does
//! not hold would report an attribute under a name no directory gives it.

/// The `lDAPDisplayName` of every attribute this plane resolves a requested attribute name
/// against, as the Active Directory schema spells it.
///
/// Keyed by nothing: it is read through [`canonical_name`], which folds both sides, so the
/// order here is only the order a reader of this file reads them in (alphabetical).
pub const CATALOG: &[&str] = &[
    "co",
    "codePage",
    "company",
    "countryCode",
    "department",
    "description",
    "displayName",
    "division",
    "employeeID",
    "employeeNumber",
    "employeeType",
    "facsimileTelephoneNumber",
    "homeDirectory",
    "homeDrive",
    "homePhone",
    "info",
    "initials",
    "ipPhone",
    "l",
    "managedBy",
    "manager",
    "middleName",
    "mobile",
    "otherTelephone",
    "pager",
    "physicalDeliveryOfficeName",
    "postOfficeBox",
    "postalCode",
    "preferredLanguage",
    "servicePrincipalName",
    "st",
    "streetAddress",
    "telephoneNumber",
    "title",
    "url",
    "wWWHomePage",
];

/// The name this API reports the attribute `name` under, or `None` when the schema holds no
/// attribute of that name.
///
/// The name is resolved case insensitively, because that is how a directory resolves an
/// attribute name, and the answer is the SCHEMA's spelling of it as this API states one: the
/// `lDAPDisplayName` with its leading letter in upper case.
#[must_use]
pub fn canonical_name(name: &str) -> Option<String> {
    let folded = super::fold(name);
    CATALOG
        .iter()
        .find(|held| super::fold(held) == folded)
        .map(|held| reported_name(held))
}

/// The name of a stored attribute, resolved against the schema: its schema spelling when the
/// schema holds it, and otherwise the name as it was written.
#[must_use]
pub fn resolved_name(name: &str) -> String {
    canonical_name(name).unwrap_or_else(|| name.to_string())
}

/// A schema name as this API states it: the leading letter in upper case, the rest of the
/// name as the schema spells it (`description` → `Description`, `wWWHomePage` →
/// `WWWHomePage`).
fn reported_name(schema_name: &str) -> String {
    let mut characters = schema_name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_name, resolved_name, CATALOG};

    /// RULE: a name of the schema resolves, in any case, to the schema's own spelling of it.
    #[test]
    fn every_catalogued_attribute_resolves_to_one_reported_name() {
        for schema_name in CATALOG {
            let reported = resolved_name(schema_name);
            for spelling in [
                schema_name.to_string(),
                schema_name.to_uppercase(),
                schema_name.to_lowercase(),
            ] {
                assert_eq!(
                    resolved_name(&spelling),
                    reported,
                    "case {spelling:?}: every spelling of a catalogued attribute must resolve \
                     to one name, got {:?}",
                    resolved_name(&spelling)
                );
            }
            assert!(
                reported
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase),
                "case {schema_name:?}: the reported name must lead with an upper-case letter, \
                 got {reported:?}"
            );
        }
    }

    /// RULE: a name the schema does not hold resolves to nothing and is reported as written.
    #[test]
    fn a_name_outside_the_schema_is_reported_as_written() {
        for name in ["notAnAttribute", "x-custom", "describe"] {
            assert_eq!(
                canonical_name(name),
                None,
                "case {name:?}: expected no schema entry, got {:?}",
                canonical_name(name)
            );
            assert_eq!(
                resolved_name(name),
                name,
                "case {name:?}: expected the name as written, got {:?}",
                resolved_name(name)
            );
        }
    }
}
