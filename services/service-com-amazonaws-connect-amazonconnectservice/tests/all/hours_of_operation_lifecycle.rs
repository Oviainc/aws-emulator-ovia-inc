// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Amazon Connect's hours-of-operation loop —
//! `CreateHoursOfOperation`, the `DescribeHoursOfOperation` that reads it back, and the
//! `ListHoursOfOperations` that reports every one of them — driven over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! Every suite asserts a RULE over a table of inputs rather than one recorded call: what
//! a create persists is exactly what every later read reports, an hours of operation's
//! identity (a fresh UUID id and the ARN built from its instance's ARN) is derived from
//! the stored record, a name is unique within its instance, a `List*` page covers each
//! stored record exactly once, and a fresh instance already owns the default hours of
//! operation AWS provisions it with.

use std::collections::BTreeSet;

use engine::EMULATOR_ACCOUNT_ID;
use serde_json::{json, Value};

use super::harness::{connect_managed, ids_of, looks_like_a_uuid, query, Answer, Connect};

/// Every `HoursOfOperationDays` member, from `com.amazonaws.connect#HoursOfOperationDays`
/// (smithy_query(query="shape",shape="HoursOfOperationDays")). A config's `Day` is stored
/// and reported verbatim, so the rule has to hold for each one — including the members
/// this issue never cited.
const DAYS: [&str; 7] = [
    "SUNDAY",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
];

/// The weekdays the emulator's default hours of operation covers — the five members of
/// [`DAYS`] that are not a weekend day.
const WEEKDAYS: [&str; 5] = ["MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY"];

/// `TimeZone` is an unconstrained string in the model, so the table spans several IANA
/// zone names rather than one: whatever the caller sends is what every read reports.
const TIME_ZONES: [&str; 4] = ["UTC", "America/New_York", "Europe/London", "Asia/Tokyo"];

impl Connect {
    async fn create_hours_of_operation(&self, instance_id: &str, body: Value) -> Answer {
        self.send_answer(
            "PUT",
            &format!("/hours-of-operations/{instance_id}"),
            Some(body),
        )
        .await
    }

    async fn describe_hours_of_operation(&self, instance_id: &str, id: &str) -> Answer {
        self.send_answer(
            "GET",
            &format!("/hours-of-operations/{instance_id}/{id}"),
            None,
        )
        .await
    }

    async fn list_hours_of_operations(
        &self,
        instance_id: &str,
        max_results: Option<i32>,
        token: Option<&str>,
    ) -> Answer {
        let suffix = query(max_results, token, "", &[]);
        self.send_answer(
            "GET",
            &format!("/hours-of-operations-summary/{instance_id}{suffix}"),
            None,
        )
        .await
    }

    /// `CreateHoursOfOperation`, asserting success and handing back `(Id, Arn)`.
    async fn hours_of_operation(&self, instance_id: &str, body: Value) -> (String, String) {
        let created = self.create_hours_of_operation(instance_id, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateHoursOfOperation {body}: got {} {}",
            created.status, created.body
        );
        (
            created.body["HoursOfOperationId"]
                .as_str()
                .expect("HoursOfOperationId")
                .to_string(),
            created.body["HoursOfOperationArn"]
                .as_str()
                .expect("HoursOfOperationArn")
                .to_string(),
        )
    }
}

/// A modelled `HoursOfOperationTimeSlice`.
fn slice(hours: i32, minutes: i32) -> Value {
    json!({ "Hours": hours, "Minutes": minutes })
}

/// A `Config` list covering `days`, open at `open:00` and closed at `close:30`.
fn config_over(days: &[&str], open: i32, close: i32) -> Value {
    Value::Array(
        days.iter()
            .map(|day| {
                json!({
                    "Day": day,
                    "StartTime": slice(open, 0),
                    "EndTime": slice(close, 30),
                })
            })
            .collect(),
    )
}

/// A minimal valid `CreateHoursOfOperation` body.
fn hours_named(name: &str, days: &[&str]) -> Value {
    json!({
        "Name": name,
        "TimeZone": TIME_ZONES[0],
        "Config": config_over(days, 9, 17),
    })
}

/// RULE: every member `CreateHoursOfOperation` accepts is persisted and reported
/// verbatim by `DescribeHoursOfOperation` and by the `ListHoursOfOperations` summary,
/// and the record's derived identity — a fresh UUID id and the ARN
/// `<instance ARN>/operating-hours/<id>` — is computed from the stored record.
///
/// The table spans every `HoursOfOperationDays` member, config lengths 0 (the published
/// lower bound of `HoursOfOperationConfigList`), 1, 5 and 7, an absent and a present
/// description, tag counts 0/1/2, every time zone in [`TIME_ZONES`], and the
/// `CommonNameLength127` bounds (1 and 127 characters).
#[tokio::test]
async fn created_hours_of_operation_members_are_what_every_later_read_reports() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("hours")).await;

    let long_name = "n".repeat(127);
    let mut cases: Vec<(String, Value)> = Vec::new();

    // One case per modelled day, cycling the optional members and the time zones so the
    // table spans them too rather than repeating one shape seven times.
    for (index, day) in DAYS.iter().enumerate() {
        let mut body = hours_named(&format!("hours-{day}"), &[day]);
        body["TimeZone"] = json!(TIME_ZONES[index % TIME_ZONES.len()]);
        body["Config"] = config_over(&[day], index as i32, 23);
        match index % 4 {
            0 => {} // no description and no tags: the plainest request there is
            1 => {
                body["Description"] = json!("one tag");
                body["Tags"] = json!({ "env": "prod" });
            }
            2 => {
                body["Description"] = json!("d");
                body["Tags"] = json!({ "env": "dev", "team": "cx" });
            }
            _ => {
                body["Description"] = json!("x".repeat(250));
            }
        }
        cases.push((format!("one day {day}"), body));
    }

    // Config lengths: the empty list the model admits, the five weekdays, and all seven
    // days at once.
    let mut empty = hours_named("hours-no-config", &[]);
    empty["Config"] = json!([]);
    cases.push(("an empty Config".to_string(), empty));
    cases.push((
        "the five weekdays".to_string(),
        hours_named("hours-weekdays", &WEEKDAYS),
    ));
    cases.push((
        "every day of the week".to_string(),
        hours_named("hours-every-day", &DAYS),
    ));
    // CommonNameLength127 bounds: 1 character and 127 characters.
    cases.push(("a 1-character name".to_string(), hours_named("h", &["MONDAY"])));
    cases.push((
        "a 127-character name".to_string(),
        hours_named(&long_name, &["FRIDAY"]),
    ));

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();
    let mut details: Vec<(String, String, Value)> = Vec::new();

    for (case, body) in &cases {
        let created = connect
            .create_hours_of_operation(&instance_id, body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateHoursOfOperation got {} {}",
            created.status, created.body
        );
        let id = created.body["HoursOfOperationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no HoursOfOperationId in {}", created.body))
            .to_string();
        let arn = created.body["HoursOfOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no HoursOfOperationArn in {}", created.body))
            .to_string();

        assert!(
            looks_like_a_uuid(&id),
            "case {case:?}: expected a v4-UUID hours-of-operation id, got {id:?}"
        );
        assert!(
            minted_ids.insert(id.clone()),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );
        assert_eq!(
            arn,
            format!("{instance_arn}/operating-hours/{id}"),
            "case {case:?}: the ARN must be the instance's ARN plus the minted id"
        );

        let read = connect.describe_hours_of_operation(&instance_id, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: DescribeHoursOfOperation got {} {}",
            read.status, read.body
        );
        let hours = &read.body["HoursOfOperation"];

        assert_eq!(
            hours["HoursOfOperationId"],
            json!(id),
            "case {case:?}: HoursOfOperationId"
        );
        assert_eq!(
            hours["HoursOfOperationArn"],
            json!(arn),
            "case {case:?}: HoursOfOperationArn"
        );
        assert_eq!(hours["Name"], body["Name"], "case {case:?}: Name");
        assert_eq!(hours["TimeZone"], body["TimeZone"], "case {case:?}: TimeZone");
        assert_eq!(hours["Config"], body["Config"], "case {case:?}: Config");
        assert_eq!(
            hours["Description"],
            body.get("Description").cloned().unwrap_or(Value::Null),
            "case {case:?}: Description"
        );
        assert_eq!(
            hours["Tags"],
            body.get("Tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: Tags"
        );
        assert!(
            hours["ParentHoursOfOperations"].is_null(),
            "case {case:?}: no parent was named, so ParentHoursOfOperations stays absent, got {}",
            hours["ParentHoursOfOperations"]
        );
        assert_eq!(
            hours["LastModifiedRegion"],
            json!("us-east-1"),
            "case {case:?}: LastModifiedRegion"
        );
        assert!(
            hours["LastModifiedTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a LastModifiedTime timestamp, got {}",
            hours["LastModifiedTime"]
        );

        // The second read answers from storage too: identical, not re-derived.
        let again = connect.describe_hours_of_operation(&instance_id, &id).await;
        assert_eq!(
            again.body["HoursOfOperation"], *hours,
            "case {case:?}: every read reports the same stored hours of operation"
        );

        details.push((case.clone(), id, hours.clone()));
    }

    // The summary `ListHoursOfOperations` reports is the same stored record, projected.
    let listed = connect
        .list_hours_of_operations(&instance_id, Some(1000), None)
        .await;
    assert_eq!(listed.status, 200, "ListHoursOfOperations got {}", listed.body);
    let summaries = listed.body["HoursOfOperationSummaryList"]
        .as_array()
        .expect("a HoursOfOperationSummaryList")
        .clone();
    for (case, id, hours) in &details {
        let summary = summaries
            .iter()
            .find(|entry| entry["Id"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {summaries:?}"));
        assert_eq!(summary["Arn"], hours["HoursOfOperationArn"], "case {case:?}: Arn");
        assert_eq!(summary["Name"], hours["Name"], "case {case:?}: Name");
        assert_eq!(
            summary["LastModifiedTime"], hours["LastModifiedTime"],
            "case {case:?}: LastModifiedTime"
        );
        assert_eq!(
            summary["LastModifiedRegion"], hours["LastModifiedRegion"],
            "case {case:?}: LastModifiedRegion"
        );
    }
}

/// RULE: an hours-of-operation name is unique within its instance — a second create
/// under a name the instance already holds is refused with `DuplicateResourceException`
/// (409) and leaves the stored record untouched — but the same name in another instance
/// is a new resource.
#[tokio::test]
async fn hours_of_operation_names_are_unique_within_an_instance_only() {
    let connect = Connect::new();
    let (first_instance, _) = connect.create_instance(connect_managed("one")).await;
    let (second_instance, _) = connect.create_instance(connect_managed("two")).await;

    // Names differing in length and character class: the rule is about the name being
    // taken, not about what the name looks like.
    let long_name = "n".repeat(127);
    for name in ["support", "h", long_name.as_str(), "hours with spaces"] {
        let (id, _) = connect
            .hours_of_operation(&first_instance, hours_named(name, &["MONDAY"]))
            .await;

        let mut repeat = hours_named(name, &["SATURDAY"]);
        repeat["TimeZone"] = json!("Asia/Tokyo");
        let refused = connect.create_hours_of_operation(&first_instance, repeat).await;
        assert_eq!(
            refused.status, 409,
            "case {name:?}: a duplicate name must be a 409, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "DuplicateResourceException",
            "case {name:?}: expected DuplicateResourceException, got {:?} {}",
            refused.error_code, refused.body
        );

        // The refused create changed nothing: the stored record is still the first one.
        let read = connect.describe_hours_of_operation(&first_instance, &id).await;
        assert_eq!(
            read.body["HoursOfOperation"]["Config"],
            config_over(&["MONDAY"], 9, 17),
            "case {name:?}: a refused duplicate must not overwrite the stored config, got {}",
            read.body
        );
        assert_eq!(
            read.body["HoursOfOperation"]["TimeZone"],
            json!(TIME_ZONES[0]),
            "case {name:?}: a refused duplicate must not overwrite the stored time zone, got {}",
            read.body
        );

        // Another instance has its own namespace.
        let elsewhere = connect
            .create_hours_of_operation(&second_instance, hours_named(name, &["MONDAY"]))
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: the same name in another instance is a new resource, got {} {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.body["HoursOfOperationId"], json!(id),
            "case {name:?}: the two instances' records are distinct resources"
        );
    }
}

/// RULE: a parent reference names another hours of operation of the same instance, and a
/// read reports each one as the parent's own name, id and ARN; a reference to an id the
/// instance does not hold is `ResourceNotFoundException` (404), and a config entry naming
/// no id at all is `InvalidParameterException` (400).
///
/// The table spans the published `ParentHoursOfOperationConfigList` bounds: 0 parents, 1,
/// 2, the maximum of 3, and one past it.
#[tokio::test]
async fn parent_references_are_resolved_against_the_instances_own_records() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("parents")).await;

    // Three possible parents, so the table can ask for 0..=4 of them.
    let mut parents: Vec<(String, String)> = Vec::new();
    for index in 0..3 {
        let name = format!("parent-{index}");
        let (id, arn) = connect
            .hours_of_operation(&instance_id, hours_named(&name, &WEEKDAYS))
            .await;
        assert_eq!(
            arn,
            format!("{instance_arn}/operating-hours/{id}"),
            "a parent's ARN is built from its instance's ARN"
        );
        parents.push((id, name));
    }

    for count in 0..=4usize {
        let case = format!("{count} parents");
        let configs: Vec<Value> = parents
            .iter()
            .cycle()
            .take(count)
            .map(|(id, _)| json!({ "HoursOfOperationId": id }))
            .collect();
        let mut body = hours_named(&format!("child-{count}"), &["MONDAY"]);
        body["ParentHoursOfOperationConfigs"] = Value::Array(configs);

        let answer = connect.create_hours_of_operation(&instance_id, body).await;
        if count > 3 {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: past the modelled maximum of 3 parents, got {} {}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the create to succeed, got {} {}",
            answer.status, answer.body
        );

        let id = answer.body["HoursOfOperationId"]
            .as_str()
            .expect("HoursOfOperationId")
            .to_string();
        let read = connect.describe_hours_of_operation(&instance_id, &id).await;
        let reported = &read.body["HoursOfOperation"]["ParentHoursOfOperations"];
        if count == 0 {
            assert!(
                reported.is_null(),
                "case {case:?}: no parent means no ParentHoursOfOperations member, got {reported}"
            );
            continue;
        }
        let expected: Vec<Value> = parents
            .iter()
            .take(count)
            .map(|(parent_id, name)| {
                json!({
                    "Id": parent_id,
                    "Name": name,
                    "Arn": format!("{instance_arn}/operating-hours/{parent_id}"),
                })
            })
            .collect();
        assert_eq!(
            *reported,
            Value::Array(expected),
            "case {case:?}: each parent is reported with its own name, id and ARN, got {reported}"
        );
    }

    // An id of the right modelled form that names nothing in this instance.
    let mut orphan = hours_named("orphan", &["MONDAY"]);
    orphan["ParentHoursOfOperationConfigs"] =
        json!([{ "HoursOfOperationId": "11111111-2222-3333-4444-555555555555" }]);
    let refused = connect.create_hours_of_operation(&instance_id, orphan).await;
    assert_eq!(
        refused.status, 404,
        "case {:?}: an unknown parent is a 404, got {} {}",
        "absent parent", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "ResourceNotFoundException",
        "case {:?}: expected ResourceNotFoundException, got {:?} {}",
        "absent parent", refused.error_code, refused.body
    );

    // A config entry that names no id names no parent.
    let mut nameless = hours_named("nameless-parent", &["MONDAY"]);
    nameless["ParentHoursOfOperationConfigs"] = json!([{}]);
    let refused = connect.create_hours_of_operation(&instance_id, nameless).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: a parent config with no HoursOfOperationId is a 400, got {} {}",
        "parent config without an id", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "InvalidParameterException",
        "case {:?}: expected InvalidParameterException, got {:?} {}",
        "parent config without an id", refused.error_code, refused.body
    );
}

/// RULE: `ListHoursOfOperations` walks the instance's store in one deterministic order,
/// returns at most `MaxResults` items per page with a `NextToken` only while items
/// remain, and resuming from that token yields every remaining item exactly once.
///
/// The table spans page sizes below, equal to and above the population, plus the
/// unpaginated call. Every population counts the default hours of operation a fresh
/// instance already owns.
#[tokio::test]
async fn list_pages_cover_every_hours_of_operation_exactly_once() {
    let cases: Vec<(usize, Option<i32>)> = vec![
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (3, Some(4)),
        (3, Some(10)),
        (7, None),
    ];

    for (created_count, page_size) in cases {
        let case = format!("{created_count} created, maxResults={page_size:?}");
        let connect = Connect::new();
        let (instance_id, _) = connect.create_instance(connect_managed("pages")).await;

        // The instance is born with its default hours of operation, so the population is
        // what the test created plus that one.
        let mut expected: BTreeSet<String> = connect
            .list_hours_of_operations(&instance_id, None, None)
            .await
            .body["HoursOfOperationSummaryList"]
            .as_array()
            .expect("a HoursOfOperationSummaryList")
            .iter()
            .map(|entry| entry["Id"].as_str().expect("an Id").to_string())
            .collect();
        for index in 0..created_count {
            let (id, _) = connect
                .hours_of_operation(&instance_id, hours_named(&format!("hours{index}"), &["MONDAY"]))
                .await;
            expected.insert(id);
        }

        let (seen, pages) = walk_hours_of_operations(&connect, &instance_id, page_size).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: the pages must cover exactly the stored records, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {case:?}: no record may appear on two pages, got {seen:?}"
        );
        let expected_pages = page_size.map_or(1, |size| {
            expected.len().div_ceil(size as usize).max(1)
        });
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// Walk `ListHoursOfOperations` to exhaustion, returning every id seen and the page
/// count.
async fn walk_hours_of_operations(
    connect: &Connect,
    instance_id: &str,
    page_size: Option<i32>,
) -> (Vec<String>, usize) {
    let mut seen = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let listed = connect
            .list_hours_of_operations(instance_id, page_size, token.as_deref())
            .await;
        assert_eq!(
            listed.status, 200,
            "ListHoursOfOperations got {} {}",
            listed.status, listed.body
        );
        pages += 1;
        let page = ids_of(&listed.body["HoursOfOperationSummaryList"], "Id");
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "a page must hold at most maxResults items, got {page:?}"
            );
        }
        seen.extend(page);
        match listed.body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 64, "ListHoursOfOperations did not terminate");
    }
    (seen, pages)
}

/// RULE: a `MaxResults` inside the modelled range is served and one outside it is
/// refused, and a `NextToken` this service did not issue is refused rather than silently
/// restarting the scan.
#[tokio::test]
async fn page_size_bounds_and_foreign_tokens_are_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("bounds")).await;
    connect
        .hours_of_operation(&instance_id, hours_named("support", &WEEKDAYS))
        .await;

    // MaxResult1000 range 1..1000 for ListHoursOfOperations — both published bounds,
    // then one past each.
    for (case, max_results, expected) in [
        ("lower bound", 1, 200),
        ("upper bound", 1000, 200),
        ("past the upper bound", 1001, 400),
        ("below the lower bound", 0, 400),
    ] {
        let answer = connect
            .list_hours_of_operations(&instance_id, Some(max_results), None)
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: maxResults={max_results} got {} {}",
            answer.status, answer.body
        );
    }

    // Tokens no page cursor this service issues could have been: non-hex text, an odd
    // hex length, and hex that decodes to bytes which are not UTF-8.
    for token in ["not-a-token", "zz", "0", "ff"] {
        let answer = connect
            .list_hours_of_operations(&instance_id, Some(10), Some(token))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: a foreign NextToken is refused, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "InvalidRequestException",
            "case {token:?}: expected InvalidRequestException, got {:?} {}",
            answer.error_code, answer.body
        );
    }
}

/// RULE: a create, a read or a list naming an instance the store does not hold — or a
/// read of an hours of operation that instance does not hold — is
/// `ResourceNotFoundException` (404), and an hours of operation is gone with its
/// instance.
#[tokio::test]
async fn unknown_instances_and_records_answer_404() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("missing")).await;
    let (id, _) = connect
        .hours_of_operation(&instance_id, hours_named("support", &WEEKDAYS))
        .await;

    // An identifier of the right modelled form that names nothing.
    let absent = "11111111-2222-3333-4444-555555555555";

    let not_found: Vec<(&str, Answer)> = vec![
        (
            "CreateHoursOfOperation in an absent instance",
            connect
                .create_hours_of_operation(absent, hours_named("orphan", &WEEKDAYS))
                .await,
        ),
        (
            "DescribeHoursOfOperation of an absent record",
            connect.describe_hours_of_operation(&instance_id, absent).await,
        ),
        (
            "DescribeHoursOfOperation inside an absent instance",
            connect.describe_hours_of_operation(absent, &id).await,
        ),
        (
            "ListHoursOfOperations inside an absent instance",
            connect.list_hours_of_operations(absent, None, None).await,
        ),
    ];
    for (case, answer) in not_found {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} {}",
            answer.error_code, answer.body
        );
        assert!(
            answer.body["message"].is_string() || answer.body["Message"].is_string(),
            "case {case:?}: the error must carry a message, got {}",
            answer.body
        );
    }

    // Deleting the instance takes its hours of operation with it.
    assert_eq!(connect.delete_instance(&instance_id).await, 200);
    let orphaned = connect.describe_hours_of_operation(&instance_id, &id).await;
    assert_eq!(
        orphaned.status, 404,
        "case {:?}: a deleted instance's records are gone with it, got {} {}",
        "read after DeleteInstance", orphaned.status, orphaned.body
    );
}

/// RULE: a freshly created instance already owns the default hours of operation AWS
/// provisions it with, so `ListHoursOfOperations` is non-empty before any caller creates
/// one, that record reads back like any other, and listing it changes nothing a later
/// caller sees.
///
/// The rows are regions, because the record's ARN and `LastModifiedRegion` come from the
/// request the instance was created by rather than from a constant.
#[tokio::test]
async fn a_new_instance_is_born_with_its_default_hours_of_operation() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let connect = Connect::in_region(region);
        let (instance_id, _) = connect.create_instance(connect_managed("defaults")).await;

        let listed = connect
            .list_hours_of_operations(&instance_id, None, None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: ListHoursOfOperations got {} {}",
            listed.status, listed.body
        );
        let summaries = listed.body["HoursOfOperationSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {region:?}: no list in {}", listed.body));
        assert!(
            !summaries.is_empty(),
            "case {region:?}: a fresh instance must already own a default hours of \
             operation, got {}",
            listed.body
        );
        assert!(
            listed.body["NextToken"].is_null(),
            "case {region:?}: the only page carries no NextToken, got {}",
            listed.body["NextToken"]
        );

        let id = summaries[0]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: no Id in {}", listed.body))
            .to_string();
        assert_eq!(
            summaries[0]["Arn"],
            json!(format!(
                "arn:aws:connect:{region}:{EMULATOR_ACCOUNT_ID}:instance/{instance_id}/operating-hours/{id}"
            )),
            "case {region:?}: the default record's ARN must carry the request's region"
        );
        assert_eq!(
            summaries[0]["LastModifiedRegion"],
            json!(region),
            "case {region:?}: LastModifiedRegion must be the request's region, got {}",
            listed.body
        );

        let read = connect.describe_hours_of_operation(&instance_id, &id).await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: the default record reads back like any other, got {} {}",
            read.status, read.body
        );
        let hours = &read.body["HoursOfOperation"];
        assert_eq!(
            hours["Name"], summaries[0]["Name"],
            "case {region:?}: the summary and the detail report one stored name"
        );
        assert!(
            hours["TimeZone"].as_str().is_some_and(|zone| !zone.is_empty()),
            "case {region:?}: the default record carries a TimeZone, got {}",
            hours["TimeZone"]
        );

        // Its config is a set of distinct modelled days, each opening before it closes.
        let config = hours["Config"]
            .as_array()
            .unwrap_or_else(|| panic!("case {region:?}: no Config in {}", read.body));
        assert!(
            !config.is_empty(),
            "case {region:?}: the default record covers at least one day, got {}",
            hours["Config"]
        );
        let mut days: BTreeSet<String> = BTreeSet::new();
        for entry in config {
            let day = entry["Day"].as_str().unwrap_or_default().to_string();
            assert!(
                DAYS.contains(&day.as_str()),
                "case {region:?}: Day must be a HoursOfOperationDays member, got {day:?}"
            );
            assert!(
                days.insert(day.clone()),
                "case {region:?}: {day:?} appears twice in the default config"
            );
            let start = entry["StartTime"]["Hours"].as_i64().unwrap_or(-1) * 60
                + entry["StartTime"]["Minutes"].as_i64().unwrap_or(-1);
            let end = entry["EndTime"]["Hours"].as_i64().unwrap_or(-1) * 60
                + entry["EndTime"]["Minutes"].as_i64().unwrap_or(-1);
            assert!(
                (0..=23 * 60 + 59).contains(&start) && (0..=23 * 60 + 59).contains(&end),
                "case {region:?}: {day:?} must open and close within the modelled \
                 Hours24Format/MinutesLimit60 ranges, got {entry}"
            );
            assert!(
                start < end,
                "case {region:?}: {day:?} must open before it closes, got {entry}"
            );
        }

        // Listing is reporting: a second list reports exactly the same records, and no
        // extra default was minted by the read.
        let again = connect
            .list_hours_of_operations(&instance_id, None, None)
            .await;
        assert_eq!(
            again.body["HoursOfOperationSummaryList"], listed.body["HoursOfOperationSummaryList"],
            "case {region:?}: a list must not change what a later caller sees"
        );
    }
}
