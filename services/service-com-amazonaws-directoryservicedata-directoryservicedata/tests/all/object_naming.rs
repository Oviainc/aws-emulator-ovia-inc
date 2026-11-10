// SPDX-License-Identifier: CC0-1.0

//! What a Directory Service Data object is NAMED, end to end over HTTP: the distinguished
//! name and the principal name the directory derives for an object the account creates, and
//! the attribute names it holds that object's `OtherAttributes` under.
//!
//! Three rules, all of them the DIRECTORY's and none of them the request's — `CreateUser`
//! and `CreateGroup` send no distinguished name, no principal name and no attribute
//! spelling:
//!
//! * an object this API creates is named inside the organizational unit the managed
//!   directory DELEGATES to the account — `CN=<SAMAccountName>,OU=Users,OU=<the NetBIOS
//!   short name of the directory>,<the `DC=` components of its domain name>` — because a
//!   managed directory gives the account no write access to the domain's own containers;
//! * a user's `UserPrincipalName` is its account name in the directory's domain with the
//!   domain in UPPER CASE, which is how a Kerberos realm is written;
//! * an attribute is held under the name the directory's SCHEMA gives it, resolved from the
//!   requested name case insensitively ("Attribute names are case insensitive" —
//!   `smithy_query(query="doc", shape="CreateUserRequest", member="OtherAttributes")`), so
//!   the case a request wrote is not the case a read reports.
//!
//! And the fourth rule here is how this API states a `ResourceNotFoundException`: the CLASS
//! of object asked about and the directory it was asked about in, with no object name.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. The rows vary what
//! each rule generalises over: the domain name's label count and form, whether the create
//! recorded a NetBIOS short name and in which case, the attribute's own spelling, and the
//! class of object a refusal is about.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, domain_components, hold_directory_named, Answer};

/// The user every naming row creates, and the group beside it: names inside `UserName`'s
/// and `GroupName`'s patterns.
const USER: &str = "namedUser";
const GROUP: &str = "namedGroup";

/// A group the domain comes with, which the directory created in a container of its own.
const BUILTIN_GROUP: &str = "Administrators";

/// One row of the naming table: a directory of this account, the domain name its create was
/// given, the NetBIOS short name that create recorded (`None` when it recorded none) and the
/// organizational unit the directory therefore delegates.
struct Row {
    case: &'static str,
    directory: &'static str,
    domain: &'static str,
    short_name: Option<&'static str>,
    /// The `OU=` the objects of this directory sit under: the recorded short name, or the
    /// default a create that names none gets — the first label of the domain name in upper
    /// case (`CreateMicrosoftADRequest$ShortName`: "If you don't specify a NetBIOS name, it
    /// will default to the first part of your directory DNS").
    delegated_ou: &'static str,
}

/// The table: one directory per form the two names of a create take.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a short name that is not a label of the domain name",
            directory: "d-0000000101",
            domain: "ad-behav-1105d345e209.example.com",
            short_name: Some("SIM2REAL"),
            delegated_ou: "SIM2REAL",
        },
        Row {
            case: "a short name recorded in lower case",
            directory: "d-0000000102",
            domain: "corp.example.com",
            short_name: Some("corpnb"),
            delegated_ou: "corpnb",
        },
        Row {
            case: "a create that recorded no short name",
            directory: "d-0000000103",
            domain: "eng-west-1.ad-sites.example.net",
            short_name: None,
            delegated_ou: "ENG-WEST-1",
        },
        Row {
            case: "a two-label domain",
            directory: "d-0000000104",
            domain: "example.org",
            short_name: Some("EXAMPLE"),
            delegated_ou: "EXAMPLE",
        },
        Row {
            case: "a five-label domain",
            directory: "d-0000000105",
            domain: "ad.eu.west.example.com",
            short_name: Some("ADEU"),
            delegated_ou: "ADEU",
        },
        Row {
            case: "an upper-case domain, no short name recorded",
            directory: "d-0000000106",
            domain: "ADCORP.EXAMPLE.ORG",
            short_name: None,
            delegated_ou: "ADCORP",
        },
        Row {
            case: "a hyphen-separated domain with no dot",
            directory: "d-0000000107",
            domain: "corp-example",
            short_name: None,
            delegated_ou: "CORP-EXAMPLE",
        },
    ]
}

/// One emulator holding every row's directory, registered the way a control-plane create
/// registers one: with the domain name and the NetBIOS short name that create was given.
fn emulator(rows: &[Row]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for row in rows {
        hold_directory_named(&engine, row.directory, row.domain, row.short_name);
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

/// A served answer, or a panic naming the case and what came back instead.
fn served(case: &str, what: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {what} must be served, got {} with {:?}",
        answer.status, answer.body
    );
}

/// RULE: an object this API creates is named inside the organizational unit the directory
/// delegates to the account, and a user's principal name is its account name in the
/// directory's domain, upper-cased.
#[tokio::test]
async fn a_created_object_is_named_inside_the_delegated_organizational_unit() {
    let rows = rows();
    let router = emulator(&rows);

    for row in &rows {
        let case = row.case;
        let directory = row.directory;
        let components = domain_components(row.domain);
        let expected_user_dn = format!(
            "CN={USER},OU=Users,OU={},{components}",
            row.delegated_ou
        );
        let expected_group_dn = format!(
            "CN={GROUP},OU=Users,OU={},{components}",
            row.delegated_ou
        );

        served(
            case,
            "CreateUser",
            &call(
                &router,
                "/Users/CreateUser",
                directory,
                &json!({"SAMAccountName": USER}),
            )
            .await,
        );
        served(
            case,
            "CreateGroup",
            &call(
                &router,
                "/Groups/CreateGroup",
                directory,
                &json!({"SAMAccountName": GROUP}),
            )
            .await,
        );

        let described_user = call(
            &router,
            "/Users/DescribeUser",
            directory,
            &json!({"SAMAccountName": USER}),
        )
        .await;
        served(case, "DescribeUser", &described_user);
        assert_eq!(
            text(&described_user, "DistinguishedName"),
            expected_user_dn,
            "case {case:?}: expected {expected_user_dn:?}, got {:?}",
            described_user.body
        );
        assert_eq!(
            text(&described_user, "UserPrincipalName"),
            format!("{USER}@{}", row.domain.to_uppercase()),
            "case {case:?}: the principal name must name the account in the domain in upper \
             case, got {:?}",
            described_user.body
        );
        assert_eq!(
            text(&described_user, "Realm"),
            row.domain,
            "case {case:?}: expected the directory's own domain, got {:?}",
            described_user.body
        );

        let described_group = call(
            &router,
            "/Groups/DescribeGroup",
            directory,
            &json!({"SAMAccountName": GROUP}),
        )
        .await;
        served(case, "DescribeGroup", &described_group);
        assert_eq!(
            text(&described_group, "DistinguishedName"),
            expected_group_dn,
            "case {case:?}: expected {expected_group_dn:?}, got {:?}",
            described_group.body
        );

        // A group the DOMAIN created sits in a container of the domain's own, not in the
        // unit the directory delegates to the account: the rule is about the objects this
        // API creates.
        let described_builtin = call(
            &router,
            "/Groups/DescribeGroup",
            directory,
            &json!({"SAMAccountName": BUILTIN_GROUP}),
        )
        .await;
        served(case, "DescribeGroup of a built-in group", &described_builtin);
        let builtin_dn = text(&described_builtin, "DistinguishedName");
        assert!(
            builtin_dn.starts_with(&format!("CN={BUILTIN_GROUP},CN="))
                && builtin_dn.ends_with(&components)
                && !builtin_dn.contains("OU="),
            "case {case:?}: a built-in group must be named in a container of the domain's \
             own, got {builtin_dn:?}"
        );

        // Every projection of one object reports one name: a search that asks for those
        // attributes answers with the same strings the describe did.
        let searched = call(
            &router,
            "/Users/SearchUsers",
            directory,
            &json!({
                "SearchString": USER,
                "SearchAttributes": ["SAMAccountName", "DistinguishedName", "UserPrincipalName"],
            }),
        )
        .await;
        served(case, "SearchUsers", &searched);
        let found = searched.body["Users"]
            .as_array()
            .and_then(|users| users.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            found["DistinguishedName"],
            json!(expected_user_dn),
            "case {case:?}: the search must report the name the describe reports, got {:?}",
            searched.body
        );
        assert_eq!(
            found["UserPrincipalName"],
            json!(format!("{USER}@{}", row.domain.to_uppercase())),
            "case {case:?}: the search must report the principal name the describe reports, \
             got {:?}",
            searched.body
        );
    }

    // No two directories name their objects in one place: the unit is the directory's own.
    let mut delegated: Vec<String> = rows
        .iter()
        .map(|row| format!("OU={},{}", row.delegated_ou, domain_components(row.domain)))
        .collect();
    delegated.sort();
    delegated.dedup();
    assert_eq!(
        delegated.len(),
        rows.len(),
        "expected one delegated unit per directory, got {delegated:?}"
    );
}

/// One attribute row: the name a create writes, and the name every read of it reports.
///
/// The reported name is the attribute's `lDAPDisplayName` in the Active Directory schema
/// with its leading letter in upper case, which is the spelling this API states an attribute
/// name in (as its own members do: `GivenName` for `givenName`, `Surname` for `sn`,
/// `EmailAddress` for `mail`). An attribute the schema does not hold is reported exactly as
/// it was written.
struct AttributeRow {
    case: &'static str,
    written: &'static str,
    reported: &'static str,
}

/// The table: one row per form an attribute name arrives in.
const ATTRIBUTE_ROWS: &[AttributeRow] = &[
    AttributeRow {
        case: "a schema name written in its schema spelling",
        written: "description",
        reported: "Description",
    },
    AttributeRow {
        case: "the same name written in capitals",
        written: "DESCRIPTION",
        reported: "Description",
    },
    AttributeRow {
        case: "the same name written as the API reports it",
        written: "Description",
        reported: "Description",
    },
    AttributeRow {
        case: "a camel-case schema name",
        written: "telephoneNumber",
        reported: "TelephoneNumber",
    },
    AttributeRow {
        case: "a camel-case schema name written in lower case",
        written: "servicePrincipalname",
        reported: "ServicePrincipalName",
    },
    AttributeRow {
        case: "a schema name whose own spelling leads with lower case letters",
        written: "wwwhomepage",
        reported: "WWWHomePage",
    },
    AttributeRow {
        case: "a two-letter schema name",
        written: "CO",
        reported: "Co",
    },
    AttributeRow {
        case: "a one-letter schema name",
        written: "l",
        reported: "L",
    },
    AttributeRow {
        case: "a name the schema does not hold",
        written: "Floor",
        reported: "Floor",
    },
];

/// RULE: an attribute is reported under the name the directory's schema holds it under,
/// whatever case the request wrote — on the describe and on the search alike.
#[tokio::test]
async fn an_attribute_is_reported_under_the_name_the_schema_holds_it_under() {
    let rows = rows();
    let router = emulator(&rows);
    let directory = rows[0].directory;
    let value = json!({"S": "a stored value"});

    for (index, row) in ATTRIBUTE_ROWS.iter().enumerate() {
        let case = row.case;
        // One user per row, so no row can read another's attribute. The name stays inside
        // `UserName`'s `@length(min: 1, max: 20)`.
        let name = format!("attrUser{index}");
        served(
            case,
            "CreateUser",
            &call(
                &router,
                "/Users/CreateUser",
                directory,
                &json!({
                    "SAMAccountName": name,
                    "OtherAttributes": {row.written: value.clone()},
                }),
            )
            .await,
        );

        // Asked for in a case the create did not use: the name is resolved, not matched.
        for asked in [
            row.written.to_string(),
            row.written.to_uppercase(),
            row.written.to_lowercase(),
        ] {
            let described = call(
                &router,
                "/Users/DescribeUser",
                directory,
                &json!({"SAMAccountName": name, "OtherAttributes": [asked.clone()]}),
            )
            .await;
            served(case, "DescribeUser", &described);
            assert_eq!(
                described.body["OtherAttributes"],
                json!({row.reported: value.clone()}),
                "case {case:?} asked as {asked:?}: expected the attribute under \
                 {:?}, got {:?}",
                row.reported,
                described.body
            );
        }

        // The search projects the same map: one attribute, one reported name.
        let searched = call(
            &router,
            "/Users/SearchUsers",
            directory,
            &json!({
                "SearchString": name,
                "SearchAttributes": ["SAMAccountName", row.written],
            }),
        )
        .await;
        served(case, "SearchUsers", &searched);
        let found = searched.body["Users"]
            .as_array()
            .and_then(|users| users.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            found["OtherAttributes"],
            json!({row.reported: value.clone()}),
            "case {case:?}: the search must report the attribute under {:?}, got {:?}",
            row.reported,
            searched.body
        );
    }
}

/// One refusal row: the operation a request that names a missing object goes to, the body it
/// carries, and the class of object the refusal is about.
struct RefusalRow {
    case: &'static str,
    route: &'static str,
    body: fn() -> Value,
    object_class: &'static str,
}

/// RULE: a `ResourceNotFoundException` of this API states the CLASS of object the request
/// asked about and the directory it asked in, and no object name.
#[tokio::test]
async fn a_not_found_refusal_states_the_object_class_and_the_directory() {
    let rows = rows();
    let router = emulator(&rows);
    let directory = rows[0].directory;

    // A user and a group that DID exist, so the rows below are about an object the
    // directory does not hold rather than about a directory nothing was written to.
    served(
        "the deleted user",
        "CreateUser",
        &call(
            &router,
            "/Users/CreateUser",
            directory,
            &json!({"SAMAccountName": "gone.user"}),
        )
        .await,
    );
    served(
        "the deleted user",
        "DeleteUser",
        &call(
            &router,
            "/Users/DeleteUser",
            directory,
            &json!({"SAMAccountName": "gone.user"}),
        )
        .await,
    );

    let refusals: &[RefusalRow] = &[
        RefusalRow {
            case: "a describe of a deleted user",
            route: "/Users/DescribeUser",
            body: || json!({"SAMAccountName": "gone.user"}),
            object_class: "User",
        },
        RefusalRow {
            case: "a describe of a user name nothing created",
            route: "/Users/DescribeUser",
            body: || json!({"SAMAccountName": "never.created"}),
            object_class: "User",
        },
        RefusalRow {
            case: "a delete of a deleted user",
            route: "/Users/DeleteUser",
            body: || json!({"SAMAccountName": "gone.user"}),
            object_class: "User",
        },
        RefusalRow {
            case: "an update of a deleted user",
            route: "/Users/UpdateUser",
            body: || json!({"SAMAccountName": "gone.user", "GivenName": "x"}),
            object_class: "User",
        },
        RefusalRow {
            case: "an update of a user name nothing created",
            route: "/Users/UpdateUser",
            body: || json!({"SAMAccountName": "never.created", "GivenName": "x"}),
            object_class: "User",
        },
        RefusalRow {
            case: "a describe of a group the directory holds none of",
            route: "/Groups/DescribeGroup",
            body: || json!({"SAMAccountName": "never.created"}),
            object_class: "Group",
        },
        RefusalRow {
            case: "a member listing of a group the directory holds none of",
            route: "/GroupMemberships/ListGroupMembers",
            body: || json!({"SAMAccountName": "never.created"}),
            object_class: "Group",
        },
        RefusalRow {
            case: "a group listing for a member the directory holds no object for",
            route: "/GroupMemberships/ListGroupsForMember",
            body: || json!({"SAMAccountName": "never.created"}),
            object_class: "Member",
        },
        RefusalRow {
            case: "an addition of a member the directory holds no object for",
            route: "/GroupMemberships/AddGroupMember",
            body: || json!({"GroupName": "Domain Users", "MemberName": "never.created"}),
            object_class: "Member",
        },
        RefusalRow {
            case: "a removal of a member a group does not hold",
            route: "/GroupMemberships/RemoveGroupMember",
            body: || json!({"GroupName": "Domain Users", "MemberName": "never.created"}),
            object_class: "Member",
        },
    ];

    for row in refusals {
        let case = row.case;
        let refused = call(&router, row.route, directory, &(row.body)()).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
        let expected = format!("{} not found in directory {directory}.", row.object_class);
        assert_eq!(
            refused.message(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.body
        );
        // The name the request asked about is not disclosed by the refusal.
        for named in ["gone.user", "never.created"] {
            assert!(
                !refused.message().unwrap_or_default().contains(named),
                "case {case:?}: the refusal must name no object, got {:?}",
                refused.body
            );
        }
    }
}
