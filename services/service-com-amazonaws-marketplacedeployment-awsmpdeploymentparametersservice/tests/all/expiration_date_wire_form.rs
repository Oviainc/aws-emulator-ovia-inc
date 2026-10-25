// SPDX-License-Identifier: CC0-1.0

//! The wire form `expirationDate` travels in, and the input judgement of `tags`.
//!
//! Both rules are driven over the production `MultiServiceRouter` and restJson1, so
//! what is asserted is what a signed SDK request observes.
//!
//! 1. A timestamp member the model marks `@timestampFormat("date-time")` travels on
//!    restJson1 as an RFC 3339 date-time STRING, and the instant that string names is
//!    what the service stores. `PutDeploymentParameterRequest$expirationDate` is that
//!    member (`smithy_query(query="operation",operation="PutDeploymentParameter")`
//!    reports `"timestampFormat": "date-time"` on it, and it is the only timestamp
//!    member of this service's whole surface), so a value in the protocol's DEFAULT
//!    timestamp form — a bare JSON number of epoch seconds — is not a value this
//!    member admits, and neither is any other JSON type: each is a bad request (HTTP
//!    400) that stores nothing.
//!
//!    The accepted table spans what the form generalises over: whole seconds and
//!    fractional seconds, dates before and after the Unix epoch, and dates on either
//!    side of now — the last of which also shows that the instant the string named is
//!    the one the service enforces, since a date already past takes the parameter out
//!    of every later path while a future one leaves it addressable. Smithy's
//!    `date-time` is RFC 3339 narrowed to the `Z` offset, so a value written at a
//!    numeric offset is among the refusals rather than among these rows.
//!
//!    Only the REQUEST direction is asserted because no operation of this model
//!    reports a timestamp back: `PutDeploymentParameter`'s output and the three
//!    tagging operations declare no timestamp member at all.
//!
//! 2. A modelled input constraint is judged on the REQUEST, ahead of the work the
//!    operation would do, so the answer does not depend on which path the handler
//!    would then take or on whether the member would be applied. `tags` is the member
//!    where that is observable on this operation: it is documented as applied on the
//!    create only, so a tag map outside the modelled `TagsMap` must be refused
//!    identically whether the request creates a parameter or updates one that already
//!    exists — and a well-formed map must be accepted on both, with the update's
//!    ignored.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::NOT_FOUND_CODE;
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    service_state, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};

use crate::deployment_parameters::{
    emulator, error_code, is_parameter_id, list_tags, provision, put, stored_parameters, tag_map,
    tags_of, CATALOG,
};

/// The status a request outside the modelled input carries.
const BAD_REQUEST_STATUS: u16 = 400;
/// The status every refusal of an expired (or never-stored) parameter carries.
const NOT_FOUND_STATUS: u16 = 404;

/// One `PutDeploymentParameter` body, with `expirationDate` set to whatever JSON
/// `expiration_date` is — including a form the member does not admit, which is the
/// point of half the table below.
fn put_body(agreement_id: &str, name: &str, expiration_date: Option<Value>) -> Value {
    let mut body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": name, "secretString": "s3cr3t" },
    });
    let map = body.as_object_mut().expect("an object body");
    if let Some(expiration_date) = expiration_date {
        map.insert("expirationDate".to_string(), expiration_date);
    }
    body
}

/// The `expirationDate` the store holds for `resource_arn`, as `(seconds,
/// nanoseconds)` since the Unix epoch — the only way to observe it, since no
/// operation of this model reports it back.
fn stored_expiration(
    engine: &engine::InMemoryEngine,
    case: &str,
    resource_arn: &str,
) -> Option<(i64, u32)> {
    let state = service_state(engine).expect("the shared engine must hold this service's store");
    let store = state.lock();
    let record = store
        .parameters
        .get(resource_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold {resource_arn}"));
    record
        .expiration_date
        .map(|at| (at.secs(), at.subsec_nanos()))
}

/// The `resourceArn` a successful put reported, with its minted id checked.
fn arn_of(case: &str, body: &Value) -> String {
    let id = body
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        is_parameter_id(id),
        "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id, got {id:?}"
    );
    body.get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the accepted-form table: the string the request carries, the instant it
/// names, and whether that instant has already passed.
struct Accepted {
    case: &'static str,
    sent: &'static str,
    /// The instant `sent` names, as `(seconds, nanoseconds)` since the Unix epoch.
    instant: (i64, u32),
    /// Whether the parameter that instant names is expired by the time the row's
    /// later calls run.
    expired: bool,
}

/// The RFC 3339 forms the member admits. Each row's `instant` is the value the string
/// denotes, so the assertion is that the service parsed the form rather than that it
/// echoed it.
fn accepted_table() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "a far-future date at midnight UTC",
            sent: "2100-01-01T00:00:00Z",
            instant: (4_102_444_800, 0),
            expired: false,
        },
        Accepted {
            case: "a future date with a time of day",
            sent: "2030-06-15T12:34:56Z",
            instant: (1_907_757_296, 0),
            expired: false,
        },
        Accepted {
            case: "a future date carrying fractional seconds",
            sent: "2030-06-15T12:34:56.789Z",
            instant: (1_907_757_296, 789_000_000),
            expired: false,
        },
        Accepted {
            case: "the Unix epoch itself",
            sent: "1970-01-01T00:00:00Z",
            instant: (0, 0),
            expired: true,
        },
        Accepted {
            case: "a date before the Unix epoch",
            sent: "1969-07-20T20:17:40Z",
            instant: (-14_182_940, 0),
            expired: true,
        },
        Accepted {
            case: "a past date with fractional seconds",
            sent: "2020-01-01T00:00:00.500Z",
            instant: (1_577_836_800, 500_000_000),
            expired: true,
        },
    ]
}

#[tokio::test]
async fn an_expiration_date_is_accepted_as_the_rfc_3339_string_its_format_declares() {
    let (router, engine) = emulator();

    for (index, row) in accepted_table().into_iter().enumerate() {
        let case = row.case;
        // One identity per row, so a row's expiry decides only its own resource.
        let product_id = format!("prod-wire-{index:02}");
        let agreement_id = format!("agmt-wire-{index:02}");
        let name = format!("Parameter{index:02}");
        provision(&engine, CATALOG, &agreement_id, &[&product_id]);

        let (status, created) = put(
            &router,
            CATALOG,
            &product_id,
            &put_body(&agreement_id, &name, Some(json!(row.sent))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} must be accepted, got {status} {created:?}",
            row.sent
        );
        let arn = arn_of(case, &created);

        // The instant the string NAMED is what was stored, to the nanosecond — not
        // the text, and not a value derived from the request some other way.
        let stored = stored_expiration(&engine, case, &arn);
        assert_eq!(
            stored,
            Some(row.instant),
            "case {case:?}: expected the stored expirationDate to be {:?}, got {stored:?}",
            row.instant
        );

        // And the instant is the one the service ENFORCES: a date already past takes
        // the parameter out of the next path that resolves it, a future one does not.
        let (status, listed) = list_tags(&router, &arn).await;
        if row.expired {
            assert_eq!(
                status, NOT_FOUND_STATUS,
                "case {case:?}: a parameter past its date must be gone, got {status} {listed:?}"
            );
            assert_eq!(
                error_code(&listed),
                NOT_FOUND_CODE,
                "case {case:?}: expected the code {NOT_FOUND_CODE}, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a parameter inside its date must still be addressable, \
                 got {status} {listed:?}"
            );
        }
    }
}

/// One row of the refused-form table: a JSON value for `expirationDate` that the
/// member's `date-time` format does not admit.
struct Refused {
    case: &'static str,
    sent: Value,
}

/// Every form that is NOT an RFC 3339 date-time string at the `Z` offset, including
/// the protocol's own DEFAULT timestamp form (a bare number of epoch seconds) and the
/// other format the model can declare for a timestamp member (`http-date`) — this
/// member declares neither, so neither is a value it carries. Smithy narrows RFC 3339
/// to the `Z` offset, which is why a numeric offset is a row here.
fn refused_table() -> Vec<Refused> {
    vec![
        Refused {
            case: "a JSON number of epoch seconds",
            sent: json!(4_102_444_800_i64),
        },
        Refused {
            case: "a JSON number of fractional epoch seconds",
            sent: json!(4_102_444_800.5_f64),
        },
        Refused {
            case: "zero",
            sent: json!(0),
        },
        Refused {
            case: "epoch seconds written as a string",
            sent: json!("4102444800"),
        },
        Refused {
            case: "an http-date string",
            sent: json!("Tue, 29 Apr 2031 12:00:00 GMT"),
        },
        Refused {
            case: "a date-time written at a numeric offset rather than at Z",
            sent: json!("2030-06-15T07:34:56-05:00"),
        },
        Refused {
            case: "a date with no time at all",
            sent: json!("2100-01-01"),
        },
        Refused {
            case: "a date-time missing its offset",
            sent: json!("2100-01-01T00:00:00"),
        },
        Refused {
            case: "text that is not a date at all",
            sent: json!("whenever"),
        },
        Refused {
            case: "the empty string",
            sent: json!(""),
        },
        Refused {
            case: "a boolean",
            sent: json!(true),
        },
        Refused {
            case: "an array",
            sent: json!(["2100-01-01T00:00:00Z"]),
        },
        Refused {
            case: "an object",
            sent: json!({ "seconds": 4_102_444_800_i64 }),
        },
    ]
}

#[tokio::test]
async fn an_expiration_date_outside_that_form_is_refused_and_writes_nothing() {
    let (router, engine) = emulator();
    let product_id = "prod-wire-refused";
    let agreement_id = "agmt-wire-refused";
    provision(&engine, CATALOG, agreement_id, &[product_id]);

    for (index, row) in refused_table().into_iter().enumerate() {
        let case = row.case;
        let before = stored_parameters(&engine);
        let (status, answer) = put(
            &router,
            CATALOG,
            product_id,
            &put_body(
                agreement_id,
                &format!("Refused{index:02}"),
                Some(row.sent.clone()),
            ),
        )
        .await;
        assert_eq!(
            status, BAD_REQUEST_STATUS,
            "case {case:?}: {} is not a date-time the member admits, so it must be \
             HTTP {BAD_REQUEST_STATUS}, got {status} {answer:?}",
            row.sent
        );
        assert!(
            answer.get("resourceArn").is_none() && answer.get("deploymentParameterId").is_none(),
            "case {case:?}: a refused write must mint nothing, got {answer:?}"
        );
        let after = stored_parameters(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must store nothing, got {after} stored \
             parameters against {before}"
        );
    }

    // The same request with the one form the member DOES admit is served, so the
    // refusals above are about the form and not about anything else in the body.
    let (status, created) = put(
        &router,
        CATALOG,
        product_id,
        &put_body(
            agreement_id,
            "Accepted",
            Some(json!("2100-01-01T00:00:00Z")),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "the RFC 3339 form must be served, got {status} {created:?}"
    );
}

/// One row of the tag-judgement table: a `tags` map and whether the member admits it.
struct Tags {
    case: &'static str,
    pairs: Vec<(String, String)>,
    admitted: bool,
}

/// The tag maps a `PutDeploymentParameter` carries, spanning both sides of the
/// modelled `TagsMap`: its key and value shapes are `TagKey` / `TagValue`, each
/// `@length`-bounded and holding the character class
/// `smithy_query(query="shape",shape="TagsMap")` states, and `aws:` is the key
/// namespace AWS reserves for its own generated tags.
fn tags_table() -> Vec<Tags> {
    let pair = |key: &str, value: &str| vec![(key.to_string(), value.to_string())];
    vec![
        Tags {
            case: "a key and value inside the member's character class",
            pairs: pair("ok.key:1", "ok/value+1=2"),
            admitted: true,
        },
        Tags {
            case: "several well-formed entries",
            pairs: vec![
                ("env".to_string(), "prod".to_string()),
                ("owner@team".to_string(), "platform-1".to_string()),
            ],
            admitted: true,
        },
        Tags {
            case: "a key carrying a bang",
            pairs: pair("bad key!", "v"),
            admitted: false,
        },
        Tags {
            case: "a key carrying a space",
            pairs: pair("bad key", "v"),
            admitted: false,
        },
        Tags {
            case: "a key carrying a character outside ASCII",
            pairs: pair("clé", "v"),
            admitted: false,
        },
        Tags {
            case: "a key one character past its maximum",
            pairs: pair(&"k".repeat(TAG_KEY_MAX_LEN + 1), "v"),
            admitted: false,
        },
        Tags {
            case: "an empty key",
            pairs: pair("", "v"),
            admitted: false,
        },
        Tags {
            case: "a value carrying a space",
            pairs: pair("k", "bad value"),
            admitted: false,
        },
        Tags {
            case: "a value one character past its maximum",
            pairs: pair("k", &"v".repeat(TAG_VALUE_MAX_LEN + 1)),
            admitted: false,
        },
        Tags {
            case: "a key in the namespace AWS reserves",
            pairs: pair("aws:created-by", "someone"),
            admitted: false,
        },
    ]
}

/// The `tags` member for one row.
fn tags_member(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

#[tokio::test]
async fn the_tags_member_is_judged_the_same_on_the_create_and_the_update_path() {
    let (router, engine) = emulator();
    let product_id = "prod-tag-judgement";
    let agreement_id = "agmt-tag-judgement";
    // The identity the UPDATE rows aim at, created once with a tag map of its own so a
    // refused update is visible as a record left exactly as it was.
    let held_name = "AlreadyThere";
    let held_tags = tag_map(&[("env", "dev")]);
    provision(&engine, CATALOG, agreement_id, &[product_id]);

    let mut body = put_body(agreement_id, held_name, None);
    body.as_object_mut()
        .expect("an object body")
        .insert("tags".to_string(), tags_member(&[("env".to_string(), "dev".to_string())]));
    let (status, created) = put(&router, CATALOG, product_id, &body).await;
    assert_eq!(
        status, 200,
        "the record the update rows aim at must be created, got {status} {created:?}"
    );
    let held_arn = arn_of("the already-created record", &created);
    assert_eq!(
        tags_of(&created),
        held_tags,
        "the created record must carry its own tag map, got {created:?}"
    );

    for (index, row) in tags_table().into_iter().enumerate() {
        let case = row.case;
        let tags = tags_member(&row.pairs);

        // --- the CREATE path: a name this identity does not hold yet -------
        let fresh_name = format!("Fresh{index:02}");
        let mut body = put_body(agreement_id, &fresh_name, None);
        body.as_object_mut()
            .expect("an object body")
            .insert("tags".to_string(), tags.clone());
        let before = stored_parameters(&engine);
        let (create_status, created) = put(&router, CATALOG, product_id, &body).await;
        let after = stored_parameters(&engine);
        if row.admitted {
            assert_eq!(
                create_status, 200,
                "case {case:?}: the create must be served, got {create_status} {created:?}"
            );
            let expected: BTreeMap<String, String> = row.pairs.iter().cloned().collect();
            assert_eq!(
                tags_of(&created),
                expected,
                "case {case:?}: the create applies its tags, got {created:?}"
            );
        } else {
            assert_eq!(
                create_status, BAD_REQUEST_STATUS,
                "case {case:?}: the create must be HTTP {BAD_REQUEST_STATUS}, \
                 got {create_status} {created:?}"
            );
            assert_eq!(
                error_code(&created),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {created:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store nothing, got {after} stored \
                 parameters against {before}"
            );
        }

        // --- the UPDATE path: the identity the store already holds ---------
        let mut body = put_body(agreement_id, held_name, None);
        body.as_object_mut()
            .expect("an object body")
            .insert("tags".to_string(), tags);
        let (update_status, updated) = put(&router, CATALOG, product_id, &body).await;
        assert_eq!(
            update_status, create_status,
            "case {case:?}: the member is judged on the INPUT, so the update must answer \
             exactly as the create did ({create_status}), got {update_status} {updated:?}"
        );
        if !row.admitted {
            assert_eq!(
                error_code(&updated),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {updated:?}"
            );
        }
        // Either way the held record's tag map is untouched: a refused request stores
        // nothing, and `tags` on an accepted update is documented as ignored.
        let (status, listed) = list_tags(&router, &held_arn).await;
        assert_eq!(
            (status, tags_of(&listed)),
            (200, held_tags.clone()),
            "case {case:?}: the held record's tags must still be {held_tags:?}, \
             got {status} {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_well_formed_update_still_rewrites_the_secret_it_carries() {
    // The judgement above refuses a request before anything is written; this pins the
    // other side of it, that a request the member DOES admit still does its work.
    let (router, engine) = emulator();
    let product_id = "prod-tag-accepted";
    let agreement_id = "agmt-tag-accepted";
    provision(&engine, CATALOG, agreement_id, &[product_id]);

    let (status, created) = put(
        &router,
        CATALOG,
        product_id,
        &put_body(agreement_id, "Rotating", Some(json!("2100-01-01T00:00:00Z"))),
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {created:?}");
    let arn = arn_of("a well-formed update", &created);
    let created_at = SystemTime::now();

    let mut body = put_body(agreement_id, "Rotating", None);
    body.as_object_mut().expect("an object body").insert(
        "tags".to_string(),
        tags_member(&[("ignored-at-update".to_string(), "yes".to_string())]),
    );
    let (status, updated) = put(&router, CATALOG, product_id, &body).await;
    assert_eq!(status, 200, "the update must succeed, got {updated:?}");
    assert_eq!(
        updated.get("resourceArn").and_then(Value::as_str),
        Some(arn.as_str()),
        "the update must rewrite the same resource, got {updated:?}"
    );
    assert_eq!(
        tags_of(&updated),
        BTreeMap::new(),
        "tags are applied on create only, so the update must report the record's \
         (empty) tag map, got {updated:?}"
    );

    // The stored record kept the create's date and took the update's secret.
    let state = service_state(&*engine).expect("the shared engine must hold this service's store");
    let store = state.lock();
    let record = store
        .parameters
        .get(&arn)
        .expect("the store must hold the updated record");
    assert_eq!(
        record.expiration_date.map(|at| at.secs()),
        Some(4_102_444_800),
        "an update that omits expirationDate must leave the create's date, got {:?}",
        record.expiration_date
    );
    assert!(
        record.updated_at >= created_at,
        "the update must have been applied to the stored record"
    );
}
