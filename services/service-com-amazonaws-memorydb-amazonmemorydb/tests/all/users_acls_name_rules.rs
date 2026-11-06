// SPDX-License-Identifier: CC0-1.0

//! The rules this slice's user / ACL / parameter-group surface follows, end to end over
//! the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `MinimumEngineVersion` is DERIVED: a user reports the earliest engine version
//!   whose RBAC supports how it authenticates (IAM needs 7.0, a password or no password
//!   works from 6.2), and an ACL reports the greatest minimum its members require, so it
//!   follows every membership change;
//! * the account ALREADY HAS the `open-access` ACL (whose only member is the built-in
//!   `default` user) before any `CreateACL` call: a describe answers with it, a create
//!   naming it collides, and a cluster can be created with it;
//! * every paginated listing accepts `MaxResults` in 20..=100 and refuses anything
//!   outside that window with the modelled `InvalidParameterValueException`, at each
//!   bound and one past it (asserted in the `page_size_window` suite);
//! * a parameter group created without a `Description` reads back with the service's own
//!   default, and one created with any description reads that back verbatim;
//! * a user, ACL, cluster or parameter-group name is refused unless it is at most 40
//!   characters, begins with a letter, holds only ASCII letters, digits and hyphens, and
//!   neither ends with a hyphen nor carries two consecutive hyphens;
//! * a resource name is a case-insensitive identifier: the create echoes the folded
//!   spelling, a folded read finds it, and a second create under any spelling collides;
//! * `CreateUser` stores the CANONICAL access string, which always states the pub/sub
//!   channel permission;
//! * `UpdateACL` moves the stored membership, and an ACL's `MinimumEngineVersion`
//!   follows it in both directions; a built-in ACL refuses the change;
//! * a user password is accepted on its length and printable-ASCII rules alone;
//! * `CreateCluster` refuses an `EngineVersion` the named `Engine` does not publish.

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::{
    DEFAULT_PARAMETER_GROUP_DESCRIPTION, DEFAULT_USER_NAME, ENGINE_VERSIONS,
    MAX_RESOURCE_NAME_LEN, MAX_USERS_PER_ACL, OPEN_ACCESS_ACL_NAME,
};

// The suite-wide helpers: ONE SigV4 request builder and ONE error-code reader for every
// suite in this crate.
use crate::cluster_lifecycle::{call, error_code, ok, router};

/// A password that breaks none of the documented password rules (16..=128 printable
/// ASCII characters).
const PASSWORD: &str = "Sim2realPassword-1";

/// An access string that names no pub/sub channel rule, so the canonical form the
/// service stores has to add one.
const ACCESS_NO_CHANNELS: &str = "on ~* +@all";

/// `CreateUser` with `name`, one password and `access_string`.
fn create_user_body(name: &str, access_string: &str) -> Value {
    json!({
        "UserName": name,
        "AccessString": access_string,
        "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
    })
}

/// `CreateUser` with `name`, authenticating through IAM.
fn create_iam_user_body(name: &str) -> Value {
    json!({
        "UserName": name,
        "AccessString": ACCESS_NO_CHANNELS,
        "AuthenticationMode": { "Type": "iam" },
    })
}

/// `MinimumEngineVersion` is DERIVED from how a user authenticates, and an ACL's is the
/// greatest its members require — so it follows the membership rather than being
/// recorded beside it.
#[tokio::test]
async fn minimum_engine_version_follows_authentication_and_acl_membership() {
    let router = router();

    // One row per authentication type, with the version its RBAC first appeared in.
    let users: &[(&str, Value, &str)] = &[
        (
            "pw-user",
            create_user_body("pw-user", ACCESS_NO_CHANNELS),
            "6.2",
        ),
        (
            "two-pw-user",
            json!({
                "UserName": "two-pw-user",
                "AccessString": ACCESS_NO_CHANNELS,
                "AuthenticationMode": {
                    "Type": "password",
                    "Passwords": [PASSWORD, "Sim2realPassword-2"],
                },
            }),
            "6.2",
        ),
        ("iam-user", create_iam_user_body("iam-user"), "7.0"),
    ];
    for (name, body, expected) in users {
        let created = ok(&router, "CreateUser", body, name).await;
        assert_eq!(
            created["User"]["MinimumEngineVersion"],
            json!(expected),
            "case {name:?}: expected MinimumEngineVersion {expected:?}, got {created}"
        );
        let described = ok(&router, "DescribeUsers", &json!({ "UserName": name }), name).await;
        assert_eq!(
            described["Users"][0]["MinimumEngineVersion"],
            json!(expected),
            "case {name:?}: the describe must report the same version, got {described}"
        );
    }
    // The built-in password-less user reports the same baseline as a password user.
    let builtin = ok(
        &router,
        "DescribeUsers",
        &json!({ "UserName": DEFAULT_USER_NAME }),
        DEFAULT_USER_NAME,
    )
    .await;
    assert_eq!(
        builtin["Users"][0]["MinimumEngineVersion"],
        json!("6.2"),
        "case {DEFAULT_USER_NAME:?}: a password-less user needs no engine past 6.2, got \
         {builtin}"
    );

    // One row per ACL membership shape: the members, and the version the ACL reports.
    let acls: &[(&str, &[&str], &str)] = &[
        ("acl-empty", &[], "6.2"),
        ("acl-password-only", &["pw-user", "two-pw-user"], "6.2"),
        ("acl-with-iam", &["pw-user", "iam-user"], "7.0"),
        ("acl-iam-only", &["iam-user"], "7.0"),
    ];
    for (name, members, expected) in acls {
        let mut body = json!({ "ACLName": name });
        if !members.is_empty() {
            body["UserNames"] = json!(members);
        }
        let created = ok(&router, "CreateACL", &body, name).await;
        assert_eq!(
            created["ACL"]["MinimumEngineVersion"],
            json!(expected),
            "case {name:?}: expected MinimumEngineVersion {expected:?}, got {created}"
        );
        let described = ok(&router, "DescribeACLs", &json!({ "ACLName": name }), name).await;
        assert_eq!(
            described["ACLs"][0]["MinimumEngineVersion"],
            json!(expected),
            "case {name:?}: the describe must report the same version, got {described}"
        );
    }

    // And the unfiltered listing agrees with every by-name describe, so no code path
    // reports a different version from another.
    let listed = ok(&router, "DescribeACLs", &json!({}), "listing").await;
    for acl in listed["ACLs"].as_array().expect("ACLs is a list") {
        let name = acl["Name"].as_str().unwrap_or_default();
        let described = ok(&router, "DescribeACLs", &json!({ "ACLName": name }), name).await;
        assert_eq!(
            described["ACLs"][0], *acl,
            "case {name:?}: the listing and the by-name describe must agree, got \
             {described}"
        );
    }
}

/// The account ALREADY HAS the `open-access` ACL before any call: it is reported with
/// the built-in `default` user as its only member, a create naming it collides, and a
/// cluster can be created with it.
#[tokio::test]
async fn the_builtin_open_access_acl_is_there_before_any_call() {
    let router = router();

    // Both spellings of the name find the one built-in ACL, from an untouched store.
    for spelling in [OPEN_ACCESS_ACL_NAME, "Open-Access"] {
        let described = ok(
            &router,
            "DescribeACLs",
            &json!({ "ACLName": spelling }),
            spelling,
        )
        .await;
        let acl = &described["ACLs"][0];
        for (member, expected) in [
            ("Name", json!(OPEN_ACCESS_ACL_NAME)),
            ("Status", json!("active")),
            ("UserNames", json!([DEFAULT_USER_NAME])),
            ("MinimumEngineVersion", json!("6.2")),
            ("Clusters", json!([])),
        ] {
            assert_eq!(
                acl[member], expected,
                "case {spelling:?}: expected {member} to be {expected}, got {described}"
            );
        }
    }

    // It is in the unfiltered listing too, and a describe changes nothing.
    let first = ok(&router, "DescribeACLs", &json!({}), "listing").await;
    assert_eq!(
        first["ACLs"].as_array().map(Vec::len),
        Some(1),
        "case \"listing\": expected exactly the built-in ACL, got {first}"
    );
    let second = ok(&router, "DescribeACLs", &json!({}), "listing").await;
    assert_eq!(
        first, second,
        "case \"listing\": a describe must not change what the next caller sees"
    );

    // Creating it again collides with the one the account already had.
    for spelling in [OPEN_ACCESS_ACL_NAME, "OPEN-ACCESS"] {
        let (status, body) = call(&router, "CreateACL", &json!({ "ACLName": spelling })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ACLAlreadyExistsFault"),
            "case {spelling:?}: expected the modelled already-exists fault, got {status} \
             with {body}"
        );
    }

    // And a cluster can be created with it without any ACL having been created first.
    let created = ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "open-access-cluster",
            "NodeType": "db.t4g.small",
            "ACLName": "Open-Access",
        }),
        "cluster on the built-in ACL",
    )
    .await;
    assert_eq!(
        created["Cluster"]["ACLName"],
        json!(OPEN_ACCESS_ACL_NAME),
        "case {:?}: the cluster must point at the built-in ACL, got {created}",
        "cluster on the built-in ACL"
    );
}

/// A parameter group created without a `Description` reads back with the service's own
/// default; one created with a description reads that back verbatim, whatever it says.
#[tokio::test]
async fn an_omitted_parameter_group_description_reads_back_as_the_service_default() {
    let router = router();
    // (group name, the Description the request carried, what must be reported).
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("no-description", None, DEFAULT_PARAMETER_GROUP_DESCRIPTION),
        ("empty-description", Some(""), ""),
        ("space-description", Some(" "), " "),
        ("worded-description", Some("the batch tier"), "the batch tier"),
    ];
    for (name, requested, expected) in cases {
        let mut body = json!({ "ParameterGroupName": name, "Family": "memorydb_valkey7" });
        if let Some(description) = requested {
            body["Description"] = json!(description);
        }
        let created = ok(&router, "CreateParameterGroup", &body, name).await;
        assert_eq!(
            created["ParameterGroup"]["Description"],
            json!(expected),
            "case {name:?}: expected Description {expected:?}, got {created}"
        );
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
            name,
        )
        .await;
        assert_eq!(
            described["ParameterGroups"][0]["Description"],
            json!(expected),
            "case {name:?}: the describe must report the stored description, got \
             {described}"
        );
    }
}

/// A name is accepted exactly when it is at most [`MAX_RESOURCE_NAME_LEN`] characters,
/// begins with a letter, holds only ASCII letters, digits and hyphens, and neither ends
/// with a hyphen nor carries two consecutive hyphens — for EVERY resource kind that
/// takes a name, with a refused name creating nothing.
#[tokio::test]
async fn a_name_outside_the_services_rule_is_refused_for_every_resource_kind() {
    let router = router();
    let at_max: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN - 1))
        .collect();
    let past_max: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN))
        .collect();
    // (case, name, accepted) — the rows differ along every clause of the rule. Each
    // name is used once per resource kind, and the kinds keep separate stores, so no
    // two rows can collide.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a plain name", "plain".to_string(), true),
        ("digits and hyphens", "a1-b2-c3".to_string(), true),
        ("a single letter", "z".to_string(), true),
        ("at the length bound", at_max, true),
        ("one past the length bound", past_max, false),
        ("beginning with a digit", "9lives".to_string(), false),
        ("beginning with a hyphen", "-leading".to_string(), false),
        ("holding an underscore", "under-score-x_y".to_string(), false),
        ("holding a dot", "dotted.name".to_string(), false),
        ("ending with a hyphen", "trailing-".to_string(), false),
        (
            "two consecutive hyphens",
            "double--hyphen".to_string(),
            false,
        ),
        ("holding a space", "two words".to_string(), false),
        ("empty", String::new(), false),
    ];

    // One row per name-taking create: how its request carries the name, and how to
    // read every name its store holds.
    let kinds: &[(&str, &str, &str)] = &[
        ("CreateUser", "DescribeUsers", "Users"),
        ("CreateACL", "DescribeACLs", "ACLs"),
        (
            "CreateParameterGroup",
            "DescribeParameterGroups",
            "ParameterGroups",
        ),
        ("CreateCluster", "DescribeClusters", "Clusters"),
    ];
    let request_for = |operation: &str, name: &str| -> Value {
        match operation {
            "CreateUser" => create_user_body(name, ACCESS_NO_CHANNELS),
            "CreateACL" => json!({ "ACLName": name }),
            "CreateParameterGroup" => {
                json!({ "ParameterGroupName": name, "Family": "memorydb_valkey7" })
            }
            _ => json!({
                "ClusterName": name,
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
            }),
        }
    };

    for (case, name, accepted) in &cases {
        for (create, reader, items) in kinds {
            let case = format!("{create} {case}");
            let (status, response) = call(&router, create, &request_for(create, name)).await;
            if *accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the name {name:?} meets every clause of the rule, so \
                     the create must succeed; got {status} with {response}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: the name {name:?} breaks the rule, so the create must be \
                 refused with the modelled invalid-value error; got {status} with \
                 {response}"
            );
            // A refused create persists nothing: the store never holds the name. The
            // whole listing is read rather than the one name, because a name the model's
            // own pattern also refuses never reaches the reader's lookup either.
            let listed = ok(&router, reader, &json!({}), &case).await;
            let held: Vec<String> = listed[items]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected {items} in {listed}"))
                .iter()
                .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !held.contains(&name.to_ascii_lowercase()),
                "case {case:?}: a refused create must leave nothing behind, got {held:?}"
            );
        }
    }
}

/// A resource name is a case-insensitive identifier: the create echoes the FOLDED
/// spelling, a read under any spelling finds the one resource, and a second create under
/// any spelling collides with it.
#[tokio::test]
async fn a_resource_name_is_folded_to_lower_case() {
    let router = router();
    // (create, the mixed-case name, the reader, the already-exists fault) — one row per
    // resource kind that takes a name.
    let created_names: &[(&str, &str, &str, &str)] = &[
        ("CreateUser", "MiXed-User", "DescribeUsers", "UserAlreadyExistsFault"),
        ("CreateACL", "MiXed-ACL", "DescribeACLs", "ACLAlreadyExistsFault"),
        (
            "CreateParameterGroup",
            "MiXed-Group",
            "DescribeParameterGroups",
            "ParameterGroupAlreadyExistsFault",
        ),
        (
            "CreateCluster",
            "MiXed-Cluster",
            "DescribeClusters",
            "ClusterAlreadyExistsFault",
        ),
    ];
    let request_for = |operation: &str, name: &str| -> Value {
        match operation {
            "CreateUser" => create_user_body(name, ACCESS_NO_CHANNELS),
            "CreateACL" => json!({ "ACLName": name }),
            // The family is folded by the same rule as the name.
            "CreateParameterGroup" => {
                json!({ "ParameterGroupName": name, "Family": "MEMORYDB_VALKEY7" })
            }
            _ => json!({
                "ClusterName": name,
                "NodeType": "db.t4g.small",
                "ACLName": "OPEN-ACCESS",
            }),
        }
    };
    let lookup_for = |operation: &str, name: &str| -> Value {
        match operation {
            "DescribeUsers" => json!({ "UserName": name }),
            "DescribeACLs" => json!({ "ACLName": name }),
            "DescribeParameterGroups" => json!({ "ParameterGroupName": name }),
            _ => json!({ "ClusterName": name }),
        }
    };
    let reported_name = |operation: &str, body: &Value| -> Value {
        match operation {
            "DescribeUsers" => body["Users"][0]["Name"].clone(),
            "DescribeACLs" => body["ACLs"][0]["Name"].clone(),
            "DescribeParameterGroups" => body["ParameterGroups"][0]["Name"].clone(),
            _ => body["Clusters"][0]["Name"].clone(),
        }
    };

    for (create, name, reader, already_exists) in created_names {
        let folded = name.to_ascii_lowercase();
        let created = ok(&router, create, &request_for(create, name), name).await;
        assert!(
            created.to_string().contains(&folded) && !created.to_string().contains(name),
            "case {name:?}: {create} must report the folded name {folded:?}, got {created}"
        );
        if *create == "CreateParameterGroup" {
            assert_eq!(
                created["ParameterGroup"]["Family"],
                json!("memorydb_valkey7"),
                "case {name:?}: the family is folded by the same rule, got {created}"
            );
        }

        // Either spelling reads the ONE resource back.
        for spelling in [*name, folded.as_str(), &name.to_ascii_uppercase()] {
            let case = format!("{reader} {spelling}");
            let described = ok(&router, reader, &lookup_for(reader, spelling), &case).await;
            assert_eq!(
                reported_name(reader, &described),
                json!(folded),
                "case {case:?}: the read must find the one resource, got {described}"
            );
        }

        // And a second create under any spelling collides rather than making a second
        // resource.
        for spelling in [*name, folded.as_str(), &name.to_ascii_uppercase()] {
            let case = format!("{create} {spelling} again");
            let (status, body) = call(&router, create, &request_for(create, spelling)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *already_exists),
                "case {case:?}: expected the modelled already-exists fault, got {status} \
                 with {body}"
            );
        }
    }

    // The parameter-group name is folded by EVERY handler that resolves one, not only
    // by the describe the rows above read: a group created in mixed case is reachable
    // through the parameter reader and the parameter writer under either spelling.
    let case = "a group's parameters under either spelling";
    for spelling in ["MiXed-Group", "mixed-group", "MIXED-GROUP"] {
        let described = ok(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": spelling }),
            case,
        )
        .await;
        assert!(
            described["Parameters"]
                .as_array()
                .is_some_and(|parameters| !parameters.is_empty()),
            "case {case:?}: {spelling:?} must resolve to the one group's parameters, got \
             {described}"
        );
        let updated = ok(
            &router,
            "UpdateParameterGroup",
            &json!({
                "ParameterGroupName": spelling,
                "ParameterNameValues": [
                    { "ParameterName": "maxmemory-policy", "ParameterValue": "allkeys-lru" },
                ],
            }),
            case,
        )
        .await;
        assert_eq!(
            updated["ParameterGroup"]["Name"],
            json!("mixed-group"),
            "case {case:?}: the update of {spelling:?} must answer the one stored group, \
             got {updated}"
        );
    }
}

/// `CreateUser` stores the CANONICAL access string: the rules one space apart, with the
/// pub/sub channel permission and the command baseline always stated — the engine's
/// `resetchannels` / `-@all` defaults when the request named neither. The whole rule is
/// tabled in `canonical_access_string`; these rows are the name-and-folding suite's own
/// check that a create stores it.
#[tokio::test]
async fn an_access_string_is_stored_in_its_canonical_form() {
    let router = router();
    // (the access string as sent, the canonical form) — the rows differ in whether the
    // string names its channels, which keyword it names them with, where the command
    // rules sit, and how it is spaced.
    let cases: &[(&str, &str)] = &[
        ("on ~* +@all", "on ~* resetchannels +@all"),
        ("on ~* &* +@all", "on ~* &* +@all"),
        (
            "on ~* resetchannels +@all",
            "on ~* resetchannels +@all",
        ),
        ("on ~* allchannels +@all", "on ~* allchannels +@all"),
        ("on ~app:* &news:* -@admin", "on ~app:* &news:* -@all -@admin"),
        (
            "off ~key1 ~key2 +get +set",
            "off ~key1 ~key2 resetchannels -@all +get +set",
        ),
        // No command rule at all: both defaults go at the end.
        ("on ~*", "on ~* resetchannels -@all"),
        // Extra whitespace is not part of a rule list.
        ("on   ~*    +@read", "on ~* resetchannels -@all +@read"),
    ];
    for (index, (sent, expected)) in cases.iter().enumerate() {
        let name = format!("access-user-{index}");
        let created = ok(&router, "CreateUser", &create_user_body(&name, sent), &name).await;
        assert_eq!(
            created["User"]["AccessString"],
            json!(expected),
            "case {sent:?}: expected the canonical form {expected:?}, got {created}"
        );
        let described = ok(&router, "DescribeUsers", &json!({ "UserName": name }), &name).await;
        assert_eq!(
            described["Users"][0]["AccessString"],
            json!(expected),
            "case {sent:?}: the describe must report the stored form, got {described}"
        );
    }
}

/// `UpdateACL` moves the stored membership, and the ACL's `MinimumEngineVersion`
/// FOLLOWS it: adding an IAM-authenticated member raises it, removing that member
/// restores it. The ACL the account already had refuses the change outright.
#[tokio::test]
async fn an_acls_minimum_engine_version_follows_every_membership_change() {
    let router = router();
    for (name, body) in [
        ("pw-one", create_user_body("pw-one", ACCESS_NO_CHANNELS)),
        ("pw-two", create_user_body("pw-two", ACCESS_NO_CHANNELS)),
        ("iam-one", create_iam_user_body("iam-one")),
    ] {
        ok(&router, "CreateUser", &body, name).await;
    }
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "moving-acl", "UserNames": ["pw-one"] }),
        "setup",
    )
    .await;

    // Each step: the request, the membership it leaves, and the version that membership
    // requires. The rows walk the membership up and back down again, so a version
    // computed once at create time fails them.
    let steps: &[(Value, &[&str], &str)] = &[
        (
            json!({ "ACLName": "moving-acl", "UserNamesToAdd": ["PW-Two"] }),
            &["pw-one", "pw-two"],
            "6.2",
        ),
        (
            json!({ "ACLName": "Moving-ACL", "UserNamesToAdd": ["iam-one"] }),
            &["pw-one", "pw-two", "iam-one"],
            "7.0",
        ),
        (
            json!({ "ACLName": "moving-acl", "UserNamesToRemove": ["pw-two"] }),
            &["pw-one", "iam-one"],
            "7.0",
        ),
        (
            json!({ "ACLName": "moving-acl", "UserNamesToRemove": ["Iam-One"] }),
            &["pw-one"],
            "6.2",
        ),
        (
            json!({
                "ACLName": "moving-acl",
                "UserNamesToAdd": ["iam-one"],
                "UserNamesToRemove": ["pw-one"],
            }),
            &["iam-one"],
            "7.0",
        ),
    ];
    for (request, expected_members, expected_version) in steps {
        let case = format!("UpdateACL {request}");
        let updated = ok(&router, "UpdateACL", request, &case).await;
        assert_eq!(
            updated["ACL"]["Status"],
            json!("modifying"),
            "case {case:?}: an accepted change leaves the ACL modifying, got {updated}"
        );
        for member_list in [
            updated["ACL"]["UserNames"].clone(),
            // The describe reads the same membership back out of state.
            ok(
                &router,
                "DescribeACLs",
                &json!({ "ACLName": "moving-acl" }),
                &case,
            )
            .await["ACLs"][0]["UserNames"]
                .clone(),
        ] {
            let mut reported: Vec<String> = member_list
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: UserNames is a list"))
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect();
            reported.sort();
            let mut expected: Vec<String> =
                expected_members.iter().map(|name| name.to_string()).collect();
            expected.sort();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the members {expected:?}, got {reported:?}"
            );
        }
        let described = ok(
            &router,
            "DescribeACLs",
            &json!({ "ACLName": "moving-acl" }),
            &case,
        )
        .await;
        assert_eq!(
            described["ACLs"][0]["MinimumEngineVersion"],
            json!(expected_version),
            "case {case:?}: expected MinimumEngineVersion {expected_version:?}, got \
             {described}"
        );
        assert_eq!(
            updated["ACL"]["MinimumEngineVersion"],
            json!(expected_version),
            "case {case:?}: the update's own answer must agree with the describe, got \
             {updated}"
        );
    }

    // The refusals, one rule per row, each leaving the membership exactly as it was.
    let over_the_cap: Vec<String> = (0..=MAX_USERS_PER_ACL)
        .map(|index| format!("bulk-user-{index}"))
        .collect();
    let refusals: &[(&str, Value, u16, &str)] = &[
        (
            "an ACL state does not hold",
            json!({ "ACLName": "no-such-acl", "UserNamesToAdd": ["pw-one"] }),
            400,
            "ACLNotFoundFault",
        ),
        (
            "the built-in ACL is not the caller's to change",
            json!({ "ACLName": OPEN_ACCESS_ACL_NAME, "UserNamesToAdd": ["pw-one"] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "the built-in ACL under another spelling",
            json!({ "ACLName": "Open-Access", "UserNamesToRemove": [DEFAULT_USER_NAME] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "nothing asked for",
            json!({ "ACLName": "moving-acl" }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "one name added and removed at once",
            json!({
                "ACLName": "moving-acl",
                "UserNamesToAdd": ["pw-one"],
                "UserNamesToRemove": ["PW-One"],
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a name added twice",
            json!({ "ACLName": "moving-acl", "UserNamesToAdd": ["pw-one", "PW-ONE"] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            // AWS answers the COMBINATION fault here: the delta contradicts the
            // membership it is applied to. A name the REQUEST repeats is the
            // duplicate-name fault above, so the two report different mistakes.
            "a member added that is already one",
            json!({ "ACLName": "moving-acl", "UserNamesToAdd": ["iam-one"] }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a member removed that is not one",
            json!({ "ACLName": "moving-acl", "UserNamesToRemove": ["pw-two"] }),
            400,
            "InvalidParameterValueException",
        ),
        // One past the membership bound, in each list that can reach it: the work a
        // request may ask for is capped before any of it runs.
        (
            "one past the membership bound, added",
            json!({ "ACLName": "moving-acl", "UserNamesToAdd": over_the_cap }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "one past the membership bound, removed",
            json!({ "ACLName": "moving-acl", "UserNamesToRemove": over_the_cap }),
            400,
            "InvalidParameterValueException",
        ),
    ];
    for (case, request, expected_status, expected_code) in refusals {
        let (status, body) = call(&router, "UpdateACL", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             with {body}"
        );
    }
    let after = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": "moving-acl" }),
        "after the refusals",
    )
    .await;
    assert_eq!(
        after["ACLs"][0]["UserNames"],
        json!(["iam-one"]),
        "case {:?}: a refused change must leave the membership alone, got {after}",
        "after the refusals"
    );
    let builtin = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": OPEN_ACCESS_ACL_NAME }),
        "after the refusals",
    )
    .await;
    assert_eq!(
        builtin["ACLs"][0]["UserNames"],
        json!([DEFAULT_USER_NAME]),
        "case {:?}: the built-in ACL must keep its member, got {builtin}",
        "after the refusals"
    );
}

/// `CreateCluster` refuses an `EngineVersion` the named `Engine` does not publish with
/// the modelled `InvalidParameterCombinationException`, and an `Engine` it does not run
/// at all with the modelled `InvalidParameterValueException` — over the whole catalog
/// `DescribeEngineVersions` publishes.
#[tokio::test]
async fn a_cluster_takes_only_an_engine_version_its_engine_publishes() {
    let router = router();

    // Every (engine, version) pair the catalog publishes is accepted, and the same
    // version under ANOTHER engine is refused.
    for (index, published) in ENGINE_VERSIONS.iter().enumerate() {
        let version = published.version();
        let case = format!("{} {version}", published.engine);
        ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("engine-ok-{index}"),
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "Engine": published.engine,
                "EngineVersion": version,
            }),
            &case,
        )
        .await;

        for other in ENGINE_VERSIONS {
            if other.engine == published.engine {
                continue;
            }
            let case = format!("{} with {version}", other.engine);
            let (status, body) = call(
                &router,
                "CreateCluster",
                &json!({
                    "ClusterName": format!("engine-bad-{index}"),
                    "NodeType": "db.t4g.small",
                    "ACLName": OPEN_ACCESS_ACL_NAME,
                    "Engine": other.engine,
                    "EngineVersion": version,
                }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterCombinationException"),
                "case {case:?}: an engine does not run another engine's version; got \
                 {status} with {body}"
            );
            break;
        }
    }

    // A version no engine publishes, and an engine this service does not run.
    let refusals: &[(&str, Value, &str)] = &[
        (
            "a version no engine publishes",
            json!({
                "ClusterName": "engine-unknown-version",
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "EngineVersion": "99.9",
            }),
            "InvalidParameterCombinationException",
        ),
        (
            "a patch version, which is not what EngineVersion names",
            json!({
                "ClusterName": "engine-patch-version",
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "Engine": "redis",
                "EngineVersion": "6.2.6",
            }),
            "InvalidParameterCombinationException",
        ),
        (
            // Every MemoryDB cluster is a replicated cluster, so an engine that does
            // not replicate can back none of them.
            "an engine that does not support replication",
            json!({
                "ClusterName": "engine-no-replication",
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "Engine": "memcached",
            }),
            "InvalidParameterCombinationException",
        ),
        (
            "an engine this service does not run",
            json!({
                "ClusterName": "engine-unknown-engine",
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "Engine": "no-such-engine",
            }),
            "InvalidParameterValueException",
        ),
        (
            // `Engine` is an enum-like value, not a resource name: it is not folded, so
            // a different spelling of a published engine is not that engine.
            "a published engine in another case",
            json!({
                "ClusterName": "engine-wrong-case",
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "Engine": "REDIS",
            }),
            "InvalidParameterValueException",
        ),
        (
            "a malformed ACL name",
            json!({
                "ClusterName": "engine-bad-acl-name",
                "NodeType": "db.t4g.small",
                "ACLName": "not__an__acl__name",
            }),
            "InvalidParameterValueException",
        ),
    ];
    for (case, body, expected_code) in refusals {
        let (status, response) = call(&router, "CreateCluster", body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case:?}: expected {expected_code}, got {status} with {response}"
        );
        let (status, described) = call(
            &router,
            "DescribeClusters",
            &json!({ "ClusterName": body["ClusterName"] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must leave no cluster behind, got {status} \
             with {described}"
        );
    }

    // A version named without an engine is checked against every engine's versions,
    // because any of them could be the one the request meant.
    for published in ENGINE_VERSIONS {
        let version = published.version();
        let case = format!("no engine, version {version}");
        ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("engine-less-{}", version.replace('.', "-")),
                "NodeType": "db.t4g.small",
                "ACLName": OPEN_ACCESS_ACL_NAME,
                "EngineVersion": version,
            }),
            &case,
        )
        .await;
    }

    // An UpdateCluster that names no `Engine` is checked against the engine the cluster
    // is already running, not against every engine: a version belongs to one engine,
    // and an update leaves the cluster on the one it had.
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "engine-update",
            "NodeType": "db.t4g.small",
            "ACLName": OPEN_ACCESS_ACL_NAME,
            "Engine": "redis",
            "EngineVersion": "6.2",
        }),
        "engine-update",
    )
    .await;
    // (case, request, accepted) — the rows differ in whether the request names the
    // engine, and whether the version belongs to the engine the cluster will run.
    let updates: &[(&str, Value, bool)] = &[
        (
            "a version its own engine publishes",
            json!({ "ClusterName": "engine-update", "EngineVersion": "7.0" }),
            true,
        ),
        (
            "a version only the other engine publishes",
            json!({ "ClusterName": "engine-update", "EngineVersion": "7.2" }),
            false,
        ),
        (
            "the other engine's version, with that engine named",
            json!({
                "ClusterName": "engine-update",
                "Engine": "valkey",
                "EngineVersion": "7.2",
            }),
            true,
        ),
    ];
    for (case, request, accepted) in updates {
        let (status, body) = call(&router, "UpdateCluster", request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status} with \
                 {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterCombinationException"),
                "case {case:?}: a cluster does not take another engine's version; got \
                 {status} with {body}"
            );
        }
    }
}
