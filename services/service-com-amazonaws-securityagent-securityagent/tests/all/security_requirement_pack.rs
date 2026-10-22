// SPDX-License-Identifier: CC0-1.0

//! The security requirement pack create/read path of the SecurityAgent emulator, end
//! to end over the wire: `CreateSecurityRequirementPack` and
//! `GetSecurityRequirementPack`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (sources: this crate's `OPERATIONS.md` "Wire protocol" section and the
//! per-operation `Route` lines). So nothing here passes against a handler that answers
//! a canned body: a value must come back out of the shared store through
//! `GetSecurityRequirementPack` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call:
//! which members survive the round trip, what the status defaults to, that the `packId`
//! is minted per call (so one name may be created any number of times, including
//! concurrently), and what an id the store does not hold answers.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    MANAGEMENT_TYPE_CUSTOMER_MANAGED, MAX_SECURITY_REQUIREMENT_PACKS, PACK_STATUS_ENABLED,
    PACK_STATUS_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness — the router over one shared engine, and the signed `POST
// /<Operation>` call — lives in `crate::support`, shared by every suite in this test
// target rather than copied per suite.
use crate::support::{call, emulator, member};

/// `call`, requiring `expected` and naming the case when it is not.
async fn expect_status(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected: u16,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected {operation} to answer {expected}, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `CreateSecurityRequirementPack`, requiring the modelled 201 Created
/// (`smithy_query(query="operation",operation="CreateSecurityRequirementPack")`:
/// `http {code: 201}`).
async fn create(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    expect_status(router, "CreateSecurityRequirementPack", body, 201, case).await
}

/// `GetSecurityRequirementPack` on `pack_id`, requiring HTTP 200.
async fn get(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Value {
    expect_status(
        router,
        "GetSecurityRequirementPack",
        &json!({"packId": pack_id}),
        200,
        case,
    )
    .await
}

/// The `packId` a create answered.
fn pack_id_of(payload: &Value, case: &str) -> String {
    let pack_id = member(payload, "packId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateSecurityRequirementPack returned no packId: {payload}")
    });
    assert!(
        !pack_id.is_empty(),
        "case {case:?}: expected a non-empty packId, got {pack_id:?}"
    );
    pack_id
}

/// A `date-time` member parsed off the wire, in epoch seconds.
///
/// The member is required to be an RFC 3339 STRING, not an epoch number: `createdAt`
/// and `updatedAt` carry `@timestampFormat("date-time")` in the model, so this is what
/// pins the wire form their member schemas declare.
fn timestamp(payload: &Value, name: &str, case: &str) -> f64 {
    let text = member(payload, name).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} to be an RFC 3339 date-time string, got {payload}"
        )
    });
    DateTime::from_str(&text, Format::DateTime)
        .unwrap_or_else(|e| {
            panic!("case {case:?}: {name} {text:?} is not an RFC 3339 date-time ({e})")
        })
        .as_secs_f64()
}

/// The input rows every round-trip case spans: they differ along which optional
/// members the request carries, the form and the size of each value, and the status
/// asked for — the dimensions `CreateSecurityRequirementPack`'s input generalises over
/// (`smithy_query(query="operation",operation="CreateSecurityRequirementPack")`:
/// `name` required, `description`, `status`, `kmsKeyId` and `tags` optional, none of
/// them length- or pattern-constrained).
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("name only, so the status defaults", json!({"name": "pack-minimal"})),
        (
            "every member, one tag",
            json!({
                "name": "pack-full",
                "description": "every member supplied",
                "status": "ENABLED",
                "kmsKeyId": "alias/securityagent",
                "tags": {"env": "test"},
            }),
        ),
        (
            "disabled at creation",
            json!({"name": "pack-disabled", "status": "DISABLED", "description": "off"}),
        ),
        (
            "enabled at creation",
            json!({"name": "pack-enabled", "status": "ENABLED"}),
        ),
        (
            "description only",
            json!({"name": "pack-described", "description": "only a description"}),
        ),
        (
            "an empty description",
            json!({"name": "pack-empty-description", "description": ""}),
        ),
        (
            // `KmsKeyId` admits "a key ID, key ARN, alias name, or alias ARN" (source:
            // the `KmsKeyId` doc from `smithy_query(query="shape",shape="KmsKeyId")`),
            // so the next four rows carry one of each form.
            "a bare KMS key id",
            json!({"name": "pack-key-id", "kmsKeyId": "12345678-1234-1234-1234-123456789012"}),
        ),
        (
            "a KMS key ARN",
            json!({
                "name": "pack-key-arn",
                "kmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012",
            }),
        ),
        (
            "a KMS alias name",
            json!({"name": "pack-alias-name", "kmsKeyId": "alias/pack-contents"}),
        ),
        (
            "a KMS alias ARN",
            json!({"name": "pack-alias-arn", "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:alias/pack"}),
        ),
        (
            "tags only, several of them",
            json!({
                "name": "pack-tagged",
                "tags": {"env": "prod", "team": "appsec", "cost-center": "4711"},
            }),
        ),
        ("an empty tag map", json!({"name": "pack-untagged", "tags": {}})),
        (
            "a name another row already used",
            json!({"name": "pack-minimal", "description": "a duplicate name"}),
        ),
        (
            "a long name and a non-ASCII description",
            json!({
                "name": format!("pack-{}", "n".repeat(200)),
                "description": "exigences de sécurité — ISO/CEI 27001 ✅",
            }),
        ),
        (
            "a name carrying surrounding whitespace",
            json!({"name": "  pack-padded  "}),
        ),
    ]
}

/// RULE: a create persists the pack and answers its minted `packId`, and the read
/// answers every member from that stored record — the supplied members verbatim, the
/// status the create resolved, and the service-chosen `managementType`, with no vendor
/// and no import status for a customer managed pack. Nothing is echoed from the read
/// request and nothing is a placeholder.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_pack() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for (case, request) in create_rows() {
        let created = create(&router, &request, case).await;
        let pack_id = pack_id_of(&created, case);
        assert!(
            seen_ids.insert(pack_id.clone()),
            "case {case:?}: CreateSecurityRequirementPack reused the packId {pack_id:?}"
        );

        // The create's own answer: the resolved status, and the KMS key as supplied.
        let expected_status = member(&request, "status")
            .unwrap_or_else(|| PACK_STATUS_ENABLED.to_string());
        assert_eq!(
            member(&created, "status").as_deref(),
            Some(expected_status.as_str()),
            "case {case:?}: expected the create to answer status {expected_status:?}, got {created}"
        );
        assert_eq!(
            member(&created, "kmsKeyId"),
            member(&request, "kmsKeyId"),
            "case {case:?}: expected the create to answer kmsKeyId {:?}, got {:?}",
            member(&request, "kmsKeyId"),
            member(&created, "kmsKeyId")
        );

        let got = get(&router, &pack_id, case).await;
        assert_eq!(
            member(&got, "packId").as_deref(),
            Some(pack_id.as_str()),
            "case {case:?}: the read reported another pack: {got}"
        );
        assert_eq!(
            member(&got, "name"),
            member(&request, "name"),
            "case {case:?}: expected the stored name {:?}, got {:?}",
            member(&request, "name"),
            member(&got, "name")
        );
        assert_eq!(
            member(&got, "description"),
            member(&request, "description"),
            "case {case:?}: expected the stored description {:?}, got {:?}",
            member(&request, "description"),
            member(&got, "description")
        );
        assert_eq!(
            member(&got, "kmsKeyId"),
            member(&request, "kmsKeyId"),
            "case {case:?}: expected the stored kmsKeyId {:?}, got {:?}",
            member(&request, "kmsKeyId"),
            member(&got, "kmsKeyId")
        );
        assert_eq!(
            member(&got, "status").as_deref(),
            Some(expected_status.as_str()),
            "case {case:?}: expected the stored status {expected_status:?}, got {got}"
        );
        // A pack created through this API is customer managed, so it carries that
        // management type and no vendor name ("The vendor name for AWS managed packs").
        assert_eq!(
            member(&got, "managementType").as_deref(),
            Some(MANAGEMENT_TYPE_CUSTOMER_MANAGED),
            "case {case:?}: expected managementType {MANAGEMENT_TYPE_CUSTOMER_MANAGED:?}, got {got}"
        );
        assert!(
            got.get("vendorName").is_none(),
            "case {case:?}: a customer managed pack carries no vendorName: {got}"
        );
        // No import has been started for a pack that was just created, so it reports no
        // import status at all rather than a fabricated one.
        assert!(
            got.get("importStatus").is_none(),
            "case {case:?}: a pack with no import carries no importStatus: {got}"
        );

        // Both stamps are real `date-time` values, and a pack never updated was last
        // changed when it was created.
        let created_at = timestamp(&got, "createdAt", case);
        let updated_at = timestamp(&got, "updatedAt", case);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: expected createdAt == updatedAt on a pack that was never \
             updated, got {created_at} and {updated_at}"
        );
        let now = DateTime::from(std::time::SystemTime::now()).as_secs_f64();
        assert!(
            (now - created_at).abs() < 600.0,
            "case {case:?}: expected createdAt near the call, got {created_at} against {now}"
        );

        // The read is a read: a second one answers exactly the same body.
        let again = get(&router, &pack_id, case).await;
        assert_eq!(
            again, got,
            "case {case:?}: two reads of one pack disagreed"
        );
    }
}

/// RULE: a status the caller does not supply defaults to ENABLED, and every value the
/// `SecurityRequirementPackStatus` enum admits is stored and read back as asked.
///
/// The catalog is `PACK_STATUS_VALUES`, taken from
/// `smithy_query(query="shape",shape="SecurityRequirementPackStatus")`
/// (`["ENABLED", "DISABLED"]`), so a value added to the enum shows up here.
#[tokio::test]
async fn the_status_defaults_to_enabled_and_every_enum_value_round_trips() {
    let router = emulator();

    let mut rows: Vec<(String, Option<&str>, &str)> = vec![(
        "no status supplied".to_string(),
        None,
        PACK_STATUS_ENABLED,
    )];
    for status in PACK_STATUS_VALUES {
        rows.push((format!("status {status}"), Some(status), status));
    }

    for (case, asked, expected) in rows {
        let mut request = json!({"name": format!("pack-{case}")});
        if let Some(asked) = asked {
            request["status"] = json!(asked);
        }
        let created = create(&router, &request, &case).await;
        assert_eq!(
            member(&created, "status").as_deref(),
            Some(expected),
            "case {case:?}: expected the create to answer status {expected:?}, got {created}"
        );

        let pack_id = pack_id_of(&created, &case);
        let got = get(&router, &pack_id, &case).await;
        assert_eq!(
            member(&got, "status").as_deref(),
            Some(expected),
            "case {case:?}: expected the stored status {expected:?}, got {got}"
        );
    }
}

/// RULE: the `packId` is minted per call, never derived from the request — so any
/// number of packs may share one name, and each is readable on its own with its own
/// members.
#[tokio::test]
async fn packs_sharing_one_name_are_distinct_records() {
    let router = emulator();

    // The rows differ in how many byte-identical creates are issued for one name.
    for duplicates in [2_usize, 3, 5] {
        let name = format!("shared-pack-name-{duplicates}");
        let case = format!("{duplicates} creates of {name:?}");
        let mut ids: Vec<String> = Vec::new();
        for _ in 0..duplicates {
            let created = create(
                &router,
                &json!({"name": name, "description": "same input every time"}),
                &case,
            )
            .await;
            ids.push(pack_id_of(&created, &case));
        }

        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            duplicates,
            "case {case:?}: expected {duplicates} distinct packIds, got {ids:?}"
        );
        for pack_id in &ids {
            let got = get(&router, pack_id, &case).await;
            assert_eq!(
                member(&got, "name").as_deref(),
                Some(name.as_str()),
                "case {case:?}: expected pack {pack_id:?} to carry the name {name:?}, got {got}"
            );
        }
    }
}

/// RULE: concurrent creates on one name all succeed and all survive — the id is minted
/// under the state lock, so no caller's record overwrites another's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_on_one_name_leave_every_pack_readable() {
    let router = Arc::new(emulator());

    // The rows differ in how many callers race on the one name.
    for racers in [2_usize, 4, 8] {
        let name = format!("raced-pack-name-{racers}");
        let case = format!("{racers} racers on {name:?}");
        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let name = name.clone();
            let case = case.clone();
            tasks.push(tokio::spawn(async move {
                let created = create(
                    &router,
                    &json!({"name": name, "description": format!("racer {racer}")}),
                    &case,
                )
                .await;
                pack_id_of(&created, &case)
            }));
        }

        let mut ids: Vec<String> = Vec::new();
        for task in tasks {
            ids.push(task.await.expect("the racing task completed"));
        }
        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            racers,
            "case {case:?}: expected {racers} distinct packIds, got {ids:?}"
        );
        for pack_id in &ids {
            let got = get(&router, pack_id, &case).await;
            assert_eq!(
                member(&got, "name").as_deref(),
                Some(name.as_str()),
                "case {case:?}: expected pack {pack_id:?} to carry the name {name:?}, got {got}"
            );
        }
    }
}

/// RULE: a request the service cannot act on is refused with HTTP 400
/// `ValidationException` and persists nothing — a blank `name`, a `status` outside the
/// `SecurityRequirementPackStatus` enum, and an empty `packId` on the read.
#[tokio::test]
async fn a_request_outside_the_service_rules_is_refused() {
    let router = emulator();

    // A pack that does exist, so the refusals can be shown not to have disturbed it.
    let before = create(&router, &json!({"name": "untouched-pack"}), "baseline").await;
    let untouched = pack_id_of(&before, "baseline");
    let baseline = get(&router, &untouched, "baseline").await;

    let refused_creates = [
        ("an empty name", json!({"name": ""})),
        ("a blank name", json!({"name": "   "})),
        ("a tab-and-newline name", json!({"name": "\t\n"})),
        (
            "a status in the wrong case",
            json!({"name": "pack-bad-status", "status": "enabled"}),
        ),
        (
            "a status from another enum",
            json!({"name": "pack-bad-status", "status": "PENDING"}),
        ),
        (
            "an empty status",
            json!({"name": "pack-bad-status", "status": ""}),
        ),
    ];
    for (case, request) in refused_creates {
        let answer = call(&router, "CreateSecurityRequirementPack", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the create to be refused with 400, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?} with {}",
            answer.error_code, answer.body
        );
        assert!(
            answer.body.get("packId").is_none(),
            "case {case:?}: a refused create must mint no packId: {}",
            answer.body
        );
    }

    let answer = call(
        &router,
        "GetSecurityRequirementPack",
        &json!({"packId": ""}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"an empty packId\": expected 400, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case \"an empty packId\": expected a ValidationException, got {:?} with {}",
        answer.error_code, answer.body
    );

    // Nothing above touched the pack that does exist.
    let after = get(&router, &untouched, "baseline after the refusals").await;
    assert_eq!(
        after, baseline,
        "the refused requests changed the stored pack"
    );
}

/// RULE: a `packId` the store does not hold is HTTP 404 `ResourceNotFoundException`;
/// the id is matched exactly, so a near miss of a stored id is not found either.
#[tokio::test]
async fn an_unknown_pack_id_is_a_not_found() {
    let router = emulator();
    // Several packs, so the id the near-miss rows mutate carries a letter as well as
    // digits and the upper-cased row really is a different id.
    let mut stored = String::new();
    for index in 0..10 {
        let case = format!("stored pack {index}");
        let created = create(
            &router,
            &json!({"name": format!("stored-pack-{index}")}),
            &case,
        )
        .await;
        stored = pack_id_of(&created, &case);
    }
    assert!(
        stored.chars().any(|c| c.is_ascii_alphabetic()),
        "expected a minted packId carrying a letter, got {stored:?}"
    );

    // The rows differ in how the id misses: never minted, wrong alphabet, wrong length,
    // and the stored id with its case flipped or a character added or dropped.
    let rows = [
        ("a never-minted hex id", "ffffffffffffffffffffffffffffffff".to_string()),
        ("an id that is not hex at all", "no-such-pack".to_string()),
        ("an id of another length", "0".to_string()),
        ("the stored id upper-cased", stored.to_uppercase()),
        ("the stored id with a suffix", format!("{stored}0")),
        (
            "the stored id with its last character dropped",
            stored[..stored.len() - 1].to_string(),
        ),
    ];
    for (case, pack_id) in rows {
        assert_ne!(
            pack_id, stored,
            "case {case:?}: the row must not name the stored pack"
        );
        let answer = call(
            &router,
            "GetSecurityRequirementPack",
            &json!({"packId": pack_id}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {pack_id:?}, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {:?} with {}",
            answer.error_code, answer.body
        );
    }

    // The stored pack is still readable, so a miss did not disturb it.
    let got = get(&router, &stored, "the last stored pack").await;
    assert_eq!(
        member(&got, "packId").as_deref(),
        Some(stored.as_str()),
        "the stored pack is no longer readable: {got}"
    );
}

/// RULE: the store a caller can grow is bounded — at the pack quota the create answers
/// the service's `ServiceQuotaExceededException` (HTTP 402) and stores nothing more,
/// while every pack created up to the bound stays readable.
#[tokio::test]
async fn create_stops_at_the_security_requirement_pack_quota() {
    let router = emulator();
    let mut last = String::new();
    for index in 0..MAX_SECURITY_REQUIREMENT_PACKS {
        let case = format!("pack {index} of the quota");
        let created = create(&router, &json!({"name": format!("quota-pack-{index}")}), &case).await;
        last = pack_id_of(&created, &case);
    }

    let answer = call(
        &router,
        "CreateSecurityRequirementPack",
        &json!({"name": "one-pack-too-many"}),
    )
    .await;
    assert_eq!(
        answer.status, 402,
        "case \"one past the quota\": expected 402, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.error_code, "ServiceQuotaExceededException",
        "case \"one past the quota\": expected a ServiceQuotaExceededException, got {:?} with {}",
        answer.error_code, answer.body
    );

    // The refusal did not drop the pack that filled the quota.
    let got = get(&router, &last, "the last pack inside the quota").await;
    assert_eq!(
        member(&got, "packId").as_deref(),
        Some(last.as_str()),
        "the last pack inside the quota is no longer readable: {got}"
    );
}
