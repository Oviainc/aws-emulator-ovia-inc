// SPDX-License-Identifier: CC0-1.0

//! `DescribeGroup` end to end over HTTP: a group read is the stored group record.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes. The
//! rule under test is the one a directory object read states: a describe reports the
//! record the create stored, under the account name matched case insensitively and in
//! the realm the directory holds its objects in, projecting only the `OtherAttributes`
//! the request asked for; a name the directory holds no such object under is
//! `ResourceNotFoundException`, and the read leaves the store exactly as it found it.
//!
//! The rows of each table vary what the rule generalises over — which members the
//! create carried, the enumerated values, the name's length, case and character
//! classes, and the attribute types and counts — because the answer is the stored
//! record whatever those are.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::validation_envelope::{call, emulator, Answer, DIRECTORY};

/// A second `DirectoryId` (`^d-[0-9a-f]{10}$`), for the cases that are about one
/// directory not answering for another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// `GroupType`'s modelled values, from
/// `smithy_query(query="shape", shape="GroupType")` — "Distribution", "Security".
const GROUP_TYPES: [&str; 2] = ["Distribution", "Security"];

/// `GroupScope`'s modelled values, from
/// `smithy_query(query="shape", shape="GroupScope")` — "DomainLocal", "Global",
/// "Universal", "BuiltinLocal".
const GROUP_SCOPES: [&str; 4] = ["DomainLocal", "Global", "Universal", "BuiltinLocal"];

/// The lowest RID Active Directory hands to an object it creates: everything below
/// 1000 is reserved for the built-in principals.
const FIRST_RID: u64 = 1000;

/// `GroupName`'s `@length` maximum, from `DescribeGroup`'s input
/// (`smithy_query(query="operation", operation="DescribeGroup")` — len 1..64).
const GROUP_NAME_CEILING: usize = 64;

async fn create_group(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> Answer {
    call(router, "/Groups/CreateGroup", DIRECTORY, body).await
}

async fn create_group_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory: &str,
    body: &Value,
) -> Answer {
    call(router, "/Groups/CreateGroup", directory, body).await
}

async fn create_user(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> Answer {
    call(router, "/Users/CreateUser", DIRECTORY, body).await
}

async fn describe_group(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> Answer {
    call(router, "/Groups/DescribeGroup", DIRECTORY, body).await
}

async fn describe_group_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory: &str,
    body: &Value,
) -> Answer {
    call(router, "/Groups/DescribeGroup", directory, body).await
}

/// A string member of an answer, or the empty string when it carries none.
fn text(answer: &Answer, member: &str) -> String {
    answer.body[member].as_str().unwrap_or_default().to_string()
}

/// The `DC=` form of a realm: `corp.example.com` → `DC=corp,DC=example,DC=com`.
///
/// Built from the realm the SAME answer reports, so what is pinned is the derivation
/// rule — a distinguished name names the object's container inside its own domain —
/// and not whichever domain name the emulator's directories carry.
fn expected_distinguished_name(realm: &str, common_name: &str) -> String {
    let components = realm
        .trim_end_matches('.')
        .split('.')
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("CN={common_name},CN=Users,{components}")
}

/// A realm is a domain name: `Realm`'s `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$` in
/// the form this suite needs it — at least two labels of letters, digits and hyphens.
fn looks_like_a_realm(value: &str) -> bool {
    let labels: Vec<&str> = value.trim_end_matches('.').split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

/// The domain SID and the RID of a Windows SID `S-1-5-21-a-b-c-rid`, or `None` when
/// `value` is not of that form.
///
/// `S-1-5` is the NT authority and `21` the sub-authority every domain SID carries; the
/// three values after it identify the domain, each a 32-bit sub-authority, and the last
/// is the object's RID.
fn split_sid(value: &str) -> Option<(String, u64)> {
    let rest = value.strip_prefix("S-1-5-21-")?;
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut numbers = Vec::new();
    for part in &parts {
        numbers.push(part.parse::<u64>().ok()?);
    }
    if numbers[..3].iter().any(|value| *value > u64::from(u32::MAX)) {
        return None;
    }
    let domain = format!("S-1-5-21-{}-{}-{}", numbers[0], numbers[1], numbers[2]);
    Some((domain, numbers[3]))
}

/// One group case: what the create carried, keyed by what makes it different from the
/// others. The rule is the same for every row — the describe answers with what the
/// create stored — so the rows vary the members, their values and their forms.
fn group_cases() -> Vec<(String, Value)> {
    let mut cases = vec![
        (
            "only the required members".to_string(),
            json!({"SAMAccountName": "engineers"}),
        ),
        (
            "one-character name, the shortest GroupName admits".to_string(),
            json!({"SAMAccountName": "g"}),
        ),
        (
            format!("{GROUP_NAME_CEILING}-character name, the longest GroupName admits"),
            json!({"SAMAccountName": "g".repeat(GROUP_NAME_CEILING)}),
        ),
        (
            "a name carrying character classes beyond letters".to_string(),
            json!({"SAMAccountName": "my-group_1.svc (eu)"}),
        ),
        (
            "a mixed-case name".to_string(),
            json!({"SAMAccountName": "MixedCaseGroup"}),
        ),
        (
            "attributes of all four AttributeValue types".to_string(),
            json!({
                "SAMAccountName": "attrgroup",
                "GroupType": "Distribution",
                "GroupScope": "Universal",
                "OtherAttributes": {
                    "description": {"S": "the whole attribute catalogue"},
                    "groupType": {"N": 4},
                    "isCriticalSystemObject": {"BOOL": false},
                    "servicePrincipalName": {"SS": ["host/a.corp.example.com", "host/b.corp.example.com"]},
                },
            }),
        ),
        (
            "a single attribute, the fewest Attributes admits".to_string(),
            json!({
                "SAMAccountName": "oneattrgroup",
                "OtherAttributes": {"info": {"S": "one attribute"}},
            }),
        ),
    ];
    // Every (GroupType, GroupScope) pair the model admits is a group the describe
    // reports back under exactly those values.
    for (type_index, group_type) in GROUP_TYPES.iter().enumerate() {
        for (scope_index, group_scope) in GROUP_SCOPES.iter().enumerate() {
            cases.push((
                format!("GroupType {group_type} with GroupScope {group_scope}"),
                json!({
                    "SAMAccountName": format!("group-{type_index}-{scope_index}"),
                    "GroupType": group_type,
                    "GroupScope": group_scope,
                }),
            ));
        }
    }
    cases
}

/// `CreateGroup` persists a group and `DescribeGroup` answers from that record: every
/// member of the describe is the value the create stored, the SID is minted once and
/// kept, and the enumerated members come back as the create named them.
#[tokio::test]
async fn a_created_group_is_described_from_its_stored_record() {
    let router = emulator();
    let mut sids: BTreeMap<String, String> = BTreeMap::new();
    let mut domains: BTreeSet<String> = BTreeSet::new();
    // What a group whose create named no GroupType/GroupScope is reported as: one
    // value from each modelled set, and the same one for every such row.
    let mut defaults: BTreeSet<(String, String)> = BTreeSet::new();

    for (case, request) in group_cases() {
        let name = request["SAMAccountName"]
            .as_str()
            .expect("every case names a group")
            .to_string();

        let created = create_group(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateGroup must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let created_sid = text(&created, "SID");

        // Ask for every attribute the create carried, so the projection has something
        // to answer with.
        let wanted: Vec<String> = request["OtherAttributes"]
            .as_object()
            .map(|attributes| attributes.keys().cloned().collect())
            .unwrap_or_default();
        let mut describe_request = json!({"SAMAccountName": name});
        if !wanted.is_empty() {
            describe_request["OtherAttributes"] = json!(wanted);
        }
        let described = describe_group(&router, &describe_request).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeGroup must find the group the create stored, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            text(&described, "DirectoryId"),
            DIRECTORY,
            "case {case:?}: DescribeGroup must report the directory it read, got {:?}",
            described.body
        );
        assert_eq!(
            text(&described, "SAMAccountName"),
            name,
            "case {case:?}: DescribeGroup must report the stored name, got {:?}",
            described.body
        );

        let sid = text(&described, "SID");
        assert_eq!(
            sid, created_sid,
            "case {case:?}: DescribeGroup must report the SID the create minted, got {:?}",
            described.body
        );
        let (domain, rid) = split_sid(&sid).unwrap_or_else(|| {
            panic!("case {case:?}: SID {sid:?} is not of the form S-1-5-21-a-b-c-rid")
        });
        assert!(
            rid >= FIRST_RID,
            "case {case:?}: SID {sid:?} must carry a RID at or above {FIRST_RID}, got {rid}"
        );
        assert!(
            sids.values().all(|held| *held != sid),
            "case {case:?}: SID {sid:?} was already handed to another object: {sids:?}"
        );
        sids.insert(name.clone(), sid.clone());
        domains.insert(domain);

        let realm = text(&described, "Realm");
        assert!(
            looks_like_a_realm(&realm),
            "case {case:?}: DescribeGroup must report the directory's domain, got {realm:?}"
        );
        assert_eq!(
            text(&described, "DistinguishedName"),
            expected_distinguished_name(&realm, &name),
            "case {case:?}: the distinguished name must name the object inside the realm \
             the same answer reports, got {:?}",
            described.body
        );

        let group_type = text(&described, "GroupType");
        let group_scope = text(&described, "GroupScope");
        assert!(
            GROUP_TYPES.contains(&group_type.as_str()),
            "case {case:?}: GroupType must be one of {GROUP_TYPES:?}, got {group_type:?}"
        );
        assert!(
            GROUP_SCOPES.contains(&group_scope.as_str()),
            "case {case:?}: GroupScope must be one of {GROUP_SCOPES:?}, got {group_scope:?}"
        );
        match (request.get("GroupType"), request.get("GroupScope")) {
            (Some(wanted_type), Some(wanted_scope)) => {
                assert_eq!(
                    json!(group_type),
                    *wanted_type,
                    "case {case:?}: GroupType must be the one the create named, got {:?}",
                    described.body
                );
                assert_eq!(
                    json!(group_scope),
                    *wanted_scope,
                    "case {case:?}: GroupScope must be the one the create named, got {:?}",
                    described.body
                );
            }
            _ => {
                defaults.insert((group_type, group_scope));
            }
        }

        assert_eq!(
            described.body["OtherAttributes"],
            request
                .get("OtherAttributes")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the requested attributes must be the stored ones, got {:?}",
            described.body
        );
    }

    // Every object of one directory extends that directory's domain SID: the RID is
    // what distinguishes them.
    assert_eq!(
        domains.len(),
        1,
        "one directory is one domain, got {domains:?}"
    );
    assert_eq!(
        defaults.len(),
        1,
        "a create that names no GroupType or GroupScope must store one answer, got {defaults:?}"
    );

    // The store answers for the groups it holds, not for the request: a name nothing
    // created is a 404 even though its spelling is legal.
    let missing = describe_group(&router, &json!({"SAMAccountName": "nosuchgroup"})).await;
    assert_eq!(
        missing.status, 404,
        "a name nothing created must not be described, got {} with {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {:?}",
        missing.body
    );
    assert_eq!(
        missing.error_type, "ResourceNotFoundException",
        "the refusal must name its type on the wire, got {:?}",
        missing.error_type
    );

    // And a group of one directory is not a group of another.
    let elsewhere = describe_group_in(
        &router,
        OTHER_DIRECTORY,
        &json!({"SAMAccountName": "engineers"}),
    )
    .await;
    assert_eq!(
        elsewhere.status, 404,
        "a group of one directory must not be described in another, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );
}

/// An account name is case insensitive, so a describe finds the group whatever case it
/// spells the name in — and answers with the spelling the create stored. A name that
/// belongs to a user is not a group, because one directory resolves one name to one
/// object.
#[tokio::test]
async fn a_group_name_resolves_case_insensitively() {
    let router = emulator();
    let stored = "PayrollAdmins";
    create_group(&router, &json!({"SAMAccountName": stored})).await;
    let sid = text(
        &describe_group(&router, &json!({"SAMAccountName": stored})).await,
        "SID",
    );

    for spelling in [
        stored.to_string(),
        stored.to_lowercase(),
        stored.to_uppercase(),
        "payrollADMINS".to_string(),
    ] {
        let described = describe_group(&router, &json!({"SAMAccountName": spelling})).await;
        assert_eq!(
            described.status, 200,
            "case {spelling:?}: an account name is case insensitive, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            text(&described, "SAMAccountName"),
            stored,
            "case {spelling:?}: the answer must carry the stored spelling, got {:?}",
            described.body
        );
        assert_eq!(
            text(&described, "SID"),
            sid,
            "case {spelling:?}: it must read the same object, got {:?}",
            described.body
        );
    }

    // A user is not a group: the group read answers only for group objects.
    create_user(&router, &json!({"SAMAccountName": "jdoe"})).await;
    let described = describe_group(&router, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        described.status, 404,
        "a user object must not be described as a group, got {} with {:?}",
        described.status, described.body
    );
}

/// Only the attributes the request names come back, matched case insensitively and
/// reported under the names the create stored them as.
#[tokio::test]
async fn describe_group_projects_only_the_requested_attributes() {
    let router = emulator();
    let stored = json!({
        "description": {"S": "the finance department"},
        "info": {"S": "owned by finance"},
        "groupType": {"N": 2},
        "isCriticalSystemObject": {"BOOL": true},
        "servicePrincipalName": {"SS": ["host/finance.corp.example.com"]},
    });
    create_group(
        &router,
        &json!({"SAMAccountName": "finance", "OtherAttributes": stored}),
    )
    .await;

    // Each row asks for a different slice of the stored attributes; the answer is that
    // slice, keyed as stored. `Attributes` is `@length(min: 1, max: 25)`, so a slice
    // with nothing in it is no member at all.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no attributes asked for",
            Value::Null,
            Value::Null,
        ),
        (
            "one of the stored attributes",
            json!(["description"]),
            json!({"description": stored["description"]}),
        ),
        (
            "a subset of three",
            json!(["description", "groupType", "servicePrincipalName"]),
            json!({
                "description": stored["description"],
                "groupType": stored["groupType"],
                "servicePrincipalName": stored["servicePrincipalName"],
            }),
        ),
        (
            "every stored attribute",
            json!([
                "description",
                "info",
                "groupType",
                "isCriticalSystemObject",
                "servicePrincipalName"
            ]),
            stored.clone(),
        ),
        (
            "an attribute name in another case",
            json!(["DESCRIPTION", "Info"]),
            json!({"description": stored["description"], "info": stored["info"]}),
        ),
        (
            "a name the group holds no attribute under",
            json!(["notStored"]),
            Value::Null,
        ),
        (
            "one held name beside one that is not",
            json!(["notStored", "info"]),
            json!({"info": stored["info"]}),
        ),
    ];

    for (case, asked, expected) in cases {
        let mut request = json!({"SAMAccountName": "finance"});
        if !asked.is_null() {
            request["OtherAttributes"] = asked;
        }
        let described = describe_group(&router, &request).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeGroup must be accepted, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["OtherAttributes"], expected,
            "case {case:?}: only the attributes asked for come back, got {:?}",
            described.body
        );
    }
}

/// `Realm` names the domain a read reads, and a read changes nothing: a group asked
/// for in another realm, or in a directory nothing was written to, is not found, and a
/// create after those reads still gets the directory's first RID.
#[tokio::test]
async fn a_realm_names_the_domain_a_group_read_reads() {
    let router = emulator();
    create_group(&router, &json!({"SAMAccountName": "engineers"})).await;
    let own_realm = text(
        &describe_group(&router, &json!({"SAMAccountName": "engineers"})).await,
        "Realm",
    );
    assert!(
        looks_like_a_realm(&own_realm),
        "the directory must report a domain name, got {own_realm:?}"
    );
    let sid = text(
        &describe_group(&router, &json!({"SAMAccountName": "engineers"})).await,
        "SID",
    );

    // The directory's own realm, in any case, reads the directory's objects: `Realm`
    // "is case insensitive".
    let mixed_case: String = own_realm
        .chars()
        .enumerate()
        .map(|(position, character)| {
            if position % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect();
    for spelling in [
        own_realm.clone(),
        own_realm.to_uppercase(),
        own_realm.to_lowercase(),
        mixed_case,
    ] {
        let described = describe_group(
            &router,
            &json!({"SAMAccountName": "engineers", "Realm": spelling}),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {spelling:?}: the directory's own realm is case insensitive, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            text(&described, "SID"),
            sid,
            "case {spelling:?}: it must read the same object, got {:?}",
            described.body
        );
        assert_eq!(
            text(&described, "Realm"),
            own_realm,
            "case {spelling:?}: the answer must report the directory's own domain, got {:?}",
            described.body
        );
    }

    // Another realm is a domain this directory holds no groups in.
    let other = describe_group(
        &router,
        &json!({"SAMAccountName": "engineers", "Realm": "trusted.example.net"}),
    )
    .await;
    assert_eq!(
        other.status, 404,
        "a group of another realm is not held here, got {} with {:?}",
        other.status, other.body
    );

    // A read never materialises a directory: the first write to OTHER_DIRECTORY still
    // gets that directory's first RID, so nothing the reads did was persisted.
    let missing = describe_group_in(
        &router,
        OTHER_DIRECTORY,
        &json!({"SAMAccountName": "engineers"}),
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "a directory nothing was written to holds no groups, got {} with {:?}",
        missing.status, missing.body
    );
    let created = create_group_in(
        &router,
        OTHER_DIRECTORY,
        &json!({"SAMAccountName": "engineers"}),
    )
    .await;
    let created_sid = text(&created, "SID");
    let (other_domain, rid) = split_sid(&created_sid)
        .unwrap_or_else(|| panic!("SID {created_sid:?} is not of the form S-1-5-21-a-b-c-rid"));
    assert_eq!(
        rid, FIRST_RID,
        "the reads must consume no RID, so the first write gets {FIRST_RID}, got {rid}"
    );

    // And two directories are two domains.
    let (own_domain, _) = split_sid(&sid).expect("the first group's SID is a Windows SID");
    assert_ne!(
        own_domain, other_domain,
        "two directories must not share one domain SID, got {own_domain:?}"
    );
}
