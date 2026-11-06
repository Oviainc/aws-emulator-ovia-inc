// SPDX-License-Identifier: CC0-1.0

//! The three renderings of a `ValidationException` message that are Directory Service
//! Data's own rather than the framework's, each read end to end over HTTP.
//!
//! Every constraint refusal of this service states the same envelope — `<N> validation
//! error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint:
//! <requirement>` — and the sibling suite `validation_envelope` covers that envelope over
//! the constraint classes the model declares. What this suite pins is the three halves of
//! it the framework cannot state, each over the SET its rule generalises across:
//!
//! 1. **An enumerated type's value set is listed in the order this service publishes for
//!    that enum**, which is a property of the enum and not of the request or the
//!    operation: the same enum reached through two operations states one order, and an
//!    enum this service publishes no order for is listed as the model declares it.
//! 2. **A clause about a LIST quotes the list the request carried** — every element, in
//!    order, comma-joined inside brackets — for every constraint class, whatever the
//!    element count, including none.
//! 3. **A constraint broken by a list's ELEMENTS is stated once at the list**, with the
//!    whole constraint set the element shape declares, so a caller is told what a value
//!    that member admits has to look like rather than only which rule one element broke.
//!
//! The value half of a SCALAR clause is the same rule as (2): it is the value the request
//! carried, however long, rather than a summary of it.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::ops::types::GROUP_TYPE_VALUES;
use service_com_amazonaws_directoryservicedata_directoryservicedata::service::validation::GROUP_TYPE_VALUE_SET;

use super::validation_envelope::{
    call, emulator, one_violation, search_attribute_element_set, Answer, DIRECTORY,
    LDAP_DISPLAY_NAME_CEILING, LDAP_DISPLAY_NAME_FLOOR, SEARCH_ATTRIBUTES_CEILING,
    SEARCH_ATTRIBUTES_FLOOR, VALIDATION_EXCEPTION,
};

/// `Realm`'s `@length` maximum (`smithy_query(query="shape", shape="Realm")` — len
/// 1..255), the longest value any member of this service admits that a refusal of it can
/// quote without the value also breaking that member's `@pattern`.
const REALM_CEILING: usize = 255;

/// A list as a clause quotes one: the elements in order, comma-separated, in brackets.
fn rendered_list<S: AsRef<str>>(elements: &[S]) -> String {
    let joined: Vec<&str> = elements.iter().map(AsRef::as_ref).collect();
    format!("[{}]", joined.join(", "))
}

/// `count` attribute names inside `LdapDisplayName`'s own pattern and length, so a list of
/// them breaks only the LIST's `@length`.
fn valid_attributes(count: usize) -> Vec<String> {
    (0..count)
        .map(|position| format!("Attr{}", "a".repeat(position % 7)))
        .collect()
}

/// Assert one refused call: 400 `ValidationException` in both the body and the header,
/// with exactly this `Message`.
fn assert_refused(case: &str, refused: &Answer, expected: &str) {
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.error_type, VALIDATION_EXCEPTION,
        "case {case:?}: expected x-amzn-errortype {VALIDATION_EXCEPTION}, got {:?}",
        refused.error_type
    );
    assert_eq!(
        refused.message(),
        Some(expected),
        "case {case:?}: expected Message {expected:?}, got {:?}",
        refused.body
    );
}

/// RULE 1: an enum value set is stated in the order this service publishes for that enum,
/// and in the model's declaration order for an enum it publishes no order for.
///
/// The rows differ along which enum the rejected value targets, which operation carried it
/// and which member it sat at — three enums over three operations, with `GroupType`
/// reached through two of them, so a per-operation or per-member answer fails here.
#[tokio::test]
async fn an_enum_value_set_is_stated_in_the_order_this_service_publishes() {
    let router = emulator();

    // Each expected set is the enum's modelled values
    // (`smithy_query(query="shape", shape="GroupType"/"GroupScope"/"UpdateType")`), in the
    // order a refusal lists them: this service's published order for `GroupType`, the
    // model's declaration order for the two it publishes no order for.
    let cases: [(&str, &str, Value, &str, &str); 5] = [
        (
            "GroupType on CreateGroup",
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "engineering", "GroupType": "NotAGroupType"}),
            "groupType",
            "[Security, Distribution]",
        ),
        (
            "the same GroupType enum reached through UpdateGroup",
            "/Groups/UpdateGroup",
            json!({"SAMAccountName": "engineering", "GroupType": "NotAGroupType"}),
            "groupType",
            "[Security, Distribution]",
        ),
        (
            "GroupScope, an enum with no published order",
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "engineering", "GroupScope": "NotAGroupType"}),
            "groupScope",
            "[DomainLocal, Global, Universal, BuiltinLocal]",
        ),
        (
            "UpdateType on UpdateGroup",
            "/Groups/UpdateGroup",
            json!({"SAMAccountName": "engineering", "UpdateType": "NotAGroupType"}),
            "updateType",
            "[ADD, REPLACE, REMOVE]",
        ),
        (
            "the same UpdateType enum reached through UpdateUser",
            "/Users/UpdateUser",
            json!({"SAMAccountName": "jdoe", "UpdateType": "NotAGroupType"}),
            "updateType",
            "[ADD, REPLACE, REMOVE]",
        ),
    ];

    for (what, route, body, member, value_set) in cases {
        let refused = call(&router, route, DIRECTORY, &body).await;
        let expected = one_violation(
            "'NotAGroupType'",
            member,
            &format!("Member must satisfy enum value set: {value_set}"),
        );
        assert_refused(what, &refused, &expected);
    }
}

/// The published order holds exactly the values the model declares for that enum: it
/// reorders the set and adds nothing to it, so a refusal can never advertise a value the
/// enum does not admit.
#[tokio::test]
async fn the_published_group_type_set_holds_exactly_the_modelled_values() {
    let mut published: Vec<&str> = GROUP_TYPE_VALUE_SET.to_vec();
    published.sort_unstable();
    let mut modelled: Vec<&str> = GROUP_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        published, modelled,
        "the published GroupType set must hold exactly the modelled values, \
         got {published:?} against {modelled:?}"
    );
}

/// RULE 2: a clause about a list quotes the list the request carried, whatever its size
/// and whichever `@length` bound it broke.
///
/// The rows differ along the element COUNT (none, one past the maximum, well past it) and
/// along which bound the count broke, over both operations that declare the member. The
/// expected value is built from the request's own elements, so a row cannot pass against a
/// count summary or against another row's list.
#[tokio::test]
async fn a_clause_about_a_list_quotes_the_list_the_request_carried() {
    let router = emulator();

    let cases: [(&str, &str, Vec<String>, String); 5] = [
        (
            "an empty list, under the @length floor, on SearchUsers",
            "/Users/SearchUsers",
            Vec::new(),
            format!("Member must have length greater than or equal to {SEARCH_ATTRIBUTES_FLOOR}"),
        ),
        (
            "an empty list, under the @length floor, on SearchGroups",
            "/Groups/SearchGroups",
            Vec::new(),
            format!("Member must have length greater than or equal to {SEARCH_ATTRIBUTES_FLOOR}"),
        ),
        (
            "one element past the @length ceiling, on SearchGroups",
            "/Groups/SearchGroups",
            valid_attributes(SEARCH_ATTRIBUTES_CEILING + 1),
            format!("Member must have length less than or equal to {SEARCH_ATTRIBUTES_CEILING}"),
        ),
        (
            "one element past the @length ceiling, on SearchUsers",
            "/Users/SearchUsers",
            valid_attributes(SEARCH_ATTRIBUTES_CEILING + 1),
            format!("Member must have length less than or equal to {SEARCH_ATTRIBUTES_CEILING}"),
        ),
        (
            "four times the @length ceiling, on SearchUsers",
            "/Users/SearchUsers",
            valid_attributes(SEARCH_ATTRIBUTES_CEILING * 4),
            format!("Member must have length less than or equal to {SEARCH_ATTRIBUTES_CEILING}"),
        ),
    ];

    for (what, route, attributes, requirement) in cases {
        let body = json!({"SearchString": "j", "SearchAttributes": attributes});
        let refused = call(&router, route, DIRECTORY, &body).await;
        let expected = one_violation(
            &format!("'{}'", rendered_list(&attributes)),
            "searchAttributes",
            &requirement,
        );
        assert_refused(what, &refused, &expected);
        assert!(
            !refused.message().unwrap_or_default().contains(" items"),
            "case {what:?}: the clause must quote the list, not count it, got {:?}",
            refused.body
        );
    }
}

/// RULE 3: a constraint one of a list's elements broke is stated ONCE at the list member,
/// quoting the whole list and naming every requirement the element shape declares.
///
/// The rows differ along which of the element shape's constraints the request broke (the
/// `@pattern`, the `@length` ceiling, the `@length` floor, two at once), how many elements
/// broke one (one, two) and where the offender sat among valid elements — and every row
/// states the SAME clause, because the clause is about the member and its element shape
/// rather than about the element that failed.
#[tokio::test]
async fn an_element_constraint_is_stated_once_at_the_list_with_the_whole_element_set() {
    let router = emulator();

    let cases: [(&str, &str, Vec<String>); 6] = [
        (
            "one element outside the element @pattern",
            "/Users/SearchUsers",
            vec!["1BadAttribute".to_string()],
        ),
        (
            "the same, on SearchGroups",
            "/Groups/SearchGroups",
            vec!["1BadAttribute".to_string()],
        ),
        (
            "one element past the element @length ceiling",
            "/Users/SearchUsers",
            vec!["A".repeat(LDAP_DISPLAY_NAME_CEILING + 1)],
        ),
        (
            "one element under the element @length floor, breaking its @pattern too",
            "/Users/SearchUsers",
            vec![String::new()],
        ),
        (
            "two offending elements",
            "/Groups/SearchGroups",
            vec!["1BadAttribute".to_string(), "2BadAttribute".to_string()],
        ),
        (
            "one offender among valid elements",
            "/Users/SearchUsers",
            vec![
                "Cn".to_string(),
                "1BadAttribute".to_string(),
                "Sn".to_string(),
            ],
        ),
    ];

    for (what, route, attributes) in cases {
        let body = json!({"SearchString": "j", "SearchAttributes": attributes});
        let refused = call(&router, route, DIRECTORY, &body).await;
        let expected = one_violation(
            &format!("'{}'", rendered_list(&attributes)),
            // The member the request carries, never a position inside it.
            "searchAttributes",
            &search_attribute_element_set(),
        );
        assert_refused(what, &refused, &expected);
        assert!(
            !refused.message().unwrap_or_default().contains(".member"),
            "case {what:?}: no clause names a path inside the list, got {:?}",
            refused.body
        );
    }
}

/// A list whose elements are all inside the element shape's constraints, and whose own
/// size is inside the list's `@length`, is NOT refused by the constraint layer: the
/// clauses above are the constraints' doing and not the renderer's.
///
/// Each published bound of `LdapDisplayNameList` is covered, with an element at each
/// published bound of `LdapDisplayName`. `SearchUsers` itself is still a scaffold, so what
/// a request past the constraint layer reaches is that scaffold's `501 NotImplemented`
/// (its own stub issue): the assertion is that no `ValidationException` is stated, which
/// is what this rule decides.
#[tokio::test]
async fn a_list_inside_every_bound_is_not_refused_as_invalid() {
    let router = emulator();

    for (what, attributes) in [
        (
            "one element, at the list's @length floor",
            valid_attributes(SEARCH_ATTRIBUTES_FLOOR),
        ),
        (
            "as many elements as the list's @length ceiling admits",
            valid_attributes(SEARCH_ATTRIBUTES_CEILING),
        ),
        (
            "an element at the element shape's @length floor",
            vec!["A".repeat(LDAP_DISPLAY_NAME_FLOOR)],
        ),
        (
            "an element at the element shape's @length ceiling",
            vec!["A".repeat(LDAP_DISPLAY_NAME_CEILING)],
        ),
    ] {
        let body = json!({"SearchString": "j", "SearchAttributes": attributes});
        let served = call(&router, "/Users/SearchUsers", DIRECTORY, &body).await;
        assert_ne!(
            served.error_code(),
            VALIDATION_EXCEPTION,
            "case {what:?}: a list inside every bound must not be refused as invalid, got {:?}",
            served.body
        );
        assert_ne!(
            served.status, 400,
            "case {what:?}: a list inside every bound must not be refused, got {:?}",
            served.body
        );
    }
}

/// The value half of a SCALAR clause is the value the request carried too: a value past
/// the length a clause would otherwise summarise is quoted in full, with no marker
/// standing in for the bytes left out.
#[tokio::test]
async fn a_scalar_clause_quotes_the_whole_value_the_request_carried() {
    let router = emulator();

    // A realm inside `Realm`'s `@pattern` (labels separated by dots) and past its
    // `@length` maximum, so the `@length` clause is the only one and it has a long value
    // to quote.
    let suffix = ".example.com";
    let label = "a".repeat(REALM_CEILING + 5 - suffix.len());
    let over_long_realm = format!("{label}{suffix}");

    let refused = call(
        &router,
        "/Groups/ListGroups",
        DIRECTORY,
        &json!({"Realm": over_long_realm}),
    )
    .await;

    let expected = one_violation(
        &format!("'{over_long_realm}'"),
        "realm",
        &format!("Member must have length less than or equal to {REALM_CEILING}"),
    );
    assert_refused("a Realm past its @length maximum", &refused, &expected);
    assert!(
        !refused.message().unwrap_or_default().contains("bytes]"),
        "the clause must quote the whole value, got {:?}",
        refused.body
    );
}
