// SPDX-License-Identifier: CC0-1.0

//! The security requirement path of the SecurityAgent emulator, end to end over the
//! wire: `BatchCreateSecurityRequirements` writing into a customer managed pack, each
//! written record read back through `BatchGetSecurityRequirements`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), as a `restJson1` `POST /<OperationName>` with a JSON body signed
//! for the `securityagent` signing name. So nothing here can pass against a handler that
//! answers a canned body: a written value must come back out of the shared store through
//! a read before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: a
//! batch create stores one record per entry whose name the pack does not hold yet and
//! reports every entry whose name it does hold in `errors` — so the rows differ along
//! entry count, which optional members an entry carries, the form of each value, and
//! which names of the batch the pack already holds.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AWS_MANAGED_PACKS, BATCH_REQUIREMENT_ERROR_CONFLICT, BATCH_REQUIREMENT_ERROR_NOT_FOUND,
    BATCH_REQUIREMENT_ERROR_NOT_PROCESSED,
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
        "case {case:?}: expected {operation} to answer {status} {code}, got {} ({}) with {}",
        answer.status,
        answer.error_code,
        answer.body
    );
    answer.body
}

/// A fresh customer managed pack, returning its minted `packId`. The model gives the
/// pack create `http {code: 201}`.
async fn create_pack(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = expect_status(
        router,
        "CreateSecurityRequirementPack",
        &json!({"name": name}),
        201,
        case,
    )
    .await;
    member(&created, "packId")
        .unwrap_or_else(|| panic!("case {case:?}: no packId was minted: {created}"))
}

/// `BatchCreateSecurityRequirements`, which the model gives `http {code: 201}`.
async fn batch_create(
    router: &MultiServiceRouter,
    pack_id: &str,
    entries: &[Value],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchCreateSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirements": entries}),
        201,
        case,
    )
    .await
}

/// `BatchGetSecurityRequirements` on `names`, requiring HTTP 200.
async fn batch_get(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchGetSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirementNames": names}),
        200,
        case,
    )
    .await
}

/// The `securityRequirements` list of a batch response.
fn results(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("securityRequirements")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no securityRequirements list in {payload}"))
        .clone()
}

/// The `errors` list of a batch response — `@required`, so it is present (and empty)
/// even when every entry succeeded.
fn errors(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no errors list in {payload}"))
        .clone()
}

/// The result or error entry naming `name`, or `None` when the list carries none.
fn entry_named<'a>(entries: &'a [Value], field: &str, name: &str) -> Option<&'a Value> {
    entries
        .iter()
        .find(|entry| member(entry, field).as_deref() == Some(name))
}

/// A `@timestampFormat("date-time")` member, in epoch seconds.
///
/// `BatchCreateSecurityRequirementResult$createdAt`/`$updatedAt` and their
/// `BatchGetSecurityRequirementResult` counterparts all carry
/// `timestampFormat: date-time` (`smithy_query(query="shape",
/// shape="BatchCreateSecurityRequirementResult")`), which restJson1 renders as a string.
fn date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {payload}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as an RFC 3339 date-time string, got {value} \
             in {payload}"
        )
    });
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {name} {text:?} is not a date-time: {e}"))
        .as_secs_f64()
}

/// One row of the create table: a batch of entries, named by what makes it different.
///
/// The rows differ along the entry COUNT, which optional member an entry carries
/// (`remediation` is the one optional member of `CreateSecurityRequirementEntry`:
/// `smithy_query(query="shape", shape="CreateSecurityRequirementEntry")`), and the FORM
/// of each value — ASCII, Unicode, long, empty, whitespace-bearing — none of which the
/// rule distinguishes: each entry becomes exactly one stored record.
fn create_rows() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "one entry, every member",
            vec![json!({
                "name": "encrypt-data-at-rest",
                "description": "All stored data is encrypted at rest.",
                "domain": "DataProtection",
                "evaluation": "Check that every store sets a KMS key.",
                "remediation": "Enable encryption on the store."
            })],
        ),
        (
            "one entry, no remediation",
            vec![json!({
                "name": "rotate-credentials",
                "description": "Credentials are rotated.",
                "domain": "IdentityAndAccessManagement",
                "evaluation": "Check the age of every long-lived credential."
            })],
        ),
        (
            "three entries, mixed remediation",
            vec![
                json!({
                    "name": "least-privilege",
                    "description": "Roles grant the least privilege needed.",
                    "domain": "IdentityAndAccessManagement",
                    "evaluation": "Review each policy statement.",
                    "remediation": "Narrow the policy."
                }),
                json!({
                    "name": "log-retention",
                    "description": "Logs are retained for the required period.",
                    "domain": "Logging",
                    "evaluation": "Read each log group's retention."
                }),
                json!({
                    "name": "tls-in-transit",
                    "description": "Traffic is encrypted in transit.",
                    "domain": "NetworkSecurity",
                    "evaluation": "Check every listener's protocol.",
                    "remediation": "Terminate TLS at the listener."
                }),
            ],
        ),
        (
            "values in unusual forms",
            vec![
                json!({
                    "name": "  surrounding whitespace  ",
                    "description": "",
                    "domain": "Données",
                    "evaluation": "Vérifier le chiffrement 🔐",
                    "remediation": ""
                }),
                json!({
                    "name": "requirement.with/odd-chars_0123",
                    "description": "x".repeat(2_048),
                    "domain": "D",
                    "evaluation": "e"
                }),
            ],
        ),
    ]
}

/// Every entry of a batch is stored as its own record, and a read of each name answers
/// that record's own values — never the request's echo, and never a canned body.
#[tokio::test]
async fn batch_create_stores_every_entry_and_reads_back() {
    let router = emulator();

    for (case, entries) in create_rows() {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        let created = batch_create(&router, &pack_id, &entries, case).await;

        let created_results = results(&created, case);
        assert_eq!(
            created_results.len(),
            entries.len(),
            "case {case:?}: expected one result per entry, got {created_results:?}"
        );
        assert!(
            errors(&created, case).is_empty(),
            "case {case:?}: expected no per-entry errors, got {:?}",
            errors(&created, case)
        );

        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().expect("the row names its entry"))
            .collect();
        let read = batch_get(&router, &pack_id, &names, case).await;
        let read_results = results(&read, case);
        assert!(
            errors(&read, case).is_empty(),
            "case {case:?}: a name just created was not found: {:?}",
            errors(&read, case)
        );

        for entry in &entries {
            let name = entry["name"].as_str().expect("the row names its entry");
            let stored = entry_named(&read_results, "name", name).unwrap_or_else(|| {
                panic!("case {case:?}: {name:?} was not read back: {read_results:?}")
            });
            let minted = entry_named(&created_results, "name", name).unwrap_or_else(|| {
                panic!("case {case:?}: {name:?} was not created: {created_results:?}")
            });

            assert_eq!(
                member(stored, "packId").as_deref(),
                Some(pack_id.as_str()),
                "case {case:?}: {name:?} reports the wrong owning pack: {stored}"
            );
            for field in ["description", "domain", "evaluation"] {
                assert_eq!(
                    stored.get(field),
                    entry.get(field),
                    "case {case:?}: {name:?} {field} came back as {:?}, expected {:?}",
                    stored.get(field),
                    entry.get(field)
                );
            }
            assert_eq!(
                stored.get("remediation"),
                entry.get("remediation"),
                "case {case:?}: {name:?} remediation came back as {:?}, expected {:?}",
                stored.get("remediation"),
                entry.get("remediation")
            );
            assert_eq!(
                minted.get("createdAt"),
                stored.get("createdAt"),
                "case {case:?}: {name:?} createdAt moved between the create and the read: \
                 {minted} vs {stored}"
            );

            // A record nothing has updated was last changed when it was created.
            let created_at = date_time(stored, "createdAt", case);
            let updated_at = date_time(stored, "updatedAt", case);
            assert_eq!(
                created_at, updated_at,
                "case {case:?}: {name:?} reports createdAt {created_at} and updatedAt \
                 {updated_at}, expected them equal on a record never updated"
            );
        }
    }
}

/// A batch STOPS at the first entry whose name the pack already holds: that entry is
/// reported as the conflict it is, every entry behind it is reported as one the service
/// never processed and is NOT written, and the entries ahead of it keep their records.
///
/// The rows differ along WHERE in the batch the already-held name sits — first, last, in
/// the middle — and along the batch's length, since that position is what the rule turns
/// on.
#[tokio::test]
async fn a_batch_stops_at_the_first_name_the_pack_already_holds() {
    // (case, the second batch's names, the name the pack already holds)
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("the only name already held", vec!["a-one"], "a-one"),
        (
            "the first of two already held",
            vec!["b-one", "b-two"],
            "b-one",
        ),
        (
            "the last of two already held",
            vec!["c-one", "c-two"],
            "c-two",
        ),
        (
            "the middle of three already held",
            vec!["d-one", "d-two", "d-three"],
            "d-two",
        ),
        (
            "the last of four already held",
            vec!["e-one", "e-two", "e-three", "e-four"],
            "e-four",
        ),
    ];

    let router = emulator();

    // The per-entry codes are WIRE values, so they are pinned here as the literals the
    // recorded duplicate-partial-failure run reports, not only as the constants the
    // handler reads.
    assert_eq!(
        BATCH_REQUIREMENT_ERROR_CONFLICT, "ConflictException",
        "case \"the conflicting entry's code\": got {BATCH_REQUIREMENT_ERROR_CONFLICT:?}"
    );
    assert_eq!(
        BATCH_REQUIREMENT_ERROR_NOT_PROCESSED, "NotProcessed",
        "case \"an unprocessed entry's code\": got {BATCH_REQUIREMENT_ERROR_NOT_PROCESSED:?}"
    );

    for (case, batch, held) in rows {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;

        // The record the second batch meets, with values a re-create would change if it
        // overwrote rather than refused.
        batch_create(
            &router,
            &pack_id,
            &[json!({
                "name": held,
                "description": "the stored description",
                "domain": "the stored domain",
                "evaluation": "the stored evaluation",
                "remediation": "the stored remediation"
            })],
            case,
        )
        .await;

        let second: Vec<Value> = batch
            .iter()
            .map(|name| {
                json!({
                    "name": name,
                    "description": "a second description",
                    "domain": "a second domain",
                    "evaluation": "a second evaluation"
                })
            })
            .collect();
        let answer = batch_create(&router, &pack_id, &second, case).await;

        let stop = batch
            .iter()
            .position(|name| *name == held)
            .expect("the row's held name is in its batch");
        let created = results(&answer, case);
        let failed = errors(&answer, case);
        assert_eq!(
            created.len(),
            stop,
            "case {case:?}: expected the {stop} entry/entries ahead of {held:?} to be \
             created, got {created:?}"
        );
        assert_eq!(
            failed.len(),
            batch.len() - stop,
            "case {case:?}: expected {} per-entry error(s), got {failed:?}",
            batch.len() - stop
        );

        let conflict = entry_named(&failed, "securityRequirementName", held).unwrap_or_else(|| {
            panic!("case {case:?}: {held:?} was not reported in errors: {failed:?}")
        });
        assert_eq!(
            member(conflict, "code").as_deref(),
            Some(BATCH_REQUIREMENT_ERROR_CONFLICT),
            "case {case:?}: {held:?} was reported with the wrong code: {conflict}"
        );
        assert!(
            member(conflict, "message").is_some_and(|message| message.contains(held)),
            "case {case:?}: {held:?} was reported without a message naming it: {conflict}"
        );
        for name in &batch[stop + 1..] {
            let error = entry_named(&failed, "securityRequirementName", name).unwrap_or_else(|| {
                panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
            });
            assert_eq!(
                member(error, "code").as_deref(),
                Some(BATCH_REQUIREMENT_ERROR_NOT_PROCESSED),
                "case {case:?}: {name:?} was reported with the wrong code: {error}"
            );
        }

        // What the STORE holds now, asserted through the read rather than through the
        // create's answer: the held record untouched, the entries ahead of it written,
        // and the entries behind it absent.
        let read = batch_get(&router, &pack_id, &batch, case).await;
        let stored = results(&read, case);
        for (index, name) in batch.iter().enumerate() {
            let record = entry_named(&stored, "name", name);
            if index > stop {
                assert!(
                    record.is_none(),
                    "case {case:?}: {name:?} sits behind the conflict and must not have \
                     been written, got {record:?}"
                );
                let error = entry_named(&errors(&read, case), "securityRequirementName", name)
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the read did not report {name:?} as missing")
                    });
                assert_eq!(
                    member(&error, "code").as_deref(),
                    Some(BATCH_REQUIREMENT_ERROR_NOT_FOUND),
                    "case {case:?}: the read reported {name:?} with the wrong code: {error}"
                );
                continue;
            }
            let record = record
                .unwrap_or_else(|| panic!("case {case:?}: {name:?} is not stored: {stored:?}"));
            let expected_description = if *name == held {
                "the stored description"
            } else {
                "a second description"
            };
            assert_eq!(
                member(record, "description").as_deref(),
                Some(expected_description),
                "case {case:?}: {name:?} carries the wrong description: {record}"
            );
        }
    }
}

/// A name repeated inside ONE batch meets the same rule: the first occurrence creates
/// the record, the second is the conflict that stops the batch, and every occurrence
/// behind it is reported as one the service never processed.
#[tokio::test]
async fn a_name_repeated_within_one_batch_is_created_once() {
    let rows: Vec<(&str, usize)> = vec![
        ("the name sent twice", 2),
        ("the name sent three times", 3),
        ("the name sent five times", 5),
    ];

    let router = emulator();
    for (case, occurrences) in rows {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        let entries: Vec<Value> = (0..occurrences)
            .map(|occurrence| {
                json!({
                    "name": "repeated-requirement",
                    "description": format!("occurrence {occurrence}"),
                    "domain": "DataProtection",
                    "evaluation": "Check it once."
                })
            })
            .collect();

        let answer = batch_create(&router, &pack_id, &entries, case).await;
        assert_eq!(
            results(&answer, case).len(),
            1,
            "case {case:?}: expected exactly one creation, got {:?}",
            results(&answer, case)
        );
        let failed = errors(&answer, case);
        assert_eq!(
            failed.len(),
            occurrences - 1,
            "case {case:?}: expected {} per-entry error(s), got {failed:?}",
            occurrences - 1
        );
        let codes: Vec<Option<String>> = failed.iter().map(|e| member(e, "code")).collect();
        assert_eq!(
            codes[0].as_deref(),
            Some(BATCH_REQUIREMENT_ERROR_CONFLICT),
            "case {case:?}: the second occurrence was reported with the wrong code: {codes:?}"
        );
        for code in &codes[1..] {
            assert_eq!(
                code.as_deref(),
                Some(BATCH_REQUIREMENT_ERROR_NOT_PROCESSED),
                "case {case:?}: an occurrence behind the conflict was reported with the \
                 wrong code: {codes:?}"
            );
        }

        let read = batch_get(&router, &pack_id, &["repeated-requirement"], case).await;
        let stored = results(&read, case);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected one stored record, got {stored:?}"
        );
        assert_eq!(
            member(&stored[0], "description").as_deref(),
            Some("occurrence 0"),
            "case {case:?}: a later occurrence overwrote the first: {}",
            stored[0]
        );
    }
}

/// A name the pack does not hold is reported per entry by the READ as well, beside the
/// names it does hold.
#[tokio::test]
async fn reading_a_name_the_pack_does_not_hold_is_a_per_entry_error() {
    let router = emulator();
    let case = "one held name beside two unheld ones";
    let pack_id = create_pack(&router, "pack for the partial read", case).await;
    batch_create(
        &router,
        &pack_id,
        &[json!({
            "name": "held-requirement",
            "description": "d",
            "domain": "dom",
            "evaluation": "e"
        })],
        case,
    )
    .await;

    // A name of ANOTHER pack is as unheld as a name nothing created: the name is the
    // requirement's identity only WITHIN its pack.
    let other_pack = create_pack(&router, "the other pack", case).await;
    batch_create(
        &router,
        &other_pack,
        &[json!({
            "name": "elsewhere-requirement",
            "description": "d",
            "domain": "dom",
            "evaluation": "e"
        })],
        case,
    )
    .await;

    let read = batch_get(
        &router,
        &pack_id,
        &["held-requirement", "never-created", "elsewhere-requirement"],
        case,
    )
    .await;
    let stored = results(&read, case);
    assert_eq!(
        stored.len(),
        1,
        "case {case:?}: expected only the held name back, got {stored:?}"
    );
    let failed = errors(&read, case);
    assert_eq!(
        failed.len(),
        2,
        "case {case:?}: expected two per-entry errors, got {failed:?}"
    );
    for name in ["never-created", "elsewhere-requirement"] {
        let error = entry_named(&failed, "securityRequirementName", name).unwrap_or_else(|| {
            panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
        });
        assert_eq!(
            member(error, "code").as_deref(),
            Some(BATCH_REQUIREMENT_ERROR_NOT_FOUND),
            "case {case:?}: {name:?} was reported with the wrong code: {error}"
        );
        assert!(
            member(error, "message").is_some_and(|message| message.contains(name)),
            "case {case:?}: {name:?} was reported without a message naming it: {error}"
        );
    }
}

/// The refusals both operations answer for the REQUEST as a whole, over a table of bad
/// requests: a pack id that names nothing, an empty pack id, an entry with no usable
/// name, and a pack AWS manages. Each one writes nothing.
#[tokio::test]
async fn whole_request_refusals() {
    let router = emulator();
    let pack_id = create_pack(&router, "pack for the refusals", "refusals").await;
    let entry = json!({
        "name": "refused-requirement",
        "description": "d",
        "domain": "dom",
        "evaluation": "e"
    });

    // An unknown pack: a requirement is reached through the pack that holds it, so there
    // is no pack to write into or read from. 404 for BOTH operations.
    for operation in [
        "BatchCreateSecurityRequirements",
        "BatchGetSecurityRequirements",
    ] {
        let case = "a packId nothing minted";
        let body = if operation == "BatchCreateSecurityRequirements" {
            json!({"packId": "00000000000000000000000000000fff", "securityRequirements": [entry]})
        } else {
            json!({
                "packId": "00000000000000000000000000000fff",
                "securityRequirementNames": ["refused-requirement"]
            })
        };
        let refusal = expect_refusal(
            &router,
            operation,
            &body,
            404,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert!(
            member(&refusal, "message")
                .is_some_and(|message| message.contains("00000000000000000000000000000fff")),
            "case {case:?}: {operation} did not name the pack it could not find: {refusal}"
        );
    }

    // An empty pack id names no pack and never could: a bad request, not a missing one.
    for operation in [
        "BatchCreateSecurityRequirements",
        "BatchGetSecurityRequirements",
    ] {
        let body = if operation == "BatchCreateSecurityRequirements" {
            json!({"packId": "", "securityRequirements": [entry]})
        } else {
            json!({"packId": "", "securityRequirementNames": ["refused-requirement"]})
        };
        expect_refusal(
            &router,
            operation,
            &body,
            400,
            "ValidationException",
            "an empty packId",
        )
        .await;
    }

    // A name that carries no characters names no requirement, so the whole batch is a
    // bad request — including when only ONE of its entries is nameless.
    let blank_rows: Vec<(&str, Value)> = vec![
        ("the only entry is nameless", json!([{
            "name": "", "description": "d", "domain": "dom", "evaluation": "e"
        }])),
        ("the second entry is nameless", json!([
            {"name": "named", "description": "d", "domain": "dom", "evaluation": "e"},
            {"name": "   ", "description": "d", "domain": "dom", "evaluation": "e"}
        ])),
    ];
    for (case, entries) in blank_rows {
        expect_refusal(
            &router,
            "BatchCreateSecurityRequirements",
            &json!({"packId": &pack_id, "securityRequirements": entries}),
            400,
            "ValidationException",
            case,
        )
        .await;
        // Nothing of the refused batch was written, not even its named entry.
        let read = batch_get(&router, &pack_id, &["named"], case).await;
        assert!(
            results(&read, case).is_empty(),
            "case {case:?}: a refused batch wrote a record: {read}"
        );
    }

    // The contents of a pack AWS ships are the service's, not a caller's to write: the
    // create conflicts with that pack's state, and the read of it still answers 200.
    // The row is every entry of the seeded catalog, so the rule is not asserted on one.
    for managed in AWS_MANAGED_PACKS {
        let case = managed.pack_id;
        let refusal = expect_refusal(
            &router,
            "BatchCreateSecurityRequirements",
            &json!({"packId": managed.pack_id, "securityRequirements": [entry]}),
            409,
            "ConflictException",
            case,
        )
        .await;
        assert!(
            member(&refusal, "message").is_some_and(|message| message.contains(managed.pack_id)),
            "case {case:?}: the refusal did not name the pack: {refusal}"
        );
        let read = batch_get(&router, managed.pack_id, &["refused-requirement"], case).await;
        assert!(
            results(&read, case).is_empty(),
            "case {case:?}: the refused create wrote a record: {read}"
        );
    }
}
