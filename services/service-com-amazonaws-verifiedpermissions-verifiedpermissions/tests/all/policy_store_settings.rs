// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the policy store's own settings:
//! `CreatePolicyStore` -> `UpdatePolicyStore` -> `GetPolicyStore` ->
//! `DeletePolicyStore`.
//!
//! The rules under test:
//!
//! * `GetPolicyStore` reports every modelled member of the policy store OUT OF
//!   THE RECORD the create/update wrote — `cedarVersion`, which no request sets,
//!   and `encryptionState`, which a create may select and otherwise defaults to
//!   an AWS owned key (the write path has its own suite,
//!   `tests/all/policy_store_encryption_settings.rs`).
//! * `UpdatePolicyStore` replaces the store's mutable settings in that record
//!   and moves `lastUpdatedDate`, leaving `createdDate` where it was; so every
//!   later reader — `GetPolicyStore` and `DeletePolicyStore`'s
//!   deletion-protection refusal alike — sees the updated values, and the one
//!   member the model documents as sticky, `deletionProtection`, is unchanged by
//!   a call that omits it.
//! * Both timestamp members of all three operations go on the wire as the
//!   RFC 3339 strings `@timestampFormat("date-time")` requires.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, over ONE shared engine, reusing the harness in
//! `tests/all/harness.rs` — so a value is only visible to a later request if it
//! really reached state.

use std::collections::{BTreeMap, HashMap};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions as service;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, ok, router, router_with_engine};

/// The Cedar version a new policy store reports, which is the service's own
/// default: the corpus's recorded `GetPolicyStore` responses for a plainly
/// created store carry `CEDAR_4`. Written out rather than imported from
/// `crate::state::DEFAULT_CEDAR_VERSION` on purpose, so a change to the
/// implementation constant fails this test instead of moving with it.
const EXPECTED_CEDAR_VERSION: &str = "CEDAR_4";

/// `CedarVersion`'s modelled values, from
/// `smithy_query(query="shape", shape="CedarVersion")`: `CEDAR_2 | CEDAR_4`.
/// The secondary check: whatever is reported is a value the enum declares.
const CEDAR_VERSION_VALUES: &[&str] = &["CEDAR_2", "CEDAR_4"];

/// Create a store from `request` and return its `policyStoreId`.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    ok(router, "CreatePolicyStore", request).await["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// Read a store back.
async fn get(router: &MultiServiceRouter, store_id: &str) -> Value {
    ok(router, "GetPolicyStore", &json!({ "policyStoreId": store_id })).await
}

/// The instant a `@timestampFormat("date-time")` member carries, refusing
/// anything that is not an RFC 3339 string — an epoch number included.
fn instant(case: &str, member: &str, body: &Value) -> f64 {
    let text = body[member].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be a date-time string, got {body}")
    });
    assert!(
        text.ends_with('Z'),
        "case {case:?}: {member} must be a UTC date-time, got {text:?}"
    );
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} is not RFC 3339: {error}")
        })
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// What a read reports
// ---------------------------------------------------------------------------

/// `GetPolicyStore` reports `cedarVersion` and the `encryptionState` union for
/// every store, from the record `CreatePolicyStore` wrote — over the whole range
/// of optional members a create may or may not carry.
#[tokio::test]
async fn get_policy_store_reports_the_cedar_version_and_encryption_state_it_persisted() {
    // Rows differ along the members a create carries, none of which has any
    // bearing on these two: the Cedar version is the service's own, and no row
    // selects an encryption key.
    let cases: &[(&str, Value)] = &[
        ("minimal", json!({ "validationSettings": { "mode": "OFF" } })),
        (
            "strict",
            json!({ "validationSettings": { "mode": "STRICT" } }),
        ),
        (
            "described and protected",
            json!({
                "validationSettings": { "mode": "OFF" },
                "description": "demo",
                "deletionProtection": "ENABLED",
            }),
        ),
        (
            "tagged",
            json!({
                "validationSettings": { "mode": "OFF" },
                "tags": { "team": "photos" },
            }),
        ),
    ];

    let router = router();
    for (case, request) in cases {
        let store_id = create(&router, request).await;
        let fetched = get(&router, &store_id).await;

        let cedar_version = fetched["cedarVersion"].as_str().unwrap_or_default();
        assert_eq!(
            cedar_version, EXPECTED_CEDAR_VERSION,
            "case {case:?}: a new store's cedarVersion is the service default, got {fetched}"
        );
        assert!(
            CEDAR_VERSION_VALUES.contains(&cedar_version),
            "case {case:?}: cedarVersion must be one of {CEDAR_VERSION_VALUES:?}, got {fetched}"
        );
        // No row selects a key, so every store these rows create is encrypted
        // with an AWS owned key and the union's `default` arm is the one
        // reported — an EMPTY object, which is what `smithy.api#Unit` denotes.
        assert_eq!(
            fetched["encryptionState"],
            json!({ "default": {} }),
            "case {case:?}: unexpected encryptionState, got {fetched}"
        );

        // The same two members are reported whether or not tags were asked for:
        // they are not part of the tag slice.
        let with_tags = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": store_id, "tags": true }),
        )
        .await;
        assert_eq!(
            with_tags["cedarVersion"], fetched["cedarVersion"],
            "case {case:?}: cedarVersion changed when tags were asked for, got {with_tags}"
        );
        assert_eq!(
            with_tags["encryptionState"], fetched["encryptionState"],
            "case {case:?}: encryptionState changed when tags were asked for, got {with_tags}"
        );
    }
}

/// The `encryptionState` a read reports is the one PERSISTED on the store, not a
/// constant: a store whose record holds the union's `kmsEncryptionState` arm is
/// reported with that arm, key and encryption context included.
///
/// The arm is seeded directly into state so this suite tests the READ in
/// isolation: that a record holding the arm is reported with it, whatever wrote
/// the record. `CreatePolicyStoreInput$encryptionSettings`, the request member
/// that selects the key, has its own suite in
/// `tests/all/policy_store_encryption_settings.rs`.
#[tokio::test]
async fn get_policy_store_reports_a_customer_managed_key_from_the_record() {
    let (router, engine) = router_with_engine();

    // Rows differ along the encryption context: none, one entry, several.
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("no encryption context", &[]),
        ("one context entry", &[("project", "photos")]),
        (
            "several context entries",
            &[("project", "photos"), ("tier", "gold")],
        ),
    ];

    for (case, context) in cases {
        let store_id = create(&router, &json!({ "validationSettings": { "mode": "OFF" } })).await;
        let key =
            ::engine::emulator_arn("kms", "us-east-1", "key/1234abcd-12ab-34cd-56ef-1234567890ab");
        let encryption_context: BTreeMap<String, String> = context
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        {
            let state = engine
                .state::<service::ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>();
            let mut state = state.lock();
            let store = state
                .policy_stores
                .get_mut(&store_id)
                .expect("the store just created is in state");
            store.encryption = service::state::StoreEncryption::CustomerManagedKey {
                key: key.clone(),
                encryption_context: encryption_context.clone(),
            };
        }

        let fetched = get(&router, &store_id).await;
        let expected_context: HashMap<String, String> = encryption_context.clone().into_iter().collect();
        assert_eq!(
            fetched["encryptionState"],
            json!({
                "kmsEncryptionState": {
                    "key": key,
                    "encryptionContext": expected_context,
                }
            }),
            "case {case:?}: expected the persisted kmsEncryptionState, got {fetched}"
        );
    }
}

/// Both timestamp members of all three policy-store lifecycle responses are
/// RFC 3339 strings, as `@timestampFormat("date-time")` requires, and they name
/// the same instants across the three.
#[tokio::test]
async fn the_policy_store_lifecycle_reports_date_time_timestamps() {
    let router = router();

    let created = ok(
        &router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "OFF" } }),
    )
    .await;
    let store_id = created["policyStoreId"].as_str().unwrap_or_default();
    let create_created = instant("CreatePolicyStore", "createdDate", &created);
    let create_updated = instant("CreatePolicyStore", "lastUpdatedDate", &created);
    assert_eq!(
        create_created, create_updated,
        "case \"CreatePolicyStore\": a new store's two timestamps are the same instant, got {created}"
    );

    let fetched = get(&router, store_id).await;
    assert_eq!(
        instant("GetPolicyStore", "createdDate", &fetched),
        create_created,
        "case \"GetPolicyStore\": createdDate must be the instant the create reported, got {fetched}"
    );

    let updated = ok(
        &router,
        "UpdatePolicyStore",
        &json!({ "policyStoreId": store_id, "validationSettings": { "mode": "STRICT" } }),
    )
    .await;
    assert_eq!(
        instant("UpdatePolicyStore", "createdDate", &updated),
        create_created,
        "case \"UpdatePolicyStore\": createdDate must not move, got {updated}"
    );
    let update_updated = instant("UpdatePolicyStore", "lastUpdatedDate", &updated);
    assert!(
        update_updated > create_updated,
        "case \"UpdatePolicyStore\": lastUpdatedDate must advance past the create's \
         {create_updated}, got {update_updated}"
    );

    let after = get(&router, store_id).await;
    assert_eq!(
        instant("GetPolicyStore after update", "lastUpdatedDate", &after),
        update_updated,
        "case \"GetPolicyStore after update\": the read must report the instant the update \
         persisted, got {after}"
    );
}

// ---------------------------------------------------------------------------
// What an update replaces
// ---------------------------------------------------------------------------

/// Every mutable setting `UpdatePolicyStore` carries is written back into the
/// store, and a later read reports exactly that; `deletionProtection` is the one
/// member an omitting call leaves alone.
#[tokio::test]
async fn update_policy_store_persists_the_settings_a_later_read_reports() {
    /// One row: the update request, and the settings the store must hold after.
    struct Step {
        case: &'static str,
        request: Value,
        mode: &'static str,
        protection: &'static str,
        description: Option<&'static str>,
    }

    let long_description = "d".repeat(150);

    // Rows differ along the validation mode, whether deletion protection is
    // named at all and what it is set to, and whether a description is carried
    // (including one at the modelled 0..150 length bound and the empty string).
    let steps = vec![
        Step {
            case: "mode only, protection untouched",
            request: json!({ "validationSettings": { "mode": "STRICT" } }),
            mode: "STRICT",
            protection: "ENABLED",
            description: None,
        },
        Step {
            case: "protection turned off",
            request: json!({
                "validationSettings": { "mode": "STRICT" },
                "deletionProtection": "DISABLED",
                "description": "turned off",
            }),
            mode: "STRICT",
            protection: "DISABLED",
            description: Some("turned off"),
        },
        Step {
            case: "mode back to OFF, description replaced",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": "replaced",
            }),
            mode: "OFF",
            protection: "DISABLED",
            description: Some("replaced"),
        },
        Step {
            case: "an omitted description is cleared",
            request: json!({ "validationSettings": { "mode": "OFF" } }),
            mode: "OFF",
            protection: "DISABLED",
            description: None,
        },
        Step {
            case: "an empty description is a description",
            request: json!({ "validationSettings": { "mode": "OFF" }, "description": "" }),
            mode: "OFF",
            protection: "DISABLED",
            description: Some(""),
        },
        Step {
            case: "a description at the modelled length bound",
            request: json!({
                "validationSettings": { "mode": "STRICT" },
                "description": long_description.clone(),
            }),
            mode: "STRICT",
            protection: "DISABLED",
            description: None, // checked against `long_description` below
        },
        Step {
            case: "protection turned back on",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
            }),
            mode: "OFF",
            protection: "ENABLED",
            description: None,
        },
        Step {
            case: "a repeated call changes nothing",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
            }),
            mode: "OFF",
            protection: "ENABLED",
            description: None,
        },
    ];

    let router = router();
    let store_id = create(
        &router,
        &json!({
            "validationSettings": { "mode": "OFF" },
            "description": "initial",
            "deletionProtection": "ENABLED",
        }),
    )
    .await;
    let arn = get(&router, &store_id).await["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for step in &steps {
        let case = step.case;
        let mut request = step.request.clone();
        request["policyStoreId"] = json!(store_id);
        let (status, body) = call(&router, "UpdatePolicyStore", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdatePolicyStore, got {status} {body}"
        );
        assert_eq!(
            body["policyStoreId"].as_str().unwrap_or_default(),
            store_id,
            "case {case:?}: the response must name the store it updated, got {body}"
        );
        assert_eq!(
            body["arn"].as_str().unwrap_or_default(),
            arn,
            "case {case:?}: a store's ARN does not change, got {body}"
        );

        let fetched = get(&router, &store_id).await;
        assert_eq!(
            fetched["validationSettings"]["mode"]
                .as_str()
                .unwrap_or_default(),
            step.mode,
            "case {case:?}: validationSettings.mode was not persisted, got {fetched}"
        );
        assert_eq!(
            fetched["deletionProtection"].as_str().unwrap_or_default(),
            step.protection,
            "case {case:?}: deletionProtection was not persisted, got {fetched}"
        );
        // The description the request carried, which for the length-bound row is
        // the long one built above and for an omitting row is absent entirely.
        let expected_description = request
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            fetched["description"].as_str().map(str::to_string),
            expected_description,
            "case {case:?}: expected description {expected_description:?}, got {fetched}"
        );
        if let Some(expected) = step.description {
            assert_eq!(
                fetched["description"].as_str().unwrap_or_default(),
                expected,
                "case {case:?}: unexpected description, got {fetched}"
            );
        }
        // An update never touches the encryption these rows did not select, nor
        // the Cedar version no request sets.
        assert_eq!(
            fetched["encryptionState"],
            json!({ "default": {} }),
            "case {case:?}: an update must not change encryptionState, got {fetched}"
        );
        assert_eq!(
            fetched["cedarVersion"].as_str().unwrap_or_default(),
            EXPECTED_CEDAR_VERSION,
            "case {case:?}: an update must not drop or change cedarVersion, got {fetched}"
        );
    }
}

/// Deletion protection is read from the CURRENT record: a store protected at
/// creation refuses the delete until an update turns protection off, and refuses
/// it again once an update turns protection back on.
#[tokio::test]
async fn delete_policy_store_follows_the_protection_the_last_update_persisted() {
    /// One row: the deletion protection a create carries, the sequence of
    /// `deletionProtection` values updates then set (`None` = the member is
    /// omitted, so the flag is unchanged), and the status the delete must answer.
    struct Case {
        case: &'static str,
        created_with: Option<&'static str>,
        updates: &'static [Option<&'static str>],
        expected_delete: u16,
    }

    // Rows differ along the created-with flag, the number of updates and what
    // each sets — so the outcome can only follow the LAST persisted value.
    let cases: &[Case] = &[
        Case {
            case: "protected, then turned off",
            created_with: Some("ENABLED"),
            updates: &[Some("DISABLED")],
            expected_delete: 200,
        },
        Case {
            case: "protected, update omits the member",
            created_with: Some("ENABLED"),
            updates: &[None],
            expected_delete: 406,
        },
        Case {
            case: "protected, off then on again",
            created_with: Some("ENABLED"),
            updates: &[Some("DISABLED"), Some("ENABLED")],
            expected_delete: 406,
        },
        Case {
            case: "unprotected, then turned on",
            created_with: Some("DISABLED"),
            updates: &[Some("ENABLED")],
            expected_delete: 406,
        },
        Case {
            case: "default (unprotected), update omits the member",
            created_with: None,
            updates: &[None],
            expected_delete: 200,
        },
        Case {
            case: "off, on, off again",
            created_with: None,
            updates: &[Some("ENABLED"), Some("DISABLED")],
            expected_delete: 200,
        },
    ];

    let router = router();
    for Case {
        case,
        created_with,
        updates,
        expected_delete,
    } in cases
    {
        let mut request = json!({ "validationSettings": { "mode": "OFF" } });
        if let Some(protection) = created_with {
            request["deletionProtection"] = json!(protection);
        }
        let store_id = create(&router, &request).await;

        for protection in *updates {
            let mut update = json!({
                "policyStoreId": store_id,
                "validationSettings": { "mode": "OFF" },
            });
            if let Some(protection) = protection {
                update["deletionProtection"] = json!(protection);
            }
            ok(&router, "UpdatePolicyStore", &update).await;
        }

        let (status, body) = call(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            status, *expected_delete,
            "case {case:?}: expected HTTP {expected_delete} from DeletePolicyStore, got {status} {body}"
        );
        if *expected_delete == 406 {
            assert_eq!(
                error_code(&body),
                "InvalidStateException",
                "case {case:?}: unexpected error code, got {body}"
            );
        }

        // A delete that succeeded makes the store unknown; one that was refused
        // left it there.
        let (get_status, get_body) =
            call(&router, "GetPolicyStore", &json!({ "policyStoreId": store_id })).await;
        let expected_get = if *expected_delete == 200 { 404 } else { 200 };
        assert_eq!(
            get_status, expected_get,
            "case {case:?}: expected HTTP {expected_get} from the following GetPolicyStore, \
             got {get_status} {get_body}"
        );
    }
}

/// An `UpdatePolicyStore` that names no live store is the one not-found error the
/// operation models, whatever form the reference takes; an alias that resolves
/// updates the store it names.
#[tokio::test]
async fn update_policy_store_resolves_aliases_and_refuses_unknown_references() {
    let router = router();

    let store_id = create(&router, &json!({ "validationSettings": { "mode": "OFF" } })).await;
    let alias = "policy-store-alias/settings-suite";
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "policyStoreId": store_id, "aliasName": alias }),
    )
    .await;

    // An alias is interchangeable with the id, so the update lands on the store.
    ok(
        &router,
        "UpdatePolicyStore",
        &json!({ "policyStoreId": alias, "validationSettings": { "mode": "STRICT" } }),
    )
    .await;
    let fetched = get(&router, &store_id).await;
    assert_eq!(
        fetched["validationSettings"]["mode"]
            .as_str()
            .unwrap_or_default(),
        "STRICT",
        "case \"through an alias\": the update must land on the aliased store, got {fetched}"
    );

    // Rows differ along the FORM of a reference that names no live store.
    let deleted_id = create(&router, &json!({ "validationSettings": { "mode": "OFF" } })).await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": deleted_id }),
    )
    .await;
    // (case, the reference, the `ResourceType` the error must name: a bare id
    // names a POLICY_STORE, a `policy-store-alias/<name>` reference names the
    // ALIAS, which is the resource the request referenced.)
    let references: &[(&str, String, &str)] = &[
        ("never existed", "PSneverexisted".to_string(), "POLICY_STORE"),
        ("a deleted store", deleted_id.clone(), "POLICY_STORE"),
        (
            "an alias no store carries",
            "policy-store-alias/nosuchalias".to_string(),
            "POLICY_STORE_ALIAS",
        ),
    ];
    for (case, reference, resource_type) in references {
        let (status, body) = call(
            &router,
            "UpdatePolicyStore",
            &json!({
                "policyStoreId": reference,
                "validationSettings": { "mode": "STRICT" },
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str().unwrap_or_default(),
            *resource_type,
            "case {case:?}: the error must name the referenced resource, got {body}"
        );
    }
}

/// A refusal's message is a CONSTANT: it embeds neither the request's reference
/// nor the store's generated id, which `ResourceNotFoundException`'s own
/// `resourceId`/`resourceType` members carry instead, and two calls that differ
/// only in which store they name get the same message.
///
/// This is what lets the deletion-protection refusal be matched on by a client:
/// the message is the same sentence for every store.
///
/// `GetPolicyStore` and `DeletePolicyStore` are the two paths the corpus pins
/// the constant for; the other interpolated messages of this service
/// (`GetPolicy`, `ListTagsForResource`, `UpdatePolicyStore`) belong to the open
/// message-interpolation issue and are not asserted here.
#[tokio::test]
async fn a_refusal_message_is_a_constant_that_names_no_store() {
    let router = router();

    // Rows differ along the unknown reference's form, so a message built from
    // the request would differ between them.
    let unknown: &[(&str, &str)] = &[
        ("an id that never existed", "PSneverexisted"),
        ("another unknown id", "PSalsoNotThere00000000"),
        ("an alias no store carries", "policy-store-alias/nope"),
    ];
    let mut not_found_messages: Vec<String> = Vec::new();
    for (case, reference) in unknown {
        let (status, body) = call(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": reference }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.contains(reference),
            "case {case:?}: the message must not embed the request's reference, got {message:?}"
        );
        not_found_messages.push(message);
    }
    let first = not_found_messages.first().cloned().unwrap_or_default();
    assert!(
        not_found_messages.iter().all(|message| *message == first),
        "case \"not-found messages\": every one must be the same constant, got \
         {not_found_messages:?}"
    );

    // The deletion-protection refusal, over two stores whose ids differ.
    let mut refusals: Vec<String> = Vec::new();
    for case in ["first protected store", "second protected store"] {
        let store_id = create(
            &router,
            &json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
            }),
        )
        .await;
        let (status, body) = call(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            status, 406,
            "case {case:?}: expected HTTP 406, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.contains(&store_id),
            "case {case:?}: the refusal must not embed the store id, got {message:?}"
        );
        refusals.push(message);
    }
    assert_eq!(
        refusals.first(),
        refusals.last(),
        "case \"deletion-protection refusal\": the message must be the same constant for \
         every store, got {refusals:?}"
    );
}

/// A validation mode the enum does not declare is refused before anything is
/// written, so the store keeps the mode it had.
#[tokio::test]
async fn update_policy_store_refuses_an_unmodelled_validation_mode() {
    let router = router();
    let store_id = create(
        &router,
        &json!({ "validationSettings": { "mode": "STRICT" } }),
    )
    .await;

    // Rows differ along how the value misses `ValidationMode`'s two members
    // (`smithy_query(query="shape", shape="ValidationMode")`: OFF | STRICT).
    let modes = ["off", "Strict", "LENIENT", ""];
    for mode in modes {
        let (status, body) = call(
            &router,
            "UpdatePolicyStore",
            &json!({
                "policyStoreId": store_id,
                "validationSettings": { "mode": mode },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {mode:?}: expected HTTP 400 for an unmodelled validation mode, got {status} {body}"
        );
        let fetched = get(&router, &store_id).await;
        assert_eq!(
            fetched["validationSettings"]["mode"]
                .as_str()
                .unwrap_or_default(),
            "STRICT",
            "case {mode:?}: a refused update must persist nothing, got {fetched}"
        );
    }
}
