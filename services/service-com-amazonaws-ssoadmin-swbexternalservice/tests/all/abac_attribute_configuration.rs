// SPDX-License-Identifier: CC0-1.0

//! The attribute-based access control (ABAC) attribute configuration of an IAM Identity
//! Center instance, end to end over the `awsJson1_1` wire.
//!
//! The rules under test:
//!
//! 1. ABAC is a feature of the ORGANIZATION instance of IAM Identity Center, so on an
//!    ACCOUNT instance every operation of the attribute-configuration surface
//!    (`Create`/`Describe`/`Update`/`DeleteInstanceAccessControlAttributeConfiguration`)
//!    is refused with the modelled `ValidationException` (HTTP 400) BEFORE any
//!    resource-existence check, and writes nothing — this is the recorded case;
//! 2. on an organization instance the surface ROUND TRIPS: a create stores the caller's
//!    attribute mapping and starts the enabling the model's `CREATION_IN_PROGRESS` status
//!    reports, a describe answers from that stored mapping attribute for attribute (and
//!    reports `ENABLED` once the enabling instant has passed), an update REPLACES the
//!    mapping, a delete removes it, and a create of a configuration that already exists
//!    is the modelled `ConflictException`;
//! 3. an `InstanceArn` the account holds no instance under is a missing resource
//!    (`ResourceNotFoundException`),
//!    and so is an instance on which ABAC was never enabled (or has been deleted);
//! 4. a mapping maps one key to one source, so a request carrying the same
//!    `AccessControlAttribute$Key` twice is refused with `ValidationException` and stores
//!    nothing, while keys differing only in case are distinct attributes;
//! 5. a request outside the modelled bounds of the configuration shapes is refused before
//!    the handler runs.
//!
//! The rows span what rule 2 generalises over: the NUMBER of attributes (the published
//! bounds 0 and 50 of `AccessControlAttributeList`), the key length (the published bounds
//! 1 and 128 of `AccessControlAttributeKey`), the key charset (`^[\p{L}\p{Z}\p{N}_.:\/=+\-@]+$`,
//! including a non-ASCII letter and a space), the source length (the published bounds 0
//! and 256 of `AccessControlAttributeValueSource`) and its charset (which also admits
//! `[]{}$\"`), and the ORDER the attributes are sent in. Every bound is tested at the
//! published value and one past it (rule 5). The constraints are those
//! `smithy_query(query="shape", shape="AccessControlAttribute,AccessControlAttributeKey,
//! AccessControlAttributeValueSource,AccessControlAttributeValueSourceList,
//! AccessControlAttributeList")` reports.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    self, ComAmazonawsSsoadminSwbexternalserviceState as Store,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router, router_over, seeded_instance_arn,
    UNKNOWN_INSTANCE_ARN,
};

/// The four operations of the surface, as `X-Amz-Target` names them.
const CREATE: &str = "CreateInstanceAccessControlAttributeConfiguration";
const DESCRIBE: &str = "DescribeInstanceAccessControlAttributeConfiguration";
const UPDATE: &str = "UpdateInstanceAccessControlAttributeConfiguration";
const DELETE: &str = "DeleteInstanceAccessControlAttributeConfiguration";

/// A source every row can be re-pointed to: an identity-store path, the form
/// `AccessControlAttributeValue$Source` documents ("The identity source to use when
/// mapping a specified attribute to IAM Identity Center").
const REPOINTED_SOURCE: &str = "${path:userName}";

/// One mapping the rules are asserted over: `(Key, Source)` pairs in the order sent.
struct Case {
    label: String,
    attributes: Vec<(String, String)>,
}

/// The rows: one mapping each, differing along the number of attributes, the key length
/// and charset, the source length and charset, and the order.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one attribute mapping a department path".to_string(),
            attributes: vec![(
                "Department".to_string(),
                "${path:enterprise.department}".to_string(),
            )],
        },
        Case {
            // `AccessControlAttributeList` is `length(min: 0, max: 50)`: the empty
            // mapping is the published minimum, and enabling ABAC with no attribute yet
            // is exactly what the create's "You can ALSO specify new attributes" allows.
            label: "the empty mapping, the published minimum of 0 attributes".to_string(),
            attributes: Vec::new(),
        },
        Case {
            label: "keys spanning the charset, in a non-alphabetical order".to_string(),
            attributes: vec![
                (
                    "cost-center.id:eu/west=1+x@acme".to_string(),
                    "${path:enterprise.costCenter}".to_string(),
                ),
                // `\p{L}` admits any Unicode letter and `\p{Z}` a space.
                (
                    "Abteilung Köln".to_string(),
                    "{\"literal\":\"Köln\"}".to_string(),
                ),
                // `AccessControlAttributeKey` is `length(min: 1, max: 128)`: one
                // character is its published minimum.
                ("A".to_string(), REPOINTED_SOURCE.to_string()),
            ],
        },
        Case {
            label: "the published bounds of the key and source lengths".to_string(),
            attributes: vec![
                // `AccessControlAttributeKey` is `length(min: 1, max: 128)`.
                ("k".repeat(128), "s".repeat(256)),
                // `AccessControlAttributeValueSource` is `length(min: 0, max: 256)`.
                ("single".to_string(), String::new()),
            ],
        },
        Case {
            label: "the published maximum of 50 attributes".to_string(),
            attributes: (0..50)
                .map(|index| (format!("attr-{index:02}"), format!("${{path:a{index}}}")))
                .collect(),
        },
        Case {
            // The key is an opaque identity-source attribute name, so these are two
            // attributes rather than one sent twice.
            label: "keys differing only in case".to_string(),
            attributes: vec![
                ("Dept".to_string(), "${path:a}".to_string()),
                ("dept".to_string(), "${path:b}".to_string()),
            ],
        },
    ]
}

/// The REPLACEMENT mapping an update sends for `case`: its attributes reversed, with the
/// first dropped and every source re-pointed, plus one key the create never carried — so
/// a describe that merged instead of replacing cannot pass.
fn updated_attributes(case: &Case) -> Vec<(String, String)> {
    let mut next: Vec<(String, String)> = case
        .attributes
        .iter()
        .rev()
        .skip(1)
        .map(|(key, _)| (key.clone(), REPOINTED_SOURCE.to_string()))
        .collect();
    next.push(("UpdatedOnly".to_string(), "${path:title}".to_string()));
    next
}

/// `AccessControlAttributes` as the wire carries it.
fn attributes_json(attributes: &[(String, String)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(key, source)| json!({"Key": key, "Value": {"Source": [source]}}))
            .collect(),
    )
}

/// The request body of a create or an update.
fn configuration_body(instance_arn: &str, attributes: &[(String, String)]) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "InstanceAccessControlAttributeConfiguration": {
            "AccessControlAttributes": attributes_json(attributes),
        },
    })
}

/// Read or write this service's store the way a handler does.
fn with_store<R>(engine: &Arc<engine::InMemoryEngine>, change: impl FnOnce(&mut Store) -> R) -> R {
    let service_state = state::service_state(&**engine).expect("the SSO Admin state store");
    let mut store = service_state.lock();
    change(&mut store)
}

/// Move the stored enabling instant of every ABAC configuration to `due_at`, so the
/// status the clock owes can be observed on both sides of it without the test sleeping.
/// The status itself is still derived by the service, exactly as in production.
fn hold_enablement(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            if let Some(configuration) = instance.access_control_attributes.as_mut() {
                configuration.enabled_at = due_at;
            }
        }
    });
}

/// A router whose account holds an ORGANIZATION instance, that instance's ARN, and the
/// engine behind it.
fn organization_router_with_engine() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// The `Status` and `AccessControlAttributes` a describe reports, as a 200 body.
async fn describe_ok(router: &MultiServiceRouter, instance_arn: &str) -> (String, Value) {
    let (status, body) = call(router, DESCRIBE, &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "{DESCRIBE} expected 200 for {instance_arn}, got {status} {body}"
    );
    (
        body["Status"].as_str().unwrap_or_default().to_string(),
        body["InstanceAccessControlAttributeConfiguration"]["AccessControlAttributes"].clone(),
    )
}

/// Assert `operation` answered `ResourceNotFoundException`, at the HTTP 400 this
/// awsJson1_1 service answers every refusal of a request with
/// (`crate::scaffolded::modeled_error_status`).
fn assert_not_found(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {label:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {label:?}: expected ResourceNotFoundException, got {body}"
    );
}

/// RULE 2 and RULE 3, over the whole table: the lifecycle round-trips on an organization
/// instance, and the configuration exists only between the create and the delete.
#[tokio::test]
async fn the_abac_configuration_round_trips_on_an_organization_instance() {
    for case in cases() {
        // One instance holds at most one ABAC configuration, so each row gets its own
        // account.
        let (router, instance_arn, engine) = organization_router_with_engine();
        let label = &case.label;
        let sent = attributes_json(&case.attributes);

        // Before the create there is no configuration to read.
        let (status, body) = call(&router, DESCRIBE, &json!({"InstanceArn": instance_arn})).await;
        assert_not_found(&format!("{label}: describe before create"), status, &body);

        let (status, body) = call(
            &router,
            CREATE,
            &configuration_body(&instance_arn, &case.attributes),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: {CREATE} expected 200, got {status} {body}"
        );

        // The enabling is asynchronous: held open, the configuration reports the
        // transient status the model declares, with the attributes already readable.
        hold_enablement(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let (reported_status, attributes) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            reported_status, "CREATION_IN_PROGRESS",
            "case {label:?}: an enabling still in flight reports CREATION_IN_PROGRESS, got \
             {reported_status:?}"
        );
        assert_eq!(
            attributes, sent,
            "case {label:?}: the describe must report the mapping the create sent, got \
             {attributes}"
        );

        // Past the enabling instant the clock owes it the terminal status.
        hold_enablement(&engine, SystemTime::now() - Duration::from_secs(1));
        let (reported_status, attributes) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            reported_status, "ENABLED",
            "case {label:?}: once the enabling is due the status is ENABLED, got \
             {reported_status:?}"
        );
        assert_eq!(
            attributes, sent,
            "case {label:?}: the mapping does not change when the status does, got \
             {attributes}"
        );

        // A second create is a conflict with the write that stored the mapping, and it
        // leaves that mapping alone.
        let (status, body) = call(
            &router,
            CREATE,
            &configuration_body(&instance_arn, &updated_attributes(&case)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a create over an existing configuration expected 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body}"
        );
        let (_, attributes) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            attributes, sent,
            "case {label:?}: a refused create must not change the stored mapping, got \
             {attributes}"
        );

        // The update REPLACES the mapping.
        let replacement = updated_attributes(&case);
        let (status, body) = call(
            &router,
            UPDATE,
            &configuration_body(&instance_arn, &replacement),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: {UPDATE} expected 200, got {status} {body}"
        );
        let (reported_status, attributes) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            attributes,
            attributes_json(&replacement),
            "case {label:?}: the describe must report the mapping the update sent, got \
             {attributes}"
        );
        assert_eq!(
            reported_status, "ENABLED",
            "case {label:?}: an update does not re-open the enabling, got \
             {reported_status:?}"
        );

        // The delete disables ABAC: the configuration is gone, and every operation that
        // needs one says so.
        let (status, body) = call(&router, DELETE, &json!({"InstanceArn": instance_arn})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {DELETE} expected 200, got {status} {body}"
        );
        for operation in [DESCRIBE, DELETE] {
            let (status, body) =
                call(&router, operation, &json!({"InstanceArn": instance_arn})).await;
            assert_not_found(&format!("{label}: {operation} after the delete"), status, &body);
        }
        let (status, body) = call(
            &router,
            UPDATE,
            &configuration_body(&instance_arn, &replacement),
        )
        .await;
        assert_not_found(&format!("{label}: {UPDATE} after the delete"), status, &body);

        // And ABAC can be enabled again, which is what makes the delete a delete rather
        // than a tombstone.
        let (status, body) = call(
            &router,
            CREATE,
            &configuration_body(&instance_arn, &case.attributes),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a create after the delete expected 200, got {status} {body}"
        );
        let (_, attributes) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            attributes, sent,
            "case {label:?}: the re-created mapping is the one just sent, got {attributes}"
        );
    }
}

/// RULE 1, over the whole table: on an ACCOUNT instance every operation of the surface
/// answers 400 `ValidationException`, whatever the request carries and whether or not a
/// configuration could exist.
#[tokio::test]
async fn the_surface_is_refused_on_an_account_instance() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for case in cases() {
        let label = &case.label;
        let requests: Vec<(&str, Value)> = vec![
            (CREATE, configuration_body(&instance_arn, &case.attributes)),
            (DESCRIBE, json!({"InstanceArn": instance_arn})),
            (
                UPDATE,
                configuration_body(&instance_arn, &updated_attributes(&case)),
            ),
            (DELETE, json!({"InstanceArn": instance_arn})),
        ];
        for (operation, body) in requests {
            let (status, body) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label:?}/{operation}: expected HTTP 400 on an account instance, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}/{operation}: expected ValidationException, got {body}"
            );
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("account instances"),
                "case {label:?}/{operation}: the refusal must say the operation is \
                 unsupported for account instances, got {message:?}"
            );
        }
    }

    // The refusal is a READ: nothing it touched is visible to a later caller.
    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        after["Instances"][0]["InstanceArn"].as_str(),
        Some(instance_arn.as_str()),
        "a refused call must change nothing a later caller sees, got {after}"
    );
}

/// RULE 3: an `InstanceArn` the account holds no instance under is a missing resource on
/// every operation of the surface — the account-instance refusal belongs to a RESOLVED
/// instance, so it is not the answer here.
#[tokio::test]
async fn an_unknown_instance_is_a_missing_resource() {
    let (router, _instance_arn, _engine) = organization_router_with_engine();
    let case = &cases()[0];
    let requests: Vec<(&str, Value)> = vec![
        (
            CREATE,
            configuration_body(UNKNOWN_INSTANCE_ARN, &case.attributes),
        ),
        (DESCRIBE, json!({"InstanceArn": UNKNOWN_INSTANCE_ARN})),
        (
            UPDATE,
            configuration_body(UNKNOWN_INSTANCE_ARN, &case.attributes),
        ),
        (DELETE, json!({"InstanceArn": UNKNOWN_INSTANCE_ARN})),
    ];
    for (operation, body) in requests {
        let (status, body) = call(&router, operation, &body).await;
        assert_not_found(&format!("unknown instance: {operation}"), status, &body);
    }
}

/// RULE 4: one key maps one source, so the same key twice is refused and stores nothing —
/// while keys differing only in case are two attributes, which the table above already
/// stores.
#[tokio::test]
async fn a_repeated_attribute_key_is_refused() {
    // Each row repeats ONE key: at the head of the list, at its tail, and with the
    // repeat separated by other attributes.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "the same key twice, adjacent",
            vec![
                ("Department".to_string(), "${path:a}".to_string()),
                ("Department".to_string(), "${path:b}".to_string()),
            ],
        ),
        (
            "the repeat separated by another attribute",
            vec![
                ("cost-center".to_string(), "${path:a}".to_string()),
                ("Title".to_string(), "${path:b}".to_string()),
                ("cost-center".to_string(), "${path:c}".to_string()),
            ],
        ),
        (
            "the same key and the same source twice",
            vec![
                ("Köln".to_string(), REPOINTED_SOURCE.to_string()),
                ("Köln".to_string(), REPOINTED_SOURCE.to_string()),
            ],
        ),
    ];

    for (label, attributes) in rows {
        let (router, instance_arn, _engine) = organization_router_with_engine();
        let repeated = attributes[0].0.clone();

        let (status, body) = call(
            &router,
            CREATE,
            &configuration_body(&instance_arn, &attributes),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a repeated key, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&repeated),
            "case {label:?}: the refusal must name the repeated key {repeated:?}, got \
             {message:?}"
        );
        // Refused, so nothing was enabled.
        let (status, body) = call(&router, DESCRIBE, &json!({"InstanceArn": instance_arn})).await;
        assert_not_found(&format!("{label}: describe after the refusal"), status, &body);

        // The same refusal on the update, with the mapping a valid create stored left
        // exactly as it was.
        let valid = vec![("Department".to_string(), "${path:a}".to_string())];
        let (status, body) = call(&router, CREATE, &configuration_body(&instance_arn, &valid)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the valid create expected 200, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            UPDATE,
            &configuration_body(&instance_arn, &attributes),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: {UPDATE} expected HTTP 400 for a repeated key, got {status} {body}"
        );
        let (_, stored) = describe_ok(&router, &instance_arn).await;
        assert_eq!(
            stored,
            attributes_json(&valid),
            "case {label:?}: a refused update must leave the stored mapping alone, got {stored}"
        );
    }
}

/// RULE 5: one step past each published bound of the configuration shapes is refused
/// before the handler runs, with the service's `ValidationException` (400) — the bounds
/// themselves are accepted by the table above.
#[tokio::test]
async fn a_request_past_the_modelled_bounds_is_refused() {
    let (router, instance_arn, _engine) = organization_router_with_engine();

    // Each row is one step past a published bound, or outside a published pattern.
    let rows: Vec<(&str, Value)> = vec![
        (
            // `AccessControlAttributeList` max 50.
            "51 attributes, one past the list maximum",
            attributes_json(
                &(0..51)
                    .map(|index| (format!("attr-{index:02}"), REPOINTED_SOURCE.to_string()))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            // `AccessControlAttributeKey` max 128.
            "a 129-character key, one past the key maximum",
            attributes_json(&[("k".repeat(129), REPOINTED_SOURCE.to_string())]),
        ),
        (
            // `AccessControlAttributeKey` min 1.
            "an empty key, one below the key minimum",
            attributes_json(&[(String::new(), REPOINTED_SOURCE.to_string())]),
        ),
        (
            // `AccessControlAttributeKey` pattern `^[\p{L}\p{Z}\p{N}_.:\/=+\-@]+$`.
            "a key outside the key pattern",
            attributes_json(&[("bad*key".to_string(), REPOINTED_SOURCE.to_string())]),
        ),
        (
            // `AccessControlAttributeValueSource` max 256.
            "a 257-character source, one past the source maximum",
            attributes_json(&[("Department".to_string(), "s".repeat(257))]),
        ),
        (
            // `AccessControlAttributeValueSourceList` length 1..1.
            "two sources, one past the source-list maximum",
            json!([{"Key": "Department", "Value": {"Source": ["${path:a}", "${path:b}"]}}]),
        ),
        (
            "no source, one below the source-list minimum",
            json!([{"Key": "Department", "Value": {"Source": []}}]),
        ),
    ];

    for (label, attributes) in rows {
        for operation in [CREATE, UPDATE] {
            let body = json!({
                "InstanceArn": instance_arn,
                "InstanceAccessControlAttributeConfiguration": {
                    "AccessControlAttributes": attributes,
                },
            });
            let (status, body) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label:?}/{operation}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}/{operation}: expected ValidationException, got {body}"
            );
        }
    }

    // None of those requests enabled anything.
    let (status, body) = call(&router, DESCRIBE, &json!({"InstanceArn": instance_arn})).await;
    assert_not_found("describe after the refused requests", status, &body);
}
