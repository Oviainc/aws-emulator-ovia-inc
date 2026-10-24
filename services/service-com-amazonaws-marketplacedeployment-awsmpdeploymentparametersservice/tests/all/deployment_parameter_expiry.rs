// SPDX-License-Identifier: CC0-1.0

//! `expirationDate` as a DEADLINE the service enforces, and as an optional member an
//! update that omits it must not erase.
//!
//! Two rules, both driven over the production `MultiServiceRouter` and restJson1 so
//! what is asserted is what a signed SDK request observes:
//!
//! 1. the expiry a deployment parameter carries is enforced on every path that
//!    resolves it — `PutDeploymentParameterRequest$expirationDate` is "the date when
//!    deployment parameters expire and are scheduled for deletion"
//!    (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="expirationDate")`),
//!    so once the clock reaches it the parameter no longer exists: the three tagging
//!    operations answer this service's not-found failure (HTTP 404
//!    `NotFoundException`) and a `PutDeploymentParameter` for the same
//!    (catalog, productId, agreementId, name) mints a FRESH resource instead of
//!    updating the expired one;
//! 2. an update applies only the members the request carries, so a
//!    `PutDeploymentParameter` that omits the optional `expirationDate` leaves the
//!    stored date exactly as it was — an absent member is not an instruction to
//!    cancel the scheduled deletion — while one that names a date replaces it.
//!
//! The expiry table below spans the class the first rule generalises over: no date at
//! all, dates well in the past (including the Unix epoch), the due instant itself,
//! and dates in the future both near and far. Each row drives the full lifecycle
//! (`PutDeploymentParameter`, `ListTagsForResource`, `TagResource`,
//! `UntagResource`, then a repeat put) under its own identity.
//!
//! `expirationDate` reaches the wire as EPOCH SECONDS: the model marks the member
//! `@timestampFormat("date-time")`, but the generated schema this emulator
//! deserializes against carries no timestamp format, so the restJson1 default is what
//! the wire accepts — the same form the `deployment_parameters` suite sends.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::{
    NOT_FOUND_CODE, NOT_FOUND_TEXT,
};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::service_state;

use crate::deployment_parameters::{
    emulator, error_code, is_parameter_id, list_tags, message, provision, put, tag, tag_map,
    tags_of, untag, CATALOG,
};

/// The status every refusal of an expired (or never-stored) parameter carries.
const NOT_FOUND_STATUS: u16 = 404;
/// `2020-01-01T00:00:00Z` as epoch seconds — a date long past.
const PAST_2020: i64 = 1_577_836_800;
/// `2100-01-01T00:00:00Z` as epoch seconds — a date long in the future.
const FUTURE_2100: i64 = 4_102_444_800;
/// A day, for a row a whole day either side of now.
const ONE_DAY: i64 = 86_400;

/// Now, as the epoch seconds the wire carries a timestamp as.
fn now_epoch_secs() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch");
    i64::try_from(since_epoch.as_secs()).expect("an epoch second inside i64")
}

/// A whole epoch second at least half a second out: near enough to keep the one test
/// that waits for the clock short, far enough that the calls made before the wait
/// cannot themselves cross it.
fn due_second_ahead() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch");
    let secs = i64::try_from(since_epoch.as_secs()).expect("an epoch second inside i64");
    if since_epoch.subsec_millis() <= 500 {
        secs + 1
    } else {
        secs + 2
    }
}

/// One `PutDeploymentParameter` body: the identity members that do not live in the
/// URI, the secret, and the optional `tags` / `expirationDate`.
fn put_body(
    agreement_id: &str,
    name: &str,
    secret: &str,
    tags: Option<&BTreeMap<String, String>>,
    expiration_date: Option<i64>,
) -> Value {
    let mut body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": name, "secretString": secret },
    });
    let map = body.as_object_mut().expect("an object body");
    if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
        map.insert(
            "tags".to_string(),
            Value::Object(tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
        );
    }
    if let Some(expiration_date) = expiration_date {
        map.insert("expirationDate".to_string(), json!(expiration_date));
    }
    body
}

/// The `deploymentParameterId` and `resourceArn` a successful put reported.
fn identity_of(case: &str, body: &Value) -> (String, String) {
    let id = body
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_parameter_id(&id),
        "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id, got {id:?}"
    );
    assert!(
        !arn.is_empty(),
        "case {case:?}: expected a minted resourceArn, got {arn:?}"
    );
    (id, arn)
}

/// The `expirationDate` the store holds for `resource_arn`, as epoch seconds — the
/// only way to observe it, since no operation of this model reports it back.
fn stored_expiration(
    engine: &engine::InMemoryEngine,
    case: &str,
    resource_arn: &str,
) -> Option<i64> {
    let state = service_state(engine).expect("the shared engine must hold this service's store");
    let store = state.lock();
    let record = store
        .parameters
        .get(resource_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store must still hold {resource_arn}"));
    record.expiration_date.map(|at| at.secs())
}

/// One row of the expiry table: a date (or none) and whether it has already passed.
struct Expiry {
    case: &'static str,
    /// `expirationDate` as epoch seconds, as the wire carries it. `None` omits the
    /// member.
    expiration_date: Option<i64>,
    /// Whether the parameter that date names is expired at the time the row runs.
    expired: bool,
}

/// The expiry table, built against `now` so the "just past" / "just ahead" rows sit
/// around the real clock rather than a fixed instant.
fn expiry_table(now: i64) -> Vec<Expiry> {
    vec![
        Expiry {
            case: "no expirationDate at all",
            expiration_date: None,
            expired: false,
        },
        Expiry {
            case: "the Unix epoch",
            expiration_date: Some(0),
            expired: true,
        },
        Expiry {
            case: "2020-01-01T00:00:00Z",
            expiration_date: Some(PAST_2020),
            expired: true,
        },
        Expiry {
            case: "a day ago",
            expiration_date: Some(now - ONE_DAY),
            expired: true,
        },
        Expiry {
            case: "one second ago",
            expiration_date: Some(now - 1),
            expired: true,
        },
        Expiry {
            case: "the due instant itself (this second)",
            expiration_date: Some(now),
            expired: true,
        },
        Expiry {
            case: "an hour from now",
            expiration_date: Some(now + 3_600),
            expired: false,
        },
        Expiry {
            case: "a day from now",
            expiration_date: Some(now + ONE_DAY),
            expired: false,
        },
        Expiry {
            case: "2100-01-01T00:00:00Z",
            expiration_date: Some(FUTURE_2100),
            expired: false,
        },
    ]
}

#[tokio::test]
async fn an_expiration_date_in_the_past_takes_the_parameter_out_of_every_path() {
    let (router, engine) = emulator();
    let created_tags = tag_map(&[("env", "test")]);

    for (index, row) in expiry_table(now_epoch_secs()).into_iter().enumerate() {
        let case = row.case;
        // One identity per row, so a row's expiry decides only its own resource.
        let product_id = format!("prod-expiry-{index:02}");
        let agreement_id = format!("agmt-expiry-{index:02}");
        let name = format!("Parameter{index:02}");
        provision(&engine, CATALOG, &agreement_id, &[&product_id]);

        // --- create -------------------------------------------------------
        let (status, created) = put(
            &router,
            CATALOG,
            &product_id,
            &put_body(
                &agreement_id,
                &name,
                "s3cr3t",
                Some(&created_tags),
                row.expiration_date,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutDeploymentParameter must succeed, got {status} {created:?}"
        );
        let (first_id, arn) = identity_of(case, &created);

        // --- ListTagsForResource ------------------------------------------
        let (status, listed) = list_tags(&router, &arn).await;
        if row.expired {
            assert_eq!(
                status, NOT_FOUND_STATUS,
                "case {case:?}: an expired parameter must not be listed, got {status} {listed:?}"
            );
            assert_eq!(
                error_code(&listed),
                NOT_FOUND_CODE,
                "case {case:?}: expected the code {NOT_FOUND_CODE}, got {listed:?}"
            );
            assert_eq!(
                message(&listed),
                NOT_FOUND_TEXT,
                "case {case:?}: expected the text {NOT_FOUND_TEXT:?}, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a live parameter must be listed, got {status} {listed:?}"
            );
            assert_eq!(
                tags_of(&listed),
                created_tags,
                "case {case:?}: expected the stored tag map, got {listed:?}"
            );
        }

        // --- TagResource --------------------------------------------------
        let added = tag_map(&[("team", "ops")]);
        let (status, tagged) = tag(&router, &arn, &added).await;
        if row.expired {
            assert_eq!(
                status, NOT_FOUND_STATUS,
                "case {case:?}: TagResource on an expired parameter must be the not-found failure, got {status} {tagged:?}"
            );
            assert_eq!(
                error_code(&tagged),
                NOT_FOUND_CODE,
                "case {case:?}: expected the code {NOT_FOUND_CODE}, got {tagged:?}"
            );
        } else {
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource on a live parameter must succeed, got {status} {tagged:?}"
            );
        }

        // --- UntagResource ------------------------------------------------
        let (status, untagged) = untag(&router, &arn, &["env".to_string()]).await;
        if row.expired {
            assert_eq!(
                status, NOT_FOUND_STATUS,
                "case {case:?}: UntagResource on an expired parameter must be the not-found failure, got {status} {untagged:?}"
            );
            assert_eq!(
                error_code(&untagged),
                NOT_FOUND_CODE,
                "case {case:?}: expected the code {NOT_FOUND_CODE}, got {untagged:?}"
            );
        } else {
            assert_eq!(
                status, 204,
                "case {case:?}: UntagResource on a live parameter must succeed, got {status} {untagged:?}"
            );
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, tags_of(&listed)),
                (200, added.clone()),
                "case {case:?}: the tag writes must leave exactly {added:?}, got {status} {listed:?}"
            );
        }

        // --- a repeat put for the SAME identity ---------------------------
        let (status, again) = put(
            &router,
            CATALOG,
            &product_id,
            &put_body(
                &agreement_id,
                &name,
                "rotated",
                Some(&created_tags),
                row.expiration_date,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat PutDeploymentParameter must succeed, got {status} {again:?}"
        );
        let (second_id, _) = identity_of(case, &again);
        if row.expired {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: an expired parameter is gone, so the repeat put must mint a NEW \
                 deploymentParameterId, got {second_id:?}"
            );
        } else {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: a live parameter is updated in place, so the repeat put must \
                 report {first_id:?}, got {second_id:?}"
            );
        }
    }
}

/// One row of the update table: the date the create names, the date the update names,
/// and the date the store must hold afterwards.
struct Update {
    case: &'static str,
    created_with: Option<i64>,
    updated_with: Option<i64>,
    expected: Option<i64>,
}

#[tokio::test]
async fn an_update_applies_only_the_expiration_date_the_request_carries() {
    let (router, engine) = emulator();
    // Every date below is far in the future, so the record under test stays live and
    // the only thing varying is which member the request carries.
    let first = FUTURE_2100;
    let second = FUTURE_2100 + ONE_DAY;
    let table = vec![
        Update {
            case: "an update that omits expirationDate keeps the create's date",
            created_with: Some(first),
            updated_with: None,
            expected: Some(first),
        },
        Update {
            case: "an update that names a date replaces it",
            created_with: Some(first),
            updated_with: Some(second),
            expected: Some(second),
        },
        Update {
            case: "an update that omits it on a record that never had one stays unset",
            created_with: None,
            updated_with: None,
            expected: None,
        },
        Update {
            case: "an update may set a date the create never named",
            created_with: None,
            updated_with: Some(second),
            expected: Some(second),
        },
    ];

    for (index, row) in table.into_iter().enumerate() {
        let case = row.case;
        let product_id = format!("prod-update-{index:02}");
        let agreement_id = format!("agmt-update-{index:02}");
        let name = format!("Rotated{index:02}");
        provision(&engine, CATALOG, &agreement_id, &[&product_id]);

        let (status, created) = put(
            &router,
            CATALOG,
            &product_id,
            &put_body(&agreement_id, &name, "first", None, row.created_with),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} {created:?}"
        );
        let (first_id, arn) = identity_of(case, &created);
        assert_eq!(
            stored_expiration(&engine, case, &arn),
            row.created_with,
            "case {case:?}: the create must store exactly the date it named"
        );

        let (status, updated) = put(
            &router,
            CATALOG,
            &product_id,
            &put_body(&agreement_id, &name, "second", None, row.updated_with),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} {updated:?}"
        );
        let (second_id, second_arn) = identity_of(case, &updated);
        assert_eq!(
            (second_id.as_str(), second_arn.as_str()),
            (first_id.as_str(), arn.as_str()),
            "case {case:?}: the same identity must update in place"
        );
        let stored = stored_expiration(&engine, case, &arn);
        assert_eq!(
            stored, row.expected,
            "case {case:?}: expected the stored expirationDate to be {:?}, got {stored:?}",
            row.expected
        );

        // The record is still addressable: none of these dates has passed.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated parameter must still be addressable, got {status} {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_update_that_omits_expiration_date_does_not_cancel_the_scheduled_deletion() {
    let (router, engine) = emulator();
    let product_id = "prod-scheduled";
    let agreement_id = "agmt-scheduled";
    let name = "SoonToExpire";
    let case = "an expiry set on create and untouched by an update still falls due";
    provision(&engine, CATALOG, agreement_id, &[product_id]);

    // A date under two seconds ahead, so the expiry falls due DURING the test: the
    // whole point is that the clock, not the request, removes the resource.
    let due_at = due_second_ahead();
    let (status, created) = put(
        &router,
        CATALOG,
        product_id,
        &put_body(
            agreement_id,
            name,
            "first",
            Some(&tag_map(&[("env", "test")])),
            Some(due_at),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} {created:?}"
    );
    let (first_id, arn) = identity_of(case, &created);

    // The update says NOTHING about expiry, so the scheduled deletion survives it.
    let (status, updated) = put(
        &router,
        CATALOG,
        product_id,
        &put_body(agreement_id, name, "second", None, None),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the update must succeed, got {status} {updated:?}"
    );
    let (second_id, _) = identity_of(case, &updated);
    assert_eq!(
        second_id, first_id,
        "case {case:?}: the update must rewrite the same resource, got {second_id:?}"
    );
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the parameter must be addressable until its date falls due, got {status} {listed:?}"
    );

    // Wait for the clock to pass the stored date, without ever sleeping in a handler.
    let due_secs = u64::try_from(due_at).expect("a positive epoch second");
    let due = SystemTime::UNIX_EPOCH + Duration::from_secs(due_secs);
    let wait = due
        .duration_since(SystemTime::now())
        .unwrap_or_default()
        .saturating_add(Duration::from_millis(150));
    tokio::time::sleep(wait).await;

    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "case {case:?}: past its date the parameter must be gone, got {status} {listed:?}"
    );
    assert_eq!(
        error_code(&listed),
        NOT_FOUND_CODE,
        "case {case:?}: expected the code {NOT_FOUND_CODE}, got {listed:?}"
    );

    // And the identity is free again: a put with no date mints a fresh resource that
    // stays addressable.
    let (status, remade) = put(
        &router,
        CATALOG,
        product_id,
        &put_body(agreement_id, name, "third", None, None),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the put after expiry must succeed, got {status} {remade:?}"
    );
    let (third_id, third_arn) = identity_of(case, &remade);
    assert_ne!(
        third_id, first_id,
        "case {case:?}: the expired resource is gone, so a fresh id must be minted, got {third_id:?}"
    );
    let (status, listed) = list_tags(&router, &third_arn).await;
    assert_eq!(
        (status, tags_of(&listed)),
        (200, BTreeMap::new()),
        "case {case:?}: the fresh resource has no expiry and no tags, got {status} {listed:?}"
    );
}
