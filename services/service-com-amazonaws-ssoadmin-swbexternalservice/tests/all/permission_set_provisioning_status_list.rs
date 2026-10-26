// SPDX-License-Identifier: CC0-1.0

//! `ListPermissionSetProvisioningStatus` as the ENUMERATION of a permission-set
//! provisioning history, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation reports the provisioning requests the instance the
//! request names TRACKS — "lists the status of the permission set provisioning requests for
//! a specified IAM Identity Center instance" — each once, as the tracking view the model's
//! `PermissionSetProvisioningStatusMetadata` declares (`Status`, `RequestId`,
//! `CreatedDate`), so
//!
//! 1. the enumeration is exactly the set of request ids `ProvisionPermissionSet` minted
//!    under that instance, in the order the history happened, and each row's `Status` and
//!    `CreatedDate` are the ones a describe of that id reports at the same instant;
//! 2. `Filter` keeps exactly the rows whose status as of the call is the `StatusValues` it
//!    names, and a status nothing is in selects no row rather than refusing;
//! 3. the enumeration pages the WHOLE history for any page size the model admits, each
//!    item once, and a `NextToken` this enumeration did not issue is refused;
//! 4. listing is a READ: however often the history is enumerated, a later caller sees
//!    exactly what it would have seen had nobody listed — no record is minted, no workflow
//!    advanced and no copy landed early;
//! 5. an instance that serves no provisioning history at all is refused, not answered with
//!    an empty page: an ARN this account holds no instance for is a
//!    `ResourceNotFoundException`, and an ACCOUNT instance — on which the permission-set
//!    surface is withdrawn — a `ValidationException`;
//! 6. the list and the describe agree about which ids exist: a provisioning whose
//!    permission set is gone is absent from both.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the number of tracked provisionings (none, one, several), both
//! `ProvisionTargetType` values the model declares, every value of the `StatusValues` enum
//! the filter can name, each page size from one to past the whole history, and both
//! channels a pagination token can come from.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance. Where a test has to put a tracked workflow on one side
//! of its due instant it moves that stored instant directly — the clock's business, not the
//! wire's.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_engine, router_over, with_store,
    UNKNOWN_INSTANCE_ARN,
};

/// `ProvisionTargetType`, as the model declares its two values
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// Every value of `StatusValues`, the enum `Filter$Status` names, as the model spells them
/// (`IN_PROGRESS | FAILED | SUCCEEDED`).
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
const STATUS_FAILED: &str = "FAILED";
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// Accounts spanning `TargetId`'s class (`length 12..12`, `^\d{12}$`): both ends of the
/// 12-digit range and one in between.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_HIGH: &str = "999999999999";

/// The principal the assignments are granted to; `PrincipalId` takes an identity-store id,
/// which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL: &str = "11111111-2222-3333-4444-555555555555";

/// The exact members the model's `PermissionSetProvisioningStatusMetadata` carries.
const METADATA_MEMBERS: [&str; 3] = ["Status", "RequestId", "CreatedDate"];

/// Members of the FULL `PermissionSetProvisioningStatus` that the metadata view does not
/// carry, so a row cannot be the describe's object wearing the list's name.
const DETAIL_MEMBERS: [&str; 3] = ["PermissionSetArn", "AccountId", "FailureReason"];

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// Move ONE tracked provisioning's stored completion instant to `due_at`, so each side of
/// that instant can be observed without the test sleeping.
///
/// The transition itself is still applied by the service's own `materialize_due`, exactly as
/// in production: this moves only the CLOCK's instant, which is the clock's business and not
/// the wire's. It moves one record rather than all of them (which
/// `permission_set_provisioning.rs`'s helper does) because the filter rule needs a history
/// whose rows are in DIFFERENT statuses at one instant.
fn hold_one(engine: &Arc<engine::InMemoryEngine>, request_id: &str, due_at: SystemTime) {
    let moved = with_store(engine, |store| {
        let mut moved = 0usize;
        for record in &mut store.permission_set_provisionings {
            if record.request_id == request_id {
                record.succeeded_due = due_at;
                moved += 1;
            }
        }
        moved
    });
    assert_eq!(
        moved, 1,
        "exactly one record must be tracked under {request_id}, moved {moved}"
    );
}

/// An instant far enough ahead that a provisioning held at it is still in flight.
fn in_flight() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3_600)
}

/// An instant far enough behind that a provisioning held at it has completed.
fn completed() -> SystemTime {
    SystemTime::now() - Duration::from_secs(60)
}

/// Create a permission set over the wire and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {body}"))
        .to_string()
}

/// Grant one account assignment over the wire — the call that first provisions the
/// permission set into `account_id`, so an `ALL_PROVISIONED_ACCOUNTS` target has accounts to
/// name.
async fn assign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Start one provisioning over the wire and return the `RequestId` it is tracked under.
async fn provision(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    target_type: &str,
    target_id: Option<&str>,
    case: &str,
) -> String {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "TargetType": target_type,
    });
    if let Some(target_id) = target_id {
        request["TargetId"] = json!(target_id);
    }
    let (status, body) = call(router, "ProvisionPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ProvisionPermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSetProvisioningStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the provisioning is tracked by no id: {body}"))
        .to_string()
}

/// Enumerate the history, as `(http status, body)`. `filter` names a `StatusValues` when the
/// row filters by one; `max_results` and `next_token` are sent only when given.
async fn list(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&str>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"InstanceArn": instance_arn});
    if let Some(status) = filter {
        request["Filter"] = json!({"Status": status});
    }
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["NextToken"] = json!(next_token);
    }
    call(router, "ListPermissionSetProvisioningStatus", &request).await
}

/// The rows of a successful enumeration.
async fn list_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&str>,
    case: &str,
) -> Vec<Value> {
    let (status, body) = list(router, instance_arn, filter, None, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the enumeration expected 200, got {status} {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: a history that fits one page carries no NextToken, got {body}"
    );
    rows_of(&body, case)
}

/// The `PermissionSetsProvisioningStatus` list of one answer.
fn rows_of(body: &Value, case: &str) -> Vec<Value> {
    body["PermissionSetsProvisioningStatus"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PermissionSetsProvisioningStatus must be a list, got {body}")
        })
        .clone()
}

/// The `RequestId`s of a set of rows, in the order they were reported.
fn ids_of(rows: &[Value], case: &str) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row["RequestId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a row carries no RequestId: {row}"))
                .to_string()
        })
        .collect()
}

/// The full status object a describe of `request_id` reports.
async fn describe(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribePermissionSetProvisioningStatus",
        &json!({
            "InstanceArn": instance_arn,
            "ProvisionPermissionSetRequestId": request_id,
        }),
    )
    .await
}

/// Everything the store holds about every tracked provisioning: the handle, whether its copy
/// has landed, the definition it carries and the instant it falls due. Two equal snapshots
/// are a store nothing moved.
fn provisioning_snapshot(
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<(String, bool, String, SystemTime)> {
    with_store(engine, |store| {
        store
            .permission_set_provisionings
            .iter()
            .map(|record| {
                (
                    record.request_id.clone(),
                    record.applied,
                    record.revision.clone(),
                    record.succeeded_due,
                )
            })
            .collect()
    })
}

/// One fixture shape: how many provisionings the history holds and what each targets.
struct Fixture {
    label: &'static str,
    /// `(target type, the account the target names)`, one per provisioning, in the order
    /// they are started.
    provisionings: Vec<(&'static str, Option<&'static str>)>,
}

/// The fixture shapes rule 1 is asserted over: no history at all, one provisioning, and a
/// history spanning both target types and several accounts.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            label: "no provisioning has ever run",
            provisionings: vec![],
        },
        Fixture {
            label: "one provisioning of one account",
            provisionings: vec![(TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID))],
        },
        Fixture {
            label: "one provisioning of every provisioned account",
            provisionings: vec![(TARGET_TYPE_ALL_PROVISIONED, None)],
        },
        Fixture {
            label: "a history spanning both target types",
            provisionings: vec![
                (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_LOW)),
                (TARGET_TYPE_ALL_PROVISIONED, None),
                (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_HIGH)),
                (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
            ],
        },
    ]
}

/// Stand a fixture up: an organization instance, one permission set assigned on every
/// fixture account, and the provisionings the shape names. Answers the router, the instance,
/// the engine, the permission set and the minted ids in mint order.
async fn stand_up(
    fixture: &Fixture,
) -> (
    MultiServiceRouter,
    String,
    Arc<engine::InMemoryEngine>,
    String,
    Vec<String>,
) {
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Tracked").await;
    for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            account_id,
            fixture.label,
        )
        .await;
    }
    let mut minted = Vec::new();
    for (target_type, target_id) in &fixture.provisionings {
        minted.push(
            provision(
                &router,
                &instance_arn,
                &permission_set_arn,
                target_type,
                *target_id,
                fixture.label,
            )
            .await,
        );
    }
    (router, instance_arn, engine, permission_set_arn, minted)
}

/// RULE 1 — the enumeration is exactly the tracked history, in the order it happened, each
/// row the model's metadata view of the record the describe reports.
#[tokio::test]
async fn the_enumeration_reports_every_tracked_provisioning_once_as_its_metadata() {
    for fixture in fixtures() {
        let case = fixture.label;
        let (router, instance_arn, engine, _permission_set_arn, minted) =
            stand_up(&fixture).await;
        // Held in flight so the status a row reports cannot move between the list and the
        // describe it is compared with.
        for request_id in &minted {
            hold_one(&engine, request_id, in_flight());
        }

        let rows = list_ok(&router, &instance_arn, None, case).await;
        assert_eq!(
            ids_of(&rows, case),
            minted,
            "case {case:?}: expected the tracked ids in the order they happened, got {rows:?}"
        );

        for row in &rows {
            let request_id = row["RequestId"].as_str().unwrap_or_default().to_string();
            for member in METADATA_MEMBERS {
                assert!(
                    !row[member].is_null(),
                    "case {case:?}: a row must carry {member}, got {row}"
                );
            }
            for member in DETAIL_MEMBERS {
                assert!(
                    row[member].is_null(),
                    "case {case:?}: the metadata view carries no {member}, got {row}"
                );
            }
            assert_eq!(
                row.as_object().map(|members| members.len()),
                Some(METADATA_MEMBERS.len()),
                "case {case:?}: a row carries exactly the metadata members, got {row}"
            );

            // The same record, read the other way: a describe of this id reports the same
            // status and creation date, so the two views cannot disagree.
            let (status, described) = describe(&router, &instance_arn, &request_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the describe of {request_id} expected 200, got {status} {described}"
            );
            let object = &described["PermissionSetProvisioningStatus"];
            for member in METADATA_MEMBERS {
                assert_eq!(
                    row[member], object[member],
                    "case {case:?}: {member} must match the describe of {request_id}, got {row} \
                     against {object}"
                );
            }
            assert_eq!(
                row["Status"].as_str(),
                Some(STATUS_IN_PROGRESS),
                "case {case:?}: a workflow held in flight is in progress, got {row}"
            );
        }
    }
}

/// RULE 2 — `Filter` keeps exactly the rows whose status as of the call is the one it names.
///
/// The history is put in a MIXED state — some workflows held in flight, some past their due
/// instant — so a filter that merely echoed the request, or ignored it, answers the wrong
/// set; and every value of `StatusValues` is asked for, including the one nothing here is in.
#[tokio::test]
async fn the_filter_keeps_exactly_the_provisionings_in_the_status_it_names() {
    let fixture = Fixture {
        label: "a mixed history",
        provisionings: vec![
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_LOW)),
            (TARGET_TYPE_ALL_PROVISIONED, None),
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_HIGH)),
        ],
    };

    // (case, which minted ids are held past their due instant)
    let splits: Vec<(&str, Vec<usize>)> = vec![
        ("every workflow still in flight", vec![]),
        ("every workflow completed", vec![0, 1, 2, 3]),
        ("the first and the last completed", vec![0, 3]),
        ("one completed in the middle", vec![2]),
    ];

    for (label, completed_indexes) in splits {
        let (router, instance_arn, engine, _permission_set_arn, minted) =
            stand_up(&fixture).await;
        for request_id in &minted {
            hold_one(&engine, request_id, in_flight());
        }
        for index in &completed_indexes {
            hold_one(&engine, &minted[*index], completed());
        }
        let expected_succeeded: Vec<String> = minted
            .iter()
            .enumerate()
            .filter(|(index, _)| completed_indexes.contains(index))
            .map(|(_, request_id)| request_id.clone())
            .collect();
        let expected_in_progress: Vec<String> = minted
            .iter()
            .enumerate()
            .filter(|(index, _)| !completed_indexes.contains(index))
            .map(|(_, request_id)| request_id.clone())
            .collect();

        // (the filter, the ids it must select)
        let rows: Vec<(Option<&str>, Vec<String>)> = vec![
            (None, minted.clone()),
            (Some(STATUS_IN_PROGRESS), expected_in_progress),
            (Some(STATUS_SUCCEEDED), expected_succeeded),
            // Nothing fails a provisioning here, so the third value of the enum selects no
            // row — an empty page, not a refusal.
            (Some(STATUS_FAILED), Vec::new()),
        ];

        for (filter, expected) in rows {
            let case = format!("{label}, filtered by {filter:?}");
            let reported = list_ok(&router, &instance_arn, filter, &case).await;
            assert_eq!(
                ids_of(&reported, &case),
                expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            if let Some(wanted) = filter {
                for row in &reported {
                    assert_eq!(
                        row["Status"].as_str(),
                        Some(wanted),
                        "case {case:?}: every row must be in the status asked for, got {row}"
                    );
                }
            }
        }
    }
}

/// RULE 2, continued — a `Filter$Status` outside the `StatusValues` enum the model declares
/// is refused as a `ValidationException`, never answered with an empty page.
#[tokio::test]
async fn a_filter_status_outside_the_modelled_enum_is_refused() {
    let fixture = Fixture {
        label: "an unknown filter status",
        provisionings: vec![(TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID))],
    };
    let (router, instance_arn, _engine, _permission_set_arn, minted) = stand_up(&fixture).await;

    // Values the enum does not declare: a near-miss in case, a near-miss in spelling, and a
    // value from another enum of this model entirely.
    for status in [
        "in_progress",
        "INPROGRESS",
        "LATEST_PERMISSION_SET_PROVISIONED",
        "",
    ] {
        let (code, body) = list(&router, &instance_arn, Some(status), None, None).await;
        assert_eq!(
            code, 400,
            "case {status:?}: expected HTTP 400, got {code} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {status:?}: expected ValidationException, got {body}"
        );
    }

    // A refused request reached no handler, so the history is untouched and still answers.
    let rows = list_ok(&router, &instance_arn, None, fixture.label).await;
    assert_eq!(
        ids_of(&rows, fixture.label),
        minted,
        "case {:?}: the history still answers, got {rows:?}",
        fixture.label
    );
}

/// RULE 3 — the enumeration pages the whole history for any page size the model admits, each
/// item once and in one order; a token this enumeration did not issue is refused.
#[tokio::test]
async fn the_enumeration_pages_the_whole_history_for_any_page_size() {
    let fixture = Fixture {
        label: "a paged history",
        provisionings: vec![
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_LOW)),
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
            (TARGET_TYPE_ALL_PROVISIONED, None),
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_HIGH)),
            (TARGET_TYPE_ALL_PROVISIONED, None),
        ],
    };
    let (router, instance_arn, engine, _permission_set_arn, minted) = stand_up(&fixture).await;
    for request_id in &minted {
        hold_one(&engine, request_id, in_flight());
    }

    // Every page size from one item to past the whole history, and the published maximum —
    // `MaxResults` is `@range(min: 1, max: 100)`.
    for page_size in [Some(1), Some(2), Some(3), Some(5), Some(6), Some(100), None] {
        let case = format!("page size {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = list(
                &router,
                &instance_arn,
                None,
                page_size,
                token.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} expected 200, got {status} {body}"
            );
            let rows = rows_of(&body, &case);
            let limit = page_size.unwrap_or(100) as usize;
            assert!(
                rows.len() <= limit,
                "case {case:?}: page {pages} carries at most {limit} rows, got {}",
                rows.len()
            );
            walked.extend(ids_of(&rows, &case));
            pages += 1;
            assert!(
                pages <= minted.len() + 1,
                "case {case:?}: the enumeration must terminate, walked {walked:?}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, minted,
            "case {case:?}: expected the whole history once, in order, got {walked:?}"
        );
    }

    // A token from a DIFFERENT scope of the same operation — the same instance under a
    // filter — resumes nothing: the cursor carries the filter that selected its rows.
    let (status, filtered) = list(
        &router,
        &instance_arn,
        Some(STATUS_IN_PROGRESS),
        Some(1),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the filtered first page expected 200, got {status} {filtered}"
    );
    let filtered_token = filtered["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the filtered page left items over, so it carries a token: {filtered}"))
        .to_string();

    // (case, the token sent, whether it belongs to this enumeration)
    let foreign: Vec<(&str, String)> = vec![
        ("a token this service never issued", "ZZZZ".to_string()),
        ("a token of hex that decodes to nothing", "abcdef".to_string()),
        ("a token issued for the filtered enumeration", filtered_token),
    ];
    for (case, token) in foreign {
        let (status, body) = list(&router, &instance_arn, None, Some(2), Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 4 — listing is a READ: with every completion held open, the history answers the same
/// rows however often it is enumerated, and nothing in the store moves.
#[tokio::test]
async fn enumerating_changes_nothing_a_later_caller_sees() {
    let fixture = Fixture {
        label: "a repeatedly enumerated history",
        provisionings: vec![
            (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
            (TARGET_TYPE_ALL_PROVISIONED, None),
        ],
    };
    let case = fixture.label;
    let (router, instance_arn, engine, _permission_set_arn, minted) = stand_up(&fixture).await;
    for request_id in &minted {
        hold_one(&engine, request_id, in_flight());
    }

    let before = provisioning_snapshot(&engine);
    let first = list_ok(&router, &instance_arn, None, case).await;
    for attempt in 1..=5 {
        let again = list_ok(&router, &instance_arn, None, case).await;
        assert_eq!(
            again, first,
            "case {case:?}: enumeration {attempt} must answer the same rows, got {again:?}"
        );
        // A filtered enumeration is just as much a read.
        let filtered = list_ok(&router, &instance_arn, Some(STATUS_SUCCEEDED), case).await;
        assert!(
            filtered.is_empty(),
            "case {case:?}: enumeration {attempt} must not complete a workflow, got {filtered:?}"
        );
    }
    let after = provisioning_snapshot(&engine);
    assert_eq!(
        after, before,
        "case {case:?}: listing must leave the tracked workflows alone, got {after:?} after \
         {before:?}"
    );
}

/// RULE 5 — an instance that serves no provisioning history is refused rather than answered
/// with an empty page: an ARN this account holds no instance for is not found, and an
/// account instance (on which the permission-set surface is withdrawn) is a validation
/// failure.
#[tokio::test]
async fn an_instance_that_serves_no_history_is_refused() {
    let (organization_router, organization_instance_arn, _engine) = organization_fixture();
    let (account_router, account_instance_arn) = account_instance_router().await;

    // (case, the router, the instance ARN, the error code expected)
    let rows: Vec<(&str, &MultiServiceRouter, String, &str)> = vec![
        (
            "an instance ARN this account holds no instance for",
            &organization_router,
            UNKNOWN_INSTANCE_ARN.to_string(),
            "ResourceNotFoundException",
        ),
        (
            "an account instance, which serves no permission sets",
            &account_router,
            account_instance_arn.clone(),
            "ValidationException",
        ),
        (
            "an unknown instance ARN on the account that holds an account instance",
            &account_router,
            UNKNOWN_INSTANCE_ARN.to_string(),
            "ResourceNotFoundException",
        ),
    ];

    for (case, router, instance_arn, expected) in rows {
        for filter in [None, Some(STATUS_IN_PROGRESS)] {
            let (status, body) = list(router, &instance_arn, filter, None, None).await;
            assert_eq!(
                status, 400,
                "case {case:?} filtered by {filter:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?} filtered by {filter:?}: expected {expected}, got {body}"
            );
        }
    }

    // The instance that DOES serve the surface answers, so the refusals above are for the
    // instance each row named.
    let rows = list_ok(
        &organization_router,
        &organization_instance_arn,
        None,
        "the organization instance",
    )
    .await;
    assert!(
        rows.is_empty(),
        "an instance nothing was provisioned under reports an empty history, got {rows:?}"
    );
}

/// RULE 6 — the list and the describe agree about which ids exist: deleting a permission set
/// takes its provisionings out of the history, and leaves every other permission set's where
/// they were.
#[tokio::test]
async fn a_provisioning_whose_permission_set_is_gone_leaves_the_history() {
    let case = "a deleted permission set";
    let (router, instance_arn, engine) = organization_fixture();
    let doomed = create_permission_set(&router, &instance_arn, "Doomed").await;
    let kept = create_permission_set(&router, &instance_arn, "Kept").await;

    let mut doomed_ids = Vec::new();
    let mut kept_ids = Vec::new();
    for account_id in [ACCOUNT_LOW, ACCOUNT_MID] {
        doomed_ids.push(
            provision(
                &router,
                &instance_arn,
                &doomed,
                TARGET_TYPE_ONE_ACCOUNT,
                Some(account_id),
                case,
            )
            .await,
        );
        kept_ids.push(
            provision(
                &router,
                &instance_arn,
                &kept,
                TARGET_TYPE_ONE_ACCOUNT,
                Some(account_id),
                case,
            )
            .await,
        );
    }
    for request_id in doomed_ids.iter().chain(kept_ids.iter()) {
        hold_one(&engine, request_id, in_flight());
    }

    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": doomed}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeletePermissionSet expected 200, got {status} {body}"
    );

    let rows = list_ok(&router, &instance_arn, None, case).await;
    assert_eq!(
        ids_of(&rows, case),
        kept_ids,
        "case {case:?}: expected only the surviving permission set's history, got {rows:?}"
    );
    for request_id in &doomed_ids {
        let (status, described) = describe(&router, &instance_arn, request_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the describe of {request_id} expected a refusal, got {status} \
             {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: the describe of {request_id} expected ResourceNotFoundException, \
             got {described}"
        );
    }
    for request_id in &kept_ids {
        let (status, described) = describe(&router, &instance_arn, request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe of {request_id} expected 200, got {status} {described}"
        );
    }
}
