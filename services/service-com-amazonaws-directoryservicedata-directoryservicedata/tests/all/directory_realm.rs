// SPDX-License-Identifier: CC0-1.0

//! WHICH AD DOMAIN a Directory Service Data answer is about: the realm every projection of
//! this data plane reports.
//!
//! The rule under test: a realm-valued member answers with the Active Directory domain name
//! of the directory the request names — the `Name` the control plane that created that
//! directory was given, which this plane reads from the shared directory inventory — so two
//! directories of one account report two domains and no answer carries a domain this
//! emulator chose. Every name formed inside that domain follows it: an object's
//! `DistinguishedName` carries the domain's `DC=` components and a user's
//! `UserPrincipalName` the domain after the `@`.
//!
//! The rows below span the forms a domain name takes (`Realm`:
//! `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`, len 1..255 —
//! `smithy_query(query="shape", shape="Realm")`): label counts, hyphens inside a label,
//! upper case, a hyphen-separated name with no dot, and a name at the published `@length`
//! maximum. Each row is one directory of the account, registered the way a control-plane
//! create registers one; every assertion goes through the router, so each case exercises
//! routing, deserialization, the handler, the persisted state and serialization.
//!
//! `Realm` as a REQUEST member is matched against that same domain: a request naming the
//! directory's own domain (in any case) is served, and one naming another directory's domain
//! is asking about a domain this directory holds no objects in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, domain_components, hold_directory_in_domain, Answer};

/// `Realm`'s `@length` maximum (`smithy_query(query="shape", shape="Realm")`).
const REALM_CEILING: usize = 255;

/// The group every row creates, and the user it creates: names inside `GroupName`'s and
/// `UserName`'s patterns.
const GROUP: &str = "realmGroup";
const USER: &str = "realmUser";

/// One row: a directory of this account and the domain name its create was given.
struct Row {
    case: &'static str,
    directory: &'static str,
    domain: String,
}

/// The table: one directory per form a domain name takes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a three-label dotted domain",
            directory: "d-0000000001",
            domain: "corp.example.com".to_string(),
        },
        Row {
            case: "labels carrying hyphens",
            directory: "d-0000000002",
            domain: "eng-west-1.ad-sites.example.net".to_string(),
        },
        Row {
            case: "an upper-case domain",
            directory: "d-0000000003",
            domain: "ADCORP.EXAMPLE.ORG".to_string(),
        },
        Row {
            case: "five labels",
            directory: "d-0000000004",
            domain: "ad.eu.west.example.com".to_string(),
        },
        Row {
            case: "a hyphen-separated name with no dot",
            directory: "d-0000000005",
            domain: "corp-example".to_string(),
        },
        Row {
            case: "a domain at Realm's @length maximum",
            directory: "d-0000000006",
            domain: longest_domain(),
        },
    ]
}

/// A domain name exactly [`REALM_CEILING`] characters long: labels of 15 characters plus a
/// last label padded to land on the bound.
fn longest_domain() -> String {
    let label = "abcdefghijklmno";
    let mut domain = String::new();
    // Each label plus its separating dot, while a last label of at least one character
    // still fits inside the bound.
    while domain.len() + label.len() < REALM_CEILING {
        domain.push_str(label);
        domain.push('.');
    }
    domain.push_str(&"z".repeat(REALM_CEILING - domain.len()));
    assert_eq!(
        domain.len(),
        REALM_CEILING,
        "the longest domain row must sit on Realm's @length maximum, got {} characters",
        domain.len()
    );
    domain
}

/// One emulator holding every row's directory, each under its OWN domain name — exactly what
/// a control-plane create leaves in the shared inventory.
fn emulator(rows: &[Row]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for row in rows {
        hold_directory_in_domain(&engine, row.directory, &row.domain);
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A string member of an answer, or the empty string when it carries none.
fn text(answer: &Answer, member: &str) -> String {
    answer.body[member].as_str().unwrap_or_default().to_string()
}

/// How many entries a list member of an answer holds.
fn count(answer: &Answer, member: &str) -> usize {
    answer.body[member].as_array().map_or(0, Vec::len)
}

/// RULE: every realm-valued member of every projection reports the domain name of the
/// directory the request names, and the names formed inside that domain follow it.
#[tokio::test]
async fn every_projection_reports_the_directory_s_own_domain() {
    let rows = rows();
    let router = emulator(&rows);

    for row in &rows {
        let case = row.case;
        let domain = row.domain.as_str();
        let directory = row.directory;

        let created_group = call(
            &router,
            "/Groups/CreateGroup",
            directory,
            &json!({"SAMAccountName": GROUP}),
        )
        .await;
        assert_eq!(
            created_group.status, 200,
            "case {case:?}: CreateGroup must be served, got {} with {:?}",
            created_group.status, created_group.body
        );
        let created_user = call(
            &router,
            "/Users/CreateUser",
            directory,
            &json!({"SAMAccountName": USER}),
        )
        .await;
        assert_eq!(
            created_user.status, 200,
            "case {case:?}: CreateUser must be served, got {} with {:?}",
            created_user.status, created_user.body
        );
        let added = call(
            &router,
            "/GroupMemberships/AddGroupMember",
            directory,
            &json!({"GroupName": GROUP, "MemberName": USER}),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: AddGroupMember must be served, got {} with {:?}",
            added.status, added.body
        );

        // Every projection that reports a realm, with the member it reports it in.
        let projections: [(&str, Value, &str); 8] = [
            (
                "/Groups/SearchGroups",
                json!({"SearchString": GROUP, "SearchAttributes": ["SAMAccountName"]}),
                "Realm",
            ),
            ("/Groups/ListGroups", json!({}), "Realm"),
            (
                "/Groups/DescribeGroup",
                json!({"SAMAccountName": GROUP}),
                "Realm",
            ),
            (
                "/GroupMemberships/ListGroupMembers",
                json!({"SAMAccountName": GROUP}),
                "Realm",
            ),
            (
                "/GroupMemberships/ListGroupsForMember",
                json!({"SAMAccountName": USER}),
                "Realm",
            ),
            (
                "/Users/SearchUsers",
                json!({"SearchString": USER, "SearchAttributes": ["SAMAccountName"]}),
                "Realm",
            ),
            ("/Users/ListUsers", json!({}), "Realm"),
            (
                "/Users/DescribeUser",
                json!({"SAMAccountName": USER}),
                "Realm",
            ),
        ];
        for (route, body, member) in projections {
            let answer = call(&router, route, directory, &body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: POST {route} must be served, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = text(&answer, member);
            assert_eq!(
                reported, domain,
                "case {case:?}: POST {route} must report {member} {domain:?}, got {reported:?}"
            );
        }

        // `MemberRealm` names the domain the MEMBERS are in, which is this directory's own.
        for (route, body) in [
            (
                "/GroupMemberships/ListGroupMembers",
                json!({"SAMAccountName": GROUP}),
            ),
            (
                "/GroupMemberships/ListGroupsForMember",
                json!({"SAMAccountName": USER}),
            ),
        ] {
            let answer = call(&router, route, directory, &body).await;
            let reported = text(&answer, "MemberRealm");
            assert_eq!(
                reported, domain,
                "case {case:?}: POST {route} must report MemberRealm {domain:?}, \
                 got {reported:?}"
            );
        }

        // The names formed inside the domain: the `DC=` suffix of a distinguished name and
        // the realm of a user principal name. What the rule decides is the DOMAIN half of
        // each name — which container an object sits in is not this rule's — so the suffix is
        // what is asserted, beside the object the name names.
        let described_user = call(
            &router,
            "/Users/DescribeUser",
            directory,
            &json!({"SAMAccountName": USER}),
        )
        .await;
        let user_dn = text(&described_user, "DistinguishedName");
        assert!(
            user_dn.starts_with(&format!("CN={USER},"))
                && user_dn.ends_with(&domain_components(domain)),
            "case {case:?}: the user's distinguished name must name {USER:?} inside \
             {domain:?}, got {user_dn:?}"
        );
        assert_eq!(
            text(&described_user, "UserPrincipalName"),
            format!("{USER}@{domain}"),
            "case {case:?}: the user principal name must name the account in {domain:?}, \
             got {:?}",
            described_user.body
        );
        let described_group = call(
            &router,
            "/Groups/DescribeGroup",
            directory,
            &json!({"SAMAccountName": GROUP}),
        )
        .await;
        let group_dn = text(&described_group, "DistinguishedName");
        assert!(
            group_dn.starts_with(&format!("CN={GROUP},"))
                && group_dn.ends_with(&domain_components(domain)),
            "case {case:?}: the group's distinguished name must name {GROUP:?} inside \
             {domain:?}, got {group_dn:?}"
        );

        // A group the domain comes with is named inside the same domain.
        let described_builtin = call(
            &router,
            "/Groups/DescribeGroup",
            directory,
            &json!({"SAMAccountName": "Administrators"}),
        )
        .await;
        let builtin_dn = text(&described_builtin, "DistinguishedName");
        assert!(
            builtin_dn.starts_with("CN=Administrators,")
                && builtin_dn.ends_with(&domain_components(domain)),
            "case {case:?}: a built-in group must be named inside {domain:?}, got {builtin_dn:?}"
        );

        // The directory's own domain, in any case, is the domain a request may name: a realm
        // is case insensitive.
        for spelling in [
            domain.to_string(),
            domain.to_uppercase(),
            domain.to_lowercase(),
        ] {
            let answer = call(
                &router,
                "/Groups/SearchGroups",
                directory,
                &json!({
                    "SearchString": GROUP,
                    "SearchAttributes": ["SAMAccountName"],
                    "Realm": spelling.clone(),
                }),
            )
            .await;
            assert_eq!(
                count(&answer, "Groups"),
                1,
                "case {case:?} Realm={spelling:?}: the directory's own domain must be \
                 searched, got {:?}",
                answer.body
            );
        }
    }

    // No two directories report one domain: the realm is the directory's, not a constant.
    let mut reported: Vec<String> = Vec::new();
    for row in &rows {
        let listed = call(&router, "/Groups/ListGroups", row.directory, &json!({})).await;
        reported.push(text(&listed, "Realm"));
    }
    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        rows.len(),
        "expected one domain per directory, got {reported:?}"
    );

    // Another directory's domain is a domain this directory holds no objects in.
    for row in &rows {
        let foreign = rows
            .iter()
            .find(|other| other.directory != row.directory)
            .expect("the table holds more than one directory")
            .domain
            .clone();
        let searched = call(
            &router,
            "/Groups/SearchGroups",
            row.directory,
            &json!({
                "SearchString": GROUP,
                "SearchAttributes": ["SAMAccountName"],
                "Realm": foreign.clone(),
            }),
        )
        .await;
        assert_eq!(
            searched.status, 200,
            "case {:?}: searching another domain must be accepted, got {} with {:?}",
            row.case, searched.status, searched.body
        );
        assert_eq!(
            count(&searched, "Groups"),
            0,
            "case {:?}: domain {foreign:?} holds no group of this directory, got {:?}",
            row.case,
            searched.body
        );
        assert_eq!(
            text(&searched, "Realm"),
            foreign,
            "case {:?}: the answer must report the domain it was asked for, got {:?}",
            row.case,
            searched.body
        );

        let described = call(
            &router,
            "/Groups/DescribeGroup",
            row.directory,
            &json!({"SAMAccountName": GROUP, "Realm": foreign.clone()}),
        )
        .await;
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {:?}: a group of domain {foreign:?} is not held here, got {} with {:?}",
            row.case,
            described.status,
            described.body
        );
    }
}

/// RULE: a read reports the directory's domain whether or not anything was ever written to
/// that directory — the domain is the control plane's record, not something a write invents.
#[tokio::test]
async fn an_unwritten_directory_still_reports_its_own_domain() {
    let rows = rows();
    let router = emulator(&rows);

    for row in &rows {
        for (route, body) in [
            ("/Groups/ListGroups", json!({})),
            ("/Users/ListUsers", json!({})),
            (
                "/Groups/SearchGroups",
                json!({"SearchString": GROUP, "SearchAttributes": ["SAMAccountName"]}),
            ),
            (
                "/Users/SearchUsers",
                json!({"SearchString": USER, "SearchAttributes": ["SAMAccountName"]}),
            ),
        ] {
            let answer = call(&router, route, row.directory, &body).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: POST {route} must be served, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            let reported = text(&answer, "Realm");
            assert_eq!(
                reported, row.domain,
                "case {:?}: POST {route} on a directory nothing was written to must report \
                 {:?}, got {reported:?}",
                row.case, row.domain
            );
        }
    }
}
