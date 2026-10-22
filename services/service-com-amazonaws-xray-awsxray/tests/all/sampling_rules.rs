// SPDX-License-Identifier: CC0-1.0

//! `CreateSamplingRule` -> `GetSamplingRules`, end to end over the wire.
//!
//! Every case drives the SAME front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by the `xray`
//! endpoint host, speaking restJson1. The assertions are about HTTP responses,
//! not about internal state. The `Xray`/`Answer` harness is the `pub(crate)` one
//! the sibling suite `trace_ingest_and_groups.rs` exposes for exactly this, so
//! this suite adds only the sampling-shaped helpers.
//!
//! The rules these tables check:
//!
//! * `CreateSamplingRule` PERSISTS the submitted rule definition under its
//!   `RuleName`, mints the `RuleARN` from the name, stamps `CreatedAt` equal to
//!   `ModifiedAt`, and answers from that stored record — so `GetSamplingRules`
//!   lists exactly what was stored, value for value, including the full
//!   precision of `FixedRate` and the modelled `Attributes` map, which is `{}`
//!   for a rule that matches on no attribute;
//! * `GetSamplingRules` lists every stored record plus the account's
//!   service-managed `Default` rule — whatever order the listing operation
//!   defines, which its own suite pins — and a repeated read answers
//!   identically: reading changes nothing;
//! * a `RuleName` is unique: a second create of a taken name is REFUSED and
//!   leaves the stored record alone, so N concurrent creates of one name have
//!   exactly one winner;
//! * a request the service cannot store — both selectors or neither, an unknown
//!   sampling-rule `Version`, a `ResourceARN` that is neither the wildcard nor
//!   an ARN, or any value outside a modelled `@range`/`@length` — is refused
//!   with HTTP 400 and persists nothing;
//! * a paginated listing emits a `NextToken` only when it was truncated, and
//!   resuming from that token yields the remainder exactly once.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_sampling_rules::SAMPLING_RULES_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::state::MAX_TAGS_PER_RESOURCE;

use crate::trace_ingest_and_groups::{Answer, Xray};

/// `CreateSamplingRule` over the front door.
async fn create_rule(xray: &Xray, body: &Value) -> Answer {
    xray.call("/CreateSamplingRule", body).await
}

/// `GetSamplingRules` over the front door, optionally resuming from `token`.
async fn list_rules(xray: &Xray, token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/GetSamplingRules", &body).await
}

/// The `SamplingRuleRecords` of a listing, or an empty slice.
fn records(answer: &Answer) -> &[Value] {
    answer
        .get("SamplingRuleRecords")
        .as_array()
        .map_or(&[][..], Vec::as_slice)
}

/// One row of the create/list table: a sampling rule definition.
#[derive(Debug, Clone)]
struct Case {
    case: &'static str,
    name: String,
    priority: i64,
    fixed_rate: f64,
    reservoir_size: i64,
    resource_arn: &'static str,
    service_name: &'static str,
    service_type: &'static str,
    host: &'static str,
    http_method: &'static str,
    url_path: &'static str,
    /// At most five entries: `AttributeMap` is modelled `@length(min: 0, max: 5)`.
    attributes: Vec<(&'static str, &'static str)>,
    /// `SamplingRateBoost` as `(MaxRate, CooldownWindowMinutes)`.
    boost: Option<(f64, i64)>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    /// The `CreateSamplingRule` request body for this row.
    fn body(&self) -> Value {
        let mut rule = json!({
            "RuleName": self.name,
            "ResourceARN": self.resource_arn,
            "Priority": self.priority,
            "FixedRate": self.fixed_rate,
            "ReservoirSize": self.reservoir_size,
            "ServiceName": self.service_name,
            "ServiceType": self.service_type,
            "Host": self.host,
            "HTTPMethod": self.http_method,
            "URLPath": self.url_path,
            // The model documents the sampling rule format as `1`.
            "Version": 1,
        });
        if !self.attributes.is_empty() {
            rule["Attributes"] = Value::Object(
                self.attributes
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        if let Some((max_rate, cooldown)) = self.boost {
            rule["SamplingRateBoost"] =
                json!({"MaxRate": max_rate, "CooldownWindowMinutes": cooldown});
        }
        let mut body = json!({ "SamplingRule": rule });
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        body
    }

    /// The ARN this rule's name mints:
    /// `arn:aws:xray:<region>:<account>:sampling-rule/<RuleName>`.
    fn expected_arn(&self) -> String {
        format!(
            "arn:aws:xray:us-east-1:000000000000:sampling-rule/{}",
            self.name
        )
    }
}

/// The table: the rows differ along the matcher values, the priority (both
/// modelled bounds and a tie), the precision of `FixedRate`, the reservoir
/// size, the number of attributes (none, one, the modelled maximum of five),
/// whether a `SamplingRateBoost` is carried, the tag count, and whether the
/// resource is the wildcard or a concrete ARN.
fn rule_cases() -> Vec<Case> {
    vec![
        Case {
            case: "every matcher wildcarded, at the modelled minimum priority/rate/reservoir",
            name: "wildcard-floor".to_string(),
            priority: 1,
            fixed_rate: 0.0,
            reservoir_size: 0,
            resource_arn: "*",
            service_name: "*",
            service_type: "*",
            host: "*",
            http_method: "*",
            url_path: "*",
            attributes: Vec::new(),
            boost: None,
            tags: Vec::new(),
        },
        Case {
            case: "the modelled maximum priority and fixed rate",
            name: "ceiling-rule".to_string(),
            priority: 9999,
            fixed_rate: 1.0,
            reservoir_size: 10,
            resource_arn: "*",
            service_name: "checkout",
            service_type: "AWS::EC2::Instance",
            host: "*",
            http_method: "GET",
            url_path: "/health",
            attributes: vec![("tier", "gold")],
            boost: None,
            tags: Vec::new(),
        },
        Case {
            case: "a fixed rate carrying fifteen significant digits",
            name: "precise-rate".to_string(),
            priority: 5000,
            fixed_rate: 0.123_456_789_012_345,
            reservoir_size: 2500,
            resource_arn: "*",
            service_name: "*",
            service_type: "*",
            host: "api.example.test",
            http_method: "POST",
            url_path: "/orders/*",
            attributes: Vec::new(),
            boost: Some((0.75, 5)),
            tags: vec![("env", "prod")],
        },
        Case {
            case: "a concrete ResourceARN, tie-break priority, two tags",
            name: "lambda-checkout".to_string(),
            priority: 42,
            fixed_rate: 0.05,
            reservoir_size: 1,
            resource_arn: "arn:aws:lambda:us-east-1:000000000000:function:checkout",
            service_name: "checkout",
            service_type: "AWS::Lambda::Function",
            host: "checkout.example.test",
            http_method: "PUT",
            url_path: "/v1/cart",
            attributes: vec![("channel", "mobile"), ("region-tier", "a")],
            boost: None,
            tags: vec![("env", "prod"), ("team", "payments")],
        },
        Case {
            case: "the attribute map at its modelled maximum of five, with a rate boost",
            name: "a".repeat(32),
            priority: 42,
            fixed_rate: 0.5,
            reservoir_size: 60,
            resource_arn: "*",
            service_name: "*",
            service_type: "*",
            host: "*",
            http_method: "*",
            url_path: "*",
            attributes: vec![
                ("one", "1"),
                ("two", "2"),
                ("three", "3"),
                ("four", "4"),
                ("five", "5"),
            ],
            boost: Some((1.0, 120)),
            tags: Vec::new(),
        },
    ]
}

/// The `SamplingRule` member of a record, as a map of its members.
fn rule_of(record: &Value) -> &Value {
    record.get("SamplingRule").unwrap_or(&Value::Null)
}

/// A record's `Attributes`, flattened for comparison.
fn attributes_of(rule: &Value) -> Option<BTreeMap<String, String>> {
    rule.get("Attributes")?.as_object().map(|map| {
        map.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect()
    })
}

/// Assert that `record` is exactly what `case` asked to be stored.
fn assert_record_matches(label: &str, where_from: &str, record: &Value, case: &Case) {
    let rule = rule_of(record);
    assert_eq!(
        rule.get("RuleName").and_then(Value::as_str),
        Some(case.name.as_str()),
        "case {label:?} ({where_from}): expected RuleName {:?}, got {rule:?}",
        case.name
    );
    assert_eq!(
        rule.get("RuleARN").and_then(Value::as_str),
        Some(case.expected_arn().as_str()),
        "case {label:?} ({where_from}): expected RuleARN {:?}, got {rule:?}",
        case.expected_arn()
    );
    for (member, expected) in [
        ("ResourceARN", case.resource_arn),
        ("ServiceName", case.service_name),
        ("ServiceType", case.service_type),
        ("Host", case.host),
        ("HTTPMethod", case.http_method),
        ("URLPath", case.url_path),
    ] {
        let actual = rule.get(member).and_then(Value::as_str);
        assert_eq!(
            actual,
            Some(expected),
            "case {label:?} ({where_from}): expected {member} {expected:?}, got {actual:?}"
        );
    }
    for (member, expected) in [
        ("Priority", case.priority),
        ("ReservoirSize", case.reservoir_size),
        ("Version", 1),
    ] {
        let actual = rule.get(member).and_then(Value::as_i64);
        assert_eq!(
            actual,
            Some(expected),
            "case {label:?} ({where_from}): expected {member} {expected}, got {actual:?}"
        );
    }
    let fixed_rate = rule.get("FixedRate").and_then(Value::as_f64);
    assert_eq!(
        fixed_rate,
        Some(case.fixed_rate),
        "case {label:?} ({where_from}): FixedRate must round-trip exactly, expected {}, got {fixed_rate:?}",
        case.fixed_rate
    );

    let expected_attributes: Option<BTreeMap<String, String>> = Some(
        case.attributes
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    );
    let actual_attributes = attributes_of(rule);
    assert_eq!(
        actual_attributes, expected_attributes,
        "case {label:?} ({where_from}): expected Attributes {expected_attributes:?}, got {actual_attributes:?}"
    );

    match case.boost {
        Some((max_rate, cooldown)) => {
            let boost = rule.get("SamplingRateBoost").unwrap_or(&Value::Null);
            assert_eq!(
                boost.get("MaxRate").and_then(Value::as_f64),
                Some(max_rate),
                "case {label:?} ({where_from}): expected MaxRate {max_rate}, got {boost:?}"
            );
            assert_eq!(
                boost.get("CooldownWindowMinutes").and_then(Value::as_i64),
                Some(cooldown),
                "case {label:?} ({where_from}): expected CooldownWindowMinutes {cooldown}, got {boost:?}"
            );
        }
        None => assert!(
            rule.get("SamplingRateBoost").is_none_or(Value::is_null),
            "case {label:?} ({where_from}): no SamplingRateBoost was submitted, got {:?}",
            rule.get("SamplingRateBoost")
        ),
    }

    let created = record.get("CreatedAt").and_then(Value::as_f64);
    let modified = record.get("ModifiedAt").and_then(Value::as_f64);
    assert!(
        created.is_some_and(|at| at > 0.0),
        "case {label:?} ({where_from}): CreatedAt must be stamped, got {created:?}"
    );
    assert_eq!(
        created, modified,
        "case {label:?} ({where_from}): an unmodified rule has ModifiedAt == CreatedAt, got {created:?} / {modified:?}"
    );
}

#[tokio::test]
async fn a_created_sampling_rule_is_answered_and_listed_from_its_stored_record() {
    let xray = Xray::new();
    let cases = rule_cases();

    for case in &cases {
        let created = create_rule(&xray, &case.body()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateSamplingRule must succeed, got {created:?}",
            case.case
        );
        let record = created.get("SamplingRuleRecord").clone();
        assert_record_matches(case.case, "CreateSamplingRule", &record, case);
    }

    // The listing is recomputed from the store. Its ORDER is the listing
    // operation's own contract (rule-name order, pinned by the suite that owns
    // `GetSamplingRules`); what this suite asserts is that every stored rule is
    // there, value for value, whatever order it arrives in.
    let mut expected_order: Vec<&Case> = cases.iter().collect();
    expected_order.sort_by(|left, right| left.name.cmp(&right.name));

    let listed = list_rules(&xray, None).await;
    assert_eq!(
        listed.status, 200,
        "GetSamplingRules must succeed, got {listed:?}"
    );
    // The created rules plus the account's service-managed `Default` rule, which
    // takes the listing one record past a page here.
    let mut listed_records: Vec<Value> = records(&listed).to_vec();
    let mut token = listed.get("NextToken").as_str().map(str::to_string);
    while let Some(next) = token {
        let page = list_rules(&xray, Some(&next)).await;
        assert_eq!(page.status, 200, "a resumed page must succeed, got {page:?}");
        listed_records.extend(records(&page).iter().cloned());
        token = page.get("NextToken").as_str().map(str::to_string);
    }
    assert_eq!(
        listed_records.len(),
        cases.len() + 1,
        "every created rule must be listed, beside the service-managed Default rule, got {listed_records:?}"
    );
    let mut listed_names: Vec<&str> = listed_records
        .iter()
        .filter_map(|record| rule_of(record).get("RuleName").and_then(Value::as_str))
        .collect();
    listed_names.sort_unstable();
    let mut expected_names: Vec<&str> =
        expected_order.iter().map(|case| case.name.as_str()).collect();
    expected_names.push("Default");
    expected_names.sort_unstable();
    assert_eq!(
        listed_names, expected_names,
        "GetSamplingRules must list exactly the created rules and the Default rule, got {listed_names:?}"
    );
    for case in &expected_order {
        let record = listed_records
            .iter()
            .find(|record| {
                rule_of(record).get("RuleName").and_then(Value::as_str) == Some(case.name.as_str())
            })
            .unwrap_or_else(|| panic!("case {:?}: must be listed, got {listed_records:?}", case.case));
        assert_record_matches(case.case, "GetSamplingRules", record, case);
    }

    // Reading changes nothing a later caller sees: the second listing is byte
    // for byte the first one, timestamps included.
    let again = list_rules(&xray, None).await;
    assert_eq!(
        again.body, listed.body,
        "a repeated GetSamplingRules must answer identically, got {:?} then {:?}",
        listed.body, again.body
    );

    // A name is unique, and the loser of a collision changes nothing.
    for case in &cases {
        let duplicate = create_rule(&xray, &case.body()).await;
        assert_eq!(
            (duplicate.status, duplicate.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {:?}: a second create of a taken RuleName must be refused, got {duplicate:?}",
            case.case
        );
    }
    let after_collisions = list_rules(&xray, None).await;
    assert_eq!(
        after_collisions.body, listed.body,
        "a refused duplicate must leave the stored records alone, got {:?}",
        after_collisions.body
    );
}

#[tokio::test]
async fn a_sampling_rule_the_service_cannot_store_is_refused_and_persists_nothing() {
    let xray = Xray::new();
    // One rule exists, so the duplicate-name row has something to collide with
    // and the "persists nothing" check has a known baseline.
    let seed = &rule_cases()[0];
    let seeded = create_rule(&xray, &seed.body()).await;
    assert_eq!(seeded.status, 200, "seeding must succeed, got {seeded:?}");
    let baseline = list_rules(&xray, None).await;

    /// A valid rule definition, as a starting point each row perturbs.
    fn valid_rule() -> Value {
        json!({
            "RuleName": "refusal-probe",
            "ResourceARN": "*",
            "Priority": 100,
            "FixedRate": 0.1,
            "ReservoirSize": 5,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            "Version": 1,
        })
    }

    /// `valid_rule()` with one member replaced (or removed, for `Value::Null`).
    fn rule_with(member: &str, value: Value) -> Value {
        let mut rule = valid_rule();
        if value.is_null() {
            rule.as_object_mut().expect("the rule is an object").remove(member);
        } else {
            rule[member] = value;
        }
        json!({ "SamplingRule": rule })
    }

    /// A valid rule definition carrying `tags` (a `TagList`).
    fn rule_with_tags(tags: Value) -> Value {
        json!({ "SamplingRule": valid_rule(), "Tags": tags })
    }

    let rows: Vec<(&str, Value)> = vec![
        (
            "both selectors specified",
            rule_with(
                "RuleARN",
                json!("arn:aws:xray:us-east-1:000000000000:sampling-rule/refusal-probe"),
            ),
        ),
        (
            "neither selector specified",
            rule_with("RuleName", Value::Null),
        ),
        (
            "RuleName shorter than the modelled minimum",
            rule_with("RuleName", json!("")),
        ),
        (
            "RuleName longer than the modelled maximum",
            rule_with("RuleName", json!("a".repeat(33))),
        ),
        (
            "RuleName already in use",
            rule_with("RuleName", json!(seed.name)),
        ),
        (
            "a sampling rule Version this service does not know",
            rule_with("Version", json!(2)),
        ),
        (
            "Version below the modelled minimum",
            rule_with("Version", json!(0)),
        ),
        (
            "Priority below the modelled minimum",
            rule_with("Priority", json!(0)),
        ),
        (
            "Priority above the modelled maximum",
            rule_with("Priority", json!(10_000)),
        ),
        (
            "FixedRate above the modelled maximum",
            rule_with("FixedRate", json!(1.5)),
        ),
        (
            "FixedRate below the modelled minimum",
            rule_with("FixedRate", json!(-0.5)),
        ),
        (
            "ReservoirSize below the modelled minimum",
            rule_with("ReservoirSize", json!(-1)),
        ),
        (
            "a ResourceARN that is neither the wildcard nor an ARN",
            rule_with("ResourceARN", json!("checkout-function")),
        ),
        (
            "an empty ResourceARN",
            rule_with("ResourceARN", json!("")),
        ),
        (
            "more Attributes than the modelled maximum of five",
            rule_with(
                "Attributes",
                json!({"a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6"}),
            ),
        ),
        (
            "an Attributes value longer than the modelled maximum",
            rule_with("Attributes", json!({"a": "v".repeat(33)})),
        ),
        (
            "a tag key carrying the reserved `aws:` prefix",
            rule_with_tags(json!([{"Key": "aws:owner", "Value": "sre"}])),
        ),
        (
            "a tag key outside the documented character set",
            rule_with_tags(json!([{"Key": "owner*", "Value": "sre"}])),
        ),
        (
            "a tag value outside the documented character set",
            rule_with_tags(json!([{"Key": "owner", "Value": "sre$"}])),
        ),
        (
            "one tag past the documented maximum per resource",
            rule_with_tags(Value::Array(
                (0..=MAX_TAGS_PER_RESOURCE)
                    .map(|index| json!({"Key": format!("key-{index:03}"), "Value": "v"}))
                    .collect(),
            )),
        ),
        ("SamplingRule absent", json!({})),
        (
            "a required matcher absent",
            rule_with("URLPath", Value::Null),
        ),
    ];

    for (label, body) in rows {
        let refused = create_rule(&xray, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected HTTP 400, got {refused:?}"
        );
        // A framework refusal renders `message`; a modelled error shape renders
        // its member's own casing (`Message`).
        let message = refused
            .get("message")
            .as_str()
            .or_else(|| refused.get("Message").as_str());
        assert!(
            message.is_some_and(|message| !message.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
    }

    let after = list_rules(&xray, None).await;
    assert_eq!(
        after.body, baseline.body,
        "a refused CreateSamplingRule must store nothing, got {:?}",
        after.body
    );

    // And the cap itself is accepted: exactly MAX_TAGS_PER_RESOURCE tags store.
    let at_cap = create_rule(
        &xray,
        &json!({
            "SamplingRule": {
                "RuleName": "tags-at-cap",
                "ResourceARN": "*",
                "Priority": 101,
                "FixedRate": 0.1,
                "ReservoirSize": 5,
                "ServiceName": "*",
                "ServiceType": "*",
                "Host": "*",
                "HTTPMethod": "*",
                "URLPath": "*",
                "Version": 1,
            },
            "Tags": Value::Array(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|index| json!({"Key": format!("key-{index:03}"), "Value": "v"}))
                    .collect(),
            ),
        }),
    )
    .await;
    assert_eq!(
        at_cap.status, 200,
        "exactly {MAX_TAGS_PER_RESOURCE} tags are within the cap, got {at_cap:?}"
    );

    // A NextToken this service never issued is refused too, rather than
    // answered with an arbitrary page.
    let bogus = list_rules(&xray, Some("zzz-never-issued")).await;
    assert_eq!(
        (bogus.status, bogus.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "a NextToken past the listed keyspace must be refused, got {bogus:?}"
    );
}

#[tokio::test]
async fn a_next_token_appears_only_when_truncated_and_resumes_exactly_once() {
    // Counts either side of one page: short of it, exactly it, past it.
    for count in [1, SAMPLING_RULES_PAGE_SIZE, SAMPLING_RULES_PAGE_SIZE + 2] {
        let xray = Xray::new();
        let mut expected = Vec::new();
        for index in 0..count {
            let name = format!("rule-{index:04}");
            // Priority ascends with the index, and so does the name, so the
            // listing order is the creation order whichever of the two the
            // listing is ordered by.
            let priority = i64::try_from(index).expect("the index fits an i64") + 1;
            let body = json!({
                "SamplingRule": {
                    "RuleName": name,
                    "ResourceARN": "*",
                    "Priority": priority,
                    "FixedRate": 0.25,
                    "ReservoirSize": 1,
                    "ServiceName": "*",
                    "ServiceType": "*",
                    "Host": "*",
                    "HTTPMethod": "*",
                    "URLPath": "*",
                    "Version": 1,
                },
            });
            let created = create_rule(&xray, &body).await;
            assert_eq!(
                created.status, 200,
                "count {count}: creating {name} must succeed, got {created:?}"
            );
            expected.push(name);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = list_rules(&xray, token.as_deref()).await;
            assert_eq!(
                page.status, 200,
                "count {count}: GetSamplingRules must succeed, got {page:?}"
            );
            let names: Vec<String> = records(&page)
                .iter()
                .filter_map(|record| rule_of(record).get("RuleName").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            assert!(
                names.len() <= SAMPLING_RULES_PAGE_SIZE,
                "count {count}: a page holds at most {SAMPLING_RULES_PAGE_SIZE}, got {names:?}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .as_str()
                .map(str::to_string);
            pages += 1;
            assert!(pages <= 10, "count {count}: pagination must terminate");
            if token.is_none() {
                break;
            }
        }

        // The created rules, in name order, then the service-managed `Default`
        // rule the listing always carries.
        expected.push("Default".to_string());
        assert_eq!(
            seen, expected,
            "count {count}: pagination must yield every rule exactly once"
        );
        // The listing carries the Default rule too, so it is one record longer
        // than the number of rules created.
        let truncated = count + 1 > SAMPLING_RULES_PAGE_SIZE;
        assert_eq!(
            pages > 1,
            truncated,
            "count {count}: a NextToken must appear only when the listing was truncated (pages={pages})"
        );
    }
}
