// SPDX-License-Identifier: CC0-1.0

//! The security requirement pack MUTATION path of the SecurityAgent emulator, end to end
//! over the wire: the AWS managed catalog a fresh account holds,
//! `UpdateSecurityRequirementPack` and `DeleteSecurityRequirementPack`, each read back
//! through `GetSecurityRequirementPack` and `ListSecurityRequirementPacks`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), as a `restJson1` `POST /<OperationName>` with a JSON body signed
//! for the `securityagent` signing name. So nothing here can pass against a handler that
//! answers a canned body: a written value must come back out of the shared store through
//! a read before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: which
//! combination of mutable members an update writes (and which it leaves alone), what a
//! read of a deleted pack answers, what the seeded catalog reports, and the wire FORM of
//! the two `@timestampFormat("date-time")` members.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AWS_MANAGED_PACKS, AWS_MANAGED_VENDOR_NAME, MANAGEMENT_TYPE_AWS_MANAGED,
    MANAGEMENT_TYPE_CUSTOMER_MANAGED, PACK_STATUS_DISABLED, PACK_STATUS_ENABLED,
    PACK_STATUS_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member};

/// `call`, requiring `expected` as the HTTP status and naming the case when it is not.
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

/// `call`, requiring a refusal with `status` and `code`, and naming the case otherwise.
async fn expect_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    status: u16,
    code: &str,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (status, code),
        "case {case:?}: expected {operation} to answer {status} {code}, got {} ({}) \
         with {}",
        answer.status,
        answer.error_code,
        answer.body
    );
    answer.body
}

/// `CreateSecurityRequirementPack`, returning the minted `packId`. The model gives the
/// create `http {code: 201}`.
async fn create_pack(router: &MultiServiceRouter, body: &Value, case: &str) -> String {
    let created = expect_status(router, "CreateSecurityRequirementPack", body, 201, case).await;
    member(&created, "packId")
        .unwrap_or_else(|| panic!("case {case:?}: no packId was minted: {created}"))
}

/// `GetSecurityRequirementPack` on `pack_id`, requiring HTTP 200.
async fn get_pack(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Value {
    expect_status(
        router,
        "GetSecurityRequirementPack",
        &json!({"packId": pack_id}),
        200,
        case,
    )
    .await
}

/// Every summary `ListSecurityRequirementPacks` reports, paged to the end.
async fn list_summaries(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"maxResults": 2});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = expect_status(
            router,
            "ListSecurityRequirementPacks",
            &request,
            200,
            case,
        )
        .await;
        let page_summaries = page
            .get("securityRequirementPackSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing carried no summary list: {page}")
            })
            .clone();
        summaries.extend(page_summaries);
        token = member(&page, "nextToken");
        if token.is_none() {
            return summaries;
        }
        assert!(
            summaries.len() < 1_000,
            "case {case:?}: the listing did not terminate"
        );
    }
}

/// The summary of `pack_id` in the listing, or `None` when the listing does not report
/// it.
async fn listed(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Option<Value> {
    list_summaries(router, case)
        .await
        .into_iter()
        .find(|summary| member(summary, "packId").as_deref() == Some(pack_id))
}

/// A `@timestampFormat("date-time")` member, in epoch seconds.
///
/// The member has to be an RFC 3339 STRING, never an epoch number:
/// `GetSecurityRequirementPackOutput$createdAt`/`$updatedAt` and
/// `SecurityRequirementPackSummary$createdAt`/`$updatedAt` all carry
/// `timestampFormat: date-time` (`smithy_query(query="shape",
/// shape="SecurityRequirementPackSummary")`), which restJson1 renders as a string. This
/// is what pins that wire form, so a member schema that lost the trait fails here.
fn date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {payload}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as an RFC 3339 date-time string, got \
             {value} in {payload}"
        )
    });
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {name} {text:?} is not a date-time: {e}"))
        .as_secs_f64()
}

/// The update requests one case spans, each with the members it is expected to write.
///
/// The rows differ along WHICH subset of the mutable members the request carries —
/// `UpdateSecurityRequirementPackInput` takes `name`, `description` and `status`, all
/// optional (`smithy_query(query="operation",
/// operation="UpdateSecurityRequirementPack")`) — and along the form of each value, so
/// the table is the subsets crossed with a long name, a Unicode description, an empty
/// description and each value of the modelled status catalog.
fn update_rows() -> Vec<(&'static str, Value)> {
    let mut rows: Vec<(&'static str, Value)> = vec![
        ("the name alone", json!({"name": "renamed"})),
        (
            "the description alone",
            json!({"description": "Updated pack description"}),
        ),
        ("the status alone", json!({"status": PACK_STATUS_DISABLED})),
        (
            "the name and the description",
            json!({"name": "renamed again", "description": "and described again"}),
        ),
        (
            "every mutable member at once",
            json!({
                "name": "fully updated",
                "description": "fully updated too",
                "status": PACK_STATUS_DISABLED,
            }),
        ),
        ("an empty description", json!({"description": ""})),
        (
            "a Unicode description",
            json!({"description": "exigences de sécurité — ISO/CEI 27001 ✅"}),
        ),
        ("a long name", json!({"name": "u".repeat(256)})),
        (
            "a name carrying surrounding whitespace",
            json!({"name": "  padded  "}),
        ),
        ("no mutable member at all", json!({})),
    ];
    // Each value the `SecurityRequirementPackStatus` enum admits is a row of its own, so
    // a value added to the enum shows up here.
    for status in PACK_STATUS_VALUES {
        rows.push((
            match status {
                PACK_STATUS_ENABLED => "the status set to ENABLED",
                _ => "the status set to DISABLED",
            },
            json!({"status": status}),
        ));
    }
    rows
}

/// RULE: an update writes every mutable member the request carries to the stored record
/// and leaves every member it does not carry exactly as it was, so a following read
/// answers the updated values; `updatedAt` moves to the instant of the write while
/// `createdAt` stays put.
#[tokio::test]
async fn an_update_is_reflected_by_every_later_read() {
    let router = emulator();

    for (case, update) in update_rows() {
        let pack_id = create_pack(
            &router,
            &json!({
                "name": "initial name",
                "description": "Initial pack description",
                "status": PACK_STATUS_ENABLED,
                "kmsKeyId": "alias/packs",
            }),
            case,
        )
        .await;
        let before = get_pack(&router, &pack_id, case).await;
        let created_at = date_time(&before, "createdAt", case);

        let answer = expect_status(
            &router,
            "UpdateSecurityRequirementPack",
            &{
                let mut request = update.clone();
                request["packId"] = json!(pack_id);
                request
            },
            200,
            case,
        )
        .await;

        // What the update's own answer reports is the record as it now stands.
        let wanted_name = member(&update, "name")
            .unwrap_or_else(|| member(&before, "name").unwrap_or_default());
        let wanted_description = member(&update, "description")
            .or_else(|| member(&before, "description"));
        let wanted_status = member(&update, "status")
            .unwrap_or_else(|| member(&before, "status").unwrap_or_default());
        assert_eq!(
            member(&answer, "packId").as_deref(),
            Some(pack_id.as_str()),
            "case {case:?}: the update answered another pack: {answer}"
        );
        assert_eq!(
            member(&answer, "name").as_deref(),
            Some(wanted_name.as_str()),
            "case {case:?}: expected the update to answer name {wanted_name:?}, got {answer}"
        );
        assert_eq!(
            member(&answer, "description"),
            wanted_description,
            "case {case:?}: expected the update to answer description \
             {wanted_description:?}, got {answer}"
        );
        assert_eq!(
            member(&answer, "status").as_deref(),
            Some(wanted_status.as_str()),
            "case {case:?}: expected the update to answer status {wanted_status:?}, got \
             {answer}"
        );

        // And the stored record agrees, read back through the single read...
        let after = get_pack(&router, &pack_id, case).await;
        assert_eq!(
            member(&after, "name").as_deref(),
            Some(wanted_name.as_str()),
            "case {case:?}: expected the stored name {wanted_name:?}, got {after}"
        );
        assert_eq!(
            member(&after, "description"),
            wanted_description,
            "case {case:?}: expected the stored description {wanted_description:?}, got \
             {after}"
        );
        assert_eq!(
            member(&after, "status").as_deref(),
            Some(wanted_status.as_str()),
            "case {case:?}: expected the stored status {wanted_status:?}, got {after}"
        );
        // An update writes only the members it carries: the pack keeps its identity,
        // its management type and the KMS key the create supplied.
        assert_eq!(
            member(&after, "kmsKeyId").as_deref(),
            Some("alias/packs"),
            "case {case:?}: an update must not clear the stored kmsKeyId: {after}"
        );
        assert_eq!(
            member(&after, "managementType").as_deref(),
            Some(MANAGEMENT_TYPE_CUSTOMER_MANAGED),
            "case {case:?}: an update must not change the management type: {after}"
        );
        assert_eq!(
            date_time(&after, "createdAt", case),
            created_at,
            "case {case:?}: an update must not move createdAt: {after}"
        );
        let updated_at = date_time(&after, "updatedAt", case);
        if update.as_object().is_some_and(|members| members.is_empty()) {
            // An update that carried nothing to write changed nothing, so the record
            // was last changed when it was created.
            assert_eq!(
                updated_at, created_at,
                "case {case:?}: an update that wrote nothing must not move updatedAt: \
                 {after}"
            );
        } else {
            assert!(
                updated_at > created_at,
                "case {case:?}: expected updatedAt {updated_at} to be later than \
                 createdAt {created_at}: {after}"
            );
        }

        // ...and through the listing, which projects the same record.
        let summary = listed(&router, &pack_id, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the listing lost pack {pack_id:?}"));
        assert_eq!(
            member(&summary, "name").as_deref(),
            Some(wanted_name.as_str()),
            "case {case:?}: expected the summary name {wanted_name:?}, got {summary}"
        );
        assert_eq!(
            member(&summary, "description"),
            wanted_description,
            "case {case:?}: expected the summary description {wanted_description:?}, got \
             {summary}"
        );
        assert_eq!(
            member(&summary, "status").as_deref(),
            Some(wanted_status.as_str()),
            "case {case:?}: expected the summary status {wanted_status:?}, got {summary}"
        );
        assert_eq!(
            date_time(&summary, "createdAt", case),
            created_at,
            "case {case:?}: the summary must report the stored createdAt: {summary}"
        );
        assert_eq!(
            date_time(&summary, "updatedAt", case),
            updated_at,
            "case {case:?}: the summary must report the stored updatedAt: {summary}"
        );
    }
}

/// RULE: successive updates compose — each one writes its own members onto the record the
/// previous one left, and `updatedAt` never goes backwards.
#[tokio::test]
async fn successive_updates_compose_on_the_stored_record() {
    let router = emulator();
    let pack_id = create_pack(
        &router,
        &json!({"name": "generation 0", "description": "description 0"}),
        "the pack every update writes to",
    )
    .await;

    let mut expected_name = "generation 0".to_string();
    let mut expected_description = "description 0".to_string();
    let mut expected_status = PACK_STATUS_ENABLED.to_string();
    let mut previous = date_time(
        &get_pack(&router, &pack_id, "generation 0").await,
        "updatedAt",
        "generation 0",
    );

    for generation in 1..=5 {
        let case = format!("generation {generation}");
        let mut request = json!({"packId": pack_id});
        // Each generation carries a different subset, so the record is only ever partly
        // rewritten and the untouched members have to survive.
        match generation % 3 {
            0 => {
                expected_name = format!("generation {generation}");
                request["name"] = json!(expected_name);
            }
            1 => {
                expected_description = format!("description {generation}");
                request["description"] = json!(expected_description);
            }
            _ => {
                expected_status = if expected_status == PACK_STATUS_ENABLED {
                    PACK_STATUS_DISABLED.to_string()
                } else {
                    PACK_STATUS_ENABLED.to_string()
                };
                request["status"] = json!(expected_status);
            }
        }
        expect_status(
            &router,
            "UpdateSecurityRequirementPack",
            &request,
            200,
            &case,
        )
        .await;

        let got = get_pack(&router, &pack_id, &case).await;
        assert_eq!(
            member(&got, "name").as_deref(),
            Some(expected_name.as_str()),
            "case {case:?}: expected the name {expected_name:?}, got {got}"
        );
        assert_eq!(
            member(&got, "description").as_deref(),
            Some(expected_description.as_str()),
            "case {case:?}: expected the description {expected_description:?}, got {got}"
        );
        assert_eq!(
            member(&got, "status").as_deref(),
            Some(expected_status.as_str()),
            "case {case:?}: expected the status {expected_status:?}, got {got}"
        );
        let updated_at = date_time(&got, "updatedAt", &case);
        assert!(
            updated_at >= previous,
            "case {case:?}: updatedAt went backwards, from {previous} to {updated_at}"
        );
        previous = updated_at;
    }
}

/// RULE: once a pack is deleted every read of its id refuses it with HTTP 404
/// `ResourceNotFoundException` naming that id, and the listing no longer reports it —
/// while every other stored pack is untouched.
#[tokio::test]
async fn a_deleted_pack_is_gone_from_every_read() {
    // Several packs, so a delete can be shown to remove exactly one of them. The rows
    // differ in WHICH of them is deleted: the first, a middle one and the last.
    for victim in [0_usize, 2, 4] {
        // A fresh account per row, so each row's delete is the only one that ran.
        let router = emulator();
        let mut ids = Vec::new();
        for index in 0..5 {
            let case = format!("victim {victim}, pack {index}");
            ids.push(
                create_pack(
                    &router,
                    &json!({"name": format!("pack-{index}"), "description": "doomed or not"}),
                    &case,
                )
                .await,
            )
        }
        let case = format!("deleting pack {victim} of 5");
        let doomed = ids[victim].clone();

        // The delete answers the model's empty 200 body.
        let deleted = expect_status(
            &router,
            "DeleteSecurityRequirementPack",
            &json!({"packId": doomed}),
            200,
            &case,
        )
        .await;
        assert!(
            deleted.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: DeleteSecurityRequirementPack models an empty output, got \
             {deleted}"
        );

        // The single read refuses the id, naming it.
        let refused = expect_refusal(
            &router,
            "GetSecurityRequirementPack",
            &json!({"packId": doomed}),
            404,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        let message = member(&refused, "message")
            .or_else(|| member(&refused, "Message"))
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carried no message: {refused}"));
        assert!(
            message.contains(&doomed),
            "case {case:?}: expected the refusal to name the requested id {doomed:?}, got \
             {message:?}"
        );

        // A second delete of the same id refuses it too: the record is gone, so there is
        // nothing left to delete.
        expect_refusal(
            &router,
            "DeleteSecurityRequirementPack",
            &json!({"packId": doomed}),
            404,
            "ResourceNotFoundException",
            &case,
        )
        .await;

        // The listing dropped exactly that one pack, and every other pack is still
        // readable with its own members.
        let reported: BTreeSet<String> = list_summaries(&router, &case)
            .await
            .iter()
            .filter_map(|summary| member(summary, "packId"))
            .collect();
        assert!(
            !reported.contains(&doomed),
            "case {case:?}: the listing still reports the deleted {doomed:?}: {reported:?}"
        );
        for (index, pack_id) in ids.iter().enumerate() {
            if index == victim {
                continue;
            }
            assert!(
                reported.contains(pack_id),
                "case {case:?}: the listing lost the surviving pack {pack_id:?}: \
                 {reported:?}"
            );
            let got = get_pack(&router, pack_id, &case).await;
            assert_eq!(
                member(&got, "name").as_deref(),
                Some(format!("pack-{index}").as_str()),
                "case {case:?}: a surviving pack lost its name: {got}"
            );
        }
    }
}

/// RULE: a fresh account already holds the AWS managed pack catalog — each entry is
/// readable by its own well-known id and reported by the listing, carrying the AWS
/// vendor, the `AWS_MANAGED` management type, the status it is shipped in, and both
/// `date-time` stamps.
///
/// The table is `AWS_MANAGED_PACKS`, the catalog itself, so an entry added to it is
/// covered here without this case changing.
#[tokio::test]
async fn a_fresh_account_holds_the_aws_managed_catalog() {
    let router = emulator();
    let summaries = list_summaries(&router, "the seeded catalog").await;

    for entry in AWS_MANAGED_PACKS {
        let case = entry.pack_id;
        let got = get_pack(&router, entry.pack_id, case).await;
        assert_eq!(
            member(&got, "packId").as_deref(),
            Some(entry.pack_id),
            "case {case:?}: the read reported another pack: {got}"
        );
        assert_eq!(
            member(&got, "name").as_deref(),
            Some(entry.name),
            "case {case:?}: expected the shipped name {:?}, got {got}",
            entry.name
        );
        assert_eq!(
            member(&got, "status").as_deref(),
            Some(entry.status),
            "case {case:?}: expected the shipped status {:?}, got {got}",
            entry.status
        );
        assert_eq!(
            member(&got, "managementType").as_deref(),
            Some(MANAGEMENT_TYPE_AWS_MANAGED),
            "case {case:?}: expected an {MANAGEMENT_TYPE_AWS_MANAGED} pack, got {got}"
        );
        assert_eq!(
            member(&got, "vendorName").as_deref(),
            Some(AWS_MANAGED_VENDOR_NAME),
            "case {case:?}: expected the vendor {AWS_MANAGED_VENDOR_NAME:?}, got {got}"
        );
        let created_at = date_time(&got, "createdAt", case);
        assert!(
            created_at > 0.0,
            "case {case:?}: expected a real createdAt, got {created_at}"
        );
        date_time(&got, "updatedAt", case);

        let summary = summaries
            .iter()
            .find(|summary| member(summary, "packId").as_deref() == Some(entry.pack_id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing does not report the seeded pack")
            });
        for name in ["name", "status", "managementType", "vendorName"] {
            assert_eq!(
                member(summary, name),
                member(&got, name),
                "case {case:?}: the summary and the read disagree on {name}: {summary} \
                 against {got}"
            );
        }
        assert_eq!(
            date_time(summary, "createdAt", case),
            created_at,
            "case {case:?}: the summary must report the stored createdAt: {summary}"
        );
    }
}

/// RULE: for an AWS managed pack only the status may be written — the metadata belongs
/// to AWS, so a request carrying `name` or `description` is refused (HTTP 400
/// `ValidationException`) and the pack cannot be deleted at all (HTTP 409
/// `ConflictException`); the status update is applied and read back.
#[tokio::test]
async fn an_aws_managed_pack_takes_a_status_update_and_nothing_else() {
    let router = emulator();
    let pack_id = AWS_MANAGED_PACKS[0].pack_id;
    let before = get_pack(&router, pack_id, "the seeded pack").await;

    // The rows differ in which immutable member the request tries to write.
    for (case, request) in [
        ("a name", json!({"packId": pack_id, "name": "renamed"})),
        (
            "a description",
            json!({"packId": pack_id, "description": "described"}),
        ),
        (
            "a name beside a status",
            json!({"packId": pack_id, "name": "renamed", "status": PACK_STATUS_DISABLED}),
        ),
    ] {
        expect_refusal(
            &router,
            "UpdateSecurityRequirementPack",
            &request,
            400,
            "ValidationException",
            case,
        )
        .await;
        let after = get_pack(&router, pack_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the pack exactly as it was"
        );
    }

    // Deleting a pack AWS ships conflicts with the state of that resource.
    expect_refusal(
        &router,
        "DeleteSecurityRequirementPack",
        &json!({"packId": pack_id}),
        409,
        "ConflictException",
        "deleting a seeded pack",
    )
    .await;
    assert_eq!(
        get_pack(&router, pack_id, "after the refused delete").await,
        before,
        "a refused delete must leave the pack readable and unchanged"
    );

    // The status alone IS writable, and the write is durable.
    for status in PACK_STATUS_VALUES {
        let case = format!("the status set to {status}");
        expect_status(
            &router,
            "UpdateSecurityRequirementPack",
            &json!({"packId": pack_id, "status": status}),
            200,
            &case,
        )
        .await;
        let got = get_pack(&router, pack_id, &case).await;
        assert_eq!(
            member(&got, "status").as_deref(),
            Some(status),
            "case {case:?}: expected the stored status {status:?}, got {got}"
        );
        assert_eq!(
            member(&got, "name"),
            member(&before, "name"),
            "case {case:?}: a status update must not touch the shipped name: {got}"
        );
    }
}

/// RULE: a request the mutations cannot act on changes nothing — an id the store does not
/// hold is HTTP 404 `ResourceNotFoundException`, and a value that could never name a
/// record or satisfy the modelled enum is HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_mutation_the_service_cannot_act_on_is_refused_and_writes_nothing() {
    let router = emulator();
    // Several packs, so the baseline id carries a letter as well as digits and the
    // upper-cased row below really is a different id.
    let mut pack_id = String::new();
    for index in 0..10 {
        let case = format!("stored pack {index}");
        pack_id = create_pack(
            &router,
            &json!({"name": "untouched", "description": "untouched too"}),
            &case,
        )
        .await;
    }
    assert!(
        pack_id.chars().any(|c| c.is_ascii_alphabetic()),
        "expected a minted packId carrying a letter, got {pack_id:?}"
    );
    let before = get_pack(&router, &pack_id, "baseline").await;

    // (case, operation, request, status, code)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an update of a never-minted id",
            "UpdateSecurityRequirementPack",
            json!({"packId": "ffffffffffffffffffffffffffffffff", "status": PACK_STATUS_DISABLED}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a delete of a never-minted id",
            "DeleteSecurityRequirementPack",
            json!({"packId": "ffffffffffffffffffffffffffffffff"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an update of the stored id upper-cased",
            "UpdateSecurityRequirementPack",
            json!({"packId": pack_id.to_uppercase(), "status": PACK_STATUS_DISABLED}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a delete of the stored id with a suffix",
            "DeleteSecurityRequirementPack",
            json!({"packId": format!("{pack_id}0")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an update with an empty packId",
            "UpdateSecurityRequirementPack",
            json!({"packId": "", "status": PACK_STATUS_DISABLED}),
            400,
            "ValidationException",
        ),
        (
            "a delete with an empty packId",
            "DeleteSecurityRequirementPack",
            json!({"packId": ""}),
            400,
            "ValidationException",
        ),
        (
            "an update to a status in the wrong case",
            "UpdateSecurityRequirementPack",
            json!({"packId": pack_id, "status": "disabled"}),
            400,
            "ValidationException",
        ),
        (
            "an update to a status from another enum",
            "UpdateSecurityRequirementPack",
            json!({"packId": pack_id, "status": "PENDING"}),
            400,
            "ValidationException",
        ),
        (
            "an update to an empty status",
            "UpdateSecurityRequirementPack",
            json!({"packId": pack_id, "status": ""}),
            400,
            "ValidationException",
        ),
        (
            "an update to a blank name",
            "UpdateSecurityRequirementPack",
            json!({"packId": pack_id, "name": "   "}),
            400,
            "ValidationException",
        ),
    ];

    for (case, operation, request, status, code) in rows {
        expect_refusal(&router, operation, &request, status, code, case).await;
        let after = get_pack(&router, &pack_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused mutation changed the stored pack"
        );
    }
}
