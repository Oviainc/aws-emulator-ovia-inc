// SPDX-License-Identifier: CC0-1.0

//! `CreateExperimentDefinition` and the read that proves it persisted,
//! `GetExperimentDefinition`, end to end over HTTP.
//!
//! The rules under test:
//!
//! * a create resolves each `Identifier` the request carries (application, environment,
//!   configuration profile) by the resource's ID **or** its name, stores a definition
//!   under the resolved application with a freshly minted `Id`, and reports the record it
//!   stored — so a later `GetExperimentDefinition`, addressed by that id or by the
//!   definition's name, answers exactly the same record;
//! * the service mints each treatment's `Key` (`TreatmentInput` carries none,
//!   `Treatment` does): the control is `c` and the compared treatments are `t1`, `t2`, …
//!   in the order the request listed them, every key inside the model's
//!   `^[a-zA-Z0-9]{1,8}$`;
//! * a definition nothing has run yet is `IDLE`, and its `UpdatedAt` is its `CreatedAt`;
//! * a definition's name addresses it, so two definitions under one application cannot
//!   share a name: the second create is HTTP 409 `ConflictException` and writes nothing,
//!   while the same name under a different application is free;
//! * an identifier that resolves to no resource is HTTP 404
//!   `ResourceNotFoundException` and persists nothing.
//!
//! The tables span the class each rule covers rather than one recorded request: the
//! number of treatments across the modelled `TreatmentInputList` bounds (1..5), which
//! optional descriptions are present and their lengths at the published `Description`
//! bounds (0..1024), `Name` at both `Name` bounds and in non-ASCII text, `AudienceRule`
//! at its 1-character bound, flag values that are enabled and disabled, with and without
//! attribute values of every `AttributeValue` variant, and treatment weights from zero
//! upwards.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_flags_profile, is_modelled_id, json_call,
    router, Answer,
};
use crate::resource_tags::pct;

/// One treatment as `TreatmentInput` models it: a weight, an optional description and a
/// required flag value.
pub(crate) fn treatment(weight: f64, description: Option<&str>, flag_value: Value) -> Value {
    let mut input = json!({ "Weight": weight, "FlagValue": flag_value });
    if let Some(description) = description {
        input["Description"] = Value::from(description);
    }
    input
}

/// A flag value with no attributes.
pub(crate) fn flag(enabled: bool) -> Value {
    json!({ "Enabled": enabled })
}

/// A flag value carrying one attribute of each `AttributeValue` variant the model lists
/// (`StringValue` | `NumberValue` | `BooleanValue` | `StringArray` | `NumberArray`), under
/// keys of the shape `AttributeKey` allows (`^(?!enabled$)[a-z][a-zA-Z0-9_-]{0,63}$`).
fn flag_with_every_attribute_variant(enabled: bool) -> Value {
    json!({
        "Enabled": enabled,
        "AttributeValues": {
            "greeting": { "StringValue": "hello" },
            "ratio": { "NumberValue": 0.25 },
            "beta": { "BooleanValue": true },
            "regions": { "StringArray": ["us-east-1", "eu-west-1"] },
            "thresholds": { "NumberArray": [1.5, 2.5, 3.5] },
        },
    })
}

/// The scaffolding one definition needs: an application, an environment and a
/// configuration profile under it.
pub(crate) struct Scope {
    pub(crate) application: String,
    pub(crate) environment: String,
    pub(crate) profile: String,
}

pub(crate) async fn scope(router: &MultiServiceRouter, label: &str) -> Scope {
    let application = create_application(router, &format!("app-{label}")).await;
    let environment = create_environment(router, &application, &format!("env-{label}")).await;
    let profile = create_flags_profile(router, &application, &format!("profile-{label}")).await;
    Scope {
        application,
        environment,
        profile,
    }
}

/// `POST` an experiment definition under `application`.
pub(crate) async fn create_definition(
    router: &MultiServiceRouter,
    application: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/applications/{}/experimentdefinitions",
            pct(application)
        ),
        body,
    )
    .await
}

/// `GET` an experiment definition.
pub(crate) async fn get_definition(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{}/experimentdefinitions/{}",
            pct(application),
            pct(definition)
        ),
        &Value::Null,
    )
    .await
}

/// A request body with every required member set, so a row can vary one part of it.
/// `pub(crate)` because more than one suite needs a definition that differs from a valid
/// one in exactly one member.
pub(crate) fn body(scope: &Scope, name: &str, treatments: Vec<Value>, control: Value) -> Value {
    json!({
        "Name": name,
        "ConfigurationProfileIdentifier": scope.profile,
        "EnvironmentIdentifier": scope.environment,
        "FlagKey": "checkout-flow",
        "AudienceRule": "attribute(\"tier\") == \"beta\"",
        "Treatments": treatments,
        "Control": control,
    })
}

/// A number member of a JSON object, for the float members strict JSON equality would
/// judge by their written form rather than their value.
pub(crate) fn number(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// Every create persists the record it answers: the ids it resolved, the members the
/// request carried, the status a definition nothing has run yet holds, and a key per
/// treatment — and `GetExperimentDefinition` reports that same record.
#[tokio::test]
async fn an_experiment_definition_round_trips_across_the_modelled_input_range() {
    let router = router();
    let scope = scope(&router, "round-trip").await;

    let long_description = "d".repeat(1024);
    let long_name = "n".repeat(64);
    let long_rule = format!("attribute(\"tier\") == \"{}\"", "x".repeat(1000));

    // Rows differ along the number of treatments (1 through the modelled maximum of 5),
    // which optional descriptions the request carries and their length at the published
    // `Description` bounds (0 and 1024), the `Name` length at both `Name` bounds (1..64)
    // and in non-ASCII text, the `AudienceRule` length at its 1-character bound, the
    // `FlagKey` form, whether a flag value is enabled, whether it carries attribute
    // values, and the treatment weights.
    let cases: Vec<(&str, Value)> = vec![
        (
            "one treatment, nothing optional",
            json!({
                "Name": "minimal",
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "a-flag",
                "AudienceRule": "t",
                "Treatments": [treatment(100.0, None, flag(true))],
                "Control": treatment(0.0, None, flag(false)),
            }),
        ),
        (
            "two treatments and every optional description",
            json!({
                "Name": "two-treatments",
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "checkout-flow",
                "AudienceRule": "attribute(\"tier\") == \"beta\"",
                "Hypothesis": "the new flow converts better",
                "AudienceDescription": "beta testers only",
                "LaunchCriteria": "a 5% lift over the control",
                "Treatments": [
                    treatment(25.5, Some("half of the beta cohort"), flag(true)),
                    treatment(24.5, Some("the other half"), flag_with_every_attribute_variant(true)),
                ],
                "Control": treatment(50.0, Some("today's experience"), flag(false)),
            }),
        ),
        (
            "five treatments, the modelled maximum",
            json!({
                "Name": "five-treatments",
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "pricing_v2-experiment",
                "AudienceRule": "true",
                "Treatments": (1..=5)
                    .map(|index| treatment(
                        f64::from(index) * 1.5,
                        Some(&format!("variation {index}")),
                        flag(index % 2 == 0),
                    ))
                    .collect::<Vec<Value>>(),
                "Control": treatment(0.0, None, flag_with_every_attribute_variant(false)),
            }),
        ),
        (
            "descriptions at the 0-character bound",
            json!({
                "Name": "e",
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "empty-descriptions",
                "AudienceRule": "t",
                "Hypothesis": "",
                "AudienceDescription": "",
                "LaunchCriteria": "",
                "Treatments": [treatment(0.0, Some(""), flag(true))],
                "Control": treatment(0.0, Some(""), flag(true)),
            }),
        ),
        (
            "descriptions at the 1024-character bound and a 64-character name",
            json!({
                "Name": long_name,
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "long-text",
                "AudienceRule": long_rule,
                "Hypothesis": long_description,
                "AudienceDescription": long_description,
                "LaunchCriteria": long_description,
                "Treatments": [treatment(99.75, Some(&long_description), flag(true))],
                "Control": treatment(0.25, Some(&long_description), flag(false)),
            }),
        ),
        (
            "non-ASCII name and descriptions",
            json!({
                "Name": "실험-definición-実験",
                "ConfigurationProfileIdentifier": scope.profile,
                "EnvironmentIdentifier": scope.environment,
                "FlagKey": "unicode-flag",
                "AudienceRule": "attribute(\"país\") == \"españa\"",
                "Hypothesis": "ユーザーの転換率が上がる",
                "AudienceDescription": "βeta — \"quoted\" & <escaped>",
                "LaunchCriteria": "🚀 ship it",
                "Treatments": [treatment(50.0, Some("変化"), flag(true))],
                "Control": treatment(50.0, Some("対照"), flag(false)),
            }),
        ),
    ];

    for (case, request) in &cases {
        let created = create_definition(&router, &scope.application, request).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExperimentDefinition expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let answered = created.json();

        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {answered}"));
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected an Id matching ^[a-z0-9]{{4,7}}$, got {id:?}"
        );

        // The ids a create REPORTS are the resolved resources', never the identifiers the
        // request happened to be written with.
        for (member, expected) in [
            ("ApplicationId", &scope.application),
            ("EnvironmentId", &scope.environment),
            ("ConfigurationProfileId", &scope.profile),
        ] {
            assert_eq!(
                created.string(member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member} {expected:?}, got {:?}",
                created.string(member)
            );
        }

        // Each member the request carried comes back as written; a member it omitted is
        // not reported at all.
        for member in [
            "Name",
            "FlagKey",
            "AudienceRule",
            "Hypothesis",
            "AudienceDescription",
            "LaunchCriteria",
        ] {
            assert_eq!(
                answered.get(member),
                request.get(member),
                "case {case:?}: expected {member} {:?}, got {:?}",
                request.get(member),
                answered.get(member)
            );
        }

        // Nothing has run this definition, so it is IDLE and was last updated when it was
        // created.
        assert_eq!(
            created.string("Status").as_deref(),
            Some("IDLE"),
            "case {case:?}: expected Status IDLE, got {:?}",
            created.string("Status")
        );
        assert_eq!(
            answered.get("CreatedAt"),
            answered.get("UpdatedAt"),
            "case {case:?}: expected UpdatedAt to equal CreatedAt, got {:?} and {:?}",
            answered.get("CreatedAt"),
            answered.get("UpdatedAt")
        );
        assert!(
            answered.get("CreatedAt").is_some(),
            "case {case:?}: expected a CreatedAt, got {answered}"
        );
        // The create carries no `KmsKeyIdentifier` member, so the definition reports none.
        assert!(
            answered.get("KmsKeyIdentifier").is_none(),
            "case {case:?}: expected no KmsKeyIdentifier, got {:?}",
            answered.get("KmsKeyIdentifier")
        );

        // The service keys the treatments: the control is `c`, the compared treatments are
        // `t1`.. in request order, and every treatment keeps its own weight, description
        // and flag value.
        let requested_control = request
            .get("Control")
            .unwrap_or_else(|| panic!("case {case:?}: the request must carry a Control"));
        let control = answered
            .get("Control")
            .unwrap_or_else(|| panic!("case {case:?}: no Control in {answered}"));
        assert_eq!(
            control.get("Key").and_then(Value::as_str),
            Some("c"),
            "case {case:?}: expected the control key c, got {:?}",
            control.get("Key")
        );
        assert_treatment_body(case, "Control", requested_control, control);

        let requested_treatments = request
            .get("Treatments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the request must carry Treatments"));
        let treatments = answered
            .get("Treatments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no Treatments in {answered}"));
        assert_eq!(
            treatments.len(),
            requested_treatments.len(),
            "case {case:?}: expected {} treatments, got {}",
            requested_treatments.len(),
            treatments.len()
        );
        for (index, (requested, answered_treatment)) in
            requested_treatments.iter().zip(treatments).enumerate()
        {
            let expected_key = format!("t{}", index + 1);
            let key = answered_treatment.get("Key").and_then(Value::as_str);
            assert_eq!(
                key,
                Some(expected_key.as_str()),
                "case {case:?}: expected treatment {index} to be keyed {expected_key:?}, got {key:?}"
            );
            let key = key.unwrap_or_default();
            assert!(
                (1..=8).contains(&key.chars().count())
                    && key.chars().all(|c| c.is_ascii_alphanumeric()),
                "case {case:?}: expected a Key matching ^[a-zA-Z0-9]{{1,8}}$, got {key:?}"
            );
            assert_treatment_body(case, &expected_key, requested, answered_treatment);
        }

        // The write is only real if a later read sees it — addressed by the minted id and
        // by the definition's name, which the model documents as equally addressing it.
        let name = created
            .string("Name")
            .unwrap_or_else(|| panic!("case {case:?}: no Name in {answered}"));
        for (addressed_by, identifier) in [("id", id.clone()), ("name", name)] {
            let fetched = get_definition(&router, &scope.application, &identifier).await;
            assert_eq!(
                fetched.status,
                200,
                "case {case:?}: GetExperimentDefinition by {addressed_by} expected 200, got {} / {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert_eq!(
                fetched.json(),
                answered,
                "case {case:?}: GetExperimentDefinition by {addressed_by} must report what the \
                 create stored, got {}",
                fetched.json()
            );
        }

        // A read changes nothing a later caller sees.
        let again = get_definition(&router, &scope.application, &id).await;
        assert_eq!(
            again.json(),
            answered,
            "case {case:?}: a second read must report the same record, got {}",
            again.json()
        );
    }
}

/// Every member of a treatment except the key the service minted is the caller's.
pub(crate) fn assert_treatment_body(case: &str, label: &str, requested: &Value, answered: &Value) {
    assert_eq!(
        number(answered, "Weight"),
        number(requested, "Weight"),
        "case {case:?}: expected {label} weight {:?}, got {:?}",
        number(requested, "Weight"),
        number(answered, "Weight")
    );
    assert_eq!(
        answered.get("Description"),
        requested.get("Description"),
        "case {case:?}: expected {label} description {:?}, got {:?}",
        requested.get("Description"),
        answered.get("Description")
    );
    let requested_flag = requested
        .get("FlagValue")
        .unwrap_or_else(|| panic!("case {case:?}: {label} must carry a FlagValue"));
    let answered_flag = answered
        .get("FlagValue")
        .unwrap_or_else(|| panic!("case {case:?}: {label} answered no FlagValue"));
    assert_eq!(
        answered_flag.get("Enabled"),
        requested_flag.get("Enabled"),
        "case {case:?}: expected {label} Enabled {:?}, got {:?}",
        requested_flag.get("Enabled"),
        answered_flag.get("Enabled")
    );
    let requested_attributes = requested_flag.get("AttributeValues");
    let answered_attributes = answered_flag.get("AttributeValues");
    match (requested_attributes, answered_attributes) {
        (None, None) => {}
        (Some(requested_attributes), Some(answered_attributes)) => {
            let requested_attributes = requested_attributes
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: {label} AttributeValues must be a map"));
            let answered_attributes = answered_attributes
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: {label} AttributeValues must be a map"));
            assert_eq!(
                answered_attributes.len(),
                requested_attributes.len(),
                "case {case:?}: expected {} attribute values on {label}, got {}",
                requested_attributes.len(),
                answered_attributes.len()
            );
            for (key, requested_value) in requested_attributes {
                let answered_value = answered_attributes
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: {label} lost attribute {key:?}"));
                let (requested_variant, requested_payload) = sole_member(case, requested_value);
                let (answered_variant, answered_payload) = sole_member(case, answered_value);
                assert_eq!(
                    answered_variant, requested_variant,
                    "case {case:?}: expected attribute {key:?} of {label} as \
                     {requested_variant:?}, got {answered_variant:?}"
                );
                assert_eq!(
                    numeric_form(answered_payload),
                    numeric_form(requested_payload),
                    "case {case:?}: expected attribute {key:?} of {label} to be \
                     {requested_payload:?}, got {answered_payload:?}"
                );
            }
        }
        (requested_attributes, answered_attributes) => panic!(
            "case {case:?}: expected {label} AttributeValues {requested_attributes:?}, got \
             {answered_attributes:?}"
        ),
    }
}

/// The variant name and payload of a union, which restJson1 writes as a one-member
/// object.
fn sole_member<'value>(case: &str, union: &'value Value) -> (&'value str, &'value Value) {
    let object = union
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a union object, got {union}"));
    let mut members = object.iter();
    let (name, payload) = members
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one union member, got {union}"));
    assert!(
        members.next().is_none(),
        "case {case:?}: expected exactly one union member, got {union}"
    );
    (name.as_str(), payload)
}

/// `value` with every number rewritten as an `f64`, so two JSON numbers of the same value
/// compare equal however each was written (`2` and `2.0`).
fn numeric_form(value: &Value) -> Value {
    match value {
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| value.clone(), Value::Number),
        Value::Array(items) => Value::Array(items.iter().map(numeric_form).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), numeric_form(member)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.clone(),
    }
}

/// A definition's name addresses it, so a second definition cannot take a name already
/// in use under the same application: the create is 409 `ConflictException` and nothing
/// about the definition that holds the name changes. The same name under a DIFFERENT
/// application is free.
#[tokio::test]
async fn two_definitions_under_one_application_cannot_share_a_name() {
    let router = router();
    let first_scope = scope(&router, "names").await;
    let second_scope = scope(&router, "other-names").await;

    // Rows differ along the form of the contested name (short, at the 64-character bound,
    // non-ASCII, carrying spaces and punctuation) and along how much else the second
    // request changes.
    let long_name = "n".repeat(64);
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "identical request",
            "duplicate".to_string(),
            json!({ "FlagKey": "checkout-flow" }),
        ),
        (
            "64-character name, everything else different",
            long_name,
            json!({ "FlagKey": "another-flag", "Hypothesis": "a different hypothesis" }),
        ),
        (
            "non-ASCII name",
            "重複した名前".to_string(),
            json!({ "FlagKey": "unicode-flag" }),
        ),
        (
            "name with spaces and punctuation",
            "my experiment (v2)".to_string(),
            json!({ "FlagKey": "spaced-flag" }),
        ),
    ];

    for (case, name, overrides) in &cases {
        let first = create_definition(
            &router,
            &first_scope.application,
            &body(
                &first_scope,
                name,
                vec![treatment(50.0, None, flag(true))],
                treatment(50.0, None, flag(false)),
            ),
        )
        .await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: the first create expected 201, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let stored = first.json();
        let id = first
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {stored}"));

        let mut second = body(
            &first_scope,
            name,
            vec![treatment(10.0, Some("a second attempt"), flag(false))],
            treatment(90.0, None, flag(true)),
        );
        for (member, value) in overrides
            .as_object()
            .expect("the overrides must be a JSON object")
        {
            second[member] = value.clone();
        }
        let conflict = create_definition(&router, &first_scope.application, &second).await;
        assert_eq!(
            conflict.status,
            409,
            "case {case:?}: the duplicate create expected 409, got {} / {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert!(
            conflict.error_code().contains("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            conflict.error_code()
        );

        // The refused call wrote nothing: the name still resolves to the first record,
        // unchanged.
        let fetched = get_definition(&router, &first_scope.application, name).await;
        assert_eq!(
            fetched.json(),
            stored,
            "case {case:?}: a refused create must persist nothing, got {}",
            fetched.json()
        );
        assert_eq!(
            fetched.string("Id").as_deref(),
            Some(id.as_str()),
            "case {case:?}: the name must still resolve to the first definition, got {:?}",
            fetched.string("Id")
        );

        // A definition is scoped to its application, so the name is free under another.
        let elsewhere = create_definition(
            &router,
            &second_scope.application,
            &body(
                &second_scope,
                name,
                vec![treatment(50.0, None, flag(true))],
                treatment(50.0, None, flag(false)),
            ),
        )
        .await;
        assert_eq!(
            elsewhere.status,
            201,
            "case {case:?}: the same name under another application expected 201, got {} / {}",
            elsewhere.status,
            String::from_utf8_lossy(&elsewhere.body)
        );
        assert_ne!(
            elsewhere.string("Id"),
            first.string("Id"),
            "case {case:?}: the two definitions must be distinct resources"
        );

        // Neither definition is visible under the other's application.
        let crossed = get_definition(&router, &second_scope.application, &id).await;
        assert_eq!(
            crossed.status,
            404,
            "case {case:?}: a definition must not be readable under another application, got {} / {}",
            crossed.status,
            String::from_utf8_lossy(&crossed.body)
        );
    }
}

/// An identifier that resolves to no resource is 404 `ResourceNotFoundException`, on the
/// create and on the read, and the create persists nothing.
#[tokio::test]
async fn an_unresolvable_identifier_is_not_found() {
    let router = router();
    let scope = scope(&router, "not-found").await;
    let existing = create_definition(
        &router,
        &scope.application,
        &body(
            &scope,
            "already-here",
            vec![treatment(50.0, None, flag(true))],
            treatment(50.0, None, flag(false)),
        ),
    )
    .await;
    assert_eq!(
        existing.status,
        201,
        "the baseline create expected 201, got {} / {}",
        existing.status,
        String::from_utf8_lossy(&existing.body)
    );
    let existing_id = existing
        .string("Id")
        .expect("the baseline definition must carry an Id");

    // Rows differ along WHICH identifier is unresolvable (the application in the URI, the
    // environment, the configuration profile) and along the shape of the unresolvable
    // value: an id no resource holds, a near miss of a real one, a name that belongs to
    // another kind of resource, and an identifier of the application itself.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "unknown application",
            "nosuchapp".to_string(),
            scope.environment.clone(),
            scope.profile.clone(),
        ),
        (
            "near-miss application id",
            format!("{}x", scope.application),
            scope.environment.clone(),
            scope.profile.clone(),
        ),
        (
            "unknown environment",
            scope.application.clone(),
            "zzzzzzz".to_string(),
            scope.profile.clone(),
        ),
        (
            "environment identifier naming the profile",
            scope.application.clone(),
            scope.profile.clone(),
            scope.profile.clone(),
        ),
        (
            "unknown configuration profile",
            scope.application.clone(),
            scope.environment.clone(),
            "0000".to_string(),
        ),
        (
            "configuration profile identifier naming the environment",
            scope.application.clone(),
            scope.environment.clone(),
            scope.environment.clone(),
        ),
        (
            "every identifier unknown",
            "no-app".to_string(),
            "no-env".to_string(),
            "no-profile".to_string(),
        ),
    ];

    for (case, application, environment, profile) in &cases {
        let request = json!({
            "Name": format!("should-not-exist-{}", case.replace(' ', "-")),
            "ConfigurationProfileIdentifier": profile,
            "EnvironmentIdentifier": environment,
            "FlagKey": "checkout-flow",
            "AudienceRule": "true",
            "Treatments": [treatment(50.0, None, flag(true))],
            "Control": treatment(50.0, None, flag(false)),
        });
        let refused = create_definition(&router, application, &request).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );

        // Nothing was written: the name the refused call carried resolves to no record.
        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .expect("the request must carry a Name");
        let fetched = get_definition(&router, &scope.application, name).await;
        assert_eq!(
            fetched.status,
            404,
            "case {case:?}: a refused create must persist nothing, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
    }

    // The same rule on the read side: an unresolvable application or definition is 404.
    let read_cases: Vec<(&str, String, String)> = vec![
        (
            "unknown application",
            "nosuchapp".to_string(),
            existing_id.clone(),
        ),
        (
            "unknown definition id",
            scope.application.clone(),
            "zzzzzzz".to_string(),
        ),
        (
            "near-miss definition id",
            scope.application.clone(),
            format!("{existing_id}x"),
        ),
        (
            "definition identifier naming the application",
            scope.application.clone(),
            scope.application.clone(),
        ),
        (
            "a name no definition holds",
            scope.application.clone(),
            "never-created".to_string(),
        ),
    ];
    for (case, application, definition) in &read_cases {
        let fetched = get_definition(&router, application, definition).await;
        assert_eq!(
            fetched.status,
            404,
            "case {case:?}: GetExperimentDefinition expected 404, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert!(
            fetched.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.error_code()
        );
    }

    // The definition that does exist is untouched by any of it.
    let still_there = get_definition(&router, &scope.application, &existing_id).await;
    assert_eq!(
        still_there.json(),
        existing.json(),
        "the stored definition must be unchanged, got {}",
        still_there.json()
    );
}

/// Each `Identifier` the create carries resolves the resource by its ID or by its NAME,
/// and the record stores the resolved ids either way.
#[tokio::test]
async fn identifiers_resolve_a_resource_by_id_or_by_name() {
    let router = router();
    let application_name = "app-identifiers";
    let environment_name = "env-identifiers";
    let profile_name = "profile-identifiers";
    let application = create_application(&router, application_name).await;
    let environment = create_environment(&router, &application, environment_name).await;
    let profile = create_flags_profile(&router, &application, profile_name).await;

    // Rows span which of the three identifiers is written as a name rather than an id:
    // none, each one alone, and all three at once.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("every identifier an id", &application, &environment, &profile),
        (
            "application by name",
            application_name,
            &environment,
            &profile,
        ),
        (
            "environment by name",
            &application,
            environment_name,
            &profile,
        ),
        ("profile by name", &application, &environment, profile_name),
        (
            "every identifier a name",
            application_name,
            environment_name,
            profile_name,
        ),
    ];

    for (case, application_identifier, environment_identifier, profile_identifier) in &cases {
        let created = create_definition(
            &router,
            application_identifier,
            &json!({
                "Name": format!("definition-{}", case.replace(' ', "-")),
                "ConfigurationProfileIdentifier": profile_identifier,
                "EnvironmentIdentifier": environment_identifier,
                "FlagKey": "checkout-flow",
                "AudienceRule": "true",
                "Treatments": [treatment(50.0, None, flag(true))],
                "Control": treatment(50.0, None, flag(false)),
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        for (member, expected) in [
            ("ApplicationId", &application),
            ("EnvironmentId", &environment),
            ("ConfigurationProfileId", &profile),
        ] {
            assert_eq!(
                created.string(member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member} {expected:?}, got {:?}",
                created.string(member)
            );
        }

        // However it was addressed, the record is readable under the application's id.
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));
        let fetched = get_definition(&router, &application, &id).await;
        assert_eq!(
            fetched.json(),
            created.json(),
            "case {case:?}: the read must report what the create stored, got {}",
            fetched.json()
        );
    }
}
