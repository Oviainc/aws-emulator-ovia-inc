// SPDX-License-Identifier: CC0-1.0

//! `SearchGroups` over HTTP: which groups a search reports, and which it must not.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. The emulator and the request walk come from the `validation_envelope`
//! suite, which owns this crate's test harness.
//!
//! The rule under test is the one the operation states: a directory's group is reported
//! when the value it holds for at least one of the attributes the request NAMED is the
//! search string, and nothing else about the group decides it. The search string is
//! matched whole, because the operation documents that wildcard searches aren't
//! supported; an attribute name and a string value are both matched case insensitively,
//! which is how a directory resolves them. A matched group comes back as the record a
//! listing reports for it, widened by the attributes the search named.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, emulator, Answer, DIRECTORY};

/// A second `DirectoryId` satisfying `^d-[0-9a-f]{10}$`, for the cases that are about
/// one directory not answering for another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// A third, for the paging case, so its groups cannot perturb the tables above.
const PAGED_DIRECTORY: &str = "d-00000000aa";

/// A `DirectoryId` nothing is ever written to.
const UNUSED_DIRECTORY: &str = "d-aaaaaaaaaa";

/// The largest page `MaxResults` admits (`@range(min: 1, max: 250)`), spelled out here
/// rather than imported so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 250;

/// Every value `GroupType` admits, from
/// `smithy_query(query="shape", shape="GroupType")`.
const GROUP_TYPES: [&str; 2] = ["Distribution", "Security"];

/// Every value `GroupScope` admits, from
/// `smithy_query(query="shape", shape="GroupScope")`.
const GROUP_SCOPES: [&str; 4] = ["DomainLocal", "Global", "Universal", "BuiltinLocal"];

async fn search_groups(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/SearchGroups", directory, body).await
}

async fn create_group(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/CreateGroup", directory, body).await
}

async fn describe_group(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/DescribeGroup", directory, body).await
}

async fn list_groups(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Groups/ListGroups", directory, body).await
}

/// One search request.
fn search(string: &str, attributes: &[&str]) -> Value {
    json!({"SearchString": string, "SearchAttributes": attributes})
}

/// A string member of an answer, or the empty string when it carries none.
fn text(answer: &Answer, member: &str) -> String {
    answer.body[member].as_str().unwrap_or_default().to_string()
}

/// The groups an answer reported, in the order it reported them.
fn reported(answer: &Answer) -> Vec<Value> {
    answer.body["Groups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `SAMAccountName`s a search reported, in the order it reported them.
fn matched_names(answer: &Answer) -> Vec<String> {
    reported(answer)
        .iter()
        .map(|group| {
            group["SAMAccountName"]
                .as_str()
                .unwrap_or_else(|| panic!("a reported group carries a name: {group:?}"))
                .to_string()
        })
        .collect()
}

/// The members an answered object carries, in a stable order.
fn members(group: &Value) -> Vec<String> {
    let mut named: Vec<String> = group
        .as_object()
        .expect("an object")
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(member, _)| member.clone())
        .collect();
    named.sort();
    named
}

/// The whole group inventory of a directory: one page as large as `MaxResults` admits.
async fn whole_inventory(router: &MultiServiceRouter, directory: &str) -> Vec<Value> {
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
    assert!(
        answer.body["NextToken"].is_null(),
        "the whole inventory must fit one page of {MAX_RESULTS_CEILING}, got {:?}",
        answer.body
    );
    reported(&answer)
}

/// The groups every case in the tables below searches: the same string sits on
/// different attributes of different groups, so only the attribute a request names can
/// decide which of them it reports. Attribute names satisfy `LdapDisplayName`'s
/// `^[A-Za-z*][A-Za-z-*]*$`, and the stored values cover every arm of the modelled
/// `AttributeValue` union (`smithy_query(query="shape", shape="AttributeValue")`).
async fn populate(router: &MultiServiceRouter) {
    let creates = [
        json!({
            "SAMAccountName": "engineers",
            "OtherAttributes": {
                "Description": {"S": "Engineering"},
                "Floor": {"N": 7},
                "Shared": {"BOOL": true},
                "Alias": {"SS": ["eng", "eng-team"]},
            },
        }),
        json!({
            "SAMAccountName": "all-staff",
            "GroupType": "Distribution",
            // The same value as the engineers group's, in another case.
            "OtherAttributes": {"Description": {"S": "engineering"}},
        }),
        json!({
            "SAMAccountName": "Zebra.Team",
            "GroupScope": "Universal",
            // "Engineering" again, but on an attribute no other group holds.
            "OtherAttributes": {"Info": {"S": "Engineering"}},
        }),
        // A name that is a prefix of another group's name, and a value of another
        // group's multi-valued attribute.
        json!({"SAMAccountName": "eng", "GroupScope": "DomainLocal"}),
    ];
    for body in creates {
        let created = create_group(router, DIRECTORY, &body).await;
        assert_eq!(
            created.status, 200,
            "case {body:?}: CreateGroup must be accepted, got {} with {:?}",
            created.status, created.body
        );
    }

    // A group of another directory, which no search of this one may ever report.
    let outsider = create_group(
        router,
        OTHER_DIRECTORY,
        &json!({
            "SAMAccountName": "engineers",
            "OtherAttributes": {"Description": {"S": "Engineering"}},
        }),
    )
    .await;
    assert_eq!(
        outsider.status, 200,
        "CreateGroup in the other directory must be accepted, got {:?}",
        outsider.body
    );
}

/// RULE: a group matches when the value it holds for one of the NAMED attributes is the
/// search string — no other attribute of that group is consulted, and the string is
/// matched whole.
///
/// The rows differ along which attribute holds the value (the naming member under the
/// directory's attribute name and under the model's member name, each arm of the
/// `AttributeValue` union, an attribute only one group holds), the case of the name and
/// of the value, how many attributes are named, whether the group is one the domain was
/// created with or one `CreateGroup` stored, and whether the string sits on an attribute
/// the request named at all.
#[tokio::test]
async fn a_search_matches_only_the_attributes_the_request_named() {
    let router = emulator();
    populate(&router).await;

    let cases: [(&str, &str, &[&str], &[&str]); 18] = [
        (
            "an account name, under the model's member name",
            "engineers",
            &["sAMAccountName"],
            &["engineers"],
        ),
        (
            "the same member, under the attribute a directory holds it in",
            "engineers",
            &["cn"],
            &["engineers"],
        ),
        (
            "an attribute name in another case",
            "engineers",
            &["SAMACCOUNTNAME"],
            &["engineers"],
        ),
        (
            "a value in another case",
            "ENGINEERS",
            &["cn"],
            &["engineers"],
        ),
        (
            "a built-in group of the domain, by name",
            "Domain Users",
            &["sAMAccountName"],
            &["Domain Users"],
        ),
        (
            "a built-in group of the BUILTIN authority, by name",
            "Backup Operators",
            &["cn"],
            &["Backup Operators"],
        ),
        (
            "a stored string attribute, matched case insensitively",
            "ENGINEERING",
            &["Description"],
            &["all-staff", "engineers"],
        ),
        ("a stored number attribute", "7", &["Floor"], &["engineers"]),
        (
            "a stored boolean attribute",
            "true",
            &["Shared"],
            &["engineers"],
        ),
        (
            "one value of a multi-valued attribute",
            "eng-team",
            &["Alias"],
            &["engineers"],
        ),
        (
            "the same string, on the one attribute another group holds it in",
            "Engineering",
            &["Info"],
            &["Zebra.Team"],
        ),
        (
            "every named attribute contributes its matches",
            "Engineering",
            &["Description", "Info"],
            &["all-staff", "engineers", "Zebra.Team"],
        ),
        (
            "a value held only under an attribute the request did not name",
            "Engineering",
            &["Alias"],
            &[],
        ),
        (
            "an attribute no group of this directory holds",
            "Engineering",
            &["Department"],
            &[],
        ),
        (
            "a value no named attribute holds",
            "Marketing",
            &["Description"],
            &[],
        ),
        (
            "a prefix of a value, which is not a match",
            "engineer",
            &["Description"],
            &[],
        ),
        (
            "an account name is matched whole, not as a prefix of a longer one",
            "eng",
            &["cn"],
            &["eng"],
        ),
        (
            "the same string as one value of another group's attribute",
            "eng",
            &["Alias"],
            &["engineers"],
        ),
    ];

    for (case, string, attributes, expected) in cases {
        let answer = search_groups(&router, DIRECTORY, &search(string, attributes)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: SearchGroups must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let matched = matched_names(&answer);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            matched, expected,
            "case {case:?}: searching {string:?} over {attributes:?} must report \
             {expected:?} in account-name order, got {matched:?}"
        );
        assert_eq!(
            text(&answer, "DirectoryId"),
            DIRECTORY,
            "case {case:?}: the answer must name the directory it searched, got {:?}",
            answer.body
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case:?}: a page with nothing left after it carries no NextToken, got {:?}",
            answer.body
        );
    }
}

/// RULE, on the two attributes a group has of its own: a search over `GroupType` or
/// `GroupScope` reports exactly the groups the inventory holds that value for — the
/// built-in groups of the domain as much as the created ones.
///
/// The rows are every value each of the two enums admits, so a value no group holds is
/// covered beside the values several do.
#[tokio::test]
async fn a_search_over_a_type_or_a_scope_reports_what_the_inventory_holds() {
    let router = emulator();
    populate(&router).await;
    let inventory = whole_inventory(&router, DIRECTORY).await;

    for (member, values) in [
        ("GroupType", GROUP_TYPES.to_vec()),
        ("GroupScope", GROUP_SCOPES.to_vec()),
    ] {
        for value in values {
            let expected: Vec<String> = inventory
                .iter()
                .filter(|group| group[member] == json!(value))
                .map(|group| {
                    group["SAMAccountName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();

            let answer = search_groups(
                &router,
                DIRECTORY,
                &json!({
                    "SearchString": value,
                    "SearchAttributes": [member],
                    "MaxResults": MAX_RESULTS_CEILING,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {member}={value:?}: SearchGroups must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                matched_names(&answer),
                expected,
                "case {member}={value:?}: the search must report the inventory's groups \
                 holding that value, got {:?}",
                answer.body
            );
        }
    }

    // Non-vacuity: the inventory does hold more than one group type and more than one
    // scope, so the rows above are not all the same set.
    for member in ["GroupType", "GroupScope"] {
        let mut held: Vec<String> = inventory
            .iter()
            .map(|group| group[member].to_string())
            .collect();
        held.sort();
        held.dedup();
        assert!(
            held.len() > 1,
            "the inventory must span more than one {member}, got {held:?}"
        );
    }
}

/// RULE: the names a directory minted for a group — its SID and its distinguished name
/// — are attributes a search matches on, under the attribute the directory holds each
/// one in as well as under the model's member name.
#[tokio::test]
async fn a_search_matches_the_names_the_directory_minted() {
    let router = emulator();
    populate(&router).await;

    let described = describe_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    assert_eq!(
        described.status, 200,
        "DescribeGroup must be accepted, got {} with {:?}",
        described.status, described.body
    );
    let sid = text(&described, "SID");
    let distinguished_name = text(&described, "DistinguishedName");
    assert!(
        !sid.is_empty() && !distinguished_name.is_empty(),
        "the record must carry a SID and a distinguished name, got {:?}",
        described.body
    );

    let cases: [(&str, &str, &[&str]); 4] = [
        ("the SID, under objectSid", &sid, &["objectSid"]),
        ("the SID, under the model's member name", &sid, &["SID"]),
        (
            "the distinguished name",
            &distinguished_name,
            &["distinguishedName"],
        ),
        (
            "either of the two minted names",
            &sid,
            &["objectSid", "distinguishedName"],
        ),
    ];
    for (case, string, attributes) in cases {
        let answer = search_groups(&router, DIRECTORY, &search(string, attributes)).await;
        assert_eq!(
            matched_names(&answer),
            vec!["engineers".to_string()],
            "case {case:?}: searching {attributes:?} must report the one group that \
             carries the value, got {:?}",
            answer.body
        );
    }

    // One object has one SID: another directory's group with the same name carries
    // another SID, so this directory's SID matches nothing there.
    let elsewhere = search_groups(&router, OTHER_DIRECTORY, &search(&sid, &["objectSid"])).await;
    assert!(
        matched_names(&elsewhere).is_empty(),
        "a SID minted in one directory must match no group of another, got {:?}",
        elsewhere.body
    );
}

/// RULE: the projection is the directory's, not the client's — a matched group comes
/// back as the record a listing reports for it, widened by the attributes the search
/// NAMED, and by nothing else.
///
/// The rows differ along which attributes the search names, hence which members widen
/// the listing's record: a member the listing already reports, the distinguished name,
/// a stored attribute, and both at once.
#[tokio::test]
async fn a_matched_group_carries_the_listing_record_widened_by_what_it_named() {
    let router = emulator();
    populate(&router).await;

    // The record the create stored, as `DescribeGroup` reports it: every projected
    // value below must equal the one this read answers with.
    let stored = describe_group(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "engineers", "OtherAttributes": ["Description"]}),
    )
    .await;

    // What `ListGroups` reports for the same object: the members every search result
    // carries too, whatever it named.
    let summary = whole_inventory(&router, DIRECTORY)
        .await
        .into_iter()
        .find(|group| group["SAMAccountName"] == json!("engineers"))
        .expect("the created group is listed");
    let summary_members = members(&summary);

    let cases: [(&str, &str, &[&str], &[&str]); 5] = [
        (
            "a member the listing already reports",
            "engineers",
            &["cn"],
            &[],
        ),
        (
            "the group's own scope, which the listing already reports",
            "Global",
            &["GroupScope"],
            &[],
        ),
        (
            "the distinguished name",
            "Engineering",
            &["Description", "distinguishedName"],
            &["DistinguishedName", "OtherAttributes"],
        ),
        (
            "a stored attribute",
            "Engineering",
            &["Description"],
            &["OtherAttributes"],
        ),
        (
            "an attribute the group does not hold",
            "engineers",
            &["cn", "Department"],
            &[],
        ),
    ];

    for (case, string, attributes, widened) in cases {
        let answer = search_groups(&router, DIRECTORY, &search(string, attributes)).await;
        let group = reported(&answer)
            .into_iter()
            .find(|group| group["SAMAccountName"] == json!("engineers"))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the group must match, got {:?}",
                    answer.body
                )
            });

        let mut expected = summary_members.clone();
        expected.extend(widened.iter().map(|member| (*member).to_string()));
        expected.sort();
        assert_eq!(
            members(&group),
            expected,
            "case {case:?}: searching {attributes:?} must answer the listing's record \
             widened by {widened:?}, got {group:?}"
        );

        // Every member it carries holds the value the record holds.
        for member in members(&group) {
            assert_eq!(
                group[&member], stored.body[&member],
                "case {case:?}: {member} must be the value the record holds, got \
                 {group:?} against {:?}",
                stored.body
            );
        }
    }
}

/// The matching groups are paged through in account-name order, whatever the page size,
/// and a token resumes the search it was issued for and no other.
#[tokio::test]
async fn a_search_pages_through_the_matching_groups_in_order() {
    let router = emulator();

    // Seven groups share the searched attribute value; three hold another value for the
    // same attribute, so a page that leaked a non-matching group would show up.
    let matching = ["zoe", "Alpha", "bravo", "charlie.d", "delta-9", "Echo", "fox_7"];
    for name in matching {
        create_group(
            &router,
            PAGED_DIRECTORY,
            &json!({"SAMAccountName": name, "OtherAttributes": {"Description": {"S": "shared"}}}),
        )
        .await;
    }
    for name in ["nancy", "olive", "pat"] {
        create_group(
            &router,
            PAGED_DIRECTORY,
            &json!({"SAMAccountName": name, "OtherAttributes": {"Description": {"S": "other"}}}),
        )
        .await;
    }

    let mut expected: Vec<String> = matching.iter().map(|name| (*name).to_string()).collect();
    expected.sort_by_key(|name| name.to_lowercase());

    for page_size in [1_i64, 2, 3, 6, 7, 8, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = search("shared", &["Description"]);
            request["MaxResults"] = json!(page_size);
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = search_groups(&router, PAGED_DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: SearchGroups must be accepted, got {} with {:?}",
                page.status, page.body
            );
            let names = matched_names(&page);
            assert!(
                i64::try_from(names.len()).expect("a page fits in i64") <= page_size,
                "case maxResults={page_size}: a page must hold at most {page_size} groups, \
                 got {names:?}"
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={page_size}: paging must terminate, walked {walked:?}"
            );
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking every page must report every matching \
             group once, in account-name order"
        );
    }

    // A token names a position in ONE search: another search string, another attribute
    // set, and another operation's listing cannot be resumed with it.
    let issued = {
        let mut request = search("shared", &["Description"]);
        request["MaxResults"] = json!(1);
        search_groups(&router, PAGED_DIRECTORY, &request).await.body["NextToken"]
            .as_str()
            .expect("a first page of seven matches carries a token")
            .to_string()
    };
    let listing_token = list_groups(&router, PAGED_DIRECTORY, &json!({"MaxResults": 1}))
        .await
        .body["NextToken"]
        .as_str()
        .expect("a first page of the inventory carries a token")
        .to_string();

    let cases: [(&str, Value); 5] = [
        ("another search string", {
            let mut request = search("other", &["Description"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("another attribute named", {
            let mut request = search("shared", &["Info"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("one more attribute named", {
            let mut request = search("shared", &["Description", "Info"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("a token from another operation's listing", {
            let mut request = search("shared", &["Description"]);
            request["NextToken"] = json!(listing_token);
            request
        }),
        ("a token with a digit changed", {
            let mut bytes = issued.clone().into_bytes();
            let last = bytes.len() - 1;
            bytes[last] = if bytes[last] == b'a' { b'b' } else { b'a' };
            let mut request = search("shared", &["Description"]);
            request["NextToken"] = json!(String::from_utf8(bytes).expect("hex stays utf-8"));
            request
        }),
    ];

    for (case, request) in cases {
        let refused = search_groups(&router, PAGED_DIRECTORY, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this search did not issue must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["Reason"],
            json!("INVALID_NEXT_TOKEN"),
            "case {case:?}: the refusal must say which rule it broke, got {:?}",
            refused.body
        );
    }

    // The token this search DID issue resumes it.
    let resumed = {
        let mut request = search("shared", &["Description"]);
        request["NextToken"] = json!(issued);
        search_groups(&router, PAGED_DIRECTORY, &request).await
    };
    assert_eq!(
        matched_names(&resumed),
        expected[1..].to_vec(),
        "the issued token must resume the search just after its first page, got {:?}",
        resumed.body
    );
}

/// A search reads ONE directory and ONE realm, and reading never materialises a
/// directory.
#[tokio::test]
async fn a_search_reads_one_directory_and_one_realm() {
    let router = emulator();
    populate(&router).await;

    let own_realm = text(
        &describe_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await,
        "Realm",
    );
    assert!(
        !own_realm.is_empty(),
        "the directory must report a domain name"
    );

    // The directory's own realm, in any case, searches the directory's objects.
    for spelling in [own_realm.clone(), own_realm.to_uppercase()] {
        let mut request = search("Engineering", &["Description"]);
        request["Realm"] = json!(spelling);
        let answer = search_groups(&router, DIRECTORY, &request).await;
        assert_eq!(
            matched_names(&answer),
            vec!["all-staff".to_string(), "engineers".to_string()],
            "case {spelling:?}: the directory's own realm is case insensitive, got {:?}",
            answer.body
        );
        assert_eq!(
            text(&answer, "Realm"),
            spelling,
            "case {spelling:?}: the answer must report the realm it read, got {:?}",
            answer.body
        );
    }

    // Another realm is a domain this directory holds no groups in.
    let other_realm = "trusted.example.net";
    let mut request = search("Engineering", &["Description"]);
    request["Realm"] = json!(other_realm);
    let foreign = search_groups(&router, DIRECTORY, &request).await;
    assert_eq!(
        foreign.status, 200,
        "searching another realm must be accepted, got {} with {:?}",
        foreign.status, foreign.body
    );
    assert!(
        matched_names(&foreign).is_empty(),
        "another realm holds no groups here, got {:?}",
        foreign.body
    );
    assert_eq!(
        text(&foreign, "Realm"),
        other_realm,
        "the answer must report the realm it read, got {:?}",
        foreign.body
    );

    // Each directory answers only for its own groups.
    let other = search_groups(
        &router,
        OTHER_DIRECTORY,
        &search("Engineering", &["Description"]),
    )
    .await;
    assert_eq!(
        matched_names(&other),
        vec!["engineers".to_string()],
        "a directory must report only its own matching groups, got {:?}",
        other.body
    );

    // A directory nothing was written to holds nothing — not even the built-in groups,
    // which belong to a directory this emulator holds — and searching it does not
    // create it.
    let unused = search_groups(
        &router,
        UNUSED_DIRECTORY,
        &search("Domain Users", &["sAMAccountName"]),
    )
    .await;
    assert_eq!(
        unused.status, 200,
        "searching an unused directory must be accepted, got {} with {:?}",
        unused.status, unused.body
    );
    assert!(
        matched_names(&unused).is_empty(),
        "an unused directory holds no groups, got {:?}",
        unused.body
    );
    assert!(
        whole_inventory(&router, UNUSED_DIRECTORY).await.is_empty(),
        "a search must not materialise the directory it read"
    );
}
