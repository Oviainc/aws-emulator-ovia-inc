// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the asynchronous-invocation configuration of a function, version
//! or alias: `PutFunctionEventInvokeConfig` "Configures options for asynchronous invocation
//! on a function, version, or alias", `UpdateFunctionEventInvokeConfig`,
//! `GetFunctionEventInvokeConfig`, `ListFunctionEventInvokeConfigs` and
//! `DeleteFunctionEventInvokeConfig`.
//!
//! The rule under test: an asynchronous-invocation configuration is a resource OF ONE
//! QUALIFIER of one function — the unpublished version, a published version or an alias —
//! that exists only once a put has stored one; a put holds exactly the settings its request
//! carries and removes the ones it excludes ("If a configuration already exists for a
//! function, version, or alias, this operation overwrites it. If you exclude any settings,
//! they are removed"), an update changes only the settings its request carries — its two
//! destination slots included, each on its own (see
//! [`crate::event_invoke_config_destination_slots`]) — every read
//! of that qualifier reports what is stored against the qualified ARN it was addressed on,
//! and a qualifier holding none answers `ResourceNotFoundException` (HTTP 404) to every read,
//! update and delete of one. A destination is written to by Lambda acting as the function's
//! execution role, so a configuration naming one the role may not write is refused
//! `InvalidParameterValueException` (HTTP 400) with nothing stored.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetFunctionEventInvokeConfig`, so a value that was answered but never stored
//! fails here. The rows differ along what the rule generalises over: WHICH qualifier the
//! configuration is put on (`$LATEST` implicitly and explicitly, a published version by
//! `Qualifier` and by a qualified `FunctionName`, an alias), WHICH settings the request
//! carries (each one alone, every combination of them, none of the destination slots, one,
//! both), WHICH KIND of destination they name (every kind the model documents), the
//! documented `FunctionName` form, and both bounds of each modelled range plus one past
//! them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::event_source_mapping_role_access::{
    allow, create_function_as, given_role, logs_only, router_with_iam,
};
use crate::event_source_mappings::{call, create_function, router_on, Answer};
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction`, `PublishVersion` and `CreateAlias` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The region the test harness's credential scope names, which is what the ARNs carry.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The wire error code of the modelled 400.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The wire error code of the modelled 404.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One destination per KIND the model documents for an asynchronous invocation
/// configuration, from `smithy_query(query="shape", shape="FunctionEventInvokeConfig")`'s
/// `DestinationConfig` member: "Function - the ARN of a Lambda function. Queue - The ARN of
/// a standard SQS queue. Bucket - The ARN of an Amazon S3 bucket. Topic - The ARN of a
/// standard SNS topic. Event Bus - The ARN of an Amazon EventBridge event bus."
const QUEUE_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:async-records";
/// See [`QUEUE_DESTINATION`].
const TOPIC_DESTINATION: &str = "arn:aws:sns:us-east-1:000000000000:async-records";
/// See [`QUEUE_DESTINATION`].
const BUS_DESTINATION: &str = "arn:aws:events:us-east-1:000000000000:event-bus/async-records";
/// See [`QUEUE_DESTINATION`].
const FUNCTION_DESTINATION: &str = "arn:aws:lambda:us-east-1:000000000000:function:async-records";
/// See [`QUEUE_DESTINATION`].
const BUCKET_DESTINATION: &str = "arn:aws:s3:::async-records";

/// The published bounds of `MaximumRetryAttempts` (`@range(min: 0, max: 2)`) and of
/// `MaximumEventAgeInSeconds` (`@range(min: 60, max: 21600)`), from
/// `smithy_query(query="operation", operation="PutFunctionEventInvokeConfig")`.
const RETRY_BOUNDS: (i64, i64) = (0, 2);
/// See [`RETRY_BOUNDS`].
const AGE_BOUNDS: (i64, i64) = (60, 21_600);

/// The settings one request carries: each is absent when the request states none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Settings {
    retries: Option<i64>,
    age: Option<i64>,
    on_success: Option<&'static str>,
    on_failure: Option<&'static str>,
}

impl Settings {
    /// The request body those settings make.
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(retries) = self.retries {
            body["MaximumRetryAttempts"] = json!(retries);
        }
        if let Some(age) = self.age {
            body["MaximumEventAgeInSeconds"] = json!(age);
        }
        if self.on_success.is_some() || self.on_failure.is_some() {
            let mut destination_config = json!({});
            if let Some(destination) = self.on_success {
                destination_config["OnSuccess"] = json!({ "Destination": destination });
            }
            if let Some(destination) = self.on_failure {
                destination_config["OnFailure"] = json!({ "Destination": destination });
            }
            body["DestinationConfig"] = destination_config;
        }
        body
    }
}

/// The ARN of the unqualified function `name`.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The `FunctionArn` a configuration of one qualifier of `name` is reported on: the
/// function's ARN qualified by that qualifier, `$LATEST` when the call addressed the
/// unpublished version — AWS's own documented answer to a put that named no qualifier
/// carries the `:$LATEST` suffix, which `FunctionArn`'s pattern admits.
pub(crate) fn configured_arn(name: &str, qualifier: Option<&str>) -> String {
    format!("{}:{}", function_arn(name), qualifier.unwrap_or("$LATEST"))
}

/// Where the four single-configuration operations are bound, for the function `reference`
/// names, optionally with the `Qualifier` query parameter.
pub(crate) fn config_uri(reference: &str, qualifier: Option<&str>) -> String {
    let base = format!("/2019-09-25/functions/{reference}/event-invoke-config");
    match qualifier {
        Some(qualifier) => format!("{base}?Qualifier={qualifier}"),
        None => base,
    }
}

/// `PutFunctionEventInvokeConfig` on the qualifier `reference` and `qualifier` name.
async fn put_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    settings: &Settings,
) -> Answer {
    call(
        router,
        "PUT",
        &config_uri(reference, qualifier),
        &settings.body(),
    )
    .await
}

/// `UpdateFunctionEventInvokeConfig` on the same qualifier.
async fn update_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    call(router, "POST", &config_uri(reference, qualifier), body).await
}

/// `GetFunctionEventInvokeConfig` on the same qualifier.
async fn get_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "GET",
        &config_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `DeleteFunctionEventInvokeConfig` on the same qualifier.
async fn delete_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &config_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `ListFunctionEventInvokeConfigs` of the function `reference` names.
async fn list_configs(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2019-09-25/functions/{reference}/event-invoke-config/list{query}"),
        &Value::Null,
    )
    .await
}

/// Publish one version of `name` and report the version label it was given.
pub(crate) async fn publish_version(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishVersion {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .string("Version")
        .unwrap_or_else(|| panic!("PublishVersion {name:?} must report a Version"))
}

/// Create the alias `alias` of `name`, invoking `version`.
async fn create_alias(router: &MultiServiceRouter, name: &str, alias: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} of {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Assert that `answer` reports exactly `settings` on `expected_arn`.
fn assert_reports(case: &str, operation: &str, answer: &Answer, settings: &Settings, arn: &str) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from {operation}, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    assert_eq!(
        body.get("FunctionArn").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: expected {operation} to report FunctionArn {arn:?}, got {body}"
    );
    assert_eq!(
        body.get("MaximumRetryAttempts").and_then(Value::as_i64),
        settings.retries,
        "case {case:?}: expected {operation} to report MaximumRetryAttempts {:?}, got {body}",
        settings.retries
    );
    assert_eq!(
        body.get("MaximumEventAgeInSeconds").and_then(Value::as_i64),
        settings.age,
        "case {case:?}: expected {operation} to report MaximumEventAgeInSeconds {:?}, got {body}",
        settings.age
    );
    // Both destination slots are reported whether or not they carry a destination, so the
    // pair reads unconditionally and an unset slot is an empty object rather than an
    // absent one.
    let destination_config = body
        .get("DestinationConfig")
        .unwrap_or_else(|| panic!("case {case:?}: expected {operation} to report a \
             DestinationConfig, got {body}"));
    for (slot, expected) in [
        ("OnSuccess", settings.on_success),
        ("OnFailure", settings.on_failure),
    ] {
        let reported = destination_config
            .get(slot)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {operation} to report the {slot} slot, got {body}")
            })
            .get("Destination")
            .and_then(Value::as_str);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {operation} to report {slot} {expected:?}, got {reported:?}"
        );
    }
    assert!(
        body.get("LastModified").is_some_and(|value| !value.is_null()),
        "case {case:?}: expected {operation} to report LastModified, got {body}"
    );
}

/// One row of the table: a function, how its configuration is addressed, and the settings
/// put on it.
struct Case {
    name: &'static str,
    function: &'static str,
    /// Whether a version is published before the configuration is put.
    publish: bool,
    /// Whether the alias `BLUE` is created over that version.
    alias: bool,
    /// How the `FunctionName` of every call names the function.
    reference: fn(&str) -> String,
    /// The `Qualifier` parameter of every call.
    qualifier: Option<&'static str>,
    /// The qualifier the reported `FunctionArn` carries, `None` for the unqualified ARN.
    arn_qualifier: Option<&'static str>,
    settings: Settings,
}

/// The table: one row per way the rule can be exercised.
fn cases() -> Vec<Case> {
    vec![
        // The unpublished version, addressed with no qualifier at all, carrying every
        // setting at once.
        Case {
            name: "$LATEST implicitly, with every setting",
            function: "event-invoke-latest",
            publish: false,
            alias: false,
            reference: |name| name.to_string(),
            qualifier: None,
            arn_qualifier: None,
            settings: Settings {
                retries: Some(RETRY_BOUNDS.1),
                age: Some(AGE_BOUNDS.1),
                on_success: Some(TOPIC_DESTINATION),
                on_failure: Some(QUEUE_DESTINATION),
            },
        },
        // The same version addressed explicitly, carrying the other end of each range and
        // only the on-failure slot.
        Case {
            name: "$LATEST explicitly, at the low bound of each range",
            function: "event-invoke-latest-explicit",
            publish: false,
            alias: false,
            reference: |name| name.to_string(),
            qualifier: Some("$LATEST"),
            arn_qualifier: None,
            settings: Settings {
                retries: Some(RETRY_BOUNDS.0),
                age: Some(AGE_BOUNDS.0),
                on_success: None,
                on_failure: Some(BUCKET_DESTINATION),
            },
        },
        // Retries alone, on a published version addressed by the `Qualifier` parameter.
        Case {
            name: "a published version by Qualifier, with retries alone",
            function: "event-invoke-published",
            publish: true,
            alias: false,
            reference: |name| name.to_string(),
            qualifier: Some("1"),
            arn_qualifier: Some("1"),
            settings: Settings {
                retries: Some(1),
                ..Settings::default()
            },
        },
        // The event age alone, on the same version addressed by a qualified ARN instead.
        Case {
            name: "a published version by a qualified function ARN, with the age alone",
            function: "event-invoke-published-arn",
            publish: true,
            alias: false,
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}:1"),
            qualifier: None,
            arn_qualifier: Some("1"),
            settings: Settings {
                age: Some(3_600),
                ..Settings::default()
            },
        },
        // An alias, addressed by a partial ARN and a qualifier, with a function as the
        // on-success destination and an event bus as the on-failure one.
        Case {
            name: "an alias by a partial ARN and a Qualifier, with both slots",
            function: "event-invoke-aliased",
            publish: true,
            alias: true,
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            qualifier: Some("BLUE"),
            arn_qualifier: Some("BLUE"),
            settings: Settings {
                retries: Some(2),
                age: None,
                on_success: Some(FUNCTION_DESTINATION),
                on_failure: Some(BUS_DESTINATION),
            },
        },
        // Destinations alone, with no retry or age setting at all, addressed by the bare
        // `function:` prefix.
        Case {
            name: "the bare function: prefix, with destinations alone",
            function: "event-invoke-destinations-only",
            publish: false,
            alias: false,
            reference: |name| format!("function:{name}"),
            qualifier: None,
            arn_qualifier: None,
            settings: Settings {
                retries: None,
                age: None,
                on_success: Some(QUEUE_DESTINATION),
                on_failure: Some(TOPIC_DESTINATION),
            },
        },
        // A request carrying NO settings at all, which holds a configuration whose every
        // setting is absent.
        Case {
            name: "no settings at all",
            function: "event-invoke-empty",
            publish: false,
            alias: false,
            reference: |name| name.to_string(),
            qualifier: None,
            arn_qualifier: None,
            settings: Settings::default(),
        },
    ]
}

#[tokio::test]
async fn a_qualifier_reports_the_event_invoke_config_put_on_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Every kind of destination the model documents is exercised by the rows above.
    for destination in [
        QUEUE_DESTINATION,
        TOPIC_DESTINATION,
        BUS_DESTINATION,
        FUNCTION_DESTINATION,
        BUCKET_DESTINATION,
    ] {
        assert!(
            cases().iter().any(|case| case.settings.on_success == Some(destination)
                || case.settings.on_failure == Some(destination)),
            "case {destination:?}: the table must cover every documented destination kind"
        );
    }

    for case in cases() {
        let label = case.name;
        create_function(&router, case.function).await;
        if case.publish {
            let published = publish_version(&router, case.function).await;
            assert_eq!(
                published, "1",
                "case {label:?}: the first published version must be 1, got {published:?}"
            );
        }
        if case.alias {
            create_alias(&router, case.function, "BLUE", "1").await;
        }
        let reference = (case.reference)(case.function);
        let expected_arn = configured_arn(case.function, case.arn_qualifier);

        // Nothing is held before the put, so the read that follows it can only report what
        // the put stored.
        let before = get_config(&router, &reference, case.qualifier).await;
        assert_eq!(
            before.status,
            404,
            "case {label:?}: expected 404 from GetFunctionEventInvokeConfig before the put, \
             got {} {}",
            before.status,
            before.text()
        );

        let put = put_config(&router, &reference, case.qualifier, &case.settings).await;
        assert_reports(
            label,
            "PutFunctionEventInvokeConfig",
            &put,
            &case.settings,
            &expected_arn,
        );

        // A report changes nothing, so both rounds read exactly what was stored.
        for round in 0..2 {
            let fetched = get_config(&router, &reference, case.qualifier).await;
            assert_reports(
                &format!("{label} (read {round})"),
                "GetFunctionEventInvokeConfig",
                &fetched,
                &case.settings,
                &expected_arn,
            );
        }
    }
}

#[tokio::test]
async fn a_put_replaces_every_setting_and_an_update_changes_only_the_ones_it_carries() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "event-invoke-overwrite";
    create_function(&router, function).await;
    let seed = Settings {
        retries: Some(2),
        age: Some(1_800),
        on_success: Some(TOPIC_DESTINATION),
        on_failure: Some(QUEUE_DESTINATION),
    };

    // (case, the call, its body, what the configuration holds afterwards)
    let rows: Vec<(&str, &str, Value, Settings)> = vec![
        (
            "a put carrying only retries removes every other setting",
            "PUT",
            Settings {
                retries: Some(0),
                ..Settings::default()
            }
            .body(),
            Settings {
                retries: Some(0),
                ..Settings::default()
            },
        ),
        (
            "a put carrying nothing at all removes every setting",
            "PUT",
            json!({}),
            Settings::default(),
        ),
        (
            "an update carrying only retries leaves every other setting alone",
            "POST",
            Settings {
                retries: Some(1),
                ..Settings::default()
            }
            .body(),
            Settings {
                retries: Some(1),
                ..seed.clone()
            },
        ),
        (
            "an update carrying only the age leaves every other setting alone",
            "POST",
            Settings {
                age: Some(AGE_BOUNDS.0),
                ..Settings::default()
            }
            .body(),
            Settings {
                age: Some(AGE_BOUNDS.0),
                ..seed.clone()
            },
        ),
        (
            "an update carrying nothing at all changes nothing",
            "POST",
            json!({}),
            seed.clone(),
        ),
        (
            "an update stating one destination slot leaves the other as it stands",
            "POST",
            json!({ "DestinationConfig": { "OnFailure": { "Destination": BUS_DESTINATION } } }),
            Settings {
                on_failure: Some(BUS_DESTINATION),
                ..seed.clone()
            },
        ),
        (
            "an update naming the empty destination the pattern admits clears that slot",
            "POST",
            json!({
                "DestinationConfig": {
                    "OnSuccess": { "Destination": "" },
                    "OnFailure": { "Destination": QUEUE_DESTINATION },
                }
            }),
            Settings {
                on_success: None,
                on_failure: Some(QUEUE_DESTINATION),
                ..seed.clone()
            },
        ),
    ];

    for (label, method, body, expected) in rows {
        // Each row starts from the same fully configured state, so what it leaves behind is
        // the work of its own call alone.
        let seeded = put_config(&router, function, None, &seed).await;
        assert_reports(
            &format!("{label} (seed)"),
            "PutFunctionEventInvokeConfig",
            &seeded,
            &seed,
            &configured_arn(function, None),
        );

        let answer = match method {
            "PUT" => {
                call(
                    &router,
                    "PUT",
                    &config_uri(function, None),
                    &body,
                )
                .await
            }
            _ => update_config(&router, function, None, &body).await,
        };
        assert_reports(
            label,
            "the call under test",
            &answer,
            &expected,
            &configured_arn(function, None),
        );
        let fetched = get_config(&router, function, None).await;
        assert_reports(
            label,
            "GetFunctionEventInvokeConfig",
            &fetched,
            &expected,
            &configured_arn(function, None),
        );
    }
}

#[tokio::test]
async fn a_qualifier_holding_no_configuration_has_none_to_read_update_or_delete() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "event-invoke-absent";
    create_function(&router, function).await;
    let published = publish_version(&router, function).await;
    assert_eq!(
        published, "1",
        "the first published version must be 1, got {published:?}"
    );
    create_alias(&router, function, "BLUE", "1").await;
    // Only the unpublished version is configured, so every other qualifier of the same
    // function holds nothing.
    let seed = Settings {
        retries: Some(1),
        ..Settings::default()
    };
    let seeded = put_config(&router, function, None, &seed).await;
    assert_eq!(
        seeded.status,
        200,
        "the seed put must succeed, got {} {}",
        seeded.status,
        seeded.text()
    );

    // One row per qualifier of a function this store holds that nobody has configured.
    for (label, qualifier, arn_qualifier) in [
        ("a published version", Some("1"), Some("1")),
        ("an alias", Some("BLUE"), Some("BLUE")),
    ] {
        let expected = format!(
            "The function {} doesn't have an EventInvokeConfig",
            configured_arn(function, arn_qualifier)
        );
        for (operation, answer) in [
            ("GetFunctionEventInvokeConfig", get_config(&router, function, qualifier).await),
            (
                "UpdateFunctionEventInvokeConfig",
                update_config(&router, function, qualifier, &json!({ "MaximumRetryAttempts": 1 }))
                    .await,
            ),
            (
                "DeleteFunctionEventInvokeConfig",
                delete_config(&router, function, qualifier).await,
            ),
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {label:?}: expected 404 from {operation}, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains(NOT_FOUND),
                "case {label:?}: expected {NOT_FOUND} from {operation}, got {:?}",
                answer.error_code()
            );
            let message = answer
                .string("message")
                .or_else(|| answer.string("Message"))
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: expected a message member from {operation}, got {}",
                        answer.text()
                    )
                });
            assert_eq!(
                message, expected,
                "case {label:?}: expected {operation} to answer {expected:?}, got {message:?}"
            );
        }
    }

    // A function this store holds no record of at all is a different absence, and names the
    // function rather than the configuration.
    let missing = get_config(&router, "event-invoke-nowhere", None).await;
    assert_eq!(
        missing.status,
        404,
        "expected 404 for a function nothing created, got {} {}",
        missing.status,
        missing.text()
    );
    assert_eq!(
        missing.string("Message").or_else(|| missing.string("message")),
        Some(format!(
            "Function not found: {}",
            function_arn("event-invoke-nowhere")
        )),
        "expected the function-not-found sentence, got {}",
        missing.text()
    );

    // The one configured qualifier is untouched by every refusal above, and deleting it
    // takes it away for good.
    let fetched = get_config(&router, function, None).await;
    assert_reports(
        "the configured qualifier",
        "GetFunctionEventInvokeConfig",
        &fetched,
        &seed,
        &configured_arn(function, None),
    );
    let deleted = delete_config(&router, function, None).await;
    assert_eq!(
        deleted.status,
        204,
        "expected 204 from DeleteFunctionEventInvokeConfig, got {} {}",
        deleted.status,
        deleted.text()
    );
    for round in 0..2 {
        let gone = get_config(&router, function, None).await;
        assert_eq!(
            gone.status,
            404,
            "round {round}: expected 404 after the delete, got {} {}",
            gone.status,
            gone.text()
        );
    }
}

#[tokio::test]
async fn the_configurations_of_a_function_are_listed_per_qualifier() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "event-invoke-listed";
    create_function(&router, function).await;
    let published = publish_version(&router, function).await;
    assert_eq!(
        published, "1",
        "the first published version must be 1, got {published:?}"
    );
    create_alias(&router, function, "BLUE", "1").await;
    create_alias(&router, function, "GREEN", "1").await;

    // Each qualifier holds DIFFERENT settings, so an entry reported against the wrong one
    // fails rather than passing by accident.
    let held: Vec<(Option<&str>, Settings)> = vec![
        (
            None,
            Settings {
                retries: Some(0),
                ..Settings::default()
            },
        ),
        (
            Some("1"),
            Settings {
                age: Some(AGE_BOUNDS.1),
                ..Settings::default()
            },
        ),
        (
            Some("BLUE"),
            Settings {
                retries: Some(2),
                on_failure: Some(QUEUE_DESTINATION),
                ..Settings::default()
            },
        ),
        (
            Some("GREEN"),
            Settings {
                on_success: Some(BUS_DESTINATION),
                ..Settings::default()
            },
        ),
    ];
    for (qualifier, settings) in &held {
        let answer = put_config(&router, function, *qualifier, settings).await;
        assert_eq!(
            answer.status,
            200,
            "seeding {:?} must succeed, got {} {}",
            qualifier.unwrap_or("$LATEST"),
            answer.status,
            answer.text()
        );
    }

    // The configurations are reported in the store's own order over the qualifier keys,
    // which is what the pagination below continues through.
    let expected_order = ["$LATEST", "1", "BLUE", "GREEN"];
    let listed = list_configs(&router, function, "").await;
    assert_eq!(
        listed.status,
        200,
        "expected 200 from ListFunctionEventInvokeConfigs, got {} {}",
        listed.status,
        listed.text()
    );
    let entries = listed.json()["FunctionEventInvokeConfigs"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("expected a list, got {}", listed.text()));
    assert_eq!(
        entries.len(),
        held.len(),
        "expected {} configurations, got {}",
        held.len(),
        listed.text()
    );
    for (index, (qualifier, settings)) in held.iter().enumerate() {
        let expected_arn = configured_arn(function, *qualifier);
        let entry = &entries[index];
        assert_eq!(
            entry.get("FunctionArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "entry {index}: expected FunctionArn {expected_arn:?}, got {entry}"
        );
        assert_eq!(
            entry.get("MaximumRetryAttempts").and_then(Value::as_i64),
            settings.retries,
            "entry {index}: expected MaximumRetryAttempts {:?}, got {entry}",
            settings.retries
        );
        assert_eq!(
            entry.get("MaximumEventAgeInSeconds").and_then(Value::as_i64),
            settings.age,
            "entry {index}: expected MaximumEventAgeInSeconds {:?}, got {entry}",
            settings.age
        );
        assert_eq!(
            entry["DestinationConfig"]["OnFailure"]
                .get("Destination")
                .and_then(Value::as_str),
            settings.on_failure,
            "entry {index}: expected OnFailure {:?}, got {entry}",
            settings.on_failure
        );
    }

    // One page per size the caller can ask for: the marker continues after the last entry
    // the previous page reported, and the last page carries no marker at all.
    for page_size in 1..=held.len() {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?MaxItems={page_size}");
        loop {
            let page = list_configs(&router, function, &query).await;
            assert_eq!(
                page.status,
                200,
                "case {page_size}: expected 200 from the page, got {} {}",
                page.status,
                page.text()
            );
            let body = page.json();
            let configs = body["FunctionEventInvokeConfigs"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                configs.len() <= page_size,
                "case {page_size}: expected at most {page_size} entries, got {}",
                configs.len()
            );
            for entry in &configs {
                let arn = entry
                    .get("FunctionArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                seen.push(arn.to_string());
            }
            match body.get("NextMarker").and_then(Value::as_str) {
                Some(marker) => query = format!("?MaxItems={page_size}&Marker={marker}"),
                None => break,
            }
        }
        let expected: Vec<String> = expected_order
            .iter()
            .map(|qualifier| configured_arn(function, Some(qualifier)))
            .collect();
        assert_eq!(
            seen, expected,
            "case {page_size}: expected the pages to report every configuration once, in \
             order, got {seen:?}"
        );
    }

    // A function that holds no configuration at all lists none rather than failing.
    create_function(&router, "event-invoke-unlisted").await;
    let empty = list_configs(&router, "event-invoke-unlisted", "").await;
    assert_eq!(
        empty.status,
        200,
        "expected 200 for a function holding no configuration, got {} {}",
        empty.status,
        empty.text()
    );
    assert_eq!(
        empty.json()["FunctionEventInvokeConfigs"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "expected no configurations, got {}",
        empty.text()
    );

    // This operation lists the configurations OF A FUNCTION and takes no `Qualifier`, so a
    // qualified name names no function whose configurations to list.
    for reference in [
        format!("{function}:1"),
        format!("{function}:BLUE"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:1"),
    ] {
        let answer = list_configs(&router, &reference, "").await;
        assert_eq!(
            answer.status,
            400,
            "case {reference:?}: expected 400 from ListFunctionEventInvokeConfigs, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(INVALID_PARAMETER),
            "case {reference:?}: expected {INVALID_PARAMETER}, got {:?}",
            answer.error_code()
        );
    }
}

#[tokio::test]
async fn settings_outside_the_modelled_ranges_are_refused_and_store_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "event-invoke-bounds";
    create_function(&router, function).await;
    let seed = Settings {
        retries: Some(1),
        age: Some(3_600),
        ..Settings::default()
    };
    let seeded = put_config(&router, function, None, &seed).await;
    assert_eq!(
        seeded.status,
        200,
        "the seed put must succeed, got {} {}",
        seeded.status,
        seeded.text()
    );

    // Each published bound is accepted and each value one past it is refused, for both
    // ranges and through both the put and the update.
    let (retry_min, retry_max) = RETRY_BOUNDS;
    let (age_min, age_max) = AGE_BOUNDS;
    let rows: Vec<(String, Value, bool)> = vec![
        (
            format!("MaximumRetryAttempts at its floor ({retry_min})"),
            json!({ "MaximumRetryAttempts": retry_min }),
            true,
        ),
        (
            format!("MaximumRetryAttempts at its ceiling ({retry_max})"),
            json!({ "MaximumRetryAttempts": retry_max }),
            true,
        ),
        (
            format!("MaximumRetryAttempts one below its floor ({})", retry_min - 1),
            json!({ "MaximumRetryAttempts": retry_min - 1 }),
            false,
        ),
        (
            format!("MaximumRetryAttempts one above its ceiling ({})", retry_max + 1),
            json!({ "MaximumRetryAttempts": retry_max + 1 }),
            false,
        ),
        (
            format!("MaximumEventAgeInSeconds at its floor ({age_min})"),
            json!({ "MaximumEventAgeInSeconds": age_min }),
            true,
        ),
        (
            format!("MaximumEventAgeInSeconds at its ceiling ({age_max})"),
            json!({ "MaximumEventAgeInSeconds": age_max }),
            true,
        ),
        (
            format!("MaximumEventAgeInSeconds one below its floor ({})", age_min - 1),
            json!({ "MaximumEventAgeInSeconds": age_min - 1 }),
            false,
        ),
        (
            format!("MaximumEventAgeInSeconds one above its ceiling ({})", age_max + 1),
            json!({ "MaximumEventAgeInSeconds": age_max + 1 }),
            false,
        ),
    ];

    for (label, body, admissible) in rows {
        for method in ["PUT", "POST"] {
            // Every row starts from the same stored configuration, so a refusal that wrote
            // something shows up as a change to it.
            let reseed = put_config(&router, function, None, &seed).await;
            assert_eq!(
                reseed.status,
                200,
                "case {label:?}: the reseed must succeed, got {} {}",
                reseed.status,
                reseed.text()
            );

            let answer = call(&router, method, &config_uri(function, None), &body).await;
            if admissible {
                assert_eq!(
                    answer.status,
                    200,
                    "case {label:?} ({method}): expected 200, got {} {}",
                    answer.status,
                    answer.text()
                );
                continue;
            }
            assert_eq!(
                answer.status,
                400,
                "case {label:?} ({method}): expected 400, got {} {}",
                answer.status,
                answer.text()
            );
            let fetched = get_config(&router, function, None).await;
            assert_reports(
                &format!("{label} ({method})"),
                "GetFunctionEventInvokeConfig",
                &fetched,
                &seed,
                &configured_arn(function, None),
            );
        }
    }
}

/// One row of the authorization table: what the function's execution role is granted, the
/// destination the configuration names, and the call the refusal must name (`None` when the
/// configuration must be stored).
struct RoleRow {
    case: &'static str,
    /// The IAM action namespace and the actions granted on the destination, if any.
    granted: Option<(&'static str, &'static [&'static str])>,
    /// Whether the grant names the destination itself or some other resource.
    on_destination: bool,
    destination: &'static str,
    /// The withheld call and the service the refusal names.
    refusal: Option<(&'static str, &'static str)>,
}

/// The authorization table: one row per kind of destination whose write is decided on, with
/// and without the grant that write takes, plus the kind whose write is decided on nothing.
fn role_rows() -> Vec<RoleRow> {
    vec![
        RoleRow {
            case: "a queue the role may write",
            granted: Some(("sqs", &["SendMessage"])),
            on_destination: true,
            destination: QUEUE_DESTINATION,
            refusal: None,
        },
        RoleRow {
            case: "a queue the role may not write",
            granted: None,
            on_destination: true,
            destination: QUEUE_DESTINATION,
            refusal: Some(("SendMessage", "SQS")),
        },
        RoleRow {
            case: "a queue granted on another queue",
            granted: Some(("sqs", &["SendMessage"])),
            on_destination: false,
            destination: QUEUE_DESTINATION,
            refusal: Some(("SendMessage", "SQS")),
        },
        RoleRow {
            case: "a topic the role may publish to",
            granted: Some(("sns", &["Publish"])),
            on_destination: true,
            destination: TOPIC_DESTINATION,
            refusal: None,
        },
        RoleRow {
            case: "a topic the role may not publish to",
            granted: Some(("sns", &["Subscribe"])),
            on_destination: true,
            destination: TOPIC_DESTINATION,
            refusal: Some(("Publish", "SNS")),
        },
        RoleRow {
            case: "an event bus the role may put events on",
            granted: Some(("events", &["PutEvents"])),
            on_destination: true,
            destination: BUS_DESTINATION,
            refusal: None,
        },
        RoleRow {
            case: "an event bus the role may not put events on",
            granted: None,
            on_destination: true,
            destination: BUS_DESTINATION,
            refusal: Some(("PutEvents", "EventBridge")),
        },
        RoleRow {
            case: "a function the role may invoke",
            granted: Some(("lambda", &["InvokeFunction"])),
            on_destination: true,
            destination: FUNCTION_DESTINATION,
            refusal: None,
        },
        RoleRow {
            case: "a function the role may not invoke",
            granted: None,
            on_destination: true,
            destination: FUNCTION_DESTINATION,
            refusal: Some(("InvokeFunction", "Lambda")),
        },
        RoleRow {
            // A bucket's write is an object-level call, so a role scoped to the objects it
            // writes is not short of anything the bucket ARN can be held to: nothing of the
            // kind can be shown, and the configuration is stored.
            case: "a bucket, whose write is decided on nothing",
            granted: None,
            on_destination: true,
            destination: BUCKET_DESTINATION,
            refusal: None,
        },
    ]
}

/// Another queue, which is what a grant that names the WRONG resource names.
const OTHER_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:other-records";

#[tokio::test]
async fn a_destination_the_execution_role_may_not_write_is_refused() {
    let router = router_with_iam();

    for (index, row) in role_rows().into_iter().enumerate() {
        let case = row.case;
        let function = format!("event-invoke-role-{index}");
        let statements = match row.granted {
            Some((namespace, actions)) => {
                let resource = if row.on_destination {
                    row.destination
                } else {
                    OTHER_DESTINATION
                };
                json!([logs_only(), allow(namespace, actions, resource)])
            }
            None => json!([logs_only()]),
        };
        let role = given_role(
            &router,
            case,
            &format!("event-invoke-role-{index}"),
            Some(statements),
        )
        .await;
        create_function_as(&router, case, &function, &role).await;

        // The on-failure slot carries the destination under test, so the decision is about
        // that one destination alone.
        let settings = Settings {
            retries: Some(1),
            age: None,
            on_success: None,
            on_failure: Some(row.destination),
        };
        let answer = put_config(&router, &function, None, &settings).await;
        let Some((withheld, service)) = row.refusal else {
            assert_reports(
                case,
                "PutFunctionEventInvokeConfig",
                &answer,
                &settings,
                &configured_arn(&function, None),
            );
            continue;
        };

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 from PutFunctionEventInvokeConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(INVALID_PARAMETER),
            "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
            answer.error_code()
        );
        let expected = format!(
            "The provided execution role does not have permissions to call {withheld} on \
             {service}"
        );
        let message = answer
            .string("message")
            .or_else(|| answer.string("Message"))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a message member, got {}",
                    answer.text()
                )
            });
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The refusal stored nothing: the qualifier still holds no configuration at all.
        let fetched = get_config(&router, &function, None).await;
        assert_eq!(
            fetched.status,
            404,
            "case {case:?}: expected the refusal to store nothing, got {} {}",
            fetched.status,
            fetched.text()
        );

        // The state of the RESOURCE is judged before the request's own members, so an
        // update of a qualifier that holds no configuration answers that absence rather
        // than the destination refusal.
        let absent = update_config(
            &router,
            &function,
            None,
            &json!({ "DestinationConfig": { "OnFailure": { "Destination": row.destination } } }),
        )
        .await;
        assert_eq!(
            absent.status,
            404,
            "case {case:?}: expected 404 from UpdateFunctionEventInvokeConfig on a qualifier \
             holding no configuration, got {} {}",
            absent.status,
            absent.text()
        );

        // The update is judged by the same rule, so a configuration the role may hold is
        // not taken past the gate by the second call either.
        let allowed = put_config(
            &router,
            &function,
            None,
            &Settings {
                retries: Some(0),
                ..Settings::default()
            },
        )
        .await;
        assert_eq!(
            allowed.status,
            200,
            "case {case:?}: a configuration naming no destination must be stored, got {} {}",
            allowed.status,
            allowed.text()
        );
        let updated = update_config(
            &router,
            &function,
            None,
            &json!({ "DestinationConfig": { "OnFailure": { "Destination": row.destination } } }),
        )
        .await;
        assert_eq!(
            updated.status,
            400,
            "case {case:?}: expected 400 from UpdateFunctionEventInvokeConfig, got {} {}",
            updated.status,
            updated.text()
        );
        let held = get_config(&router, &function, None).await;
        assert_reports(
            case,
            "GetFunctionEventInvokeConfig",
            &held,
            &Settings {
                retries: Some(0),
                ..Settings::default()
            },
            &configured_arn(&function, None),
        );
    }
}
