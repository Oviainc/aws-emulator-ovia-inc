// SPDX-License-Identifier: CC0-1.0

//! The guards a MemoryDB write applies to the REQUEST before it persists anything, end
//! to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a member that NAMES ANOTHER RESOURCE is resolved against state before anything is
//!   written, so an ACL's membership only ever names users that exist and a cluster only
//!   ever points at a parameter group (and a subnet group) that exists — on EVERY write
//!   path of each reference, with the modelled not-found fault for the kind of resource
//!   the name missed;
//! * two members that select different things cannot be sent together: `DescribeUsers`
//!   takes either the one `UserName` or a set of `Filters`, never both;
//! * a member whose VALUE the service cannot serve is refused whatever else the request
//!   carries: a free-text `Description` longer than the service's bound, and an
//!   `AccessString` that is not a rule list of the engine's ACL syntax;
//! * and a refused request writes NOTHING: after each refusal the resource is read back
//!   and is either absent or exactly as it was.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// A node type MemoryDB publishes, for every cluster this suite creates.
const NODE_TYPE: &str = "db.t4g.small";

/// The ACL MemoryDB provides in every account, so a cluster can be created without one
/// being made first (`CreateCluster$ACLName` is required).
const BUILTIN_ACL: &str = "open-access";

/// A parameter group family MemoryDB publishes
/// (`DescribeEngineVersions$ParameterGroupFamily`).
const FAMILY: &str = "memorydb_redis7";

/// An access string every user this suite seeds is created with (enabled, all keys, all
/// channels, all commands).
const FULL_ACCESS: &str = "on ~* &* +@all";

/// A password breaking none of the documented user password rules (16..=128 printable
/// ASCII characters).
const PASSWORD: &str = "Aa1!Aa1!Aa1!Aa1!";

/// The longest `Description` the service stores, and the sentence a longer one is
/// refused with, as the recorded `CreateParameterGroup` refusal carries them.
const MAX_DESCRIPTION: usize = 255;
const TOO_LONG_DESCRIPTION_MESSAGE: &str = "Description is too long. Maximum length is 255";

/// The sentence a `UserName` sent together with `Filters` is refused with, and the one
/// an access string the engine cannot read is refused with, as recorded.
const NAME_WITH_FILTERS_MESSAGE: &str =
    "User name can't be specified at the same time as filters.";
const INVALID_ACCESS_STRING_MESSAGE: &str = "The access-string is invalid.";

/// The sentence a cluster write naming a subnet group that is not in state is refused
/// with, as recorded.
const SUBNET_GROUP_NOT_FOUND_MESSAGE: &str = "Subnet Group not found.";

/// `CreateUser` for `name` with `access_string` and one password.
fn create_user_body(name: &str, access_string: &str) -> Value {
    json!({
        "UserName": name,
        "AccessString": access_string,
        "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
    })
}

/// A description exactly `length` characters long.
fn description_of_length(length: usize) -> String {
    "d".repeat(length)
}

/// Assert one request is refused with `code` and `message`.
async fn assert_refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
    code: &str,
    message: &str,
) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, code),
        "case {case:?}: expected {operation} to answer 400 {code}, got {status} with \
         {answered}"
    );
    assert_eq!(
        answered.get("message").and_then(Value::as_str),
        Some(message),
        "case {case:?}: expected the recorded sentence, got {answered}"
    );
}

/// The `UserNames` an `ACL` body reports, sorted, so a comparison is about the members
/// and not about the order they were named in.
fn member_names(acl: &Value, case: &str) -> Vec<String> {
    let mut names: Vec<String> = acl
        .get("UserNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected UserNames in {acl}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// An ACL's members ARE users: both write paths of a membership resolve every name they
/// are given against the user store, so neither can leave an ACL listing a member no
/// user exists for.
#[tokio::test]
async fn an_acl_membership_only_names_users_that_exist() {
    let router = router();
    for name in ["member-one", "member-two"] {
        ok(
            &router,
            "CreateUser",
            &create_user_body(name, FULL_ACCESS),
            name,
        )
        .await;
    }

    // The rows differ along what the rule generalises over: how long the list is, where
    // in it the name that is not a user sits, and the letter case it is written in (a
    // user name is a case-insensitive identifier, so the refusal names the folded
    // spelling).
    let rows: &[(&str, &[&str], &str)] = &[
        ("the only name is not a user", &["ghost-user"], "ghost-user"),
        (
            "the last of three is not a user",
            &["member-one", "member-two", "ghost-user"],
            "ghost-user",
        ),
        (
            "the first of two is not a user",
            &["ghost-user", "member-one"],
            "ghost-user",
        ),
        (
            "the name that is not a user is written in mixed case",
            &["Ghost-User"],
            "ghost-user",
        ),
    ];
    for (index, (case, members, absent)) in rows.iter().enumerate() {
        let acl_name = format!("create-guard-{index}");
        assert_refused(
            &router,
            "CreateACL",
            &json!({ "ACLName": &acl_name, "UserNames": members }),
            case,
            "UserNotFoundFault",
            &format!("User {absent} not found."),
        )
        .await;
        // A refused create wrote nothing: the name holds no ACL at all.
        let (status, answered) = call(
            &router,
            "DescribeACLs",
            &json!({ "ACLName": &acl_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ACLNotFoundFault"),
            "case {case:?}: a refused create must store no ACL, got {status} with \
             {answered}"
        );
    }

    // The same rule on the OTHER write path of a membership, against an ACL that does
    // exist, which the refusals must leave exactly as it was.
    let acl_name = "update-guard-acl";
    let created = ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": acl_name, "UserNames": ["member-one"] }),
        "setup",
    )
    .await;
    assert_eq!(
        member_names(&created["ACL"], "setup"),
        vec!["member-one".to_string()],
        "case \"setup\": expected the created membership, got {created}"
    );
    for (case, to_add, absent) in [
        ("one name added is not a user", vec!["ghost-user"], "ghost-user"),
        (
            "one of two names added is not a user",
            vec!["member-two", "ghost-user"],
            "ghost-user",
        ),
        (
            "the name added is written in mixed case",
            vec!["GHOST-USER"],
            "ghost-user",
        ),
    ] {
        assert_refused(
            &router,
            "UpdateACL",
            &json!({ "ACLName": acl_name, "UserNamesToAdd": to_add }),
            case,
            "UserNotFoundFault",
            &format!("User {absent} not found."),
        )
        .await;
        let described = ok(
            &router,
            "DescribeACLs",
            &json!({ "ACLName": acl_name }),
            case,
        )
        .await;
        assert_eq!(
            member_names(&described["ACLs"][0], case),
            vec!["member-one".to_string()],
            "case {case:?}: a refused update must leave the membership alone, got \
             {described}"
        );
    }

    // The CONTROL: the same two write paths with names that ARE users are served, so the
    // guard refuses the missing member and nothing else.
    let served = ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "served-acl", "UserNames": ["member-one", "Member-Two"] }),
        "every name is a user",
    )
    .await;
    assert_eq!(
        member_names(&served["ACL"], "every name is a user"),
        vec!["member-one".to_string(), "member-two".to_string()],
        "case \"every name is a user\": expected both members, got {served}"
    );
    let updated = ok(
        &router,
        "UpdateACL",
        &json!({ "ACLName": acl_name, "UserNamesToAdd": ["member-two"] }),
        "the name added is a user",
    )
    .await;
    assert_eq!(
        member_names(&updated["ACL"], "the name added is a user"),
        vec!["member-one".to_string(), "member-two".to_string()],
        "case \"the name added is a user\": expected the added member, got {updated}"
    );
}

/// A cluster's references are resolved against state on BOTH of its write paths: the
/// parameter group it runs with, and the subnet group it runs in.
#[tokio::test]
async fn a_cluster_write_only_names_resources_that_exist() {
    let router = router();
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": "present-group", "Family": FAMILY }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "stored-cluster",
            "NodeType": NODE_TYPE,
            "ACLName": BUILTIN_ACL,
            "ParameterGroupName": "present-group",
        }),
        "setup",
    )
    .await;

    // The rows differ in the SHAPE of the name that resolves to nothing: a name of a
    // kind that can be created but was not, one written in mixed case, and the name of a
    // group MemoryDB owns with a character off.
    let absent_groups = [
        "no-such-group",
        "No-Such-Group",
        "default.memorydb-redis99",
    ];
    for (index, group) in absent_groups.iter().enumerate() {
        let folded = group.to_ascii_lowercase();
        let case = format!("CreateCluster names {group:?}");
        let cluster = format!("guarded-cluster-{index}");
        assert_refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": &cluster,
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "ParameterGroupName": group,
            }),
            &case,
            "ParameterGroupNotFoundFault",
            &format!("ParameterGroup not found: {folded}"),
        )
        .await;
        // A refused create wrote nothing.
        let (status, answered) = call(
            &router,
            "DescribeClusters",
            &json!({ "ClusterName": &cluster }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ClusterNotFoundFault"),
            "case {case:?}: a refused create must store no cluster, got {status} with \
             {answered}"
        );

        // And the same reference on the UPDATE path, which must leave the stored
        // cluster pointing where it was.
        let case = format!("UpdateCluster repoints at {group:?}");
        assert_refused(
            &router,
            "UpdateCluster",
            &json!({ "ClusterName": "stored-cluster", "ParameterGroupName": group }),
            &case,
            "ParameterGroupNotFoundFault",
            &format!("ParameterGroup not found: {folded}"),
        )
        .await;
        let described = ok(
            &router,
            "DescribeClusters",
            &json!({ "ClusterName": "stored-cluster" }),
            &case,
        )
        .await;
        assert_eq!(
            described["Clusters"][0]["ParameterGroupName"],
            json!("present-group"),
            "case {case:?}: a refused update must leave the reference alone, got \
             {described}"
        );
    }

    // A subnet group is a resource of this service too, and none can be in state, so
    // every name a cluster write gives for one resolves to nothing.
    for (index, subnet_group) in ["no-such-subnets", "Default"].iter().enumerate() {
        let case = format!("CreateCluster names subnet group {subnet_group:?}");
        assert_refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("subnet-cluster-{index}"),
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "SubnetGroupName": subnet_group,
            }),
            &case,
            "InvalidParameterCombinationException",
            SUBNET_GROUP_NOT_FOUND_MESSAGE,
        )
        .await;
    }

    // The CONTROL: every group that IS in state is accepted on both paths — the one
    // this test created, and one MemoryDB owns in every account, named in mixed case.
    for (index, (case, group, stored)) in [
        ("a group the caller created", "Present-Group", "present-group"),
        (
            "a group MemoryDB owns",
            "DEFAULT.MEMORYDB-REDIS7",
            "default.memorydb-redis7",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let created = ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("served-cluster-{index}"),
                "NodeType": NODE_TYPE,
                "ACLName": BUILTIN_ACL,
                "ParameterGroupName": group,
            }),
            case,
        )
        .await;
        assert_eq!(
            created["Cluster"]["ParameterGroupName"],
            json!(stored),
            "case {case:?}: expected the cluster to point at {stored:?}, got {created}"
        );
        let updated = ok(
            &router,
            "UpdateCluster",
            &json!({ "ClusterName": "stored-cluster", "ParameterGroupName": group }),
            case,
        )
        .await;
        assert_eq!(
            updated["Cluster"]["ParameterGroupName"],
            json!(stored),
            "case {case:?}: expected the repointed cluster, got {updated}"
        );
    }
}

/// `DescribeUsers` takes either the one `UserName` or a set of `Filters`: the two select
/// different things, so a request carrying both is refused rather than served as one of
/// them.
#[tokio::test]
async fn describe_users_refuses_a_name_and_filters_together() {
    let router = router();
    ok(
        &router,
        "CreateUser",
        &create_user_body("filtered-user", FULL_ACCESS),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "filtered-acl", "UserNames": ["filtered-user"] }),
        "setup",
    )
    .await;

    // The rows differ in which property the filters name, how many of them there are,
    // and whether the named user would have satisfied them at all.
    let rows: &[(&str, Value)] = &[
        (
            "one filter on the user name",
            json!({
                "UserName": "filtered-user",
                "Filters": [{ "Name": "user-name", "Values": ["filtered-user"] }],
            }),
        ),
        (
            "one filter on the ACL name",
            json!({
                "UserName": "filtered-user",
                "Filters": [{ "Name": "acl-name", "Values": ["filtered-acl"] }],
            }),
        ),
        (
            "two filters",
            json!({
                "UserName": "filtered-user",
                "Filters": [
                    { "Name": "acl-name", "Values": ["filtered-acl"] },
                    { "Name": "user-name", "Values": ["filtered-user"] },
                ],
            }),
        ),
        (
            "a filter the named user does not satisfy",
            json!({
                "UserName": "filtered-user",
                "Filters": [{ "Name": "acl-name", "Values": ["other-acl"] }],
            }),
        ),
        (
            "a filter naming a property the service does not filter on",
            json!({
                "UserName": "filtered-user",
                "Filters": [{ "Name": "nothing-like-it", "Values": ["x"] }],
            }),
        ),
    ];
    for (case, request) in rows {
        assert_refused(
            &router,
            "DescribeUsers",
            request,
            case,
            "InvalidParameterCombinationException",
            NAME_WITH_FILTERS_MESSAGE,
        )
        .await;
    }

    // The CONTROLS: either member ALONE is served, and a filter list that names no
    // filter selects nothing, so it conflicts with nothing.
    for (case, request) in [
        ("the name alone", json!({ "UserName": "filtered-user" })),
        (
            "the filters alone",
            json!({ "Filters": [{ "Name": "acl-name", "Values": ["filtered-acl"] }] }),
        ),
        (
            "the name beside an empty filter list",
            json!({ "UserName": "filtered-user", "Filters": [] }),
        ),
    ] {
        let answered = ok(&router, "DescribeUsers", &request, case).await;
        assert_eq!(
            answered["Users"][0]["Name"],
            json!("filtered-user"),
            "case {case:?}: expected the one seeded user, got {answered}"
        );
    }
}

/// A free-text `Description` is bounded by the service, and every write path that takes
/// one refuses a longer value before anything is persisted.
#[tokio::test]
async fn a_description_longer_than_the_bound_is_refused_on_every_write_path() {
    let router = router();
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "described-cluster",
            "NodeType": NODE_TYPE,
            "ACLName": BUILTIN_ACL,
            "Description": "the description this update must leave in place",
        }),
        "setup",
    )
    .await;

    // The rows are lengths AT the bound, one past it, and far past it, over each
    // operation that takes a `Description`.
    for (index, length) in [MAX_DESCRIPTION + 1, MAX_DESCRIPTION + 45, 4_096]
        .into_iter()
        .enumerate()
    {
        let description = description_of_length(length);
        for (operation, body) in [
            (
                "CreateParameterGroup",
                json!({
                    "ParameterGroupName": format!("long-group-{index}"),
                    "Family": FAMILY,
                    "Description": &description,
                }),
            ),
            (
                "CreateCluster",
                json!({
                    "ClusterName": format!("long-cluster-{index}"),
                    "NodeType": NODE_TYPE,
                    "ACLName": BUILTIN_ACL,
                    "Description": &description,
                }),
            ),
            (
                "UpdateCluster",
                json!({
                    "ClusterName": "described-cluster",
                    "Description": &description,
                }),
            ),
        ] {
            let case = format!("{operation} with a description of {length} characters");
            assert_refused(
                &router,
                operation,
                &body,
                &case,
                "InvalidParameterValueException",
                TOO_LONG_DESCRIPTION_MESSAGE,
            )
            .await;
        }
    }

    // Nothing the refusals named was written: neither name holds a resource, and the
    // stored cluster still carries the description it was created with.
    for (operation, body, code) in [
        (
            "DescribeParameterGroups",
            json!({ "ParameterGroupName": "long-group-0" }),
            "ParameterGroupNotFoundFault",
        ),
        (
            "DescribeClusters",
            json!({ "ClusterName": "long-cluster-0" }),
            "ClusterNotFoundFault",
        ),
    ] {
        let (status, answered) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, code),
            "case \"a refused create stores nothing\": expected {operation} to report \
             {code}, got {status} with {answered}"
        );
    }
    let described = ok(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": "described-cluster" }),
        "a refused update changes nothing",
    )
    .await;
    assert_eq!(
        described["Clusters"][0]["Description"],
        json!("the description this update must leave in place"),
        "case \"a refused update changes nothing\": got {described}"
    );

    // The CONTROL: a description AT the bound, and a short one, are stored and read
    // back exactly as sent.
    for (index, length) in [1, MAX_DESCRIPTION].into_iter().enumerate() {
        let description = description_of_length(length);
        let case = format!("a description of {length} characters");
        let group = format!("bounded-group-{index}");
        let created = ok(
            &router,
            "CreateParameterGroup",
            &json!({
                "ParameterGroupName": &group,
                "Family": FAMILY,
                "Description": &description,
            }),
            &case,
        )
        .await;
        assert_eq!(
            created["ParameterGroup"]["Description"],
            json!(&description),
            "case {case:?}: expected the description it was created with, got {created}"
        );
        let read_back = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": &group }),
            &case,
        )
        .await;
        assert_eq!(
            read_back["ParameterGroups"][0]["Description"],
            json!(&description),
            "case {case:?}: expected the stored description, got {read_back}"
        );
        let updated = ok(
            &router,
            "UpdateCluster",
            &json!({ "ClusterName": "described-cluster", "Description": &description }),
            &case,
        )
        .await;
        assert_eq!(
            updated["Cluster"]["Description"],
            json!(&description),
            "case {case:?}: expected the updated description, got {updated}"
        );
    }
}

/// Every rule of the engine's ACL syntax, as one access string per row — the catalog of
/// rule forms `User$AccessString` carries (the engine's own syntax: the enabled flag,
/// key patterns with and without a read/write restriction, the keyword that sets a whole
/// permission at once, pub/sub channel patterns, command and category rules, a
/// subcommand, and the reset).
const VALID_ACCESS_STRINGS: &[&str] = &[
    "on ~* &* +@all",
    "off ~app:* resetchannels -@all +get +set",
    "on allkeys allchannels allcommands",
    "on resetkeys ~cache:* &news:* nocommands",
    "on %R~read:* %W~write:* %RW~both:* +@read",
    "on ~* &* +client|no-evict -@admin",
    "reset on ~* &* +@all",
];

/// Access strings that are NOT rule lists of that syntax, in every shape the mistake
/// takes: a word that is no rule at all, a rule of the engine's syntax that a MemoryDB
/// access string does not carry (credentials are set through `AuthenticationMode`), a
/// separator that is not whitespace, and each rule form with nothing named after its
/// sign.
const INVALID_ACCESS_STRINGS: &[&str] = &[
    "invalid-access-string",
    "on ~* &* +@all >Aa1!Aa1!Aa1!Aa1!",
    "on ~* &* +@all nopass",
    "on,~*,+@all",
    "on ~ &* +@all",
    "on ~* & +@all",
    "on ~* &* +",
    "on ~* &* +@",
    "on ~* &* @read",
    "on %X~bad &* +@all",
    "on ~* &* +@all|first",
];

/// `AccessString` is a rule list of the engine's ACL syntax, and both write paths of a
/// user's access string parse it before anything is stored: a string carrying a rule the
/// engine does not define names no permission at all.
#[tokio::test]
async fn an_access_string_must_be_a_rule_list_of_the_engine_syntax() {
    let router = router();

    for (index, access_string) in INVALID_ACCESS_STRINGS.iter().enumerate() {
        let case = format!("CreateUser with {access_string:?}");
        let name = format!("refused-user-{index}");
        assert_refused(
            &router,
            "CreateUser",
            &create_user_body(&name, access_string),
            &case,
            "InvalidParameterValueException",
            INVALID_ACCESS_STRING_MESSAGE,
        )
        .await;
        // A refused create wrote nothing: the name holds no user at all.
        let (status, answered) =
            call(&router, "DescribeUsers", &json!({ "UserName": &name })).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "UserNotFoundFault"),
            "case {case:?}: a refused create must store no user, got {status} with \
             {answered}"
        );
    }

    // Every valid row is served on the create path and read back with an access string
    // the service stores for it.
    for (index, access_string) in VALID_ACCESS_STRINGS.iter().enumerate() {
        let case = format!("CreateUser with {access_string:?}");
        let name = format!("served-user-{index}");
        let created = ok(
            &router,
            "CreateUser",
            &create_user_body(&name, access_string),
            &case,
        )
        .await;
        let stored = created["User"]["AccessString"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stored.is_empty(),
            "case {case:?}: expected the stored access string, got {created}"
        );
        let read_back = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": &name }),
            &case,
        )
        .await;
        assert_eq!(
            read_back["Users"][0]["AccessString"],
            json!(stored),
            "case {case:?}: the describe must report what the create stored, got \
             {read_back}"
        );
    }

    // The same rule on the OTHER write path of an access string: every refusal leaves
    // the stored user exactly as it was, and a valid replacement is applied.
    let name = "updated-user";
    let created = ok(
        &router,
        "CreateUser",
        &create_user_body(name, FULL_ACCESS),
        "setup",
    )
    .await;
    let stored = created["User"]["AccessString"].clone();
    for access_string in INVALID_ACCESS_STRINGS {
        let case = format!("UpdateUser with {access_string:?}");
        assert_refused(
            &router,
            "UpdateUser",
            &json!({ "UserName": name, "AccessString": access_string }),
            &case,
            "InvalidParameterValueException",
            INVALID_ACCESS_STRING_MESSAGE,
        )
        .await;
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": name }),
            &case,
        )
        .await;
        assert_eq!(
            described["Users"][0]["AccessString"], stored,
            "case {case:?}: a refused update must leave the access string alone, got \
             {described}"
        );
    }
    let updated = ok(
        &router,
        "UpdateUser",
        &json!({ "UserName": name, "AccessString": VALID_ACCESS_STRINGS[1] }),
        "a valid replacement",
    )
    .await;
    assert_ne!(
        updated["User"]["AccessString"], stored,
        "case \"a valid replacement\": expected the replacement to be applied, got \
         {updated}"
    );
}
