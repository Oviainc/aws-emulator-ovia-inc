// SPDX-License-Identifier: CC0-1.0

//! Which DOMAIN a Directory Service Data GROUP projection answers for, across the two
//! services that share the directory.
//!
//! `ds:CreateDirectory` and `ds:CreateMicrosoftAD` are where a directory's fully qualified
//! domain name is chosen — the `Name` the create carries, which `ds:DescribeDirectories`
//! reports back — and the Directory Service DATA plane (`ds-data`) creates no directory of
//! its own. So the realm every `ds-data` projection reports is knowable there only because
//! the control plane recorded it, and the rule these rows pin is that derivation: a
//! realm-valued member answers with the domain name the create of THAT directory was given,
//! whatever that name is, so two directories of one account report two domains.
//!
//! The sibling suite `ds_data_plane_access_and_realm` pins this for the USER projections of
//! a Managed Microsoft AD; these rows cover the GROUP projections — `SearchGroups`,
//! `ListGroups`, `DescribeGroup`, `ListGroupMembers`, `ListGroupsForMember` — and the second
//! create that takes a domain name, `CreateDirectory`, so neither the object kind nor the
//! create that minted the directory can decide the realm.
//!
//! The harness is the one `directory_data_plane_authorization` exports — one emulator, one
//! control-plane call, one data-plane call — so every cross-plane suite addresses the two
//! planes the same way.

use serde_json::{json, Value};

use crate::directory_data_plane_authorization::{
    create_microsoft_ad, Answer, Emulator, PAST_ONE_SPAN,
};
use crate::ds_data_plane_access_and_realm::domain_components;

/// The group and user each directory's data plane is given, inside `GroupName`'s and
/// `UserName`'s patterns.
const GROUP: &str = "realmGroup";
const USER: &str = "realmUser";

/// One row: a directory of this account, the control-plane create that mints it and the
/// domain name that create is given.
struct Row {
    case: &'static str,
    operation: &'static str,
    name: &'static str,
}

/// The table: both creates that take a domain name, over the forms `DirectoryName` admits
/// (`^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$` —
/// `smithy_query(query="shape", shape="DirectoryName")`).
const ROWS: [Row; 4] = [
    Row {
        case: "a Managed Microsoft AD, three dotted labels",
        operation: "CreateMicrosoftAD",
        name: "corp.example.com",
    },
    Row {
        case: "a Managed Microsoft AD, hyphens inside the labels",
        operation: "CreateMicrosoftAD",
        name: "ad-east.sim-corp.example.net",
    },
    Row {
        case: "a Simple AD, upper case",
        operation: "CreateDirectory",
        name: "SALES.EXAMPLE.ORG",
    },
    Row {
        case: "a Simple AD, five labels",
        operation: "CreateDirectory",
        name: "ad.eu.west.example.com",
    },
];

/// The create body for a row: the `CreateMicrosoftAD` body the harness exports, plus the
/// `Size` member `CreateDirectory` declares `@required` and `CreateMicrosoftAD` does not
/// (that one carries an `Edition` its size is derived from instead).
fn create_body(row: &Row) -> Value {
    let mut body = create_microsoft_ad(row.name);
    if row.operation == "CreateDirectory" {
        body["Size"] = json!("Small");
    }
    body
}

/// The domain name the control plane reports for `directory`.
async fn described_name(emulator: &Emulator, directory: &str) -> String {
    let listed = emulator
        .control_plane("DescribeDirectories", json!({"DirectoryIds": [directory]}))
        .await;
    listed.body["DirectoryDescriptions"][0]["Name"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How many entries a list member of an answer holds.
fn count(answer: &Answer, member: &str) -> usize {
    answer.body[member].as_array().map_or(0, Vec::len)
}

/// RULE: every realm-valued member the data plane reports for a directory is the domain name
/// the control-plane create of that directory was given, and the names formed inside the
/// domain follow it.
#[tokio::test]
async fn the_group_projections_realm_is_the_name_the_create_was_given() {
    let emulator = Emulator::new();

    // Every row's directory exists first, so no row can pass by serving "the only one".
    let mut created: Vec<(&Row, String)> = Vec::new();
    for row in &ROWS {
        let answer = emulator.control_plane(row.operation, create_body(row)).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: {} must be accepted, got {} with {:?}",
            row.case, row.operation, answer.status, answer.body
        );
        let directory = answer.text("DirectoryId");
        assert!(
            !directory.is_empty(),
            "case {:?}: the create must answer a DirectoryId, got {:?}",
            row.case,
            answer.body
        );
        created.push((row, directory));
    }

    // The data plane answers for a directory only while the control plane's DS Data feature
    // is on for it, and a create leaves it off: the stage transition each create owes has to
    // fall due before the enable is admitted, and the enable's own transition before the
    // data plane serves anything.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, directory) in &created {
        let enabled = emulator
            .control_plane(
                "EnableDirectoryDataAccess",
                json!({"DirectoryId": directory}),
            )
            .await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: EnableDirectoryDataAccess must be accepted, got {} with {:?}",
            row.case, enabled.status, enabled.body
        );
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, directory) in &created {
        let case = row.case;
        let name = row.name;

        // The plane that OWNS the directory reports that name: what follows is about a
        // directory that really carries it.
        let reported = described_name(&emulator, directory).await;
        assert_eq!(
            reported, name,
            "case {case:?}: DescribeDirectories must report the created name, got {reported:?}"
        );

        for (route, body) in [
            ("/Groups/CreateGroup", json!({"SAMAccountName": GROUP})),
            ("/Users/CreateUser", json!({"SAMAccountName": USER})),
            (
                "/GroupMemberships/AddGroupMember",
                json!({"GroupName": GROUP, "MemberName": USER}),
            ),
        ] {
            let answer = emulator.data_plane(route, directory, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: POST {route} must be served, got {} with {:?}",
                answer.status, answer.body
            );
        }

        // Every GROUP projection that reports a realm reports THIS directory's domain.
        let projections: [(&str, Value, &str); 7] = [
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
                "/GroupMemberships/ListGroupMembers",
                json!({"SAMAccountName": GROUP}),
                "MemberRealm",
            ),
            (
                "/GroupMemberships/ListGroupsForMember",
                json!({"SAMAccountName": USER}),
                "Realm",
            ),
            (
                "/GroupMemberships/ListGroupsForMember",
                json!({"SAMAccountName": USER}),
                "MemberRealm",
            ),
        ];
        for (route, body, member) in projections {
            let answer = emulator.data_plane(route, directory, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: POST {route} must be served, got {} with {:?}",
                answer.status, answer.body
            );
            let realm = answer.text(member);
            assert_eq!(
                realm, name,
                "case {case:?}: POST {route} must report {member} {name:?}, got {realm:?}"
            );
        }

        // A group is named inside that same domain. What this rule decides is the DOMAIN
        // half of the name — which container an object sits in is not this rule's — so the
        // suffix is what is asserted, beside the object the name names.
        let described = emulator
            .data_plane(
                "/Groups/DescribeGroup",
                directory,
                json!({"SAMAccountName": GROUP}),
            )
            .await;
        let distinguished_name = described.text("DistinguishedName");
        assert!(
            distinguished_name.starts_with(&format!("CN={GROUP},"))
                && distinguished_name.ends_with(&domain_components(name)),
            "case {case:?}: the group must be named inside {name:?}, got {distinguished_name:?}"
        );

        // A group the domain comes with is named inside it too, so the derivation is the
        // directory's and not something a create of this plane carried.
        let builtin = emulator
            .data_plane(
                "/Groups/DescribeGroup",
                directory,
                json!({"SAMAccountName": "Administrators"}),
            )
            .await;
        let builtin_name = builtin.text("DistinguishedName");
        assert!(
            builtin_name.starts_with("CN=Administrators,")
                && builtin_name.ends_with(&domain_components(name)),
            "case {case:?}: a built-in group must be named inside {name:?}, got {builtin_name:?}"
        );

        // The directory's own domain is the domain a request may name, and another
        // directory's domain holds none of its objects.
        let own = emulator
            .data_plane(
                "/Groups/SearchGroups",
                directory,
                json!({
                    "SearchString": GROUP,
                    "SearchAttributes": ["SAMAccountName"],
                    "Realm": name,
                }),
            )
            .await;
        assert_eq!(
            count(&own, "Groups"),
            1,
            "case {case:?}: naming its own domain must search this directory, got {:?}",
            own.body
        );
        let foreign_name = ROWS
            .iter()
            .find(|other| other.name != row.name)
            .expect("the table holds more than one directory")
            .name;
        let foreign = emulator
            .data_plane(
                "/Groups/SearchGroups",
                directory,
                json!({
                    "SearchString": GROUP,
                    "SearchAttributes": ["SAMAccountName"],
                    "Realm": foreign_name,
                }),
            )
            .await;
        assert_eq!(
            count(&foreign, "Groups"),
            0,
            "case {case:?}: domain {foreign_name:?} holds no group of this directory, got {:?}",
            foreign.body
        );
    }

    // No two directories report one domain: the realm is the directory's own, not a value
    // this emulator chose.
    let mut realms: Vec<String> = Vec::new();
    for (_, directory) in &created {
        let listed = emulator
            .data_plane("/Groups/ListGroups", directory, json!({}))
            .await;
        realms.push(listed.text("Realm"));
    }
    let mut distinct = realms.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        created.len(),
        "expected one domain per directory, got {realms:?}"
    );
}
