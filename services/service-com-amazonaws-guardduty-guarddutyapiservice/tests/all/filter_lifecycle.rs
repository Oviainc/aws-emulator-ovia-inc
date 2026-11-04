// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GuardDuty saved-filter lifecycle: `CreateFilter`,
//! `GetFilter`, `UpdateFilter` and `DeleteFilter`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, the modelled-constraint walk and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A create persists every member the request carries on the detector it names, and
//!    a later read reports exactly those values back — with the documented default for
//!    a member the request omitted (`Action` "Default: NOOP") and nothing invented for
//!    one that has no default.
//! 2. An update writes the members the request names onto the stored filter, leaves
//!    the members it does not name as they were, and increments the filter's `Version`
//!    by 1 while moving `UpdatedAt` and leaving `CreatedAt` where the create put it.
//! 3. A delete is observable: the name the detector no longer holds is refused by
//!    every operation that addresses a filter by name — including a second delete —
//!    and the detector's other filters are untouched.
//! 4. A filter is scoped to the detector and the Region it was created under, and a
//!    refused call changes nothing a later read can see.
//!
//! The member catalogue the rows span — `FilterAction` (NOOP | ARCHIVE), `FilterRank`
//! (range 1..100), `FilterDescription` (len 0..512), `FilterVersion` (range min 1),
//! `TagMap` (len 1..200) and the `Condition` members a `Criterion` entry can carry —
//! comes from `smithy_query(operation="CreateFilter,UpdateFilter,GetFilter")` and
//! `smithy_query(shape="Condition,FilterAction,FilterVersion,TagMap")`, not from any
//! recorded call.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, ABSENT_DETECTOR_ID,
};

/// The only client error GuardDuty's model binds to any of its operations
/// (`smithy_query(query="service_info")`), and the shape restJson1 resolves an error
/// response to through `x-amzn-errortype`.
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// The code the framework names a refusal by when a service states none of its own.
/// No GuardDuty operation declares a shape by this name, so it must never reach a
/// client of this service.
const UNMODELED_CODE: &str = "ValidationException";

/// `Action`'s documented default when a create names none: "Default: NOOP"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`).
const DEFAULT_ACTION: &str = "NOOP";

/// A `CreateFilter` body: the required `name` and `findingCriteria`, plus whatever
/// members a row adds or overrides.
fn create_body(name: &str, extra: &[(&str, Value)]) -> Value {
    let mut body = json!({
        "name": name,
        "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
    });
    for (key, value) in extra {
        body[*key] = value.clone();
    }
    body
}

/// A string of `len` characters drawn from the documented filter-name alphabet, so a
/// length row differs from its neighbours ONLY in length.
fn name_of(len: usize) -> String {
    "a".repeat(len)
}

async fn create_filter(router: &crate::detector_lifecycle::Router, detector_id: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body.clone()),
    )
    .await
}

async fn get_filter(
    router: &crate::detector_lifecycle::Router,
    detector_id: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn update_filter(
    router: &crate::detector_lifecycle::Router,
    detector_id: &str,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body.clone()),
    )
    .await
}

async fn delete_filter(
    router: &crate::detector_lifecycle::Router,
    detector_id: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

/// Assert one refusal: HTTP 400, the modelled error shape on the wire, and a code the
/// model declares rather than the framework's default.
fn assert_modeled_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        MODELED_CLIENT_ERROR,
        "case {case}: expected the modelled error shape in x-amzn-errortype, got {body:?}"
    );
    assert_ne!(
        error_code(body),
        UNMODELED_CODE,
        "case {case}: refused under a code this service's model declares for nothing, got {body:?}"
    );
}

/// The epoch-second timestamp a read reports for `member`, as a number.
fn timestamp(case: &str, read: &Value, member: &str) -> f64 {
    read[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {read:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1: a create persists what it was sent, and a read reports exactly that.
// ---------------------------------------------------------------------------

/// Rows spanning what a create can carry: the criterion conditions, the name alphabet
/// and both ends of every published bound. Each is `(why, name, body)`.
fn create_rows() -> Vec<(String, String, Value)> {
    // The condition members come from `smithy_query(shape="Condition")`: the current
    // `Equals`/`NotEquals`/`GreaterThan`/`GreaterThanOrEqual`/`LessThan`/
    // `LessThanOrEqual`/`Matches`/`NotMatches` set plus the deprecated `Eq`/`Gte`
    // aliases, which a saved filter must still round-trip.
    let criteria: Vec<(&str, Value)> = vec![
        (
            "an equals condition on a string field",
            json!({"criterion": {"type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]}}}),
        ),
        (
            "a notEquals condition with several values",
            json!({"criterion": {"accountId": {"notEquals": ["111122223333", "444455556666"]}}}),
        ),
        (
            "a long-valued comparison on createdAt, in Unix epoch milliseconds",
            json!({"criterion": {"createdAt": {"greaterThanOrEqual": 1486685375000i64}}}),
        ),
        (
            "two comparisons bounding one field",
            json!({"criterion": {"severity": {"greaterThan": 2, "lessThanOrEqual": 8}}}),
        ),
        (
            "a matches condition, which create-filter and update-filter alone accept",
            json!({"criterion": {"type": {"matches": ["Recon*", "UnauthorizedAccess*"]}}}),
        ),
        (
            "a notMatches condition",
            json!({"criterion": {"region": {"notMatches": ["us-*"]}}}),
        ),
        (
            "the deprecated eq and gte aliases",
            json!({"criterion": {"region": {"eq": ["us-east-1"]}, "confidence": {"gte": 5}}}),
        ),
        (
            "conditions on three fields at once",
            json!({"criterion": {
                "severity": {"greaterThanOrEqual": 7},
                "service.archived": {"equals": ["false"]},
                "resource.resourceType": {"equals": ["Instance"]},
            }}),
        ),
    ];

    let mut rows: Vec<(String, String, Value)> = Vec::new();
    for (index, (why, criterion)) in criteria.into_iter().enumerate() {
        let name = format!("criteria-{index}");
        rows.push((
            format!("CreateFilter with {why}"),
            name.clone(),
            create_body(&name, &[("findingCriteria", criterion)]),
        ));
    }

    // `Action` is the whole `FilterAction` value set plus "named nothing", `Rank` both
    // ends of its 1..100 range and a value inside it, `Description` both ends of its
    // 0..512 length.
    for action in ["NOOP", "ARCHIVE"] {
        let name = format!("action-{}", action.to_lowercase());
        rows.push((
            format!("CreateFilter with action {action}"),
            name.clone(),
            create_body(&name, &[("action", json!(action))]),
        ));
    }
    rows.push((
        "CreateFilter naming no action, which takes the documented default".to_string(),
        "action-absent".to_string(),
        create_body("action-absent", &[]),
    ));
    for rank in [1, 50, 100] {
        let name = format!("rank-{rank}");
        rows.push((
            format!("CreateFilter with rank {rank}"),
            name.clone(),
            create_body(&name, &[("rank", json!(rank))]),
        ));
    }
    for (len, why) in [
        (0, "a description at the published minimum length of 0"),
        (512, "a description at the published maximum length of 512"),
    ] {
        let name = format!("description-{len}");
        rows.push((
            format!("CreateFilter with {why}"),
            name.clone(),
            create_body(&name, &[("description", json!("d".repeat(len)))]),
        ));
    }

    // The name alphabet: "period (.), underscore (_), dash (-), and alphanumeric
    // characters" (`smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`),
    // at both ends of `FilterName`'s 3..64 length.
    for (name, why) in [
        (name_of(3), "a name at the published minimum length of 3"),
        (name_of(64), "a name at the published maximum length of 64"),
        (
            "Dots.and_under-scores.0123456789".to_string(),
            "every character class the documented alphabet admits",
        ),
    ] {
        rows.push((
            format!("CreateFilter with {why}"),
            name.clone(),
            create_body(&name, &[]),
        ));
    }

    // `TagMap` is `len 1..200`, its keys `^(?!aws:)[a-zA-Z+-=._:/]+$`.
    for (tags, why) in [
        (json!({"env": "prod"}), "a single tag"),
        (
            json!({"env": "prod", "team.name": "security", "cost/center": "cc-42", "empty": ""}),
            "several tags, including a key with punctuation and an empty value",
        ),
    ] {
        let name = format!("tags-{}", tags.as_object().map(|o| o.len()).unwrap_or(0));
        rows.push((
            format!("CreateFilter with {why}"),
            name.clone(),
            create_body(&name, &[("tags", tags)]),
        ));
    }

    // Every member at once, so no row leaves a member untested in company.
    rows.push((
        "CreateFilter naming every member at once".to_string(),
        "everything.at-once_1".to_string(),
        create_body(
            "everything.at-once_1",
            &[
                ("description", json!("every member")),
                ("action", json!("ARCHIVE")),
                ("rank", json!(42)),
                ("tags", json!({"env": "prod"})),
            ],
        ),
    ));

    rows
}

/// Assert a read reports exactly what the create sent: each member the body carried,
/// the documented default for `Action` when it carried none, nothing for a member with
/// no default, and the initial version.
fn assert_create_round_trip(case: &str, sent: &Value, read: &Value) {
    assert_eq!(
        read["name"], sent["name"],
        "case {case}: expected the saved name back, got {read:?}"
    );
    // The tag map is always reported, empty when the create named none; the other
    // members with no documented default are reported only when the create carried
    // them.
    let expected_tags = sent.get("tags").cloned().unwrap_or_else(|| json!({}));
    assert_eq!(
        read["tags"], expected_tags,
        "case {case}: expected tags {expected_tags} as stored, got {read:?}"
    );
    for member in ["description", "rank", "findingCriteria"] {
        match sent.get(member) {
            Some(expected) => {
                // A read reports every condition under both spellings of its pair
                // (`filter_condition_spellings`), so the criteria the create stored
                // are compared in that reported form.
                let expected = if member == "findingCriteria" {
                    crate::filter_condition_spellings::reported_criteria(expected)
                } else {
                    expected.clone()
                };
                assert_eq!(
                    read[member], expected,
                    "case {case}: expected {member} {expected} as stored, got {read:?}"
                )
            }
            None => assert!(
                read[member].is_null(),
                "case {case}: the create named no {member}, so the read must invent none, got {read:?}"
            ),
        }
    }
    let expected_action = sent
        .get("action")
        .cloned()
        .unwrap_or_else(|| json!(DEFAULT_ACTION));
    assert_eq!(
        read["action"], expected_action,
        "case {case}: expected action {expected_action}, got {read:?}"
    );
    // "`FilterVersion` range min 1" and "every time the filter is updated, the version
    // increments by 1" (`smithy_query(operation="GetFilter")`): a filter nothing has
    // updated is at 1.
    assert_eq!(
        read["version"],
        json!(1),
        "case {case}: expected version 1 on a filter nothing has updated, got {read:?}"
    );
    let created = timestamp(case, read, "createdAt");
    let updated = timestamp(case, read, "updatedAt");
    assert!(
        created > 0.0,
        "case {case}: expected a real createdAt, got {read:?}"
    );
    assert_eq!(
        created, updated,
        "case {case}: a filter nothing has updated reports its create instant as updatedAt, got {read:?}"
    );
}

#[tokio::test]
async fn a_created_filter_is_reported_as_it_was_stored() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (case, name, body) in create_rows() {
        let (status, created) = create_filter(&router, &detector_id, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {created:?}"
        );
        assert_eq!(
            created["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the saved name back, got {created:?}"
        );

        let (status, read) = get_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the saved filter to read back, got {status} {read:?}"
        );
        assert_create_round_trip(&case, &body, &read);
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an update writes what it names and leaves the rest.
// ---------------------------------------------------------------------------

/// The filter every update row starts from: every updatable member set, so "the ones
/// the update did not name stand" is observable on each of them.
fn baseline_body(name: &str) -> Value {
    create_body(
        name,
        &[
            ("description", json!("the filter as created")),
            ("action", json!("NOOP")),
            ("rank", json!(10)),
            ("tags", json!({"env": "prod"})),
        ],
    )
}

/// Update rows spanning the subsets a request can name: one member at a time, a pair,
/// all of them, and none. Each is `(why, body)`.
fn update_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("only description", json!({"description": "changed"})),
        (
            "only description, emptied to the published minimum length of 0",
            json!({"description": ""}),
        ),
        ("only action", json!({"action": "ARCHIVE"})),
        (
            "only rank, at the published minimum of 1",
            json!({"rank": 1}),
        ),
        (
            "only rank, at the published maximum of 100",
            json!({"rank": 100}),
        ),
        (
            "only findingCriteria",
            json!({"findingCriteria": {"criterion": {"type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]}}}}),
        ),
        (
            "description and rank together",
            json!({"description": "two members", "rank": 7}),
        ),
        (
            "every updatable member at once",
            json!({
                "description": "all four",
                "action": "ARCHIVE",
                "rank": 99,
                "findingCriteria": {"criterion": {"severity": {"lessThan": 3}}},
            }),
        ),
        ("no member at all", json!({})),
    ]
}

#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (index, (why, update)) in update_rows().into_iter().enumerate() {
        let case = format!("UpdateFilter with {why}");
        let name = format!("update-{index}");
        let created_body = baseline_body(&name);

        let (status, created) = create_filter(&router, &detector_id, &created_body).await;
        assert_eq!(
            status, 200,
            "case {case}: the baseline create must succeed, got {status} {created:?}"
        );
        let (_, before) = get_filter(&router, &detector_id, &name).await;
        let created_at = timestamp(&case, &before, "createdAt");

        let (status, updated) = update_filter(&router, &detector_id, &name, &update).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the updated filter's name back, got {updated:?}"
        );

        let (status, read) = get_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the updated filter to read back, got {status} {read:?}"
        );
        // A member the update named carries its new value; one it did not name carries
        // the value the create stored.
        for member in ["description", "action", "rank", "findingCriteria"] {
            let mut expected = update
                .get(member)
                .or_else(|| created_body.get(member))
                .cloned()
                .unwrap_or(Value::Null);
            if member == "findingCriteria" {
                // A read reports every condition under both spellings of its pair
                // (`filter_condition_spellings`), so the criteria the write stored
                // are compared in that reported form.
                expected = crate::filter_condition_spellings::reported_criteria(&expected);
            }
            assert_eq!(
                read[member], expected,
                "case {case}: expected {member} {expected}, got {read:?}"
            );
        }
        // `UpdateFilter` carries no `Tags` member at all
        // (`smithy_query(operation="UpdateFilter")`), so the create's tags stand.
        assert_eq!(
            read["tags"],
            json!({"env": "prod"}),
            "case {case}: an update cannot change tags, got {read:?}"
        );
        assert_eq!(
            read["version"],
            json!(2),
            "case {case}: one update must increment the version by 1, got {read:?}"
        );
        assert_eq!(
            timestamp(&case, &read, "createdAt"),
            created_at,
            "case {case}: an update must not move createdAt, got {read:?}"
        );
        assert!(
            timestamp(&case, &read, "updatedAt") > created_at,
            "case {case}: an update must move updatedAt past createdAt, got {read:?}"
        );
    }
}

#[tokio::test]
async fn each_update_increments_the_version_by_one() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let name = "version.counter_1";
    let (status, created) = create_filter(&router, &detector_id, &baseline_body(name)).await;
    assert_eq!(status, 200, "baseline create: got {status} {created:?}");

    let mut previous_updated_at = {
        let (_, read) = get_filter(&router, &detector_id, name).await;
        timestamp("the baseline read", &read, "updatedAt")
    };
    // Four updates, each naming a different member, so the count and not the payload
    // is what moves the version.
    for (index, update) in [
        json!({"rank": 3}),
        json!({"description": "second"}),
        json!({"action": "ARCHIVE"}),
        json!({}),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("update {} of 4 ({update})", index + 1);
        let (status, body) = update_filter(&router, &detector_id, name, &update).await;
        assert_eq!(status, 200, "case {case}: got {status} {body:?}");

        let (_, read) = get_filter(&router, &detector_id, name).await;
        let expected_version = i64::try_from(index).expect("small index") + 2;
        assert_eq!(
            read["version"],
            json!(expected_version),
            "case {case}: expected version {expected_version}, got {read:?}"
        );
        let updated_at = timestamp(&case, &read, "updatedAt");
        assert!(
            updated_at > previous_updated_at,
            "case {case}: expected updatedAt past {previous_updated_at}, got {read:?}"
        );
        previous_updated_at = updated_at;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a delete is observable, and only the named filter goes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_filter_is_gone_and_only_that_one() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let names = ["keep.first", "drop.middle", "keep.last"];
    for name in names {
        let (status, created) = create_filter(&router, &detector_id, &baseline_body(name)).await;
        assert_eq!(status, 200, "create {name}: got {status} {created:?}");
    }

    let (status, deleted) = delete_filter(&router, &detector_id, "drop.middle").await;
    assert_eq!(
        status, 200,
        "case DeleteFilter on a saved filter: expected HTTP 200, got {status} {deleted:?}"
    );

    // The deleted name is held by nothing any longer: every operation that addresses a
    // filter by name refuses it, including a second delete.
    let (status, read) = get_filter(&router, &detector_id, "drop.middle").await;
    assert_modeled_refusal("GetFilter on a deleted filter", status, &read);
    let (status, refused) = delete_filter(&router, &detector_id, "drop.middle").await;
    assert_modeled_refusal("DeleteFilter on an already deleted filter", status, &refused);
    let (status, refused) =
        update_filter(&router, &detector_id, "drop.middle", &json!({"rank": 5})).await;
    assert_modeled_refusal("UpdateFilter on a deleted filter", status, &refused);

    // Its siblings are untouched.
    for name in ["keep.first", "keep.last"] {
        let (status, read) = get_filter(&router, &detector_id, name).await;
        assert_eq!(
            status, 200,
            "case {name} after deleting a sibling: expected HTTP 200, got {status} {read:?}"
        );
        assert_eq!(
            read["version"],
            json!(1),
            "case {name}: deleting a sibling must not change it, got {read:?}"
        );
    }

    // The record really went: the name is free again, and the filter created under it
    // starts its own lifecycle at version 1 rather than resuming the deleted one's.
    let reborn = create_body("drop.middle", &[("rank", json!(77))]);
    let (status, created) = create_filter(&router, &detector_id, &reborn).await;
    assert_eq!(
        status, 200,
        "case CreateFilter under a deleted name: expected HTTP 200, got {status} {created:?}"
    );
    let (status, read) = get_filter(&router, &detector_id, "drop.middle").await;
    assert_eq!(status, 200, "re-created filter: got {status} {read:?}");
    assert_create_round_trip("CreateFilter under a deleted name", &reborn, &read);
}

// ---------------------------------------------------------------------------
// Rule 4: a filter belongs to one detector in one Region, and a refusal writes
// nothing.
// ---------------------------------------------------------------------------

/// Names no detector holds: two outside `FilterName`'s published 3..64 length, one
/// outside the documented alphabet, and one that is well formed but absent.
fn absent_filter_names() -> Vec<(String, &'static str)> {
    vec![
        (name_of(2), "two characters: below the published minimum of 3"),
        (
            name_of(65),
            "65 characters: one above the published maximum of 64",
        ),
        (
            "bad!name".to_string(),
            "a character outside the documented alphabet",
        ),
        (
            "absent.but-well_formed".to_string(),
            "well formed, and held by no detector",
        ),
    ]
}

#[tokio::test]
async fn an_update_or_delete_of_something_absent_is_refused_and_writes_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let name = "untouched.filter_1";
    let (status, created) = create_filter(&router, &detector_id, &baseline_body(name)).await;
    assert_eq!(status, 200, "baseline create: got {status} {created:?}");

    for (absent, why) in absent_filter_names() {
        let (status, refused) = update_filter(&router, &detector_id, &absent, &json!({"rank": 5})).await;
        assert_modeled_refusal(&format!("UpdateFilter on {absent:?} ({why})"), status, &refused);
        let (status, refused) = delete_filter(&router, &detector_id, &absent).await;
        assert_modeled_refusal(&format!("DeleteFilter on {absent:?} ({why})"), status, &refused);
    }

    // A detector id no create ever minted is refused the same way, for both.
    let (status, refused) =
        update_filter(&router, ABSENT_DETECTOR_ID, name, &json!({"rank": 5})).await;
    assert_modeled_refusal("UpdateFilter under a detector id nothing minted", status, &refused);
    let (status, refused) = delete_filter(&router, ABSENT_DETECTOR_ID, name).await;
    assert_modeled_refusal("DeleteFilter under a detector id nothing minted", status, &refused);

    // A detector is regional, so the filter it holds is not addressable through
    // another Region's endpoint.
    for method in ["POST", "DELETE"] {
        let (status, refused) = call_in(
            &router,
            "eu-west-1",
            method,
            &format!("/detector/{detector_id}/filter/{name}"),
            (method == "POST").then(|| json!({"rank": 5})),
        )
        .await;
        assert_modeled_refusal(
            &format!("{method} on the filter through another Region's endpoint"),
            status,
            &refused,
        );
    }

    // A criterion key that names no field of a finding is refused by an update the way
    // a create refuses it: the check is the one a findings read applies, so an update
    // cannot save a criterion a create would have rejected.
    for (path, why) in [
        (
            "resource.instanceDetails",
            "a detail document rather than a field in it",
        ),
        ("Severity", "a known field in the wrong case"),
        ("not.an.attribute", "no field at all"),
    ] {
        let (status, refused) = update_filter(
            &router,
            &detector_id,
            name,
            &json!({"findingCriteria": {"criterion": {path: {"equals": ["1"]}}}}),
        )
        .await;
        assert_modeled_refusal(
            &format!("UpdateFilter with criterion key {path:?} ({why})"),
            status,
            &refused,
        );
    }

    // None of those refusals touched the saved filter.
    let (status, read) = get_filter(&router, &detector_id, name).await;
    assert_eq!(status, 200, "the saved filter after the refusals: got {status} {read:?}");
    assert_create_round_trip("the saved filter after the refusals", &baseline_body(name), &read);
}
