// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue usage profile slice: `CreateUsageProfile`, the
//! `GetUsageProfile` that reads back what it stored, and the `DeleteUsageProfile` that
//! takes the record away again.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a table whose rows differ along the axes the rule generalises
//! over: whether each member of the submitted `ProfileConfiguration` is ABSENT, an EMPTY
//! map or a populated one, how many parameters a populated map carries, which members of
//! a `ConfigurationObject` are filled, and whether the create named a description or
//! tags. A profile carrying one job parameter — the shape a lifecycle client sends — is
//! one row among them.
//!
//! The refusal suites are tables too: one over the ways a parameter's bounds can fail to
//! admit its own values, one over the configurations whose bounds DO admit them, and one
//! over the forms of a name, which every refusal of this resource carries.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateUsageProfileInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, UsageProfileEntry, MAX_USAGE_PROFILES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=CreateUsageProfile`), which is also the length bound on a
/// `ConfigurationMap` key.
const MAX_NAME_LENGTH: usize = 255;

/// The longest `Description` the model accepts: "len 0..2048" on `DescriptionString`
/// (`smithy_query operation=CreateUsageProfile`). The minimum of that range is 0, so an
/// EMPTY description is a value a create may send.
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// The longest `DefaultValue`/`MinValue`/`MaxValue` a `ConfigurationObject` accepts:
/// "len 1..128" on `ConfigValueString` (`smithy_query shape=ConfigurationObject`), whose
/// pattern is `^[a-zA-Z0-9_.-]+$`.
const MAX_CONFIG_VALUE_LENGTH: usize = 128;

/// Create a usage profile and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateUsageProfile", body).await
}

/// The `GetUsageProfile` response for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetUsageProfile", &json!({ "Name": name })).await
}

/// Assert that the `ProfileConfiguration` a write submitted is the one a read reports,
/// member for member — INCLUDING the difference between a configuration map that was sent
/// empty and one that was not sent at all, which awsJson spells as `{}` and as an omitted
/// member.
fn assert_configuration(case: &str, sent: &Value, got: &Value) {
    for member in ["SessionConfiguration", "JobConfiguration"] {
        let expected = sent.get(member);
        let actual = got.get(member).filter(|value| !value.is_null());
        match expected {
            None => assert!(
                actual.is_none(),
                "case {case}: {member} was not sent, so the read must report none, got \
                 {got}"
            ),
            Some(expected) => assert_eq!(
                actual,
                Some(expected),
                "case {case}: {member} must be reported as sent; expected {expected}, got \
                 {got}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateUsageProfile` persists is what `GetUsageProfile` reports: the create
/// answers with the name it stored, and every member of the profile the request carried
/// comes back from the store unchanged — an empty configuration map as an empty map, an
/// absent one as absent.
#[tokio::test]
async fn created_usage_profiles_are_read_back_from_the_store() {
    let router = emulator();

    let long_name = "n".repeat(MAX_NAME_LENGTH);
    let long_description = "d".repeat(MAX_DESCRIPTION_LENGTH);
    let long_value = "v".repeat(MAX_CONFIG_VALUE_LENGTH);
    let many_parameters: Value = Value::Object(
        (0..25)
            .map(|index| {
                (
                    format!("parameter-{index}"),
                    json!({ "DefaultValue": format!("{index}") }),
                )
            })
            .collect(),
    );
    // A `ConfigurationMap` key is a `NameString` too, so its length bound is the name's.
    let long_key_parameter: Value = Value::Object(
        [(long_name.clone(), json!({ "DefaultValue": "1" }))]
            .into_iter()
            .collect(),
    );

    let cases: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: one job parameter and nothing else.
            "one job parameter",
            json!({
                "Name": "etl-team",
                "Configuration": {
                    "JobConfiguration": {
                        "--enable-metrics": { "DefaultValue": "true" },
                    },
                },
            }),
        ),
        (
            "an empty job configuration beside a populated session configuration",
            json!({
                "Name": "empty-job-config",
                "Configuration": {
                    "JobConfiguration": {},
                    "SessionConfiguration": {
                        "idle_timeout": { "DefaultValue": "30", "MinValue": "1", "MaxValue": "60" },
                    },
                },
            }),
        ),
        (
            "an empty session configuration and no job configuration at all",
            json!({
                "Name": "empty-session-config",
                "Configuration": { "SessionConfiguration": {} },
            }),
        ),
        (
            "both configuration maps empty",
            json!({
                "Name": "both-empty",
                "Configuration": { "JobConfiguration": {}, "SessionConfiguration": {} },
            }),
        ),
        (
            "both configuration maps absent",
            json!({ "Name": "both-absent", "Configuration": {} }),
        ),
        (
            "a parameter constrained by allowed values",
            json!({
                "Name": "allowed-values",
                "Description": "analysts",
                "Configuration": {
                    "SessionConfiguration": {
                        "--worker-type": {
                            "DefaultValue": "G.1X",
                            "AllowedValues": ["G.1X", "G.2X", "G.4X"],
                        },
                    },
                },
            }),
        ),
        (
            "every ConfigurationObject member filled, at the modelled value length",
            json!({
                "Name": "full-configuration-object",
                "Configuration": {
                    "JobConfiguration": {
                        "max_retries": {
                            "DefaultValue": long_value,
                            "AllowedValues": [long_value],
                            "MinValue": "0",
                            "MaxValue": long_value,
                        },
                    },
                },
            }),
        ),
        (
            "many job parameters in one map",
            json!({
                "Name": "many-parameters",
                "Configuration": { "JobConfiguration": many_parameters },
            }),
        ),
        (
            "an empty description, which the modelled length range admits",
            json!({
                "Name": "empty-description",
                "Description": "",
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
            }),
        ),
        (
            "a description at the modelled length bound",
            json!({
                "Name": "long-description",
                "Description": long_description,
                "Configuration": { "SessionConfiguration": { "k": { "MinValue": "1" } } },
            }),
        ),
        (
            "a name at the modelled length bound, and a parameter key at the same bound",
            json!({
                "Name": long_name,
                "Configuration": { "JobConfiguration": long_key_parameter },
            }),
        ),
        (
            "a name carrying spaces and a tab",
            json!({
                "Name": "profile one\ttwo",
                "Configuration": { "SessionConfiguration": { "two words": {} } },
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "tagged-profile",
                "Description": "owned by data platform",
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "2" } } },
                "Tags": { "owner": "data-platform", "tier": "gold" },
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // "The name of the usage profile that was created": the create reports the name it
        // stored, which is what a later read addresses the profile by.
        let created = create(&router, request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateUsageProfile must report the stored name, got {created}"
        );

        let read = get(&router, name).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: GetUsageProfile must report the stored name, got {read}"
        );
        assert_eq!(
            read.get("Description").filter(|value| !value.is_null()),
            request.get("Description"),
            "case {case}: the description must be reported as sent, got {read}"
        );
        assert_configuration(
            case,
            &request["Configuration"],
            read.get("Configuration").unwrap_or(&Value::Null),
        );

        // `GetUsageProfileResponse` models no `Tags` member, so the tags a create carried
        // are not part of the profile a read reports.
        assert!(
            read.get("Tags").is_none(),
            "case {case}: GetUsageProfile must report no Tags, got {read}"
        );

        // Both server-set instants are stamped, and a profile nothing has updated reports
        // the same instant for each.
        let created_on = read.get("CreatedOn").and_then(Value::as_f64);
        let last_modified_on = read.get("LastModifiedOn").and_then(Value::as_f64);
        assert!(
            created_on.is_some_and(|at| at > 0.0),
            "case {case}: expected a CreatedOn instant, got {read}"
        );
        assert_eq!(
            created_on, last_modified_on,
            "case {case}: a profile nothing has updated reports one instant for both \
             members, got {read}"
        );
    }

    // Each profile is its own record: reading one back after every create has landed still
    // reports that profile's own configuration, so one create never overwrote another, and
    // a second read reports the same instants as the first.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let first = get(&router, name).await;
        let second = get(&router, name).await;
        assert_configuration(
            case,
            &request["Configuration"],
            second.get("Configuration").unwrap_or(&Value::Null),
        );
        assert_eq!(
            first.get("CreatedOn"),
            second.get("CreatedOn"),
            "case {case}: two reads of one profile report the same CreatedOn, got {first} \
             then {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A profile name is unique per account: the second create under a name already taken is
/// refused with the modelled `AlreadyExistsException`, and the stored profile is left as
/// the first create wrote it.
#[tokio::test]
async fn a_duplicate_profile_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, a different configuration",
            json!({
                "Name": "duplicate-one",
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
            }),
            json!({
                "Name": "duplicate-one",
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "2" } } },
            }),
        ),
        (
            "same name, an empty configuration map where the first had none",
            json!({
                "Name": "duplicate-two",
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
            }),
            json!({
                "Name": "duplicate-two",
                "Configuration": {
                    "JobConfiguration": { "k": { "DefaultValue": "1" } },
                    "SessionConfiguration": {},
                },
            }),
        ),
        (
            "same name and the identical profile",
            json!({
                "Name": "duplicate-three",
                "Description": "kept",
                "Configuration": { "SessionConfiguration": {} },
            }),
            json!({
                "Name": "duplicate-three",
                "Description": "kept",
                "Configuration": { "SessionConfiguration": {} },
            }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;

        let (status, body) = call(&router, "CreateUsageProfile", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        // The stored profile is still the FIRST create's: the configuration map the
        // refused create added is not there.
        let name = first["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_configuration(
            case,
            &first["Configuration"],
            read.get("Configuration").unwrap_or(&Value::Null),
        );
    }
}

/// A read for a name this account holds no profile under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the requested name misses: never
/// created, a prefix of a stored name, a suffix of one, and a stored name in another case
/// — the model documents no case fold for a usage profile name, so a differently-cased
/// name is a different profile.
#[tokio::test]
async fn reading_an_unknown_usage_profile_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "StoredProfile",
            "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
        }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-profile"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Profile"),
        ("a stored name lowercased", "storedprofile"),
        ("a stored name uppercased", "STOREDPROFILE"),
    ] {
        let (status, body) = call(&router, "GetUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredProfile").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredProfile"),
        "case the stored name: expected the stored profile, got {read}"
    );
}

/// A parameter's own bounds have to admit the parameter's own values: a create whose
/// `ConfigurationObject` puts its `DefaultValue` outside the `MinValue`/`MaxValue` range,
/// or its minimum above its maximum, is refused with the modelled `InvalidInputException`
/// naming the parameter, the configuration section it arrived under and the bound it
/// broke — and nothing is stored, so the name is still free afterwards.
///
/// The rows differ along which bound the defect breaks, which section the map arrived
/// under, the form of the numbers (integer, fraction, negative), which bounds the
/// parameter sets at all, and how many parameters are incoherent at once.
#[tokio::test]
async fn an_incoherent_configuration_is_refused_and_stores_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a default above the maximum, with a minimum above it too",
            json!({
                "JobConfiguration": {
                    "workerType": { "DefaultValue": "G.1X", "AllowedValues": ["G.1X", "G.2X"] },
                    "numberOfWorkers": { "DefaultValue": "5", "MinValue": "10", "MaxValue": "1" },
                },
            }),
            "default value for parameter numberOfWorkers in JobConfiguration should be less \
             than or equal to max value 1.",
        ),
        (
            "a default above the maximum, no minimum set",
            json!({ "JobConfiguration": { "timeout": { "DefaultValue": "120", "MaxValue": "60" } } }),
            "default value for parameter timeout in JobConfiguration should be less than or \
             equal to max value 60.",
        ),
        (
            "a fractional default above a fractional maximum, in the session section",
            json!({
                "SessionConfiguration": {
                    "max-capacity": { "DefaultValue": "0.125", "MaxValue": "0.0625" },
                },
            }),
            "default value for parameter max-capacity in SessionConfiguration should be less \
             than or equal to max value 0.0625.",
        ),
        (
            "a default below the minimum, in the session section",
            json!({
                "SessionConfiguration": {
                    "idle_timeout": { "DefaultValue": "1", "MinValue": "30", "MaxValue": "60" },
                },
            }),
            "default value for parameter idle_timeout in SessionConfiguration should be \
             greater than or equal to min value 30.",
        ),
        (
            "a negative default below the minimum",
            json!({
                "JobConfiguration": {
                    "offset": { "DefaultValue": "-1", "MinValue": "0", "MaxValue": "100" },
                },
            }),
            "default value for parameter offset in JobConfiguration should be greater than \
             or equal to min value 0.",
        ),
        (
            "a minimum above the maximum, with no default at all",
            json!({ "JobConfiguration": { "workers": { "MinValue": "10", "MaxValue": "1" } } }),
            "min value for parameter workers in JobConfiguration should be less than or \
             equal to max value 1.",
        ),
        (
            "a negative maximum below a positive minimum",
            json!({
                "SessionConfiguration": { "drift": { "MinValue": "0.5", "MaxValue": "-0.5" } },
            }),
            "min value for parameter drift in SessionConfiguration should be less than or \
             equal to max value -0.5.",
        ),
        (
            "a coherent job parameter beside an incoherent session one",
            json!({
                "JobConfiguration": {
                    "retries": { "DefaultValue": "1", "MinValue": "1", "MaxValue": "2" },
                },
                "SessionConfiguration": { "ttl": { "DefaultValue": "3", "MaxValue": "2" } },
            }),
            "default value for parameter ttl in SessionConfiguration should be less than or \
             equal to max value 2.",
        ),
        (
            "two incoherent parameters in one map, the first by name reported",
            json!({
                "JobConfiguration": {
                    "b-param": { "DefaultValue": "9", "MaxValue": "1" },
                    "a-param": { "DefaultValue": "8", "MaxValue": "2" },
                },
            }),
            "default value for parameter a-param in JobConfiguration should be less than or \
             equal to max value 2.",
        ),
    ];

    for (index, (case, configuration, expected)) in cases.iter().enumerate() {
        let name = format!("incoherent-{index}");
        let (status, body) = call(
            &router,
            "CreateUsageProfile",
            &json!({ "Name": name, "Configuration": configuration }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an incoherent configuration must be refused with 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            *expected,
            "case {case}: expected the refusal to name the parameter, its section and the \
             bound; got {body}"
        );

        // A refused create leaves nothing behind: the name holds no profile, so the read
        // misses, and a later create under the same name is free to succeed.
        let (status, body) = call(&router, "GetUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused name must hold no profile, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("UsageProfile {name} does not exists"),
            "case {case}: expected the miss to name the requested profile, got {body}"
        );

        create(
            &router,
            &json!({
                "Name": name,
                "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
            }),
        )
        .await;
    }
}

/// A configuration whose bounds DO admit its values is stored: the comparison is numeric
/// and inclusive, so a value sitting exactly on a bound is accepted, and a parameter whose
/// values are names rather than quantities carries no range to compare at all — what
/// restricts it is its `AllowedValues` list.
#[tokio::test]
async fn a_coherent_configuration_is_stored() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a default inside its range",
            json!({
                "JobConfiguration": {
                    "numberOfWorkers": { "DefaultValue": "5", "MinValue": "2", "MaxValue": "10" },
                },
            }),
        ),
        (
            "a default sitting exactly on the maximum",
            json!({ "JobConfiguration": { "p": { "DefaultValue": "10", "MaxValue": "10" } } }),
        ),
        (
            "a default sitting exactly on the minimum",
            json!({ "SessionConfiguration": { "p": { "DefaultValue": "2", "MinValue": "2" } } }),
        ),
        (
            "a single-point range the default sits on",
            json!({
                "JobConfiguration": {
                    "p": { "DefaultValue": "1.0", "MinValue": "1", "MaxValue": "1.00" },
                },
            }),
        ),
        (
            "fractional bounds the default lies between",
            json!({
                "SessionConfiguration": {
                    "max-capacity": {
                        "DefaultValue": "0.125",
                        "MinValue": "0.0625",
                        "MaxValue": "1",
                    },
                },
            }),
        ),
        (
            "negative bounds the default lies between",
            json!({
                "JobConfiguration": {
                    "offset": { "DefaultValue": "-1", "MinValue": "-10", "MaxValue": "0" },
                },
            }),
        ),
        (
            // `ConfigValueString` is `^[a-zA-Z0-9_.-]+$` (`smithy_query
            // shape=ConfigurationObject`), so a bound may be a NAME; names are not
            // ordered, and a worker type reading as "above" another is not a defect.
            "bounds that are names rather than quantities",
            json!({
                "JobConfiguration": {
                    "workerType": {
                        "DefaultValue": "G.2X",
                        "MinValue": "G.1X",
                        "MaxValue": "G.1X",
                        "AllowedValues": ["G.1X", "G.2X"],
                    },
                },
            }),
        ),
        (
            // Under that same pattern these are names too, not the floating-point
            // infinities a number parser would read them as.
            "bounds spelled like a float parser's specials",
            json!({
                "SessionConfiguration": {
                    "p": { "DefaultValue": "inf", "MinValue": "NaN", "MaxValue": "1" },
                    "q": { "DefaultValue": "1e9", "MaxValue": "1" },
                },
            }),
        ),
        (
            "a parameter that sets no bound at all",
            json!({ "JobConfiguration": { "--enable-metrics": { "DefaultValue": "true" } } }),
        ),
        (
            "an empty configuration object",
            json!({ "JobConfiguration": { "p": {} }, "SessionConfiguration": {} }),
        ),
        ("no configuration map at all", json!({})),
    ];

    for (index, (case, configuration)) in cases.iter().enumerate() {
        let name = format!("coherent-{index}");
        let request = json!({ "Name": name, "Configuration": configuration });
        let created = create(&router, &request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case}: the create must report the stored name, got {created}"
        );

        let read = get(&router, &name).await;
        assert_configuration(
            case,
            configuration,
            read.get("Configuration").unwrap_or(&Value::Null),
        );
    }
}

/// The refusals this resource answers name the profile the request addressed: a duplicate
/// create and a read that misses both carry the requested `Name`, so a client handling a
/// batch can tell WHICH name the refusal is about. The rows differ along the form of the
/// name — a plain one, one carrying the spaces and the tab `NameString` admits, and one at
/// the modelled length bound.
#[tokio::test]
async fn a_usage_profile_refusal_names_the_profile() {
    let router = emulator();

    let long_name = "p".repeat(MAX_NAME_LENGTH);
    let names: Vec<(&str, String)> = vec![
        ("a plain name", "named-profile".to_string()),
        ("a name carrying spaces and a tab", "profile one\ttwo".to_string()),
        ("a name at the modelled length bound", long_name),
    ];

    for (case, name) in &names {
        // The miss comes first: the name holds nothing yet, so the read reports it as the
        // profile that does not exist.
        let (status, body) = call(&router, "GetUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("UsageProfile {name} does not exists"),
            "case {case}: expected the miss to name {name:?}, got {body}"
        );

        let request = json!({
            "Name": name,
            "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
        });
        create(&router, &request).await;

        let (status, body) = call(&router, "CreateUsageProfile", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for the duplicate {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("UsageProfile {name} already exists."),
            "case {case}: expected the duplicate refusal to name {name:?}, got {body}"
        );
    }

    // One name's refusal is not another's: every message above carried its own name, so
    // two duplicates of different names cannot read the same.
    let first = call(
        &router,
        "CreateUsageProfile",
        &json!({ "Name": names[0].1, "Configuration": {} }),
    )
    .await;
    let second = call(
        &router,
        "CreateUsageProfile",
        &json!({ "Name": names[1].1, "Configuration": {} }),
    )
    .await;
    assert_ne!(
        error_message(&first.1),
        error_message(&second.1),
        "case two duplicates of different names: each refusal must name its own profile, \
         got {} and {}",
        error_message(&first.1),
        error_message(&second.1)
    );
}

/// `DeleteUsageProfile` removes the record its name addresses and nothing else: the read
/// that followed it misses, the other profiles are still there, the name is free for a
/// fresh create, and a delete of a name this account holds no profile under changes
/// nothing. The rows differ along whether the name holds a profile, whether it ever held
/// one, and the form of the name.
#[tokio::test]
async fn deleting_a_usage_profile_removes_only_that_profile() {
    let router = emulator();

    let kept = json!({
        "Name": "kept-profile",
        "Description": "left alone",
        "Configuration": { "JobConfiguration": { "k": { "DefaultValue": "1" } } },
    });
    create(&router, &kept).await;

    let stored: Vec<(&str, String)> = vec![
        ("a plain name", "deleted-profile".to_string()),
        ("a name carrying spaces and a tab", "delete one\ttwo".to_string()),
        ("a name at the modelled length bound", "d".repeat(MAX_NAME_LENGTH)),
    ];

    for (case, name) in &stored {
        create(
            &router,
            &json!({
                "Name": name,
                "Configuration": { "SessionConfiguration": { "k": { "MaxValue": "9" } } },
            }),
        )
        .await;

        // The delete answers an empty body: `DeleteUsageProfileResponse` models no member.
        let (status, body) = call(&router, "DeleteUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: the delete must answer 200, got {status} with {body}"
        );

        let (status, body) = call(&router, "GetUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the deleted profile must be gone, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("UsageProfile {name} does not exists"),
            "case {case}: expected the miss to name {name:?}, got {body}"
        );

        // The name is free again, so the create that was a duplicate before now succeeds.
        create(&router, &json!({ "Name": name, "Configuration": {} })).await;
    }

    for (case, name) in [
        ("a name that never held a profile", "no-such-profile"),
        ("a prefix of a stored name", "kept"),
    ] {
        let (status, body) = call(&router, "DeleteUsageProfile", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: deleting nothing must answer 200, got {status} with {body}"
        );
    }

    // The profile no delete addressed is still the one its create stored.
    let read = get(&router, "kept-profile").await;
    assert_eq!(
        read.get("Description").and_then(Value::as_str),
        Some("left alone"),
        "case the untouched profile: expected the stored profile, got {read}"
    );
    assert_configuration(
        "the untouched profile",
        &kept["Configuration"],
        read.get("Configuration").unwrap_or(&Value::Null),
    );
}

/// The store holds a bounded number of usage profiles: the create at the ceiling
/// succeeds, the one past it is refused with the modelled limit error, and freeing a slot
/// lets a create succeed again — so the ceiling bounds what is stored rather than latching
/// on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_usage_profile_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_USAGE_PROFILES - 1 {
            let name = format!("seeded-{index}");
            let input = CreateUsageProfileInput {
                name: name.clone(),
                ..CreateUsageProfileInput::default()
            };
            store.usage_profiles.insert(
                name,
                UsageProfileEntry::from_create_input(&input, std::time::SystemTime::now()),
            );
        }
    }

    let last = json!({ "Name": "last-profile", "Configuration": {} });
    let past = json!({ "Name": "one-too-many", "Configuration": {} });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateUsageProfile", &past).await;
    assert_eq!(
        status, 400,
        "case profile {} of {MAX_USAGE_PROFILES}: expected 400, got {status} with {body}",
        MAX_USAGE_PROFILES + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one profile too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().usage_profiles.remove("seeded-0");
    }
    create(&router, &past).await;
}
