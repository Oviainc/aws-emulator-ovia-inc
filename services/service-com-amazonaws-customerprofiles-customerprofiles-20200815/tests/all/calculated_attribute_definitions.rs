// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateCalculatedAttributeDefinition` and the readers that serve
//! what it stored (`GetCalculatedAttributeDefinition`,
//! `ListCalculatedAttributeDefinitions`).
//!
//! The rule under test: `CalculatedAttributeName` is the definition's identity within its
//! domain, so the FIRST create of a name is admitted with HTTP 200 and persists the
//! definition the request carried, and every later create of a name the domain already
//! holds — including every loser of a race between concurrent creates of one name — is
//! refused `BadRequestException` (400) without changing what the winner stored. The
//! identity is the (domain, name) PAIR: the same name in another domain is a different
//! definition.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::HISTORICAL_DATA_INDEXING_SPAN;

use crate::domain_profile_lifecycle::{error_code, Emulator};
use crate::profile_object_types::error_message;

/// Every value the modelled `Statistic` enum declares, in the order the model lists them.
const STATISTICS: [&str; 9] = [
    "FIRST_OCCURRENCE",
    "LAST_OCCURRENCE",
    "COUNT",
    "SUM",
    "MINIMUM",
    "MAXIMUM",
    "AVERAGE",
    "MAX_OCCURRENCE",
    "RECENT_OCCURRENCES",
];

/// The sentence the service refuses a create of an already-existing calculated attribute
/// with: fixed text whose only variable part is the contested name, taken from the
/// request's `{CalculatedAttributeName}` path label.
pub(crate) fn exists_message(name: &str) -> String {
    format!("Calculated attribute definition with name {name} already exists")
}

/// `CreateCalculatedAttributeDefinition` over the wire:
/// `POST /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}`.
pub(crate) async fn create(
    emulator: &Emulator,
    domain: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .post(
            &format!("/domains/{domain}/calculated-attributes/{name}"),
            body,
        )
        .await
}

/// `GetCalculatedAttributeDefinition` over the wire:
/// `GET /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}`.
pub(crate) async fn get(emulator: &Emulator, domain: &str, name: &str) -> (u16, Value) {
    emulator
        .send(
            "GET",
            &format!("/domains/{domain}/calculated-attributes/{name}"),
            None,
        )
        .await
}

/// `ListCalculatedAttributeDefinitions` over the wire:
/// `GET /domains/{DomainName}/calculated-attributes`, with any query string appended.
pub(crate) async fn list(emulator: &Emulator, domain: &str, query: &str) -> (u16, Value) {
    emulator
        .send(
            "GET",
            &format!("/domains/{domain}/calculated-attributes{query}"),
            None,
        )
        .await
}

/// The `CalculatedAttributeName`s a `ListCalculatedAttributeDefinitions` response reports,
/// in the order it reported them.
pub(crate) fn item_names(body: &Value) -> Vec<String> {
    body.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("CalculatedAttributeName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An `AttributeDetails`: the attribute items named in the expression, and the expression
/// itself, which follows the structure `AttributeDetails$Expression` documents
/// (`{ObjectTypeName.AttributeName}`).
pub(crate) fn attribute_details(object_type: &str, attributes: &[&str]) -> Value {
    json!({
        "Attributes": attributes
            .iter()
            .map(|name| json!({ "Name": name }))
            .collect::<Vec<Value>>(),
        "Expression": attributes
            .iter()
            .map(|name| format!("{{{object_type}.{name}}}"))
            .collect::<Vec<String>>()
            .join(" - "),
    })
}

/// The smallest request the model accepts: the two required body members.
pub(crate) fn definition(statistic: &str) -> Value {
    json!({
        "AttributeDetails": attribute_details("Orders", &["Amount"]),
        "Statistic": statistic,
    })
}

/// The definitions the success table stores, spanning what the rule generalises over: the
/// form of the name, the statistic, and how much optional configuration the request
/// carries.
fn success_rows() -> Vec<(&'static str, String, Value)> {
    let mut rows: Vec<(&'static str, String, Value)> = vec![
        (
            "the required members only",
            "Minimal".to_string(),
            definition("SUM"),
        ),
        (
            "a name that starts with the underscore the pattern allows",
            "_leading_underscore".to_string(),
            definition("COUNT"),
        ),
        (
            "a name carrying the digits and hyphens the pattern allows",
            "Order-Total-90d".to_string(),
            definition("AVERAGE"),
        ),
        (
            "a name at the modelled maximum length",
            format!("L{}", "o".repeat(254)),
            definition("MAXIMUM"),
        ),
        (
            "a name at the modelled minimum length",
            "A".to_string(),
            definition("MINIMUM"),
        ),
        (
            "two attributes in one expression",
            "Two_Attributes".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount", "Discount"]),
                "Statistic": "SUM",
            }),
        ),
        (
            "every optional member a request may carry",
            "Fully_Configured".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "DisplayName": "Order total",
                "Description": "the total of every order",
                "Conditions": {
                    "Range": { "Value": 30, "Unit": "DAYS" },
                    "ObjectCount": 10,
                    "Threshold": { "Value": "100", "Operator": "GREATER_THAN" },
                },
                "Filter": {
                    "Include": "ALL",
                    "Groups": [{
                        "Type": "ANY",
                        "Dimensions": [{
                            "Attributes": {
                                "Amount": {
                                    "DimensionType": "GREATER_THAN",
                                    "Values": ["10"],
                                },
                            },
                        }],
                    }],
                },
                "UseHistoricalData": false,
                "Tags": { "team": "crm" },
            }),
        ),
        (
            "a definition that asks for historical data",
            "Historical".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "UseHistoricalData": true,
            }),
        ),
    ];
    // One row per modelled statistic, so the stored aggregation is not pinned to the one
    // the other rows use.
    for statistic in STATISTICS {
        rows.push((
            "one definition per modelled Statistic",
            format!("Stat_{statistic}"),
            definition(statistic),
        ));
    }
    rows
}

/// RULE, success half: a create of a name the domain does not hold persists the
/// definition the request carried and answers from the record it stored, so the create's
/// echo, `GetCalculatedAttributeDefinition` and `ListCalculatedAttributeDefinitions` all
/// report the same definition.
#[tokio::test]
async fn a_create_persists_the_definition_and_the_readers_report_it() {
    let emulator = Emulator::new();
    emulator.create_domain("cadom", 30).await;

    let rows = success_rows();
    let mut expected_names: Vec<String> = Vec::new();

    for (case, name, body) in &rows {
        let (status, created) = create(&emulator, "cadom", name, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({name}): expected HTTP 200 for {body:?}, got {status} {created:?}"
        );
        assert_eq!(
            created.get("CalculatedAttributeName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the created name echoed, got {:?}",
            created.get("CalculatedAttributeName")
        );

        let (status, read) = get(&emulator, "cadom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetCalculatedAttributeDefinition 200, got {status} {read:?}"
        );

        // Every member the request carried comes back, from the create and from the read.
        for (member, expected) in body.as_object().expect("an object request") {
            assert_eq!(
                created.get(member),
                Some(expected),
                "case {case:?}: expected the create to echo {member} = {expected:?}, got {:?}",
                created.get(member)
            );
            assert_eq!(
                read.get(member),
                Some(expected),
                "case {case:?}: expected the read to report {member} = {expected:?} as the \
                 create stored it, got {:?}",
                read.get(member)
            );
        }
        assert_eq!(
            read.get("CalculatedAttributeName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the read to report the name, got {read:?}"
        );
        assert_eq!(
            read.get("CreatedAt"),
            created.get("CreatedAt"),
            "case {case:?}: expected the read to report the instant the create stored, \
             got {:?} then {:?}",
            created.get("CreatedAt"),
            read.get("CreatedAt")
        );
        assert!(
            read.get("LastUpdatedAt").is_some(),
            "case {case:?}: expected LastUpdatedAt, got {read:?}"
        );

        // A definition with no historical data to index is ready at once; one that asked
        // for historical data is still preparing at the instant it was created.
        let asked_for_history = body.get("UseHistoricalData") == Some(&json!(true));
        let expected_status = if asked_for_history {
            "PREPARING"
        } else {
            "COMPLETED"
        };
        for (reader, answer) in [("the create", &created), ("the read", &read)] {
            assert_eq!(
                answer.get("Status").and_then(Value::as_str),
                Some(expected_status),
                "case {case:?}: expected {reader} to report Status {expected_status}, got {:?}",
                answer.get("Status")
            );
        }
        if !asked_for_history {
            assert_eq!(
                read.get("Readiness")
                    .and_then(|readiness| readiness.get("ProgressPercentage"))
                    .and_then(Value::as_i64),
                Some(100),
                "case {case:?}: a definition with nothing to index must report full \
                 progress, got {read:?}"
            );
        }

        // The read changed nothing a later caller sees: the stored members are identical.
        let (status, again) = get(&emulator, "cadom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second read to answer 200, got {status} {again:?}"
        );
        for member in ["CalculatedAttributeName", "Statistic", "AttributeDetails", "CreatedAt"] {
            assert_eq!(
                again.get(member),
                read.get(member),
                "case {case:?}: two reads of an unchanged definition must agree on \
                 {member}, got {:?} then {:?}",
                read.get(member),
                again.get(member)
            );
        }

        expected_names.push(name.clone());
    }

    // The listing reports every definition the domain holds, in name order.
    expected_names.sort();
    let (status, listed) = list(&emulator, "cadom", "?max-results=100").await;
    assert_eq!(
        status, 200,
        "case \"the listing\": expected 200, got {status} {listed:?}"
    );
    assert_eq!(
        item_names(&listed),
        expected_names,
        "case \"the listing\": expected every stored definition in name order, got {listed:?}"
    );
}

/// RULE, the clock-owed half: the indexing of a definition's historical data is owed, so
/// the definition reaches `COMPLETED` once the span has passed, with no further call.
#[tokio::test]
async fn a_definition_that_asked_for_historical_data_completes_when_the_span_has_passed() {
    let emulator = Emulator::new();
    emulator.create_domain("spandom", 30).await;

    let body = json!({
        "AttributeDetails": attribute_details("Orders", &["Amount"]),
        "Statistic": "SUM",
        "UseHistoricalData": true,
    });
    let (status, created) = create(&emulator, "spandom", "Indexed", body.clone()).await;
    assert_eq!(
        status, 200,
        "case \"the create\": expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created.get("Status").and_then(Value::as_str),
        Some("PREPARING"),
        "case \"the create\": expected the transient status, got {created:?}"
    );

    tokio::time::sleep(HISTORICAL_DATA_INDEXING_SPAN + Duration::from_millis(250)).await;

    let (status, read) = get(&emulator, "spandom", "Indexed").await;
    assert_eq!(
        status, 200,
        "case \"after the span\": expected 200, got {status} {read:?}"
    );
    assert_eq!(
        read.get("Status").and_then(Value::as_str),
        Some("COMPLETED"),
        "case \"after the span\": expected COMPLETED once the indexing is due, got {read:?}"
    );
    assert_eq!(
        read.get("Readiness")
            .and_then(|readiness| readiness.get("ProgressPercentage"))
            .and_then(Value::as_i64),
        Some(100),
        "case \"after the span\": expected full progress, got {read:?}"
    );
    assert_eq!(
        read.get("CreatedAt"),
        created.get("CreatedAt"),
        "case \"after the span\": the materialised transition must not move CreatedAt, \
         got {read:?}"
    );
}

/// RULE, refusal half: a create of a name the domain already holds is refused
/// `BadRequestException` (400), the refusal names the contested name, and the definition
/// the winner stored is untouched. The rows differ in how the second request differs from
/// the first: identical, a different statistic, more configuration, less.
#[tokio::test]
async fn a_create_of_a_held_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    emulator.create_domain("dupdom", 30).await;

    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "the identical request again",
            "Repeated".to_string(),
            definition("SUM"),
            definition("SUM"),
        ),
        (
            "a different statistic",
            "Restated".to_string(),
            definition("SUM"),
            definition("COUNT"),
        ),
        (
            "more configuration than the first",
            "Widened".to_string(),
            definition("MINIMUM"),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount", "Discount"]),
                "Statistic": "MAXIMUM",
                "DisplayName": "Second attempt",
                "Description": "the loser's description",
                "UseHistoricalData": true,
                "Tags": { "team": "other" },
            }),
        ),
        (
            "less configuration than the first",
            "Narrowed".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "AVERAGE",
                "DisplayName": "First attempt",
                "Tags": { "team": "crm" },
            }),
            definition("COUNT"),
        ),
        (
            "a name at the modelled maximum length",
            format!("D{}", "d".repeat(254)),
            definition("LAST_OCCURRENCE"),
            definition("FIRST_OCCURRENCE"),
        ),
    ];

    for (case, name, first, second) in &rows {
        let (status, winner) = create(&emulator, "dupdom", name, first.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to win, got {status} {winner:?}"
        );

        let (status, refused) = create(&emulator, "dupdom", name, second.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a name the domain holds, got {status} \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message,
            exists_message(name),
            "case {case:?}: expected the already-exists sentence for {name}, got {message:?}"
        );

        // The refused create changed nothing: the read still reports the winner.
        let (status, read) = get(&emulator, "dupdom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the winner still readable, got {status} {read:?}"
        );
        for (member, expected) in first.as_object().expect("an object request") {
            assert_eq!(
                read.get(member),
                Some(expected),
                "case {case:?}: the refused create must leave {member} = {expected:?}, got {:?}",
                read.get(member)
            );
        }
        for member in ["DisplayName", "Description", "UseHistoricalData", "Tags"] {
            if first.get(member).is_none() {
                assert_eq!(
                    read.get(member),
                    None,
                    "case {case:?}: the refused create must not add {member}, got {:?}",
                    read.get(member)
                );
            }
        }
    }

    // One definition per contested name, never two.
    let (status, listed) = list(&emulator, "dupdom", "?max-results=100").await;
    assert_eq!(
        status, 200,
        "case \"the listing\": expected 200, got {status} {listed:?}"
    );
    let mut expected: Vec<String> = rows.iter().map(|(_, name, _, _)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        item_names(&listed),
        expected,
        "case \"the listing\": expected exactly one definition per contested name, \
         got {listed:?}"
    );
}

/// RULE, reached concurrently: of N callers creating one name in one domain at the same
/// time exactly ONE is admitted, every other is refused `BadRequestException` (400), and
/// the domain is left holding exactly one definition of that name — the winner's. The
/// rows differ along what a race generalises over: how many racers there are, and the
/// form of the contested name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_of_many_concurrent_creates_of_one_name_wins() {
    let emulator = Arc::new(Emulator::new());
    emulator.create_domain("racedom", 30).await;

    let rows: Vec<(&str, usize, String)> = vec![
        ("two racers", 2, "Two_Racers".to_string()),
        ("six racers", 6, "Six-Racers".to_string()),
        ("eleven racers", 11, "_Eleven_Racers".to_string()),
        (
            "six racers over a name at the modelled maximum length",
            6,
            format!("R{}", "r".repeat(254)),
        ),
    ];

    for (case, racers, name) in &rows {
        let mut pending = Vec::new();
        for racer in 0..*racers {
            let emulator = Arc::clone(&emulator);
            let name = name.clone();
            // Every racer sends a DIFFERENT statistic, so the winner's record identifies
            // which call was admitted.
            let statistic = STATISTICS[racer % STATISTICS.len()].to_string();
            pending.push(tokio::spawn(async move {
                let answer = create(&emulator, "racedom", &name, definition(&statistic)).await;
                (racer, statistic, answer)
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        let mut refused = 0usize;
        for task in pending {
            let (racer, statistic, (status, body)) = task.await.expect("the racing create ran");
            match status {
                200 => winners.push(statistic),
                400 => {
                    assert_eq!(
                        error_code(&body),
                        "BadRequestException",
                        "case {case:?} racer {racer}: a refused racer must be refused \
                         BadRequestException, got {body:?}"
                    );
                    assert_eq!(
                        error_message(&body),
                        exists_message(name),
                        "case {case:?} racer {racer}: expected the already-exists sentence \
                         for {name}, got {body:?}"
                    );
                    refused += 1;
                }
                status => panic!(
                    "case {case:?} racer {racer}: expected 200 or 400, got {status} {body:?}"
                ),
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly 1 winner of {racers} racers, got {winners:?}"
        );
        assert_eq!(
            refused,
            racers - 1,
            "case {case:?}: expected the other {} racers refused, got {refused}",
            racers - 1
        );

        // Exactly one definition of that name survives, and it is the winner's.
        let (status, read) = get(&emulator, "racedom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the survivor readable, got {status} {read:?}"
        );
        assert_eq!(
            read.get("Statistic").and_then(Value::as_str),
            Some(winners[0].as_str()),
            "case {case:?}: expected the winner's Statistic {:?}, got {read:?}",
            winners[0]
        );
    }

    let (status, listed) = list(&emulator, "racedom", "?max-results=100").await;
    assert_eq!(
        status, 200,
        "case \"the listing\": expected 200, got {status} {listed:?}"
    );
    let mut expected: Vec<String> = rows.iter().map(|(_, _, name)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        item_names(&listed),
        expected,
        "case \"the listing\": expected exactly one definition per raced name, got {listed:?}"
    );
}

/// RULE, the identity is the PAIR: the same calculated attribute name is a different
/// definition in a different domain, so every domain admits it, each read answers from its
/// own domain, and each listing reports only its own.
#[tokio::test]
async fn a_calculated_attribute_name_is_unique_within_one_domain_only() {
    let emulator = Emulator::new();

    let domains = ["scopeone", "scopetwo", "scope-three"];
    for (index, domain) in domains.iter().enumerate() {
        emulator.create_domain(domain, 30).await;
        let statistic = STATISTICS[index];
        let (status, created) = create(&emulator, domain, "Shared_Name", definition(statistic))
            .await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected a shared name admitted per domain, got {status} \
             {created:?}"
        );
        assert_eq!(
            created.get("Statistic").and_then(Value::as_str),
            Some(statistic),
            "case {domain:?}: expected the statistic it was given, got {created:?}"
        );
    }

    for (index, domain) in domains.iter().enumerate() {
        let (status, read) = get(&emulator, domain, "Shared_Name").await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected the definition readable, got {status} {read:?}"
        );
        assert_eq!(
            read.get("Statistic").and_then(Value::as_str),
            Some(STATISTICS[index]),
            "case {domain:?}: expected this domain's own definition, got {read:?}"
        );

        let (status, listed) = list(&emulator, domain, "").await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected the listing to answer 200, got {status} {listed:?}"
        );
        assert_eq!(
            item_names(&listed),
            vec!["Shared_Name".to_string()],
            "case {domain:?}: expected only this domain's definition, got {listed:?}"
        );
    }
}

/// The listing is paginated: it walks the domain's definitions in name order, hands back
/// the cursor of the first item that did not fit, and resumes there. Asserted over a set
/// of page sizes rather than one.
#[tokio::test]
async fn the_listing_pages_through_the_definitions_in_name_order() {
    let emulator = Emulator::new();
    emulator.create_domain("pagedom", 30).await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("Attr_{index}");
        let (status, created) = create(&emulator, "pagedom", &name, definition("SUM")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the create to succeed, got {status} {created:?}"
        );
        stored.push(name);
    }
    stored.sort();

    // `MaxResults` is modelled range 1..100, so the page sizes below include its minimum,
    // a size that divides the stored count exactly, one that does not, and one above it.
    for page_size in [1usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(cursor) => format!("?max-results={page_size}&next-token={cursor}"),
                None => format!("?max-results={page_size}"),
            };
            let (status, body) = list(&emulator, "pagedom", &query).await;
            assert_eq!(
                status, 200,
                "case \"page size {page_size}\": expected 200, got {status} {body:?}"
            );
            let page = item_names(&body);
            assert!(
                page.len() <= page_size,
                "case \"page size {page_size}\": a page must not exceed the requested size, \
                 got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case \"page size {page_size}\": the cursor must terminate, got {seen:?}"
            );
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, stored,
            "case \"page size {page_size}\": expected every definition once, in name order, \
             got {seen:?}"
        );
    }
}

/// The refusals around the operations: a domain this emulator does not hold, a name the
/// domain does not hold, and input outside a modelled constraint — each on its published
/// bound and one past it.
#[tokio::test]
async fn the_calculated_attribute_operations_refuse_absent_resources_and_invalid_input() {
    let emulator = Emulator::new();
    emulator.create_domain("bounddom", 30).await;

    for (case, status_and_code) in [
        (
            "a create into a domain that is not stored",
            create(&emulator, "nosuchdomain", "Anything", definition("SUM")).await,
        ),
        (
            "a read in a domain that is not stored",
            get(&emulator, "nosuchdomain", "Anything").await,
        ),
        (
            "a listing of a domain that is not stored",
            list(&emulator, "nosuchdomain", "").await,
        ),
        (
            "a read of a name the domain does not hold",
            get(&emulator, "bounddom", "NeverDefined").await,
        ),
    ] {
        let (status, body) = status_and_code;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
    }

    // An empty domain lists nothing rather than refusing.
    let (status, listed) = list(&emulator, "bounddom", "").await;
    assert_eq!(
        (status, item_names(&listed)),
        (200, Vec::new()),
        "case \"an empty domain\": expected an empty listing, got {status} {listed:?}"
    );

    // `Description` is `sensitiveText` len 1..1000, `AttributeDetails$Attributes` is a
    // list of length 1..50, `AttributeDetails$Expression` is `string1To255`,
    // `Conditions$ObjectCount` has range min 1, and `Statistic` is a closed enum.
    let invalid: Vec<(&str, &str, Value)> = vec![
        (
            "a Description one past the maximum length",
            "Long_Description",
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "Description": "d".repeat(1001),
            }),
        ),
        (
            "a Description below the minimum length",
            "Empty_Description",
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "Description": "",
            }),
        ),
        (
            "no AttributeDetails at all",
            "No_Details",
            json!({ "Statistic": "SUM" }),
        ),
        (
            "no Statistic at all",
            "No_Statistic",
            json!({ "AttributeDetails": attribute_details("Orders", &["Amount"]) }),
        ),
        (
            "a Statistic outside the modelled enum",
            "Bad_Statistic",
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "MEDIAN",
            }),
        ),
        (
            "an empty Attributes list",
            "No_Attributes",
            json!({
                "AttributeDetails": { "Attributes": [], "Expression": "{Orders.Amount}" },
                "Statistic": "SUM",
            }),
        ),
        (
            "an Expression one past the maximum length",
            "Long_Expression",
            json!({
                "AttributeDetails": {
                    "Attributes": [{ "Name": "Amount" }],
                    "Expression": "e".repeat(256),
                },
                "Statistic": "SUM",
            }),
        ),
        (
            "an ObjectCount below the modelled minimum",
            "Zero_Object_Count",
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "Conditions": { "ObjectCount": 0 },
            }),
        ),
    ];

    for (case, name, body) in invalid {
        let (status, response) = create(&emulator, "bounddom", name, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {body:?}, got {status} {response:?}"
        );
        let (status, read) = get(&emulator, "bounddom", name).await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create must store nothing, got {status} {read:?}"
        );
    }

    // The bounds themselves are accepted: an Expression and a Description at their
    // maximum length, and the smallest Attributes list.
    let (status, created) = create(
        &emulator,
        "bounddom",
        "At_The_Bounds",
        json!({
            "AttributeDetails": {
                "Attributes": [{ "Name": "Amount" }],
                "Expression": "e".repeat(255),
            },
            "Statistic": "SUM",
            "Description": "d".repeat(1000),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"input at the published bounds\": expected 200, got {status} {created:?}"
    );
}
