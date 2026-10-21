// SPDX-License-Identifier: CC0-1.0

//! The email-archive lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` —
//! so what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the rule covers
//! (every modelled `RetentionPeriod`, several `ArchiveName` shapes, several
//! archive-count/`PageSize` combinations, each published `PageSize` bound and one past
//! it). The archive the bootstrap issue happened to name is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::ops::types::RETENTION_PERIOD_VALUES;
use service_com_amazonaws_mailmanager_mailmanagersvc::state::MAX_ARCHIVES;
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the ARNs the service returns must carry it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field
            // is this service's signing name (`ses`) and whose 3rd is the region the
            // response's ARNs must use.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an archive and return its `ArchiveId`, failing the test on any refusal.
    async fn create(&self, body: Value) -> String {
        let (status, out) = self.call("CreateArchive", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body:?}: expected CreateArchive to answer 200, got {status} {out:?}"
        );
        out["ArchiveId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {body:?}: CreateArchive returned no ArchiveId: {out:?}"))
            .to_string()
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// `Retention` round-trips for EVERY value the model's `RetentionPeriod` enum declares,
/// and an archive created without one still reports the required member.
///
/// The catalog is `RETENTION_PERIOD_VALUES`, generated from the model's `RetentionPeriod`
/// enum (`smithy_query(query="shape",shape="RetentionPeriod")`), so the row set cannot
/// drift from the model.
#[tokio::test]
async fn create_then_get_round_trips_every_modelled_retention_period() {
    let harness = Harness::new();

    for (index, period) in RETENTION_PERIOD_VALUES.iter().enumerate() {
        let name = format!("retention-{index}");
        let archive_id = harness
            .create(json!({
                "ArchiveName": name,
                "Retention": { "RetentionPeriod": period },
                "Tags": [{ "Key": "team", "Value": "mail" }],
            }))
            .await;

        let (status, out) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {period:?}: expected GetArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out["ArchiveId"].as_str(),
            Some(archive_id.as_str()),
            "case {period:?}: GetArchive must report the created id, got {out:?}"
        );
        assert_eq!(
            out["ArchiveName"].as_str(),
            Some(name.as_str()),
            "case {period:?}: GetArchive must report the created name, got {out:?}"
        );
        assert_eq!(
            out["ArchiveState"].as_str(),
            Some("ACTIVE"),
            "case {period:?}: a freshly created archive is ACTIVE, got {out:?}"
        );
        assert_eq!(
            out["Retention"]["RetentionPeriod"].as_str(),
            Some(*period),
            "case {period:?}: GetArchive must report the stored retention, got {out:?}"
        );
        assert_eq!(
            out["ArchiveArn"].as_str(),
            Some(
                format!("arn:aws:ses:{REGION}:{ACCOUNT}:mail-manager-archive/{archive_id}")
                    .as_str()
            ),
            "case {period:?}: unexpected ArchiveArn, got {out:?}"
        );
        assert!(
            out["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
            "case {period:?}: GetArchive must report CreatedTimestamp, got {out:?}"
        );
        assert!(
            out["LastUpdatedTimestamp"]
                .as_f64()
                .is_some_and(|t| t > 0.0),
            "case {period:?}: GetArchive must report LastUpdatedTimestamp, got {out:?}"
        );
    }

    // No `Retention` supplied: the model declares no default for the input member yet
    // makes the output member required, so the record must still carry one.
    let archive_id = harness
        .create(json!({ "ArchiveName": "retention-unspecified" }))
        .await;
    let (status, out) = harness
        .call("GetArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "case retention-unspecified: expected 200, got {status} {out:?}"
    );
    let reported = out["Retention"]["RetentionPeriod"].as_str();
    assert!(
        reported.is_some_and(|value| RETENTION_PERIOD_VALUES.contains(&value)),
        "case retention-unspecified: Retention must be a modelled RetentionPeriod, got {out:?}"
    );
}

/// An `ArchiveId` the service never minted is a `ResourceNotFoundException` (404) on
/// `GetArchive` — the operation that models it — for ids of every shape, and never a
/// hollow 200.
#[tokio::test]
async fn get_archive_refuses_an_unknown_id() {
    let harness = Harness::new();
    // One real archive exists, so the refusal is about the id and not about emptiness.
    harness
        .create(json!({ "ArchiveName": "the-only-archive" }))
        .await;

    // `ArchiveIdString` is len 1..66: the single character, a plausible-looking id, a
    // 66-character id at the modelled maximum, and the id of a never-created name.
    let cases = [
        "x",
        "does-not-exist",
        &"0".repeat(66),
        "00000000000000000000000000000000",
    ];
    for case in cases {
        let (status, out) = harness.call("GetArchive", json!({ "ArchiveId": case })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetArchive to answer 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// An `ArchiveName` already in use is a `ConflictException` (409), whatever the name's
/// shape; a repeat with the SAME `ClientToken` is a retry and returns the original id.
#[tokio::test]
async fn archive_names_are_unique_and_client_tokens_are_idempotent() {
    let harness = Harness::new();

    // `ArchiveNameString` is len 1..64 with pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$`: the shortest name the pattern admits,
    // one using both inner punctuation characters, a digits-only name, and a name at the
    // modelled 64-character maximum.
    let names = [
        "ab".to_string(),
        "demo_archive-01".to_string(),
        "2024".to_string(),
        format!("a{}z", "x".repeat(62)),
    ];

    for (index, name) in names.iter().enumerate() {
        let token = format!("tok-{index}");
        let first = harness
            .create(json!({ "ArchiveName": name, "ClientToken": token }))
            .await;

        // Same token: a retry, not a second archive.
        let repeat = harness
            .create(json!({ "ArchiveName": name, "ClientToken": token }))
            .await;
        assert_eq!(
            repeat, first,
            "case {name:?}: a retry with ClientToken {token:?} must return the original id"
        );

        // Fresh token, same name: a duplicate.
        let (status, out) = harness
            .call(
                "CreateArchive",
                json!({ "ArchiveName": name, "ClientToken": format!("fresh-{index}") }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected a duplicate name to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {name:?}: unexpected error code, got {out:?}"
        );
    }

    // Exactly one archive per name survived: no retry created a duplicate.
    let (status, out) = harness.call("ListArchives", json!({})).await;
    assert_eq!(status, 200, "ListArchives must answer 200, got {out:?}");
    let listed = out["Archives"].as_array().expect("Archives is a list").len();
    assert_eq!(
        listed,
        names.len(),
        "expected exactly one archive per name ({}), got {listed}: {out:?}",
        names.len()
    );

    // A token is a retry of ONE request: presented with a different request it is not a
    // retry, and must not be answered with the first request's archive. Rows vary which
    // member differs.
    let differing: [(&str, Value); 4] = [
        (
            "different name",
            json!({ "ArchiveName": "token-reuse-other", "ClientToken": "tok-0" }),
        ),
        (
            "different retention",
            json!({ "ArchiveName": "ab", "ClientToken": "tok-0", "Retention": { "RetentionPeriod": "ONE_YEAR" } }),
        ),
        (
            "different kms key",
            json!({
                "ArchiveName": "ab",
                "ClientToken": "tok-0",
                "KmsKeyArn": "arn:aws:kms:eu-west-1:000000000000:key/abcd",
            }),
        ),
        (
            "different tags",
            json!({
                "ArchiveName": "ab",
                "ClientToken": "tok-0",
                "Tags": [{ "Key": "team", "Value": "mail" }],
            }),
        ),
    ];
    for (case, body) in differing {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a ClientToken for a different request must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // ...and none of those refusals created anything.
    let (_, after) = harness.call("ListArchives", json!({})).await;
    assert_eq!(
        after["Archives"].as_array().map(Vec::len),
        Some(names.len()),
        "a refused token reuse must not create an archive, got {after:?}"
    );
}

/// `ListArchives` reports exactly the archives created so far, honours `PageSize` and
/// round-trips `NextToken`: every archive appears exactly once across the pages, and the
/// final page carries no `NextToken`.
#[tokio::test]
async fn list_archives_pages_every_archive_exactly_once() {
    // Rows vary the two things the rule generalises over: how many archives exist and how
    // large a page is — including no archives at all, a page size of 1, a page that
    // divides the set exactly, one that leaves a remainder, and a page larger than the set.
    let cases: [(usize, Option<i32>); 7] = [
        (0, None),
        (0, Some(5)),
        (1, Some(1)),
        (3, Some(2)),
        (4, Some(2)),
        (5, Some(1)),
        (7, Some(50)),
    ];

    for (total, page_size) in cases {
        let harness = Harness::new();
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            created.push(
                harness
                    .create(json!({ "ArchiveName": format!("page-{index:02}") }))
                    .await,
            );
        }
        created.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let (status, out) = harness.call("ListArchives", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected ListArchives to answer 200, got {status} {out:?}",
                (total, page_size, &body)
            );
            let items = out["Archives"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: Archives must be a list, got {out:?}", (total, page_size)))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "case {:?}: a page must not exceed PageSize, got {} items: {out:?}",
                    (total, page_size),
                    items.len()
                );
            }
            for item in &items {
                let id = item["ArchiveId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {:?}: a listed archive must carry ArchiveId, got {item:?}", (total, page_size)))
                    .to_string();
                assert!(
                    item["ArchiveName"].as_str().is_some(),
                    "case {:?}: a listed archive must carry ArchiveName, got {item:?}",
                    (total, page_size)
                );
                assert_eq!(
                    item["ArchiveState"].as_str(),
                    Some("ACTIVE"),
                    "case {:?}: a listed archive must carry its state, got {item:?}",
                    (total, page_size)
                );
                assert!(
                    item["LastUpdatedTimestamp"].as_f64().is_some(),
                    "case {:?}: a listed archive must carry LastUpdatedTimestamp, got {item:?}",
                    (total, page_size)
                );
                seen.push(id);
            }

            pages += 1;
            token = out
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            match &token {
                // A page that reports more to come must have been a FULL page, otherwise
                // the token is reporting work the page could have carried.
                Some(_) => assert_eq!(
                    Some(items.len()),
                    page_size.map(|size| size as usize),
                    "case {:?}: a non-final page must be full, got {} items: {out:?}",
                    (total, page_size),
                    items.len()
                ),
                None => break,
            }
            assert!(
                pages <= total + 1,
                "case {:?}: paging did not terminate after {pages} pages",
                (total, page_size)
            );
        }

        assert_eq!(
            seen, created,
            "case {:?}: paging must yield every archive exactly once, in id order ({pages} pages)",
            (total, page_size)
        );
    }
}

/// `PageSize` is modelled `range(1..50)` and `NextToken` `length(1..2048)`: each published
/// bound is served and a value past it is refused with `ValidationException` (400).
#[tokio::test]
async fn list_archives_enforces_the_modelled_pagination_bounds() {
    let harness = Harness::new();
    for index in 0..3 {
        harness
            .create(json!({ "ArchiveName": format!("bound-{index}") }))
            .await;
    }

    // Published bounds (`smithy_query(query="operation",operation="ListArchives")`).
    for accepted in [1, 2, 49, 50] {
        let (status, out) = harness
            .call("ListArchives", json!({ "PageSize": accepted }))
            .await;
        assert_eq!(
            status, 200,
            "case PageSize={accepted}: a value inside the modelled range must be served, got {status} {out:?}"
        );
    }

    // One past each bound, an out-of-range token length, and a well-formed token that
    // names no archive this service minted.
    let refused: [(&str, Value); 6] = [
        ("PageSize=0", json!({ "PageSize": 0 })),
        ("PageSize=51", json!({ "PageSize": 51 })),
        ("PageSize=-1", json!({ "PageSize": -1 })),
        ("NextToken=\"\"", json!({ "NextToken": "" })),
        ("NextToken=unknown", json!({ "NextToken": "zzzzzzzzzzzzzzzz" })),
        (
            "NextToken=other-run",
            json!({ "NextToken": "00000000000000000000000000000000" }),
        ),
    ];
    for (case, body) in refused {
        let (status, out) = harness.call("ListArchives", body).await;
        assert_eq!(
            status, 400,
            "case {case}: a value outside the modelled bound must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case}: unexpected error code, got {out:?}"
        );
    }
}

/// `UpdateArchive` mutates the stored record and bumps `LastUpdatedTimestamp`; the next
/// `GetArchive` and `ListArchives` observe the new values. Rows vary which members the
/// update names.
#[tokio::test]
async fn update_archive_is_observed_by_later_reads() {
    // (case, what the update sets, expected name, expected retention)
    let cases: [(&str, Value, Option<&str>, &str); 4] = [
        (
            "rename only",
            json!({ "ArchiveName": "renamed-1" }),
            Some("renamed-1"),
            "SIX_MONTHS",
        ),
        (
            "retention only",
            json!({ "Retention": { "RetentionPeriod": "TEN_YEARS" } }),
            None,
            "TEN_YEARS",
        ),
        (
            "both members",
            json!({ "ArchiveName": "renamed-3", "Retention": { "RetentionPeriod": "PERMANENT" } }),
            Some("renamed-3"),
            "PERMANENT",
        ),
        (
            "no member",
            json!({}),
            None,
            "SIX_MONTHS",
        ),
    ];

    for (index, (case, patch, expected_name, expected_retention)) in cases.into_iter().enumerate() {
        let harness = Harness::new();
        let original_name = format!("update-{index}");
        let archive_id = harness
            .create(json!({
                "ArchiveName": original_name,
                "Retention": { "RetentionPeriod": "SIX_MONTHS" },
            }))
            .await;

        let (_, before) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        let created_at = before["CreatedTimestamp"].as_f64().expect("CreatedTimestamp");
        let updated_before = before["LastUpdatedTimestamp"]
            .as_f64()
            .expect("LastUpdatedTimestamp");

        let mut request = patch.clone();
        request["ArchiveId"] = json!(archive_id);
        let (status, out) = harness.call("UpdateArchive", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: UpdateArchive has an empty output body, got {out:?}"
        );

        let (status, after) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetArchive to answer 200, got {status} {after:?}"
        );
        assert_eq!(
            after["ArchiveName"].as_str(),
            Some(expected_name.unwrap_or(original_name.as_str())),
            "case {case:?}: unexpected ArchiveName after the update, got {after:?}"
        );
        assert_eq!(
            after["Retention"]["RetentionPeriod"].as_str(),
            Some(expected_retention),
            "case {case:?}: unexpected Retention after the update, got {after:?}"
        );
        let updated_after = after["LastUpdatedTimestamp"]
            .as_f64()
            .expect("LastUpdatedTimestamp");
        assert!(
            updated_after > updated_before,
            "case {case:?}: LastUpdatedTimestamp must advance, got {updated_before} then {updated_after}"
        );
        assert_eq!(
            after["CreatedTimestamp"].as_f64(),
            Some(created_at),
            "case {case:?}: CreatedTimestamp must not move, got {after:?}"
        );

        // ListArchives reads the same record.
        let (_, listed) = harness.call("ListArchives", json!({})).await;
        let item = listed["Archives"]
            .as_array()
            .and_then(|items| items.first().cloned())
            .unwrap_or_else(|| panic!("case {case:?}: ListArchives reported nothing: {listed:?}"));
        assert_eq!(
            item["ArchiveName"].as_str(),
            Some(expected_name.unwrap_or(original_name.as_str())),
            "case {case:?}: ListArchives must observe the update, got {item:?}"
        );
    }

    // A rename onto a name another archive already holds conflicts, exactly as a create
    // under that name would.
    let harness = Harness::new();
    let taken = harness.create(json!({ "ArchiveName": "taken" })).await;
    let other = harness.create(json!({ "ArchiveName": "other" })).await;
    let (status, out) = harness
        .call(
            "UpdateArchive",
            json!({ "ArchiveId": other, "ArchiveName": "taken" }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case rename-onto-taken: expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case rename-onto-taken: unexpected error code, got {out:?}"
    );
    // ...and the archive that holds the name is untouched.
    let (_, held) = harness
        .call("GetArchive", json!({ "ArchiveId": taken }))
        .await;
    assert_eq!(
        held["ArchiveName"].as_str(),
        Some("taken"),
        "case rename-onto-taken: the holder of the name must be unchanged, got {held:?}"
    );
}

/// An unknown `ArchiveId` is refused by `UpdateArchive` with the `ResourceNotFoundException`
/// it models, and by `DeleteArchive` — which models no such error — with the modelled
/// `ValidationException`.
#[tokio::test]
async fn mutating_an_unknown_archive_is_refused_with_a_modelled_error() {
    let harness = Harness::new();
    harness.create(json!({ "ArchiveName": "present" })).await;

    for case in ["missing", "00000000000000000000000000000000"] {
        let (status, out) = harness
            .call(
                "UpdateArchive",
                json!({ "ArchiveId": case, "ArchiveName": "whatever" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UpdateArchive to answer 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {out:?}"
        );

        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DeleteArchive to answer 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// `DeleteArchive` MARKS the archive `PENDING_DELETION` rather than erasing it: the
/// archive still resolves through `GetArchive` and still appears in `ListArchives` with
/// that state, and no further modification is accepted in that state.
#[tokio::test]
async fn delete_archive_marks_pending_deletion_and_blocks_further_modification() {
    let harness = Harness::new();

    // Rows: the archive deleted straight after create, and one that was renamed first —
    // the state transition does not depend on the record's history.
    for (index, rename) in [false, true].into_iter().enumerate() {
        let case = if rename { "renamed then deleted" } else { "deleted" };
        let name = format!("delete-{index}");
        let archive_id = harness.create(json!({ "ArchiveName": name })).await;
        if rename {
            let (status, out) = harness
                .call(
                    "UpdateArchive",
                    json!({ "ArchiveId": archive_id, "ArchiveName": format!("{name}-renamed") }),
                )
                .await;
            assert_eq!(status, 200, "case {case:?}: rename failed: {status} {out:?}");
        }

        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: DeleteArchive has an empty output body, got {out:?}"
        );

        // Still readable, now PENDING_DELETION.
        let (status, after) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pending-deletion archive must still resolve, got {status} {after:?}"
        );
        assert_eq!(
            after["ArchiveState"].as_str(),
            Some("PENDING_DELETION"),
            "case {case:?}: unexpected ArchiveState after delete, got {after:?}"
        );

        // Still listed, with that state.
        let (_, listed) = harness.call("ListArchives", json!({})).await;
        let entry = listed["Archives"]
            .as_array()
            .expect("Archives is a list")
            .iter()
            .find(|item| item["ArchiveId"].as_str() == Some(archive_id.as_str()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a pending-deletion archive must still be listed: {listed:?}")
            });
        assert_eq!(
            entry["ArchiveState"].as_str(),
            Some("PENDING_DELETION"),
            "case {case:?}: ListArchives must report the new state, got {entry:?}"
        );

        // "No further modifications can be made in this state."
        let (status, out) = harness
            .call(
                "UpdateArchive",
                json!({ "ArchiveId": archive_id, "ArchiveName": format!("{name}-again") }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected UpdateArchive on a pending-deletion archive to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );

        // And deleting it again is a conflict, not a second success.
        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a repeat DeleteArchive to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// The name a pending-deletion archive holds is still taken: it is marked, not erased.
#[tokio::test]
async fn a_pending_deletion_archive_still_holds_its_name() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "held-name" })).await;
    let (status, _) = harness
        .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status}");

    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "held-name" }))
        .await;
    assert_eq!(
        status, 409,
        "case reuse-pending-name: expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case reuse-pending-name: unexpected error code, got {out:?}"
    );
}

/// The archive store is BOUNDED: a create past the per-account quota is refused with the
/// modelled `ServiceQuotaExceededException`, and the refusal does not consume or corrupt
/// the archives already stored.
#[tokio::test]
async fn create_archive_refuses_a_create_past_the_archive_quota() {
    let harness = Harness::new();

    // Fill the store exactly to the quota the service declares.
    for index in 0..MAX_ARCHIVES {
        harness
            .create(json!({ "ArchiveName": format!("quota-{index:04}") }))
            .await;
    }

    // Rows: a plain create, and one carrying a ClientToken that was never used — neither
    // may be served past the quota.
    let cases: [(&str, Value); 2] = [
        ("plain", json!({ "ArchiveName": "quota-overflow" })),
        (
            "with a fresh ClientToken",
            json!({ "ArchiveName": "quota-overflow-2", "ClientToken": "never-seen" }),
        ),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a create past the quota must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // The refusals stored nothing, and a retry of an existing archive still works.
    let (_, listed) = harness
        .call("ListArchives", json!({ "PageSize": 50 }))
        .await;
    assert_eq!(
        listed["Archives"].as_array().map(Vec::len),
        Some(50),
        "the first page must still be a full page of stored archives, got {listed:?}"
    );

    // Freeing a slot (the 30-day purge is the only eviction, so delete alone does not)
    // still leaves the quota reached, which is the honest answer.
    let (status, _) = harness
        .call("DeleteArchive", json!({ "ArchiveId": first_archive_id(&harness).await }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status}");
    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "quota-after-delete" }))
        .await;
    assert_eq!(
        status, 402,
        "a pending-deletion archive still occupies its slot, got {status} {out:?}"
    );
}

/// The `ArchiveId` of the first archive in `ListArchives` order.
async fn first_archive_id(harness: &Harness) -> String {
    let (_, listed) = harness.call("ListArchives", json!({ "PageSize": 1 })).await;
    listed["Archives"][0]["ArchiveId"]
        .as_str()
        .expect("ListArchives must report at least one archive")
        .to_string()
}

/// `ArchiveName`'s modelled constraints are refused before any handler runs, so a create
/// that would be stored under an invalid name never happens.
#[tokio::test]
async fn create_archive_refuses_names_outside_the_modelled_constraints() {
    let harness = Harness::new();

    // `ArchiveNameString`: len 1..64, pattern `^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$` —
    // one past the length bound, a leading separator, a trailing separator, and a
    // character the pattern excludes.
    let cases: [(&str, Value); 5] = [
        ("65 characters", json!({ "ArchiveName": "a".repeat(65) })),
        ("empty", json!({ "ArchiveName": "" })),
        ("leading dash", json!({ "ArchiveName": "-leading" })),
        ("trailing underscore", json!({ "ArchiveName": "trailing_" })),
        ("excluded character", json!({ "ArchiveName": "has space" })),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ArchiveName outside the model must be refused, got {status} {out:?}"
        );
    }

    // A missing required member is refused too.
    let (status, out) = harness.call("CreateArchive", json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no ArchiveName\": the required member must be enforced, got {status} {out:?}"
    );
}
