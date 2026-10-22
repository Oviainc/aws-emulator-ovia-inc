// SPDX-License-Identifier: CC0-1.0

//! The account's service-managed `Default` sampling rule, the modelled
//! `Attributes` map, and `UpdateSamplingRule`'s write path, end to end over the
//! wire.
//!
//! Every case drives the SAME front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by an `xray`
//! endpoint host, speaking restJson1. The `Xray`/`Answer` harness is the
//! `pub(crate)` one `trace_ingest_and_groups.rs` exposes, so this suite adds
//! only the sampling-shaped helpers; `sampling_rules.rs` owns create/list and
//! `sampling_rule_deletion.rs` owns delete.
//!
//! The rules these tables check:
//!
//! * every account holds ONE service-managed catch-all sampling rule it did not
//!   create, named `Default`: `GetSamplingRules` reports it on an account with no
//!   rules of its own and beside any number of created rules, in every region,
//!   with `sampling-rule/Default` as its whole ARN resource (no generated id),
//!   every matcher wildcarded, and the fixed configuration AWS seeds it with;
//!   reading it changes nothing, no delete removes it, and its name cannot be
//!   taken by a created rule;
//! * a serialized `SamplingRule` always carries `Attributes`, as `{}` when the
//!   rule matches on no attribute — `AttributeMap` is modelled
//!   `@length(min: 0, max: 5)`, so the empty map is a VALUE of the member, not an
//!   absent member — on every operation that answers a `SamplingRuleRecord`;
//! * `UpdateSamplingRule` writes each member the update supplies through to the
//!   STORED rule and leaves each member it omits alone, so every later reader of
//!   that rule reports the updated values with a bumped `ModifiedAt` and the
//!   `CreatedAt` it was stored with; a value outside the bounds the modelled
//!   `SamplingRule` carries, or a selector the model forbids, is refused with
//!   HTTP 400 and writes nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::trace_ingest_and_groups::{Answer, Xray, DEFAULT_REGION};

/// The rule every account holds without creating it.
const DEFAULT_RULE_NAME: &str = "Default";

/// A complete, valid `SamplingRule` definition a case only perturbs.
fn base_rule(name: &str) -> Value {
    json!({
        "RuleName": name,
        "ResourceARN": "*",
        "Priority": 1000,
        "FixedRate": 0.05,
        "ReservoirSize": 2,
        "ServiceName": "*",
        "ServiceType": "*",
        "Host": "*",
        "HTTPMethod": "*",
        "URLPath": "*",
        "Version": 1
    })
}

/// `arn:aws:xray:<region>:<account>:sampling-rule/<RuleName>` — a sampling
/// rule's whole ARN, generated id suffix included: there is none.
fn rule_arn(region: &str, rule_name: &str) -> String {
    format!(
        "arn:aws:xray:{region}:{}:sampling-rule/{rule_name}",
        ::emulator_server::emulator_account_id()
    )
}

/// `CreateSamplingRule` in `region`.
async fn create_in(xray: &Xray, region: &str, rule: &Value) -> Answer {
    xray.call_in_region(region, "/CreateSamplingRule", &json!({ "SamplingRule": rule }))
        .await
}

/// `UpdateSamplingRule` in `region`.
async fn update_in(xray: &Xray, region: &str, update: &Value) -> Answer {
    xray.call_in_region(
        region,
        "/UpdateSamplingRule",
        &json!({ "SamplingRuleUpdate": update }),
    )
    .await
}

/// `GetSamplingRules` in `region`, resuming from `token`.
async fn list_in(xray: &Xray, region: &str, token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call_in_region(region, "/GetSamplingRules", &body).await
}

/// `DeleteSamplingRule` in `region`.
async fn delete_in(xray: &Xray, region: &str, body: &Value) -> Answer {
    xray.call_in_region(region, "/DeleteSamplingRule", body).await
}

/// Every record one listing in `region` carries, across as many pages as it
/// takes, in listing order.
async fn all_records(xray: &Xray, region: &str, case: &str) -> Vec<Value> {
    let mut records: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..32 {
        let answer = list_in(xray, region, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: listing page {page} must answer 200, got {answer:?}"
        );
        records.extend(
            answer
                .get("SamplingRuleRecords")
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        match answer.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return records,
        }
    }
    panic!("case {case:?}: the listing never terminated: {records:?}");
}

/// The `RuleName` of one `SamplingRuleRecord`.
fn record_name(record: &Value) -> &str {
    record
        .get("SamplingRule")
        .and_then(|rule| rule.get("RuleName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The record the listing in `region` carries for `rule_name`.
async fn listed_record(xray: &Xray, region: &str, case: &str, rule_name: &str) -> Value {
    let records = all_records(xray, region, case).await;
    records
        .iter()
        .find(|record| record_name(record) == rule_name)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {rule_name:?} must be listed, got {records:?}")
        })
}

/// What one `SamplingRule` is expected to say, member for member.
#[derive(Debug, Clone, PartialEq)]
struct ExpectedRule {
    name: String,
    arn: String,
    resource_arn: String,
    priority: i64,
    fixed_rate: f64,
    reservoir_size: i64,
    service_name: String,
    service_type: String,
    host: String,
    http_method: String,
    url_path: String,
    version: i64,
    /// The modelled `AttributeMap`: `{}` for a rule matching on no attribute.
    attributes: BTreeMap<String, String>,
    /// `SamplingRateBoost` as `(MaxRate, CooldownWindowMinutes)`.
    boost: Option<(f64, i64)>,
}

impl ExpectedRule {
    /// The rule [`base_rule`] creates, as it is expected to be read back.
    fn created(region: &str, name: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: rule_arn(region, name),
            resource_arn: "*".to_string(),
            priority: 1000,
            fixed_rate: 0.05,
            reservoir_size: 2,
            service_name: "*".to_string(),
            service_type: "*".to_string(),
            host: "*".to_string(),
            http_method: "*".to_string(),
            url_path: "*".to_string(),
            version: 1,
            attributes: BTreeMap::new(),
            boost: None,
        }
    }

    /// The account's service-managed `Default` rule as a caller in `region`
    /// reads it.
    ///
    /// The values are the ones AWS seeds every account's catch-all with, as the
    /// recorded `GetSamplingRules` responses state them: priority 10000 (past
    /// the `@range(min: 1, max: 9999)` a created rule may carry), a 5% fixed
    /// rate, a reservoir of one, every matcher the wildcard, rule format
    /// version 1, and no attributes.
    fn service_managed(region: &str) -> Self {
        Self {
            name: DEFAULT_RULE_NAME.to_string(),
            arn: rule_arn(region, DEFAULT_RULE_NAME),
            resource_arn: "*".to_string(),
            priority: 10_000,
            fixed_rate: 0.05,
            reservoir_size: 1,
            service_name: "*".to_string(),
            service_type: "*".to_string(),
            host: "*".to_string(),
            http_method: "*".to_string(),
            url_path: "*".to_string(),
            version: 1,
            attributes: BTreeMap::new(),
            boost: None,
        }
    }

    /// This rule with every member the `SamplingRuleUpdate` `update` carries
    /// replaced — the expectation the update states, read straight off the
    /// request rather than off the answer.
    fn with_update(&self, update: &Value) -> Self {
        let mut expected = self.clone();
        let string = |member: &str| update.get(member).and_then(Value::as_str).map(str::to_string);
        if let Some(value) = string("ResourceARN") {
            expected.resource_arn = value;
        }
        if let Some(value) = update.get("Priority").and_then(Value::as_i64) {
            expected.priority = value;
        }
        if let Some(value) = update.get("FixedRate").and_then(Value::as_f64) {
            expected.fixed_rate = value;
        }
        if let Some(value) = update.get("ReservoirSize").and_then(Value::as_i64) {
            expected.reservoir_size = value;
        }
        if let Some(value) = string("ServiceName") {
            expected.service_name = value;
        }
        if let Some(value) = string("ServiceType") {
            expected.service_type = value;
        }
        if let Some(value) = string("Host") {
            expected.host = value;
        }
        if let Some(value) = string("HTTPMethod") {
            expected.http_method = value;
        }
        if let Some(value) = string("URLPath") {
            expected.url_path = value;
        }
        if let Some(map) = update.get("Attributes").and_then(Value::as_object) {
            expected.attributes = map
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
        }
        if let Some(boost) = update.get("SamplingRateBoost") {
            expected.boost = Some((
                boost.get("MaxRate").and_then(Value::as_f64).unwrap_or(0.0),
                boost
                    .get("CooldownWindowMinutes")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            ));
        }
        expected
    }
}

/// Assert that the `SamplingRule` inside `record` says exactly what `expected`
/// does, `Attributes` included.
fn assert_rule(label: &str, where_from: &str, record: &Value, expected: &ExpectedRule) {
    let rule = record.get("SamplingRule").unwrap_or(&Value::Null);
    for (member, want) in [
        ("RuleName", expected.name.as_str()),
        ("RuleARN", expected.arn.as_str()),
        ("ResourceARN", expected.resource_arn.as_str()),
        ("ServiceName", expected.service_name.as_str()),
        ("ServiceType", expected.service_type.as_str()),
        ("Host", expected.host.as_str()),
        ("HTTPMethod", expected.http_method.as_str()),
        ("URLPath", expected.url_path.as_str()),
    ] {
        let actual = rule.get(member).and_then(Value::as_str);
        assert_eq!(
            actual,
            Some(want),
            "case {label:?} ({where_from}): expected {member} {want:?}, got {actual:?}"
        );
    }
    for (member, want) in [
        ("Priority", expected.priority),
        ("ReservoirSize", expected.reservoir_size),
        ("Version", expected.version),
    ] {
        let actual = rule.get(member).and_then(Value::as_i64);
        assert_eq!(
            actual,
            Some(want),
            "case {label:?} ({where_from}): expected {member} {want}, got {actual:?}"
        );
    }
    let fixed_rate = rule.get("FixedRate").and_then(Value::as_f64);
    assert_eq!(
        fixed_rate,
        Some(expected.fixed_rate),
        "case {label:?} ({where_from}): expected FixedRate {}, got {fixed_rate:?}",
        expected.fixed_rate
    );

    // `Attributes` is always PRESENT, as the modelled map — `{}` for a rule
    // matching on no attribute.
    let attributes = rule.get("Attributes");
    let actual: Option<BTreeMap<String, String>> = attributes.and_then(Value::as_object).map(|map| {
        map.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect()
    });
    assert_eq!(
        actual.as_ref(),
        Some(&expected.attributes),
        "case {label:?} ({where_from}): expected Attributes {:?}, got {attributes:?}",
        expected.attributes
    );

    match expected.boost {
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
            "case {label:?} ({where_from}): no SamplingRateBoost is configured, got {:?}",
            rule.get("SamplingRateBoost")
        ),
    }
}

#[tokio::test]
async fn the_service_managed_default_rule_is_listed_on_every_account_and_region() {
    // The rows are the regional endpoints an account is read through, each on
    // its own emulator so the listing is read on an account with no rule of its
    // own first.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let xray = Xray::new();
        let expected = ExpectedRule::service_managed(region);

        let fresh = all_records(&xray, region, region).await;
        assert_eq!(
            fresh.len(),
            1,
            "case {region:?}: an account with no rule of its own still holds the Default rule, got {fresh:?}"
        );
        assert_rule(region, "fresh account", &fresh[0], &expected);
        let created_at = fresh[0].get("CreatedAt").and_then(Value::as_f64);
        let modified_at = fresh[0].get("ModifiedAt").and_then(Value::as_f64);
        assert_eq!(
            (created_at, modified_at),
            (Some(0.0), Some(0.0)),
            "case {region:?}: nothing created or modified the Default rule, got {created_at:?} / {modified_at:?}"
        );

        // Reading it changes nothing a later caller sees.
        let again = all_records(&xray, region, region).await;
        assert_eq!(
            again, fresh,
            "case {region:?}: a repeated listing must answer identically, got {again:?}"
        );

        // And it stays, and stays LAST, beside rules whose names sort either
        // side of it — one past a page, so the pagination carries it too.
        let names = [
            "0-first",
            "Alpha",
            "Charlie",
            "beta",
            "zeta",
            "another-rule",
        ];
        for name in names {
            let created = create_in(&xray, region, &base_rule(name)).await;
            assert_eq!(
                created.status, 200,
                "case {region:?}: creating {name:?} must succeed, got {created:?}"
            );
            // Every created record carries the modelled attribute map too.
            assert_rule(
                region,
                "CreateSamplingRule",
                created.get("SamplingRuleRecord"),
                &ExpectedRule::created(region, name),
            );
        }

        let listed = all_records(&xray, region, region).await;
        let listed_names: Vec<&str> = listed.iter().map(record_name).collect();
        let mut expected_names: Vec<&str> = names.to_vec();
        expected_names.sort_unstable();
        expected_names.push(DEFAULT_RULE_NAME);
        assert_eq!(
            listed_names, expected_names,
            "case {region:?}: the Default rule is listed after the created rules, got {listed_names:?}"
        );
        assert_rule(
            region,
            "beside created rules",
            listed.last().expect("the listing is not empty"),
            &expected,
        );

        // Its name is not the caller's to take, and no delete removes it —
        // by name or by the ARN it is read with.
        let taken = create_in(&xray, region, &base_rule(DEFAULT_RULE_NAME)).await;
        assert_eq!(
            (taken.status, taken.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {region:?}: the Default rule's name is already in use, got {taken:?}"
        );
        for (selector, body) in [
            ("by name", json!({ "RuleName": DEFAULT_RULE_NAME })),
            (
                "by ARN",
                json!({ "RuleARN": rule_arn(region, DEFAULT_RULE_NAME) }),
            ),
        ] {
            let refused = delete_in(&xray, region, &body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {region:?} ({selector}): deleting the Default rule must be refused, got {refused:?}"
            );
        }
        let after = listed_record(&xray, region, region, DEFAULT_RULE_NAME).await;
        assert_rule(region, "after a refused delete", &after, &expected);
    }
}

/// One row of the update table: what the update carries, and what the rule said
/// when it was created.
#[derive(Debug, Clone)]
struct UpdateCase {
    case: &'static str,
    /// The `Attributes` the rule is CREATED with, when it carries any.
    created_attributes: Option<Value>,
    /// The `SamplingRuleUpdate` members under test, without the selector.
    update: Value,
}

/// The table: the rows differ along WHICH members an update carries (one, a
/// pair, every matcher at once, none at all), the modelled bounds of the numeric
/// members, whether the attribute map grows, shrinks or is emptied, and whether
/// a `SamplingRateBoost` is configured.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the fixed rate and the priority together",
            created_attributes: None,
            update: json!({ "FixedRate": 0.5, "Priority": 500 }),
        },
        UpdateCase {
            case: "the fixed rate at the modelled floor",
            created_attributes: None,
            update: json!({ "FixedRate": 0.0 }),
        },
        UpdateCase {
            case: "the fixed rate at the modelled ceiling",
            created_attributes: None,
            update: json!({ "FixedRate": 1.0 }),
        },
        UpdateCase {
            case: "a fixed rate carrying fifteen significant digits",
            created_attributes: None,
            update: json!({ "FixedRate": 0.123_456_789_012_345 }),
        },
        UpdateCase {
            case: "the priority at the modelled floor",
            created_attributes: None,
            update: json!({ "Priority": 1 }),
        },
        UpdateCase {
            case: "the priority at the modelled ceiling",
            created_attributes: None,
            update: json!({ "Priority": 9999 }),
        },
        UpdateCase {
            case: "the reservoir at the modelled floor",
            created_attributes: None,
            update: json!({ "ReservoirSize": 0 }),
        },
        UpdateCase {
            case: "a larger reservoir",
            created_attributes: None,
            update: json!({ "ReservoirSize": 2500 }),
        },
        UpdateCase {
            case: "every matcher at once",
            created_attributes: None,
            update: json!({
                "ServiceName": "checkout",
                "ServiceType": "AWS::Lambda::Function",
                "Host": "api.example.test",
                "HTTPMethod": "POST",
                "URLPath": "/v1/orders"
            }),
        },
        UpdateCase {
            case: "a concrete ResourceARN in place of the wildcard",
            created_attributes: None,
            update: json!({
                "ResourceARN": "arn:aws:lambda:us-east-1:000000000000:function:checkout"
            }),
        },
        UpdateCase {
            case: "the attribute map at its modelled maximum of five",
            created_attributes: None,
            update: json!({
                "Attributes": {"one": "1", "two": "2", "three": "3", "four": "4", "five": "5"}
            }),
        },
        UpdateCase {
            case: "an attribute map replacing the one the rule was created with",
            created_attributes: Some(json!({"channel": "mobile", "tier": "gold"})),
            update: json!({ "Attributes": {"channel": "web"} }),
        },
        UpdateCase {
            case: "an empty attribute map on a rule that matched on attributes",
            created_attributes: Some(json!({"channel": "mobile"})),
            update: json!({ "Attributes": {} }),
        },
        UpdateCase {
            case: "a sampling rate boost",
            created_attributes: None,
            update: json!({
                "SamplingRateBoost": {"MaxRate": 0.9, "CooldownWindowMinutes": 30}
            }),
        },
        UpdateCase {
            case: "every changeable member at once",
            created_attributes: Some(json!({"channel": "mobile"})),
            update: json!({
                "ResourceARN": "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
                "Priority": 77,
                "FixedRate": 0.25,
                "ReservoirSize": 7,
                "ServiceName": "orders",
                "ServiceType": "AWS::EC2::Instance",
                "Host": "orders.example.test",
                "HTTPMethod": "PUT",
                "URLPath": "/v2/*",
                "Attributes": {"channel": "web", "tier": "silver"},
                "SamplingRateBoost": {"MaxRate": 1.0, "CooldownWindowMinutes": 0}
            }),
        },
        UpdateCase {
            case: "an update carrying no changeable member at all",
            created_attributes: None,
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_is_written_through_to_the_store_and_read_by_every_reader() {
    let xray = Xray::new();
    let region = DEFAULT_REGION;

    // Each row is run through BOTH selectors the model documents, on a rule of
    // its own, so a write can never be attributed to the other row's rule.
    for (index, case) in update_cases().iter().enumerate() {
        for (selector, by_arn) in [("RuleName", false), ("RuleARN", true)] {
            let label = format!("{} [{selector}]", case.case);
            let name = format!("rule-{index:02}-{}", if by_arn { "arn" } else { "name" });

            let mut rule = base_rule(&name);
            if let Some(attributes) = case.created_attributes.clone() {
                rule["Attributes"] = attributes;
            }
            let created = create_in(&xray, region, &rule).await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: the fixture rule must be created, got {created:?}"
            );
            let created_record = created.get("SamplingRuleRecord").clone();
            let created_at = created_record
                .get("CreatedAt")
                .and_then(Value::as_f64)
                .unwrap_or_default();

            let mut before = ExpectedRule::created(region, &name);
            if let Some(attributes) = case.created_attributes.as_ref().and_then(Value::as_object) {
                before.attributes = attributes
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect();
            }
            assert_rule(&label, "CreateSamplingRule", &created_record, &before);

            let mut submitted = case.update.clone();
            if by_arn {
                submitted["RuleARN"] = json!(rule_arn(region, &name));
            } else {
                submitted["RuleName"] = json!(name);
            }
            let updated = update_in(&xray, region, &submitted).await;
            assert_eq!(
                updated.status, 200,
                "case {label:?}: UpdateSamplingRule must succeed, got {updated:?}"
            );

            // The expectation comes from the REQUEST: every member it carries
            // replaces the stored one, every member it omits is left alone.
            let after = before.with_update(&case.update);
            let answered = updated.get("SamplingRuleRecord").clone();
            assert_rule(&label, "UpdateSamplingRule", &answered, &after);

            // The write landed in the STORE, so every later reader reports it.
            let listed = listed_record(&xray, region, &label, &name).await;
            assert_rule(&label, "GetSamplingRules", &listed, &after);

            // `CreatedAt` is the one the rule was stored with, `ModifiedAt` has
            // moved to the write.
            for (where_from, record) in [
                ("UpdateSamplingRule", &answered),
                ("GetSamplingRules", &listed),
            ] {
                let created_again = record.get("CreatedAt").and_then(Value::as_f64);
                let modified = record.get("ModifiedAt").and_then(Value::as_f64);
                assert_eq!(
                    created_again,
                    Some(created_at),
                    "case {label:?} ({where_from}): an update does not move CreatedAt, got {created_again:?}"
                );
                assert!(
                    modified.is_some_and(|at| at >= created_at),
                    "case {label:?} ({where_from}): ModifiedAt must be bumped to the write, got {modified:?} against CreatedAt {created_at}"
                );
            }

            // And so does the delete that removes it.
            let removed = delete_in(&xray, region, &json!({ "RuleName": name })).await;
            assert_eq!(
                removed.status, 200,
                "case {label:?}: deleting the rule must succeed, got {removed:?}"
            );
            assert_rule(
                &label,
                "DeleteSamplingRule",
                removed.get("SamplingRuleRecord"),
                &after,
            );
        }
    }
}

#[tokio::test]
async fn an_update_the_service_cannot_apply_is_refused_and_writes_nothing() {
    let xray = Xray::new();
    let region = DEFAULT_REGION;
    let created = create_in(&xray, region, &base_rule("refusal-probe")).await;
    assert_eq!(
        created.status, 200,
        "the fixture rule must be created, got {created:?}"
    );
    let baseline = listed_record(&xray, region, "baseline", "refusal-probe").await;

    // The rows span what an update cannot state: a value outside a bound the
    // modelled `SamplingRule` carries (`Priority` @range 1..9999, `FixedRate`
    // @range 0..1, `ReservoirSize` @range 0.., `AttributeMap` @length 0..5 with
    // 1..32 keys and values), a `ResourceARN` that is neither an ARN nor the
    // wildcard, and a selector the model forbids ("either name or ARN, but not
    // both") or that names no rule.
    let rows: Vec<(&str, Value)> = vec![
        (
            "Priority below the modelled minimum",
            json!({ "RuleName": "refusal-probe", "Priority": 0 }),
        ),
        (
            "Priority above the modelled maximum",
            json!({ "RuleName": "refusal-probe", "Priority": 10_000 }),
        ),
        (
            "a negative Priority",
            json!({ "RuleName": "refusal-probe", "Priority": -1 }),
        ),
        (
            "FixedRate below the modelled minimum",
            json!({ "RuleName": "refusal-probe", "FixedRate": -0.1 }),
        ),
        (
            "FixedRate above the modelled maximum",
            json!({ "RuleName": "refusal-probe", "FixedRate": 1.1 }),
        ),
        (
            "ReservoirSize below the modelled minimum",
            json!({ "RuleName": "refusal-probe", "ReservoirSize": -1 }),
        ),
        (
            "a ResourceARN that is neither an ARN nor the wildcard",
            json!({ "RuleName": "refusal-probe", "ResourceARN": "checkout-function" }),
        ),
        (
            "more Attributes than the modelled maximum of five",
            json!({
                "RuleName": "refusal-probe",
                "Attributes": {"a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6"}
            }),
        ),
        (
            "an Attributes value longer than the modelled maximum",
            json!({ "RuleName": "refusal-probe", "Attributes": {"a": "v".repeat(33)} }),
        ),
        ("neither selector", json!({ "FixedRate": 0.5 })),
        (
            "both selectors",
            json!({
                "RuleName": "refusal-probe",
                "RuleARN": rule_arn(region, "refusal-probe"),
                "FixedRate": 0.5
            }),
        ),
        (
            "a RuleName naming no rule",
            json!({ "RuleName": "never-created", "FixedRate": 0.5 }),
        ),
        (
            "a RuleARN naming no rule",
            json!({ "RuleARN": rule_arn(region, "never-created"), "FixedRate": 0.5 }),
        ),
        (
            "a RuleARN naming another region's rule",
            json!({ "RuleARN": rule_arn("eu-west-1", "refusal-probe"), "FixedRate": 0.5 }),
        ),
    ];

    for (label, update) in rows {
        let refused = update_in(&xray, region, &update).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: the update must be refused, got {refused:?}"
        );
        assert!(
            refused.message().is_some_and(|message| !message.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
        assert_eq!(
            refused.get("SamplingRuleRecord"),
            &Value::Null,
            "case {label:?}: a refused update answers no record, got {refused:?}"
        );
        let after = listed_record(&xray, region, label, "refusal-probe").await;
        assert_eq!(
            after, baseline,
            "case {label:?}: a refused update must write nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_default_rule_is_updatable_and_keeps_its_identity() {
    let xray = Xray::new();
    let region = DEFAULT_REGION;

    // The service-managed rule is derived rather than stored, so the first
    // update of it is what gives it a record — and it keeps the name and the
    // ARN it was already read with.
    let update = json!({ "FixedRate": 0.2, "ReservoirSize": 10 });
    let mut submitted = update.clone();
    submitted["RuleName"] = json!(DEFAULT_RULE_NAME);
    let updated = update_in(&xray, region, &submitted).await;
    assert_eq!(
        updated.status, 200,
        "updating the Default rule must succeed, got {updated:?}"
    );

    let expected = ExpectedRule::service_managed(region).with_update(&update);
    assert_rule(
        "the Default rule",
        "UpdateSamplingRule",
        updated.get("SamplingRuleRecord"),
        &expected,
    );

    // It is still the account's catch-all: still listed, still last, still
    // undeletable, and its `CreatedAt` is still the one nothing created.
    let listed = all_records(&xray, region, "the Default rule").await;
    assert_eq!(
        listed.iter().map(record_name).collect::<Vec<_>>(),
        vec![DEFAULT_RULE_NAME],
        "the updated Default rule is listed exactly once, got {listed:?}"
    );
    let record = &listed[0];
    assert_rule("the Default rule", "GetSamplingRules", record, &expected);
    let created_at = record.get("CreatedAt").and_then(Value::as_f64);
    let modified_at = record.get("ModifiedAt").and_then(Value::as_f64);
    assert_eq!(
        created_at,
        Some(0.0),
        "an update does not create the Default rule, got {created_at:?}"
    );
    assert!(
        modified_at.is_some_and(|at| at > 0.0),
        "the update must move the Default rule's ModifiedAt, got {modified_at:?}"
    );

    let refused = delete_in(&xray, region, &json!({ "RuleName": DEFAULT_RULE_NAME })).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "the updated Default rule is still not deletable, got {refused:?}"
    );

    // Its identity stays DERIVED per region: a reader in another region is
    // answered that region's ARN for the same configuration, which is also the
    // ARN that selects the rule for that reader.
    for reader in ["eu-west-1", "ap-southeast-2"] {
        let elsewhere = all_records(&xray, reader, reader).await;
        assert_eq!(
            elsewhere.iter().map(record_name).collect::<Vec<_>>(),
            vec![DEFAULT_RULE_NAME],
            "case {reader:?}: the Default rule is listed once in every region, got {elsewhere:?}"
        );
        let mut expected_there = expected.clone();
        expected_there.arn = rule_arn(reader, DEFAULT_RULE_NAME);
        assert_rule(reader, "GetSamplingRules", &elsewhere[0], &expected_there);
    }

    // A second update builds on the first: the stored record is what the next
    // write reads.
    let again = json!({ "Priority": 9999 });
    let mut submitted = again.clone();
    submitted["RuleARN"] = json!(rule_arn(region, DEFAULT_RULE_NAME));
    let second = update_in(&xray, region, &submitted).await;
    assert_eq!(
        second.status, 200,
        "a second update of the Default rule must succeed, got {second:?}"
    );
    assert_rule(
        "the Default rule, twice updated",
        "UpdateSamplingRule",
        second.get("SamplingRuleRecord"),
        &expected.with_update(&again),
    );
}
