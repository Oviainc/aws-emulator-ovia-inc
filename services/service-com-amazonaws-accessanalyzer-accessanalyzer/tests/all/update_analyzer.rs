// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateAnalyzer`, driven over the wire.
//!
//! Every case goes through the real router via the shared wire harness
//! (`tests/all/harness.rs`), so there is ONE copy of it in this crate: an
//! `http::Request` is dispatched to the mounted service, so restJson1 routing, boundary
//! validation, the handler and response serialization all run as they do in the
//! emulator binary. An update is only believed when a later
//! `GetAnalyzer`/`ListAnalyzers` reports it back.
//!
//! The tables span what the rules generalise over — every analyzer `type`, both
//! `AnalyzerConfiguration` union members, a configuration present and absent before the
//! update, and `unusedAccessAge` at each documented bound and one past it. The recorded
//! `ACCOUNT_UNUSED_ACCESS` + `unusedAccess` case is one row.

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    UNUSED_ACCESS_AGE_MAX_DAYS, UNUSED_ACCESS_AGE_MIN_DAYS,
};

// The wire harness is shared by every suite in this target; see `tests/all/harness.rs`.
use crate::harness::{Emulator, Reply, VALIDATION_REASONS};

/// The analyzer `type`s that carry an `unusedAccess` configuration, from the model's
/// `Type` enum (`smithy_query(query="shape", shape="Type")`).
const UNUSED_ACCESS_TYPES: &[&str] = &["ACCOUNT_UNUSED_ACCESS", "ORGANIZATION_UNUSED_ACCESS"];

/// The analyzer `type`s that carry an `internalAccess` configuration, from the same enum.
const INTERNAL_ACCESS_TYPES: &[&str] =
    &["ACCOUNT_INTERNAL_ACCESS", "ORGANIZATION_INTERNAL_ACCESS"];

/// The analyzer `type`s that carry NO configuration — the external access analyzers
/// `UpdateAnalyzer`'s documentation says the action is not supported for.
const EXTERNAL_ACCESS_TYPES: &[&str] = &["ACCOUNT", "ORGANIZATION"];

/// Create `name` as `analyzer_type`, optionally with a configuration, and fail loudly if
/// the create itself did not succeed.
async fn create(emulator: &Emulator, case: &str, name: &str, analyzer_type: &str, configuration: Option<&Value>) {
    let mut request = json!({"analyzerName": name, "type": analyzer_type});
    if let Some(configuration) = configuration {
        request["configuration"] = configuration.clone();
    }
    let reply = emulator.create_analyzer(request).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: CreateAnalyzer must succeed, got {reply:?}"
    );
}

/// The `configuration` a `GetAnalyzer` reports for `name`, as JSON (absent as `None`).
async fn stored_configuration(emulator: &Emulator, case: &str, name: &str) -> Option<Value> {
    let reply = emulator.get_analyzer(name).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetAnalyzer must succeed, got {reply:?}"
    );
    let analyzer = reply
        .member("analyzer")
        .unwrap_or_else(|| panic!("case {case:?}: GetAnalyzer must carry `analyzer`: {reply:?}"));
    analyzer
        .get("configuration")
        .filter(|value| !value.is_null())
        .cloned()
}

/// The `configuration` `ListAnalyzers` reports for `name`, so an update is observable by
/// every reader of the record and not just by `GetAnalyzer`.
async fn listed_configuration(emulator: &Emulator, case: &str, name: &str) -> Option<Value> {
    let reply = emulator.list_analyzers("").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListAnalyzers must succeed, got {reply:?}"
    );
    let entry = reply
        .member("analyzers")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: ListAnalyzers must report {name}: {reply:?}"));
    entry
        .get("configuration")
        .filter(|value| !value.is_null())
        .cloned()
}

/// Assert a reply is this service's modelled `ValidationException`: HTTP 400, the wire
/// code, and a `reason` the model's own `ValidationExceptionReason` enum names.
///
/// The accepted values are the model's SCREAMING_SNAKE NAMES, which is what a response
/// carries (`harness::VALIDATION_REASONS` holds the pairs and cites them); the lowerCamel
/// discriminators `VALIDATION_EXCEPTION_REASON_VALUES` lists are the other half of those
/// pairs.
fn assert_validation_exception(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a refused configuration is HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        "ValidationException",
        "case {case:?}: the error code must be ValidationException, got {:?}",
        reply.code()
    );
    let reason = reply
        .member("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let names: Vec<&str> = VALIDATION_REASONS.iter().map(|(_, name)| *name).collect();
    assert!(
        names.contains(&reason),
        "case {case:?}: reason must be one of {names:?}, got {reason:?}"
    );
    assert_eq!(
        names.len(),
        VALIDATION_EXCEPTION_REASON_VALUES.len(),
        "the catalog must name every modelled discriminator: {names:?} vs {VALIDATION_EXCEPTION_REASON_VALUES:?}"
    );
}

/// One row of the replace-and-read-back table.
#[derive(Debug)]
struct UpdateCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    name: &'static str,
    analyzer_type: &'static str,
    /// The configuration the analyzer was CREATED with, `None` when it was created
    /// without one.
    created_with: Option<Value>,
    /// The configuration the first update supplies.
    first_update: Value,
    /// A second, different configuration of the same family: the update replaces rather
    /// than merges, so this one is what the record ends up holding.
    second_update: Value,
}

/// `UpdateAnalyzer` stores the configuration it is given and reports back what is
/// stored, over the whole class of configured analyzers — not just the recorded one.
#[tokio::test]
async fn update_analyzer_replaces_the_stored_configuration() {
    // The rows differ along analyzer type (account and organization scope), union
    // member (`unusedAccess`, `internalAccess`), whether a configuration existed
    // before, and the shape of the configuration (a scalar age, an analysis rule by
    // account id, by resource tag, by resource type, by resource ARN).
    let cases = vec![
        UpdateCase {
            case: "the recorded case: an unused-access age is set on an analyzer created without one",
            name: "demo",
            analyzer_type: "ACCOUNT_UNUSED_ACCESS",
            created_with: None,
            first_update: json!({"unusedAccess": {"unusedAccessAge": 90}}),
            second_update: json!({"unusedAccess": {"unusedAccessAge": 30}}),
        },
        UpdateCase {
            case: "organization unused access: an age replaced by an analysis rule alone",
            name: "org_unused",
            analyzer_type: "ORGANIZATION_UNUSED_ACCESS",
            created_with: Some(json!({"unusedAccess": {"unusedAccessAge": 1}})),
            first_update: json!({"unusedAccess": {
                "unusedAccessAge": 365,
                "analysisRule": {"exclusions": [{"accountIds": ["123456789012"]}]}
            }}),
            // No `unusedAccessAge` at all: a replace drops the member the previous
            // configuration carried, a merge would keep it.
            second_update: json!({"unusedAccess": {
                "analysisRule": {"exclusions": [{"resourceTags": [{"env": "prod"}]}]}
            }}),
        },
        UpdateCase {
            case: "account internal access: inclusions replaced",
            name: "acct.internal-1",
            analyzer_type: "ACCOUNT_INTERNAL_ACCESS",
            created_with: Some(json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::S3::Bucket"]}
            ]}}})),
            first_update: json!({"internalAccess": {"analysisRule": {"inclusions": [
                // A resource type the issue did not cite, from the model's
                // `AccessCheckResourceType` list.
                {"resourceTypes": ["AWS::KMS::Key", "AWS::DynamoDB::Table"]}
            ]}}}),
            second_update: json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"accountIds": ["123456789012"], "resourceArns": ["arn:aws:s3:::example-bucket"]}
            ]}}}),
        },
        UpdateCase {
            case: "organization internal access: an analyzer created without a configuration",
            name: "_org_internal",
            analyzer_type: "ORGANIZATION_INTERNAL_ACCESS",
            created_with: None,
            first_update: json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"accountIds": ["210987654321"]}
            ]}}}),
            second_update: json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::SQS::Queue"]}
            ]}}}),
        },
    ];

    let emulator = Emulator::new();

    for UpdateCase {
        case,
        name,
        analyzer_type,
        created_with,
        first_update,
        second_update,
    } in &cases
    {
        create(&emulator, case, name, analyzer_type, created_with.as_ref()).await;
        assert_eq!(
            stored_configuration(&emulator, case, name).await,
            created_with.clone(),
            "case {case:?}: the create must store exactly the configuration it carried"
        );

        for (step, configuration) in [("first", first_update), ("second", second_update)] {
            let update = emulator
                .update_analyzer(name, json!({"configuration": configuration.clone()}))
                .await;
            assert_eq!(
                update.status, 200,
                "case {case:?} ({step} update): UpdateAnalyzer must succeed, got {update:?}"
            );
            assert_eq!(
                update.member("configuration"),
                Some(configuration),
                "case {case:?} ({step} update): the response must carry the stored configuration, got {:?}",
                update.member("configuration")
            );
            assert_eq!(
                stored_configuration(&emulator, case, name).await.as_ref(),
                Some(configuration),
                "case {case:?} ({step} update): GetAnalyzer must report the updated configuration"
            );
            assert_eq!(
                listed_configuration(&emulator, case, name).await.as_ref(),
                Some(configuration),
                "case {case:?} ({step} update): ListAnalyzers must report the updated configuration"
            );
        }

        // Nothing else about the record moves: the analyzer keeps its identity.
        let reply = emulator.get_analyzer(name).await;
        let analyzer = reply.member("analyzer").expect("the summary");
        assert_eq!(
            analyzer.get("type").and_then(Value::as_str),
            Some(*analyzer_type),
            "case {case:?}: an update must not change the analyzer type, got {:?}",
            analyzer.get("type")
        );
        assert_eq!(
            analyzer.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: an update must not change the status, got {:?}",
            analyzer.get("status")
        );
    }
}

/// An update that names no `configuration` asks for no change: it answers with what is
/// stored and leaves it there.
#[tokio::test]
async fn update_analyzer_without_a_configuration_leaves_the_record_alone() {
    // Both union members, and an analyzer that has no configuration at all.
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "unused access, configuration already stored",
            "kept_unused",
            "ACCOUNT_UNUSED_ACCESS",
            Some(json!({"unusedAccess": {"unusedAccessAge": 7}})),
        ),
        (
            "internal access, configuration already stored",
            "kept_internal",
            "ORGANIZATION_INTERNAL_ACCESS",
            Some(json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::SNS::Topic"]}
            ]}}})),
        ),
        (
            "unused access, no configuration stored",
            "kept_empty",
            "ORGANIZATION_UNUSED_ACCESS",
            None,
        ),
    ];

    let emulator = Emulator::new();

    for (case, name, analyzer_type, configuration) in &cases {
        create(&emulator, case, name, analyzer_type, configuration.as_ref()).await;

        let update = emulator.update_analyzer(name, json!({})).await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: an update with no configuration must succeed, got {update:?}"
        );
        assert_eq!(
            update.member("configuration"),
            configuration.as_ref(),
            "case {case:?}: the response must carry the configuration that is stored, got {:?}",
            update.member("configuration")
        );
        assert_eq!(
            stored_configuration(&emulator, case, name).await,
            configuration.clone(),
            "case {case:?}: the stored configuration must be unchanged"
        );
    }
}

/// `UpdateAnalyzer` modifies an EXISTING analyzer: an unknown name is a 404 and creates
/// nothing.
#[tokio::test]
async fn update_analyzer_refuses_an_unknown_analyzer() {
    // Names differing in form and length, none of them created.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "never created, unused-access configuration",
            "missing_analyzer",
            json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}}),
        ),
        (
            "never created, internal-access configuration",
            "Other.Missing-1",
            json!({"configuration": {"internalAccess": {"analysisRule": {"inclusions": [
                {"accountIds": ["123456789012"]}
            ]}}}}),
        ),
        ("never created, no configuration", "z", json!({})),
    ];

    let emulator = Emulator::new();
    // One analyzer DOES exist, so the refusal is about the name and not about an empty
    // store.
    create(
        &emulator,
        "a neighbour analyzer exists",
        "neighbour",
        "ACCOUNT_UNUSED_ACCESS",
        None,
    )
    .await;

    for (case, name, body) in &cases {
        let update = emulator.update_analyzer(name, body.clone()).await;
        assert_eq!(
            update.status, 404,
            "case {case:?}: an unknown analyzer is HTTP 404, got {update:?}"
        );
        assert_eq!(
            update.code(),
            "ResourceNotFoundException",
            "case {case:?}: the error code must be ResourceNotFoundException, got {:?}",
            update.code()
        );

        let get = emulator.get_analyzer(name).await;
        assert_eq!(
            get.status, 404,
            "case {case:?}: a refused update must not create the analyzer, got {get:?}"
        );
    }
}

/// The action is documented as not supported for external access analyzers, so every
/// request against one is refused and the record keeps no configuration.
#[tokio::test]
async fn update_analyzer_is_refused_for_external_access_analyzers() {
    let bodies = [
        ("an unused-access configuration", json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}})),
        (
            "an internal-access configuration",
            json!({"configuration": {"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::S3::Bucket"]}
            ]}}}}),
        ),
        ("no configuration at all", json!({})),
    ];

    let emulator = Emulator::new();

    for analyzer_type in EXTERNAL_ACCESS_TYPES {
        let name = format!("external_{}", analyzer_type.to_lowercase());
        let case = format!("{analyzer_type}: the action is not supported");
        create(&emulator, &case, &name, analyzer_type, None).await;

        for (body_case, body) in &bodies {
            let case = format!("{analyzer_type} with {body_case}");
            let update = emulator.update_analyzer(&name, body.clone()).await;
            assert_validation_exception(&case, &update);
            // The refusal is about the analyzer the path names rather than about one
            // member, so it classifies as the field-validation discriminator AWS answers
            // with and names no member in `fieldList`.
            assert_eq!(
                update.member("reason").and_then(Value::as_str),
                Some(crate::harness::wire_reason("fieldValidationFailed")),
                "case {case:?}: the refusal must carry the field-validation reason, got {update:?}"
            );
            assert!(
                update.member("fieldList").is_none(),
                "case {case:?}: no single member is at fault, so there is no fieldList, got {update:?}"
            );
            assert_eq!(
                stored_configuration(&emulator, &case, &name).await,
                None,
                "case {case:?}: an external access analyzer must still carry no configuration"
            );
        }
    }
}

/// An analyzer is configured by exactly the union member its `type` names, so a
/// configuration for the other family is refused and nothing is stored.
#[tokio::test]
async fn update_analyzer_refuses_a_configuration_for_the_other_family() {
    let internal = json!({"internalAccess": {"analysisRule": {"inclusions": [
        {"resourceTypes": ["AWS::S3::Bucket"]}
    ]}}});
    let unused = json!({"unusedAccess": {"unusedAccessAge": 90}});

    // Each configured family paired with the configuration it does NOT take, and the
    // one it does (which must still be accepted afterwards).
    let families: Vec<(&[&str], &Value, &Value)> = vec![
        (UNUSED_ACCESS_TYPES, &internal, &unused),
        (INTERNAL_ACCESS_TYPES, &unused, &internal),
    ];

    let emulator = Emulator::new();

    for (types, wrong, right) in families {
        for analyzer_type in types {
            let name = format!("family_{}", analyzer_type.to_lowercase());
            let case = format!("{analyzer_type} refuses the other union member");
            create(&emulator, &case, &name, analyzer_type, None).await;

            let refused = emulator
                .update_analyzer(&name, json!({"configuration": wrong.clone()}))
                .await;
            assert_validation_exception(&case, &refused);
            assert_eq!(
                stored_configuration(&emulator, &case, &name).await,
                None,
                "case {case:?}: a refused update must store nothing"
            );

            let accepted = emulator
                .update_analyzer(&name, json!({"configuration": right.clone()}))
                .await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the matching union member must still be accepted, got {accepted:?}"
            );
            assert_eq!(
                stored_configuration(&emulator, &case, &name).await.as_ref(),
                Some(right),
                "case {case:?}: the matching configuration must be stored"
            );
        }
    }
}

/// `unusedAccessAge` is documented as "a value between 1 and 365 days"
/// (`smithy_query(query="doc", shape="UnusedAccessConfiguration",
/// member="unusedAccessAge")`), a bound the model carries no `@range` for — so both
/// operations that accept a configuration enforce it: each published bound is accepted
/// and one step past it is refused.
#[tokio::test]
async fn unused_access_age_is_bounded_by_its_documented_range() {
    let min = UNUSED_ACCESS_AGE_MIN_DAYS;
    let max = UNUSED_ACCESS_AGE_MAX_DAYS;
    // Each published bound, an interior value, one step past each bound, and a
    // negative — the class is "inside the range" versus "outside it".
    let cases: Vec<(String, i32, bool)> = vec![
        (format!("the documented minimum ({min})"), min, true),
        ("an interior value".to_string(), 90, true),
        (format!("the documented maximum ({max})"), max, true),
        (format!("one below the minimum ({})", min - 1), min - 1, false),
        (format!("one above the maximum ({})", max + 1), max + 1, false),
        ("a negative age".to_string(), -30, false),
    ];

    let emulator = Emulator::new();
    // The analyzer the updates run against keeps a known-good configuration, so a
    // refusal is visible as "the stored configuration did not move".
    let name = "age_bounds";
    let stored = json!({"unusedAccess": {"unusedAccessAge": 90}});
    create(
        &emulator,
        "the age-bounds analyzer",
        name,
        "ACCOUNT_UNUSED_ACCESS",
        Some(&stored),
    )
    .await;

    for (case, age, accepted) in &cases {
        let configuration = json!({"unusedAccess": {"unusedAccessAge": age}});

        // UpdateAnalyzer.
        let update = emulator
            .update_analyzer(name, json!({"configuration": configuration.clone()}))
            .await;
        if *accepted {
            assert_eq!(
                update.status, 200,
                "case {case:?}: an age inside the documented range must be accepted, got {update:?}"
            );
            assert_eq!(
                stored_configuration(&emulator, case, name).await.as_ref(),
                Some(&configuration),
                "case {case:?}: the accepted age must be stored"
            );
            // Put the known-good configuration back for the next row.
            let reset = emulator
                .update_analyzer(name, json!({"configuration": stored.clone()}))
                .await;
            assert_eq!(
                reset.status, 200,
                "case {case:?}: restoring the known-good configuration must succeed, got {reset:?}"
            );
        } else {
            assert_validation_exception(case, &update);
            assert_eq!(
                stored_configuration(&emulator, case, name).await.as_ref(),
                Some(&stored),
                "case {case:?}: a refused update must leave the stored configuration alone"
            );
        }

        // CreateAnalyzer accepts the same `AnalyzerConfiguration`, so the same bound
        // holds there. The name carries the row's own age (with the sign spelled out,
        // since `AnalyzerName`'s pattern allows no `-` at the front), so two rows can
        // never collide on one name.
        let created = if *age < 0 {
            format!("created_minus_{}", age.unsigned_abs())
        } else {
            format!("created_{age}")
        };
        let create_reply = emulator
            .create_analyzer(json!({
                "analyzerName": created,
                "type": "ACCOUNT_UNUSED_ACCESS",
                "configuration": configuration.clone(),
            }))
            .await;
        if *accepted {
            assert_eq!(
                create_reply.status, 200,
                "case {case:?}: CreateAnalyzer must accept an age inside the range, got {create_reply:?}"
            );
            assert_eq!(
                stored_configuration(&emulator, case, &created).await.as_ref(),
                Some(&configuration),
                "case {case:?}: CreateAnalyzer must store the accepted configuration"
            );
        } else {
            assert_validation_exception(case, &create_reply);
            let get = emulator.get_analyzer(&created).await;
            assert_eq!(
                get.status, 404,
                "case {case:?}: a refused create must store no analyzer, got {get:?}"
            );
        }
    }
}
