// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Cost Category definition lifecycle
//! (`CreateCostCategoryDefinition`, `DescribeCostCategoryDefinition`,
//! `ListCostCategoryDefinitions`, `UpdateCostCategoryDefinition`,
//! `DeleteCostCategoryDefinition`).
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::{
    month_start_shifted, Timestamp, EFFECTIVE_START_MONTHS_BACK, MAX_LIVE_COST_CATEGORIES,
    PROCESSING_SPAN,
};

use crate::harness::{error_code, Emulator};

/// The only value `CostCategoryRuleVersion` admits (model enum
/// `CostCategoryRuleVersion`).
const RULE_VERSION: &str = "CostCategoryExpression.v1";

/// A `CostCategoryRule` assigning `value` to one linked account.
fn rule(value: &str) -> Value {
    json!({
        "Value": value,
        "Type": "REGULAR",
        "Rule": {
            "Dimensions": {
                "Key": "LINKED_ACCOUNT",
                "Values": [engine::emulator_account_id()],
                "MatchOptions": ["EQUALS"],
            }
        }
    })
}

/// A minimal valid `CreateCostCategoryDefinition` body for `name`.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "RuleVersion": RULE_VERSION,
        "Rules": [rule("Engineering")],
    })
}

/// The first day of the month `shift` months from the current one, as a
/// `ZonedDateTime`.
fn month_shifted(shift: i64) -> String {
    month_start_shifted(Timestamp::now(), shift).expect("the shifted month is representable")
}

fn arn_of(created: &Value) -> String {
    created["CostCategoryArn"]
        .as_str()
        .expect("CostCategoryArn is present")
        .to_string()
}

/// Assert `value` is the `ZonedDateTime` form the model's pattern admits:
/// `^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(([+-]\d\d:\d\d)|Z)$`, in the seconds-precision `Z`
/// shape this service renders.
fn assert_zoned_date_time(label: &str, value: &str) {
    assert_eq!(
        value.len(),
        20,
        "case {label:?}: a rendered ZonedDateTime is 20 characters, got {value:?}"
    );
    let bytes = value.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':'), (19, b'Z')];
    for (index, expected) in separators {
        assert_eq!(
            bytes[index], expected,
            "case {label:?}: expected {:?} at index {index} of {value:?}",
            char::from(expected)
        );
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        assert!(
            bytes[index].is_ascii_digit(),
            "case {label:?}: expected a digit at index {index} of {value:?}"
        );
    }
}

/// Assert the resource part of a cost category ARN is the UUID shape AWS's cost category
/// ARNs carry: 8-4-4-4-12 lowercase hex.
fn assert_uuid_shaped(label: &str, arn: &str) {
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {label:?}: expected an 8-4-4-4-12 identifier, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase() || c == '-'),
        "case {label:?}: expected lowercase hex groups, got {id:?}"
    );
}

/// The whole lifecycle from one client's point of view: everything read back is what
/// this client wrote, and the ARN it was handed is the one the registry answers on.
#[tokio::test]
async fn a_definition_round_trips_through_create_describe_list_update_and_delete() {
    let emulator = Emulator::start();

    let created = emulator
        .ok(
            "CreateCostCategoryDefinition",
            &json!({
                "Name": "demoCostCategory",
                "RuleVersion": RULE_VERSION,
                "Rules": [rule("Engineering")],
                "ResourceTags": [{"Key": "team", "Value": "platform"}],
            }),
        )
        .await;
    let arn = arn_of(&created);

    // A minted ARN must satisfy the modelled `Arn` pattern, and must name THIS service
    // and the emulator's own account.
    assert!(
        arn.starts_with(&format!(
            "arn:aws:ce::{}:costcategory/",
            engine::emulator_account_id()
        )),
        "the created ARN must be a ce cost category ARN on the emulator account, got {arn:?}"
    );
    assert!(
        (20..=2048).contains(&arn.len()),
        "the created ARN must satisfy the modelled length 20..2048, got {} chars",
        arn.len()
    );
    assert_uuid_shaped("the created ARN", &arn);

    // Create answers with the stored start date, which defaults to this month.
    assert_zoned_date_time(
        "the created EffectiveStart",
        created["EffectiveStart"].as_str().unwrap_or_default(),
    );
    assert_eq!(
        created["EffectiveStart"].as_str(),
        Some(month_shifted(0).as_str()),
        "an absent EffectiveStart defaults to the first day of the current month, got {created}"
    );

    // Describe reports what was stored, not what any request echoed.
    let described = emulator
        .ok(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    let category = &described["CostCategory"];
    assert_eq!(
        category["Name"].as_str(),
        Some("demoCostCategory"),
        "Describe must report the stored Name, got {described}"
    );
    assert_eq!(
        category["RuleVersion"].as_str(),
        Some(RULE_VERSION),
        "Describe must report the stored RuleVersion, got {described}"
    );
    assert_eq!(
        category["Rules"],
        json!([rule("Engineering")]),
        "Describe must report the stored Rules, got {described}"
    );
    assert_eq!(
        category["EffectiveStart"], created["EffectiveStart"],
        "Describe must report the same EffectiveStart Create answered with, got {described}"
    );
    assert!(
        category.get("EffectiveEnd").is_none(),
        "a live definition has no EffectiveEnd, got {described}"
    );

    // List reports the same definition, with the rule count derived from the rules.
    let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
    let references = listed["CostCategoryReferences"]
        .as_array()
        .expect("CostCategoryReferences is a list");
    assert_eq!(
        references.len(),
        1,
        "List must report the one stored definition, got {listed}"
    );
    assert_eq!(
        references[0]["CostCategoryArn"].as_str(),
        Some(arn.as_str()),
        "List must report the created ARN, got {listed}"
    );
    assert_eq!(
        references[0]["NumberOfRules"].as_i64(),
        Some(1),
        "NumberOfRules must be the stored rule count, got {listed}"
    );
    assert_eq!(
        references[0]["Values"],
        json!(["Engineering"]),
        "Values must be the stored rules' values, got {listed}"
    );

    // Update mutates the STORED definition.
    let updated = emulator
        .ok(
            "UpdateCostCategoryDefinition",
            &json!({
                "CostCategoryArn": arn,
                "RuleVersion": RULE_VERSION,
                "Rules": [rule("Engineering"), rule("Research")],
                "DefaultValue": "Other",
            }),
        )
        .await;
    assert_eq!(
        updated["CostCategoryArn"].as_str(),
        Some(arn.as_str()),
        "Update must answer with the ARN it updated, got {updated}"
    );

    let described = emulator
        .ok(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    let category = &described["CostCategory"];
    assert_eq!(
        category["Rules"],
        json!([rule("Engineering"), rule("Research")]),
        "Describe must report the updated Rules, got {described}"
    );
    assert_eq!(
        category["DefaultValue"].as_str(),
        Some("Other"),
        "Describe must report the updated DefaultValue, got {described}"
    );

    let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
    assert_eq!(
        listed["CostCategoryReferences"][0]["NumberOfRules"].as_i64(),
        Some(2),
        "NumberOfRules must follow the update, got {listed}"
    );
    assert_eq!(
        listed["CostCategoryReferences"][0]["Values"],
        json!(["Engineering", "Research"]),
        "Values must follow the update, got {listed}"
    );

    // Delete ends it: the ARN comes back with the end date that was stored.
    let deleted = emulator
        .ok(
            "DeleteCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    assert_eq!(
        deleted["CostCategoryArn"].as_str(),
        Some(arn.as_str()),
        "Delete must answer with the ARN it ended, got {deleted}"
    );
    let effective_end = deleted["EffectiveEnd"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_zoned_date_time("the deleted EffectiveEnd", &effective_end);

    // And afterwards it is gone from both readers.
    let (status, body) = emulator
        .call(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "Describe on a deleted ARN must be 404 ResourceNotFoundException, got {status}: {body}"
    );
    let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
    assert_eq!(
        listed["CostCategoryReferences"],
        json!([]),
        "List must not include a deleted definition, got {listed}"
    );
}

/// RULE: `EffectiveStart` is a billing start date — the first day of a month at
/// midnight — no earlier than twelve months before the current month and no later than
/// the current month; absent, it is the first day of the current month.
///
/// The table spans the class the rule generalises over: the window's published bounds
/// and one month past each, the two ways a date can fail to be a billing start date,
/// and a non-UTC offset form the modelled pattern admits.
#[tokio::test]
async fn effective_start_is_a_billing_start_date_inside_the_documented_window() {
    struct Case {
        label: &'static str,
        requested: Option<String>,
        /// The `EffectiveStart` the service must store, or `None` when it must refuse.
        expected: Option<String>,
    }

    // `EFFECTIVE_START_MONTHS_BACK` is the documented window: "Dates can't be before
    // the previous twelve months, or in the future."
    let back = EFFECTIVE_START_MONTHS_BACK;
    let cases = vec![
        Case {
            label: "absent",
            requested: None,
            expected: Some(month_shifted(0)),
        },
        Case {
            label: "the current month (the window's upper bound)",
            requested: Some(month_shifted(0)),
            expected: Some(month_shifted(0)),
        },
        Case {
            label: "one month back",
            requested: Some(month_shifted(-1)),
            expected: Some(month_shifted(-1)),
        },
        Case {
            label: "twelve months back (the window's lower bound)",
            requested: Some(month_shifted(-back)),
            expected: Some(month_shifted(-back)),
        },
        Case {
            label: "thirteen months back (one month past the lower bound)",
            requested: Some(month_shifted(-back - 1)),
            expected: None,
        },
        Case {
            label: "next month (one month past the upper bound)",
            requested: Some(month_shifted(1)),
            expected: None,
        },
        Case {
            label: "a mid-month day",
            requested: Some(month_shifted(0).replace("-01T", "-15T")),
            expected: None,
        },
        Case {
            label: "the first of the month but not at midnight",
            requested: Some(month_shifted(0).replace("T00:00:00", "T12:30:00")),
            expected: None,
        },
        Case {
            label: "a month start written with a non-UTC offset",
            requested: Some(month_shifted(0).replace('Z', "+05:00")),
            expected: Some(month_shifted(0).replace('Z', "+05:00")),
        },
    ];

    let emulator = Emulator::start();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("effectiveStart{index}");
        let mut body = create_body(&name);
        if let Some(requested) = &case.requested {
            body["EffectiveStart"] = json!(requested);
        }
        let (status, response) = emulator.call("CreateCostCategoryDefinition", &body).await;

        match &case.expected {
            None => {
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "ValidationException"),
                    "case {:?} ({:?}): expected a 400 ValidationException, got {status}: {response}",
                    case.label,
                    case.requested
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {:?} ({:?}): expected 200, got {status}: {response}",
                    case.label, case.requested
                );
                assert_eq!(
                    response["EffectiveStart"].as_str(),
                    Some(expected.as_str()),
                    "case {:?}: expected EffectiveStart {expected:?}, got {response}",
                    case.label
                );
                // And the stored definition reports the same date, not the response's.
                let described = emulator
                    .ok(
                        "DescribeCostCategoryDefinition",
                        &json!({"CostCategoryArn": arn_of(&response)}),
                    )
                    .await;
                assert_eq!(
                    described["CostCategory"]["EffectiveStart"].as_str(),
                    Some(expected.as_str()),
                    "case {:?}: Describe must report EffectiveStart {expected:?}, got {described}",
                    case.label
                );
            }
        }
    }
}

/// RULE: `NumberOfRules` is the stored rule count and `Values` is the stored rules'
/// distinct values in rule order — both DERIVED from the rules, for any rule list.
#[tokio::test]
async fn rule_count_and_values_are_derived_from_the_stored_rules() {
    struct Case {
        label: &'static str,
        values: Vec<&'static str>,
        expected_values: Vec<&'static str>,
    }

    let cases = [
        Case {
            label: "one rule",
            values: vec!["Engineering"],
            expected_values: vec!["Engineering"],
        },
        Case {
            label: "two rules with distinct values",
            values: vec!["Engineering", "Research"],
            expected_values: vec!["Engineering", "Research"],
        },
        Case {
            label: "three rules, one value repeated",
            values: vec!["Engineering", "Research", "Engineering"],
            expected_values: vec!["Engineering", "Research"],
        },
        Case {
            label: "five rules",
            values: vec!["A", "B", "C", "D", "E"],
            expected_values: vec!["A", "B", "C", "D", "E"],
        },
    ];
    let emulator = Emulator::start();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("ruleCount{index}");
        let rules: Vec<Value> = case.values.iter().map(|value| rule(value)).collect();
        let created = emulator
            .ok(
                "CreateCostCategoryDefinition",
                &json!({"Name": name, "RuleVersion": RULE_VERSION, "Rules": rules}),
            )
            .await;
        let arn = arn_of(&created);

        let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
        let reference = listed["CostCategoryReferences"]
            .as_array()
            .expect("CostCategoryReferences is a list")
            .iter()
            .find(|entry| entry["CostCategoryArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: {arn} must be listed, got {listed}", case.label));

        assert_eq!(
            reference["NumberOfRules"].as_i64(),
            Some(i64::try_from(case.values.len()).expect("a small rule count")),
            "case {:?}: expected NumberOfRules {}, got {reference}",
            case.label,
            case.values.len()
        );
        assert_eq!(
            reference["Values"],
            json!(case.expected_values),
            "case {:?}: expected Values {:?}, got {reference}",
            case.label,
            case.expected_values
        );
        assert_eq!(
            reference["Name"].as_str(),
            Some(name.as_str()),
            "case {:?}: expected Name {name:?}, got {reference}",
            case.label
        );
    }
}

/// RULE: a page holds at most `MaxResults` references and a `NextToken` is present
/// exactly when definitions remain; following it walks every definition once.
///
/// The table spans the page sizes from one per page to more than the whole set,
/// including the published bounds of `CostCategoryMaxResults` (1..100).
#[tokio::test]
async fn list_paginates_every_definition_exactly_once() {
    const DEFINITIONS: usize = 7;

    let emulator = Emulator::start();
    let mut created = Vec::new();
    for index in 0..DEFINITIONS {
        let response = emulator
            .ok(
                "CreateCostCategoryDefinition",
                &create_body(&format!("page{index}")),
            )
            .await;
        created.push(arn_of(&response));
    }
    created.sort();
    let mut distinct = created.clone();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        DEFINITIONS,
        "every create must mint a distinct ARN, got {created:?}"
    );

    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let listed = emulator.ok("ListCostCategoryDefinitions", &body).await;
            let references = listed["CostCategoryReferences"]
                .as_array()
                .expect("CostCategoryReferences is a list")
                .clone();
            pages += 1;
            assert!(
                pages <= DEFINITIONS + 1,
                "case MaxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            if let Some(size) = page_size {
                assert!(
                    references.len() <= size,
                    "case MaxResults={page_size:?}: a page held {} references, more than the \
                     page size",
                    references.len()
                );
            }
            for reference in &references {
                seen.push(
                    reference["CostCategoryArn"]
                        .as_str()
                        .expect("every reference carries its ARN")
                        .to_string(),
                );
            }
            next_token = listed["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case MaxResults={page_size:?}: expected to walk {created:?} exactly once, got {seen:?}"
        );
        let expected_pages = page_size.map_or(1, |size| DEFINITIONS.div_ceil(size));
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={page_size:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: `MaxResults` outside the modelled range 1..100 is refused, and a `NextToken`
/// this service never issued is refused — neither is served as a page.
#[tokio::test]
async fn list_refuses_an_out_of_range_page_size_and_an_unissued_token() {
    let emulator = Emulator::start();
    emulator
        .ok("CreateCostCategoryDefinition", &create_body("tokenCheck"))
        .await;

    // The published bounds of `CostCategoryMaxResults` (range 1..100) are served, and
    // one past each bound is refused.
    for (label, max_results, expect_ok) in [
        ("the lower bound", 1, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let (status, body) = emulator
            .call(
                "ListCostCategoryDefinitions",
                &json!({"MaxResults": max_results}),
            )
            .await;
        if expect_ok {
            assert_eq!(
                status, 200,
                "case {label:?} (MaxResults={max_results}): expected 200, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?} (MaxResults={max_results}): expected a 400 refusal, got \
                 {status}: {body}"
            );
        }
    }

    for (label, token) in [
        ("not hex", "not-a-token"),
        ("hex of bytes that are not UTF-8", "00ff"),
    ] {
        let (status, body) = emulator
            .call("ListCostCategoryDefinitions", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?} (NextToken={token:?}): expected a 400 ValidationException, got \
             {status}: {body}"
        );
    }

    // A WELL-FORMED cursor naming no stored ARN is not malformed: it resumes after that
    // ARN, which is an empty page when nothing sorts after it.
    let past_the_end = hex::encode(b"arn:aws:ce::000000000000:costcategory/zzzzzzzz");
    let listed = emulator
        .ok(
            "ListCostCategoryDefinitions",
            &json!({"NextToken": past_the_end}),
        )
        .await;
    assert_eq!(
        listed["CostCategoryReferences"],
        json!([]),
        "case \"a well-formed cursor past the last ARN\": expected an empty page, got {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "case \"a well-formed cursor past the last ARN\": expected no NextToken, got {listed}"
    );
}

/// RULE: a read answers as of `EffectiveOn`, or as of now when it is absent — so a
/// definition is invisible before it starts and after it ends, and still visible at a
/// date inside its lifetime.
#[tokio::test]
async fn reads_are_answered_as_of_the_effective_date() {
    let emulator = Emulator::start();
    let start = month_shifted(-2);
    let created = emulator
        .ok(
            "CreateCostCategoryDefinition",
            &json!({
                "Name": "effectiveOn",
                "RuleVersion": RULE_VERSION,
                "Rules": [rule("Engineering")],
                "EffectiveStart": start,
            }),
        )
        .await;
    let arn = arn_of(&created);

    // Before it started: not found, and not listed.
    let before = month_shifted(-3);
    let (status, body) = emulator
        .call(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn, "EffectiveOn": before}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {before:?} (before EffectiveStart): expected 404, got {status}: {body}"
    );
    let listed = emulator
        .ok(
            "ListCostCategoryDefinitions",
            &json!({"EffectiveOn": before}),
        )
        .await;
    assert_eq!(
        listed["CostCategoryReferences"],
        json!([]),
        "case {before:?} (before EffectiveStart): expected an empty list, got {listed}"
    );

    // On its start date, and now: visible.
    for label in [start.clone(), month_shifted(0)] {
        let described = emulator
            .ok(
                "DescribeCostCategoryDefinition",
                &json!({"CostCategoryArn": arn, "EffectiveOn": label}),
            )
            .await;
        assert_eq!(
            described["CostCategory"]["CostCategoryArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?} (inside the lifetime): expected the definition, got {described}"
        );
    }

    emulator
        .ok(
            "DeleteCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;

    // After it ended: gone as of now, but still describable as of its start date — and
    // then it carries the EffectiveEnd the delete stored.
    let (status, body) = emulator
        .call(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"as of now, after the delete\": expected 404, got {status}: {body}"
    );
    let described = emulator
        .ok(
            "DescribeCostCategoryDefinition",
            &json!({"CostCategoryArn": arn, "EffectiveOn": start}),
        )
        .await;
    assert_eq!(
        described["CostCategory"]["EffectiveStart"].as_str(),
        Some(start.as_str()),
        "case {start:?} (inside the ended lifetime): expected the definition, got {described}"
    );
    assert!(
        described["CostCategory"]["EffectiveEnd"].is_string(),
        "case {start:?}: an ended definition reports its EffectiveEnd, got {described}"
    );
    let listed = emulator
        .ok("ListCostCategoryDefinitions", &json!({"EffectiveOn": start}))
        .await;
    assert_eq!(
        listed["CostCategoryReferences"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {start:?}: List as of a date inside the ended lifetime still reports it, got \
         {listed}"
    );
}

/// RULE: an operation naming a cost category the service does not hold as of now
/// answers `ResourceNotFoundException` (404) — whether the ARN was never created or has
/// already been ended.
#[tokio::test]
async fn an_unheld_arn_is_a_resource_not_found_everywhere() {
    let emulator = Emulator::start();
    let created = emulator
        .ok("CreateCostCategoryDefinition", &create_body("notFound"))
        .await;
    let live = arn_of(&created);
    let never_created = engine::emulator_arn(
        "ce",
        "",
        "costcategory/00000000-0000-4000-8000-000000000000",
    );

    emulator
        .ok(
            "DeleteCostCategoryDefinition",
            &json!({"CostCategoryArn": live}),
        )
        .await;

    let update = json!({
        "RuleVersion": RULE_VERSION,
        "Rules": [rule("Engineering")],
    });
    for (label, arn) in [
        ("an ARN that was never created", never_created.as_str()),
        ("an ARN that was already deleted", live.as_str()),
    ] {
        for operation in [
            "DescribeCostCategoryDefinition",
            "UpdateCostCategoryDefinition",
            "DeleteCostCategoryDefinition",
        ] {
            let mut body = update.clone();
            body["CostCategoryArn"] = json!(arn);
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?} on {operation}: expected 404 ResourceNotFoundException, got \
                 {status}: {response}"
            );
            assert_eq!(
                response["resourceName"].as_str().or(response["ResourceName"].as_str()),
                Some(arn),
                "case {label:?} on {operation}: the refusal must name the ARN, got {response}"
            );
        }
    }
}

/// RULE: `CostCategoryName` is unique among the definitions in effect, so a name a live
/// definition holds is refused — and becomes available again once that definition ends.
#[tokio::test]
async fn a_name_a_live_definition_holds_is_refused() {
    let emulator = Emulator::start();
    let created = emulator
        .ok("CreateCostCategoryDefinition", &create_body("sharedName"))
        .await;
    let arn = arn_of(&created);

    let (status, body) = emulator
        .call("CreateCostCategoryDefinition", &create_body("sharedName"))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"a name a live definition holds\": expected a 400 ValidationException, got \
         {status}: {body}"
    );

    // A different name is accepted, which is what makes the refusal above about the name.
    emulator
        .ok(
            "CreateCostCategoryDefinition",
            &create_body("anotherSharedName"),
        )
        .await;

    emulator
        .ok(
            "DeleteCostCategoryDefinition",
            &json!({"CostCategoryArn": arn}),
        )
        .await;
    let reused = emulator
        .ok("CreateCostCategoryDefinition", &create_body("sharedName"))
        .await;
    assert_ne!(
        arn_of(&reused),
        arn,
        "case \"a name freed by a delete\": the reused name must mint a NEW definition"
    );
}

/// RULE: `SupportedResourceTypes` accepts only the resource types the operation
/// documents; a value outside that catalog is refused, and a well-formed filter the
/// emulator cannot decide is answered with 501 rather than an empty page.
#[tokio::test]
async fn the_supported_resource_types_filter_takes_only_documented_values() {
    let emulator = Emulator::start();
    emulator
        .ok("CreateCostCategoryDefinition", &create_body("resourceTypes"))
        .await;

    // Catalog source: `ListCostCategoryDefinitionsRequest$SupportedResourceTypes` —
    // "The valid values are billing:rispgroupsharing, billing:billingview, and
    // billing:creditsharing."
    for documented in [
        "billing:rispgroupsharing",
        "billing:billingview",
        "billing:creditsharing",
    ] {
        let (status, body) = emulator
            .call(
                "ListCostCategoryDefinitions",
                &json!({"SupportedResourceTypes": [documented]}),
            )
            .await;
        // What makes a cost category support a resource type is the resource-association
        // surface, which is still a stub, so the filter cannot be decided from state.
        // It is answered as unimplemented, never as a successful empty page.
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {documented:?} (a documented value the emulator cannot filter on): \
             expected 501 NotImplemented, got {status}: {body}"
        );
    }

    for outside in ["billing:notareal_type", "ec2:instance"] {
        let (status, body) = emulator
            .call(
                "ListCostCategoryDefinitions",
                &json!({"SupportedResourceTypes": [outside]}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {outside:?} (outside the documented catalog): expected a 400 \
             ValidationException, got {status}: {body}"
        );
    }

    // No filter means no filtering, so the definition is still reported.
    let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
    assert_eq!(
        listed["CostCategoryReferences"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case \"no filter\": expected the stored definition, got {listed}"
    );
}

/// RULE: a write leaves the Cost Explorer component `PROCESSING` and the clock moves it
/// to `APPLIED` — no caller action moves it, and a caller reading straight after the
/// write still sees the transient status.
#[tokio::test]
async fn processing_status_is_moved_to_applied_by_the_clock() {
    let emulator = Emulator::start();

    let component_status = |described: &Value| -> (String, String) {
        let entry = &described["CostCategory"]["ProcessingStatus"][0];
        (
            entry["Component"].as_str().unwrap_or_default().to_string(),
            entry["Status"].as_str().unwrap_or_default().to_string(),
        )
    };

    let created = emulator
        .ok("CreateCostCategoryDefinition", &create_body("processing"))
        .await;
    let arn = arn_of(&created);
    let describe = json!({"CostCategoryArn": arn});

    let described = emulator
        .ok("DescribeCostCategoryDefinition", &describe)
        .await;
    assert_eq!(
        component_status(&described),
        ("COST_EXPLORER".to_string(), "PROCESSING".to_string()),
        "case \"straight after a create\": expected COST_EXPLORER/PROCESSING, got {described}"
    );

    // The status is compared against the clock when a reader asks, so real time has to
    // pass; the wait is awaited rather than blocking the runtime worker.
    tokio::time::sleep(PROCESSING_SPAN + std::time::Duration::from_millis(100)).await;
    let described = emulator
        .ok("DescribeCostCategoryDefinition", &describe)
        .await;
    assert_eq!(
        component_status(&described),
        ("COST_EXPLORER".to_string(), "APPLIED".to_string()),
        "case \"after the processing span\": expected COST_EXPLORER/APPLIED, got {described}"
    );

    // An update restarts processing; a read does not move it back.
    emulator
        .ok(
            "UpdateCostCategoryDefinition",
            &json!({
                "CostCategoryArn": arn,
                "RuleVersion": RULE_VERSION,
                "Rules": [rule("Engineering"), rule("Research")],
            }),
        )
        .await;
    let described = emulator
        .ok("DescribeCostCategoryDefinition", &describe)
        .await;
    assert_eq!(
        component_status(&described),
        ("COST_EXPLORER".to_string(), "PROCESSING".to_string()),
        "case \"straight after an update\": expected COST_EXPLORER/PROCESSING, got {described}"
    );
}

/// A read must not change what a later caller sees: repeating every reader leaves the
/// registry exactly as it was.
#[tokio::test]
async fn repeated_reads_do_not_change_the_registry() {
    let emulator = Emulator::start();
    for index in 0..3 {
        emulator
            .ok(
                "CreateCostCategoryDefinition",
                &create_body(&format!("idempotent{index}")),
            )
            .await;
    }

    let first = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
    for round in 0..3 {
        let listed = emulator.ok("ListCostCategoryDefinitions", &json!({})).await;
        let arns = |value: &Value| -> Vec<String> {
            value["CostCategoryReferences"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| entry["CostCategoryArn"].to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(
            arns(&listed),
            arns(&first),
            "case \"List round {round}\": a read changed what the next caller sees, got {listed}"
        );
        for arn in arns(&listed) {
            let arn = arn.trim_matches('"').to_string();
            emulator
                .ok(
                    "DescribeCostCategoryDefinition",
                    &json!({"CostCategoryArn": arn}),
                )
                .await;
        }
    }
}

/// RULE: the account's cost category quota bounds how many definitions are IN EFFECT at
/// once; a create past it is refused with the modelled `ServiceQuotaExceededException`
/// (402), and ending one frees a slot again.
///
/// This also pins the registry's bound: a create/delete cycle cannot grow the store
/// without limit, because a tombstone occupies no slot and the create that follows must
/// still fit under the quota.
#[tokio::test]
async fn the_account_quota_bounds_the_definitions_in_effect() {
    let emulator = Emulator::start();

    let mut arns = Vec::new();
    for index in 0..MAX_LIVE_COST_CATEGORIES {
        let created = emulator
            .ok(
                "CreateCostCategoryDefinition",
                &create_body(&format!("quota{index}")),
            )
            .await;
        arns.push(arn_of(&created));
    }

    let (status, body) = emulator
        .call(
            "CreateCostCategoryDefinition",
            &create_body("quotaOneTooMany"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"one definition past the quota of {MAX_LIVE_COST_CATEGORIES}\": expected a 402 \
         ServiceQuotaExceededException, got {status}: {body}"
    );

    // Ending one frees a slot, and only one.
    emulator
        .ok(
            "DeleteCostCategoryDefinition",
            &json!({"CostCategoryArn": arns[0]}),
        )
        .await;
    emulator
        .ok(
            "CreateCostCategoryDefinition",
            &create_body("quotaAfterDelete"),
        )
        .await;
    let (status, body) = emulator
        .call(
            "CreateCostCategoryDefinition",
            &create_body("quotaStillFull"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"back at the quota after one delete and one create\": expected a 402 \
         ServiceQuotaExceededException, got {status}: {body}"
    );

    // And the readers agree with the quota: exactly the quota's worth are in effect.
    let mut in_effect = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({"MaxResults": 100});
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = emulator.ok("ListCostCategoryDefinitions", &body).await;
        in_effect += listed["CostCategoryReferences"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        next_token = listed["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        in_effect, MAX_LIVE_COST_CATEGORIES,
        "case \"at the quota\": expected {MAX_LIVE_COST_CATEGORIES} definitions in effect, \
         got {in_effect}"
    );
}
