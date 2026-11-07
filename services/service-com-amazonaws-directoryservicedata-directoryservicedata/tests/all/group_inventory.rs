// SPDX-License-Identifier: CC0-1.0

//! `ListGroups` over HTTP: the group inventory of a directory.
//!
//! The rule under test: `ListGroups` reports every group the directory holds — the
//! built-in groups an Active Directory domain is created with as much as the groups
//! `CreateGroup` stored — as one page of `GroupSummary` in `SAMAccountName` order, no
//! longer than `MaxResults`, with a `NextToken` exactly when more groups follow, and it
//! reports a directory only to a request addressed to that directory's own realm.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. The emulator and the request walk come from the `validation_envelope`
//! suite, which owns this crate's test harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, emulator, Answer, DIRECTORY, OTHER_DIRECTORY};

/// The largest page `MaxResults` admits (`@range(min: 1, max: 250)`), spelled out here
/// rather than imported so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 250;

/// The `GroupType` every built-in group carries: each one exists to carry rights, and
/// `GroupType`'s modelled values are "Distribution" and "Security".
const SECURITY: &str = "Security";

/// Where a built-in group's SID comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    /// A group of the domain: the directory's own domain SID plus this relative
    /// identifier (RID).
    Domain(u64),
    /// A group of the local `BUILTIN` authority: `S-1-5-32-<rid>` in every domain.
    Builtin(u64),
}

/// Built-in groups of an Active Directory domain, with the well-known SID and group
/// scope Microsoft documents for each one (its "Security identifiers" reference: the
/// domain-relative RIDs 498..=572 and the `S-1-5-32-*` SIDs of the `BUILTIN`
/// authority). A sample, spanning both authorities and every scope a built-in group
/// takes; `Domain Users` is only one row of it.
const BUILTIN_SAMPLE: &[(&str, &str, Authority)] = &[
    ("Domain Users", "Global", Authority::Domain(513)),
    ("Domain Admins", "Global", Authority::Domain(512)),
    ("Domain Computers", "Global", Authority::Domain(515)),
    ("Enterprise Admins", "Universal", Authority::Domain(519)),
    ("Schema Admins", "Universal", Authority::Domain(518)),
    ("Cert Publishers", "DomainLocal", Authority::Domain(517)),
    (
        "Denied RODC Password Replication Group",
        "DomainLocal",
        Authority::Domain(572),
    ),
    ("Administrators", "BuiltinLocal", Authority::Builtin(544)),
    ("Guests", "BuiltinLocal", Authority::Builtin(546)),
    ("Backup Operators", "BuiltinLocal", Authority::Builtin(551)),
    (
        "Remote Management Users",
        "BuiltinLocal",
        Authority::Builtin(580),
    ),
];

async fn list_groups(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/ListGroups", directory, body).await
}

async fn create_group(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/CreateGroup", directory, body).await
}

async fn create_user(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/CreateUser", directory, body).await
}

async fn list_users(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/ListUsers", directory, body).await
}

/// A string member of an answer, or the empty string when it carries none.
fn text(answer: &Answer, member: &str) -> String {
    answer.body[member].as_str().unwrap_or_default().to_string()
}

/// The `Groups` an answer reported, in the order it reported them.
fn groups(answer: &Answer) -> Vec<Value> {
    answer.body["Groups"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListGroups must report a Groups list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `SAMAccountName`s an answer reported, in order.
fn group_names(answer: &Answer) -> Vec<String> {
    groups(answer)
        .iter()
        .map(|group| {
            group["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The reported groups keyed by `SAMAccountName`.
fn by_name(answer: &Answer) -> BTreeMap<String, Value> {
    groups(answer)
        .iter()
        .map(|group| {
            (
                group["SAMAccountName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                group.clone(),
            )
        })
        .collect()
}

/// The `NextToken` an answer carried, if it carried one.
fn next_token(answer: &Answer) -> Option<String> {
    answer.body["NextToken"].as_str().map(str::to_string)
}

/// A group's `SID` as the answer reported it.
fn sid_of(group: &Value) -> String {
    group["SID"].as_str().unwrap_or_default().to_string()
}

/// The domain SID a minted SID `S-1-5-21-a-b-c-<rid>` belongs to.
fn domain_part(sid: &str) -> String {
    let (domain, _) = sid
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("SID {sid:?} must end in a relative identifier"));
    domain.to_string()
}

/// The `ListGroups` answer with the whole inventory in it: one page as large as
/// `MaxResults` admits.
async fn whole_inventory(router: &MultiServiceRouter, directory: &str) -> Answer {
    let answer = list_groups(
        router,
        directory,
        &json!({"MaxResults": MAX_RESULTS_CEILING}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "ListGroups must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// A directory holds the built-in groups of an Active Directory domain: each one is
/// reported with the well-known SID and scope its domain gives it, and nothing in this
/// data plane created any of them.
#[tokio::test]
async fn the_inventory_holds_the_builtin_groups_of_the_domain() {
    let router = emulator();
    // A write this test makes to the directory, so what follows cannot be read as this
    // test having created any group.
    let created_user = create_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        created_user.status, 200,
        "CreateUser must be accepted, got {} with {:?}",
        created_user.status, created_user.body
    );
    let domain_sid = domain_part(&text(&created_user, "SID"));

    let listed = whole_inventory(&router, DIRECTORY).await;
    let held = by_name(&listed);
    for (name, scope, authority) in BUILTIN_SAMPLE {
        let group = held
            .get(*name)
            .unwrap_or_else(|| panic!("case {name:?}: the inventory must hold it, got {held:?}"));
        let expected_sid = match authority {
            Authority::Domain(rid) => format!("{domain_sid}-{rid}"),
            Authority::Builtin(rid) => format!("S-1-5-32-{rid}"),
        };
        assert_eq!(
            sid_of(group),
            expected_sid,
            "case {name:?}: expected the well-known SID {expected_sid:?}, got {group:?}"
        );
        assert_eq!(
            group["GroupScope"],
            json!(scope),
            "case {name:?}: expected GroupScope {scope:?}, got {group:?}"
        );
        assert_eq!(
            group["GroupType"],
            json!(SECURITY),
            "case {name:?}: a built-in group is a security group, got {group:?}"
        );
    }

    // The inventory is of GROUPS: a user is not one of them.
    assert!(
        !group_names(&listed).contains(&"jdoe".to_string()),
        "a user must not be reported as a group, got {:?}",
        group_names(&listed)
    );

    // One object has one SID: no two groups of a directory share one.
    let mut sids: Vec<String> = groups(&listed).iter().map(sid_of).collect();
    let total = sids.len();
    sids.sort();
    sids.dedup();
    assert_eq!(
        sids.len(),
        total,
        "each group must carry its own SID, got {:?}",
        groups(&listed)
    );
}

/// A built-in group of the DOMAIN carries its own directory's domain SID, while one of
/// the local `BUILTIN` authority carries the same SID in every domain.
#[tokio::test]
async fn a_builtin_sid_is_domain_relative_only_where_the_domain_owns_it() {
    let router = emulator();
    for directory in [DIRECTORY, OTHER_DIRECTORY] {
        create_user(&router, directory, &json!({"SAMAccountName": "jdoe"})).await;
    }

    let here = by_name(&whole_inventory(&router, DIRECTORY).await);
    let there = by_name(&whole_inventory(&router, OTHER_DIRECTORY).await);

    for (name, _, authority) in BUILTIN_SAMPLE {
        let mine = here
            .get(*name)
            .map(sid_of)
            .unwrap_or_else(|| panic!("case {name:?}: {DIRECTORY} must hold it"));
        let theirs = there
            .get(*name)
            .map(sid_of)
            .unwrap_or_else(|| panic!("case {name:?}: {OTHER_DIRECTORY} must hold it"));
        match authority {
            Authority::Domain(_) => assert_ne!(
                mine, theirs,
                "case {name:?}: a group of the domain must carry its own domain's SID, \
                 got {mine:?} in both"
            ),
            Authority::Builtin(_) => assert_eq!(
                mine, theirs,
                "case {name:?}: a group of the BUILTIN authority has one SID in every \
                 domain, got {mine:?} and {theirs:?}"
            ),
        }
    }
}

/// A group `CreateGroup` stored is reported beside the built-in ones, with what the
/// create stored, and only in the directory it was created in.
#[tokio::test]
async fn a_created_group_joins_the_inventory_of_its_own_directory() {
    let router = emulator();
    // The rows differ in what decides the stored record: the name's form and case, and
    // whether the create named a type or a scope at all.
    let cases: [(&str, Value, &str, &str); 5] = [
        (
            "only the required member",
            json!({"SAMAccountName": "engineers"}),
            SECURITY,
            "Global",
        ),
        (
            "a distribution group",
            json!({"SAMAccountName": "all-staff", "GroupType": "Distribution"}),
            "Distribution",
            "Global",
        ),
        (
            "a universal group",
            json!({"SAMAccountName": "Zebra.Team", "GroupScope": "Universal"}),
            SECURITY,
            "Universal",
        ),
        (
            "a domain-local group",
            json!({
                "SAMAccountName": "APAC_Admins",
                "GroupType": "Security",
                "GroupScope": "DomainLocal",
            }),
            SECURITY,
            "DomainLocal",
        ),
        (
            "a name that sorts before every built-in one",
            json!({"SAMAccountName": "a", "GroupScope": "BuiltinLocal"}),
            SECURITY,
            "BuiltinLocal",
        ),
    ];

    let mut domain_sid = String::new();
    for (case, request, _, _) in &cases {
        let created = create_group(&router, DIRECTORY, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateGroup must be accepted, got {} with {:?}",
            created.status, created.body
        );
        domain_sid = domain_part(&text(&created, "SID"));
    }

    let listed = whole_inventory(&router, DIRECTORY).await;
    let held = by_name(&listed);
    for (case, request, group_type, group_scope) in &cases {
        let name = request["SAMAccountName"].as_str().expect("a name");
        let group = held
            .get(name)
            .unwrap_or_else(|| panic!("case {case:?}: {name:?} must be reported, got {held:?}"));
        assert_eq!(
            group["GroupType"],
            json!(group_type),
            "case {case:?}: expected GroupType {group_type:?}, got {group:?}"
        );
        assert_eq!(
            group["GroupScope"],
            json!(group_scope),
            "case {case:?}: expected GroupScope {group_scope:?}, got {group:?}"
        );
        let sid = sid_of(group);
        assert_eq!(
            domain_part(&sid),
            domain_sid,
            "case {case:?}: a created group's SID extends its own domain's, got {sid:?}"
        );
    }

    // The built-in groups are still there, and the whole inventory is reported in
    // case-insensitive name order.
    assert!(
        held.contains_key("Domain Users"),
        "creating a group must not displace the built-in ones, got {held:?}"
    );
    let reported = group_names(&listed);
    let mut sorted = reported.clone();
    sorted.sort_by_key(|name| name.to_lowercase());
    assert_eq!(
        reported, sorted,
        "the inventory must be reported in SAMAccountName order, got {reported:?}"
    );

    // A directory nothing wrote to has no objects at all — not even the built-in
    // groups, which belong to a directory this emulator holds.
    let elsewhere = whole_inventory(&router, OTHER_DIRECTORY).await;
    assert!(
        groups(&elsewhere).is_empty(),
        "a directory this emulator holds nothing for reports no groups, got {:?}",
        elsewhere.body
    );
    // Reading it must not have brought it into being either.
    assert!(
        groups(&whole_inventory(&router, OTHER_DIRECTORY).await).is_empty(),
        "a read must not materialise a directory"
    );
}

/// `MaxResults` is a hard page size, and `NextToken` resumes exactly after the group the
/// previous page ended on: every page size walks the same inventory.
#[tokio::test]
async fn the_inventory_pages_through_in_a_stable_order() {
    let router = emulator();
    for name in ["engineers", "Zebra", "aardvarks", "mid-level"] {
        create_group(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
    }
    let whole = whole_inventory(&router, DIRECTORY).await;
    let expected = group_names(&whole);
    assert!(
        next_token(&whole).is_none(),
        "a page holding the whole inventory must carry no NextToken, got {:?}",
        whole.body
    );
    assert!(
        expected.len() > 4,
        "the inventory must hold the built-in groups as well, got {expected:?}"
    );

    for page_size in [1_i64, 2, 7, 13, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = list_groups(&router, DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case MaxResults={page_size}: expected HTTP 200, got {} with {:?}",
                page.status, page.body
            );
            let names = group_names(&page);
            assert!(
                i64::try_from(names.len()).unwrap_or(i64::MAX) <= page_size,
                "case MaxResults={page_size}: a page must hold no more than it asked for, \
                 got {names:?}"
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxResults={page_size}: the walk must end, got {walked:?}"
            );
            match next_token(&page) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case MaxResults={page_size}: paging must walk the whole inventory once, in order"
        );
    }
}

/// A `NextToken` is only ever spent on the walk it was issued for: anything else is a
/// `ValidationException`, never a page read from the wrong position.
#[tokio::test]
async fn a_token_from_another_walk_is_refused() {
    let router = emulator();
    for name in ["engineers", "Zebra"] {
        create_group(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
    }
    create_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    create_user(&router, DIRECTORY, &json!({"SAMAccountName": "asmith"})).await;
    create_group(
        &router,
        OTHER_DIRECTORY,
        &json!({"SAMAccountName": "engineers"}),
    )
    .await;

    let own = next_token(&list_groups(&router, DIRECTORY, &json!({"MaxResults": 1})).await)
        .expect("a first page of one must carry a NextToken");
    let from_users = next_token(&list_users(&router, DIRECTORY, &json!({"MaxResults": 1})).await)
        .expect("a ListUsers page of one must carry a NextToken");
    let from_other_directory =
        next_token(&list_groups(&router, OTHER_DIRECTORY, &json!({"MaxResults": 1})).await)
            .expect("the other directory's first page must carry a NextToken");

    let cases = [
        (
            "a token this service never issued",
            "not-a-token".to_string(),
        ),
        ("a truncated token", own[..own.len() / 2].to_string()),
        ("a token with a flipped byte", flip_last(&own)),
        ("a ListUsers token", from_users),
        ("another directory's token", from_other_directory),
    ];
    for (case, token) in cases {
        let refused = list_groups(&router, DIRECTORY, &json!({"NextToken": token})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // The non-vacuity guard: the directory's own token does resume the walk.
    let resumed = list_groups(&router, DIRECTORY, &json!({"NextToken": own})).await;
    assert_eq!(
        resumed.status, 200,
        "the token this walk issued must resume it, got {} with {:?}",
        resumed.status, resumed.body
    );
}

/// The last character of `token`, replaced — a token that was edited, not issued.
fn flip_last(token: &str) -> String {
    let mut flipped = token.to_string();
    let last = flipped.pop().expect("a non-empty token");
    let replacement = if last == '0' { '1' } else { '0' };
    flipped.push(replacement);
    flipped
}

/// `Realm` names the domain to read from: the directory's own realm (in any case)
/// reports its inventory, and any other realm reports none of it while echoing the
/// realm that was asked for.
#[tokio::test]
async fn a_realm_decides_which_domain_is_reported() {
    let router = emulator();
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;

    let own = whole_inventory(&router, DIRECTORY).await;
    let realm = text(&own, "Realm");
    assert!(
        !realm.is_empty(),
        "ListGroups must report the realm it answered for, got {:?}",
        own.body
    );
    let expected = group_names(&own);

    let same_domain = [
        ("the realm as reported", realm.clone()),
        ("the realm upper-cased", realm.to_uppercase()),
        ("the realm with mixed case", mixed_case(&realm)),
    ];
    for (case, requested) in same_domain {
        let answer = list_groups(
            &router,
            DIRECTORY,
            &json!({"Realm": requested, "MaxResults": MAX_RESULTS_CEILING}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            group_names(&answer),
            expected,
            "case {case:?}: a realm is case insensitive, so the same inventory must be \
             reported, got {:?}",
            answer.body
        );
        assert_eq!(
            text(&answer, "Realm"),
            requested,
            "case {case:?}: the answer must report the realm that was asked for, got {:?}",
            answer.body
        );
    }

    for case in ["example.org", "trusted.corp.example.com", "corp.example"] {
        let answer = list_groups(
            &router,
            DIRECTORY,
            &json!({"Realm": case, "MaxResults": MAX_RESULTS_CEILING}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            groups(&answer).is_empty(),
            "case {case:?}: this directory holds no groups in another domain, got {:?}",
            answer.body
        );
        assert_eq!(
            text(&answer, "Realm"),
            case,
            "case {case:?}: the answer must report the realm that was asked for, got {:?}",
            answer.body
        );
        assert_eq!(
            text(&answer, "DirectoryId"),
            DIRECTORY,
            "case {case:?}: the answer must report the directory it was addressed to, got {:?}",
            answer.body
        );
    }
}

/// `value` with every other character upper-cased.
fn mixed_case(value: &str) -> String {
    value
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}
