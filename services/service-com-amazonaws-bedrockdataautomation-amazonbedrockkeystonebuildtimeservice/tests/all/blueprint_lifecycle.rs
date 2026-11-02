// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Blueprint lifecycle.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The suites are TABLE-driven: each asserts a rule over a set of inputs that differ
//! along what the rule generalises over (blueprint type, stage, schema size and
//! shape, page size, record count, filter value), not over one recorded request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`,
/// per `OPERATIONS.md`). `can_handle` resolves the route key from it, and the
/// handlers read the region out of its credential scope when minting an ARN.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with a handle on the shared engine, so a case can assert
/// what the store HOLDS and not only what it answers.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request and return its status and decoded JSON body (`Value::Null`
/// for an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, method, uri, body, "us-east-1").await
}

async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
        .body(Bytes::from(serde_json::to_vec(body).expect("serializable body")))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateBlueprint` -> the created `blueprint` object. Panics with the case name
/// on any non-201, so a broken create fails where it happened.
async fn create(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, payload) = call(router, "PUT", "/blueprints/", body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: CreateBlueprint returned no blueprint: {payload:?}"))
}

async fn get_blueprint(router: &MultiServiceRouter, arn: &str, body: &Value) -> (u16, Value) {
    let uri = format!("/blueprints/{}/", encode_label(arn));
    call(router, "POST", &uri, body).await
}

async fn list_blueprints(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/blueprints/", body).await
}

/// Structural check of a minted blueprint ARN against the modeled
/// `BlueprintArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`
/// (source: `smithy_query(shape="Blueprint")`), plus its 128-character bound.
fn assert_well_formed_arn(arn: &str, expected_region: &str, case: &str) {
    assert!(
        arn.len() <= 128,
        "case {case:?}: blueprintArn is {} characters, over the modeled 128: {arn}",
        arn.len()
    );
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: blueprintArn has {} colon-separated fields, expected 6: {arn}",
        fields.len()
    );
    assert_eq!(fields[0], "arn", "case {case:?}: ARN prefix, got {arn}");
    assert_eq!(fields[1], "aws", "case {case:?}: ARN partition, got {arn}");
    assert_eq!(fields[2], "bedrock", "case {case:?}: ARN service, got {arn}");
    assert_eq!(
        fields[3], expected_region,
        "case {case:?}: ARN region, got {arn}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: ARN account must be 12 digits, got {arn}"
    );
    let id = fields[5].strip_prefix("blueprint/").unwrap_or_else(|| {
        panic!("case {case:?}: ARN resource must be blueprint/<id>, got {arn}")
    });
    assert!(
        (12..=36).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "case {case:?}: ARN resource id must be 12..36 of [a-zA-Z0-9-], got {id:?}"
    );
}

/// Every `Type` the model accepts, from `smithy_query(shape="Blueprint")` ->
/// `type` -> `Type` (`["AUDIO", "DOCUMENT", "IMAGE", "VIDEO"]`), crossed with
/// every `BlueprintStage` (`["DEVELOPMENT", "LIVE"]`) plus the absent default,
/// over schemas that differ in size and shape.
const CREATE_CASES: &[(&str, &str, Option<&str>, &str)] = &[
    ("audio-default-stage", "AUDIO", None, r#"{"class":"call"}"#),
    ("document-development", "DOCUMENT", Some("DEVELOPMENT"), r#"{"class":"invoice"}"#),
    ("document-live", "DOCUMENT", Some("LIVE"), r#"{"class":"receipt","fields":[]}"#),
    ("image-live", "IMAGE", Some("LIVE"), "{}"),
    ("video-development", "VIDEO", Some("DEVELOPMENT"), r#"{"class":"ad","nested":{"a":{"b":[1,2,3]}}}"#),
];

#[tokio::test]
async fn create_blueprint_persists_the_record_get_blueprint_reads_back() {
    let router = emulator();

    for (case, kind, stage, schema) in CREATE_CASES {
        let mut body = json!({
            "blueprintName": format!("bp-{case}"),
            "type": kind,
            "schema": schema,
        });
        if let Some(stage) = stage {
            body["blueprintStage"] = json!(stage);
        }

        let created = create(&router, case, &body).await;
        let arn = string_at(&created, "blueprintArn", case);
        assert_well_formed_arn(&arn, "us-east-1", case);
        assert_eq!(
            string_at(&created, "blueprintName", case),
            format!("bp-{case}"),
            "case {case:?}: CreateBlueprint returned the wrong blueprintName: {created:?}"
        );
        assert_eq!(
            string_at(&created, "type", case),
            *kind,
            "case {case:?}: CreateBlueprint returned the wrong type: {created:?}"
        );
        assert_eq!(
            string_at(&created, "schema", case),
            *schema,
            "case {case:?}: CreateBlueprint returned the wrong schema: {created:?}"
        );
        // An absent `blueprintStage` settles at LIVE (`state::DEFAULT_BLUEPRINT_STAGE`).
        assert_eq!(
            string_at(&created, "blueprintStage", case),
            stage.unwrap_or("LIVE"),
            "case {case:?}: CreateBlueprint returned the wrong blueprintStage: {created:?}"
        );
        let created_at = timestamp_at(&created, "creationTime", case);
        assert_eq!(
            timestamp_at(&created, "lastModifiedTime", case),
            created_at,
            "case {case:?}: a blueprint nothing has modified must report \
             lastModifiedTime == creationTime: {created:?}"
        );

        // ROUND TRIP: the read must answer with the record that was stored.
        let (status, payload) = get_blueprint(&router, &arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBlueprint 200, got {status} with {payload:?}"
        );
        let fetched = payload
            .get("blueprint")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: GetBlueprint returned no blueprint"));
        for member in ["blueprintArn", "blueprintName", "type", "schema", "blueprintStage"] {
            assert_eq!(
                string_at(&fetched, member, case),
                string_at(&created, member, case),
                "case {case:?}: GetBlueprint disagreed with CreateBlueprint on {member:?}: \
                 {fetched:?}"
            );
        }
        assert_eq!(
            timestamp_at(&fetched, "creationTime", case),
            created_at,
            "case {case:?}: GetBlueprint must report the stored creationTime: {fetched:?}"
        );
    }
}

/// The region an ARN carries comes from the request, not from a constant.
#[tokio::test]
async fn minted_arns_carry_the_requests_region() {
    // Regions of different shapes and lengths; the ARN's region field is
    // `[a-zA-Z0-9-]*` in the modeled pattern, so each must appear verbatim.
    for (index, region) in ["us-east-1", "eu-west-3", "ap-southeast-4"]
        .into_iter()
        .enumerate()
    {
        let router = emulator();
        let body = json!({
            "blueprintName": format!("region-bp-{index}"),
            "type": "DOCUMENT",
            "schema": r#"{"class":"invoice"}"#,
        });
        let (status, payload) =
            call_in_region(&router, "PUT", "/blueprints/", &body, region).await;
        assert_eq!(
            status, 201,
            "case {region:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
        );
        let arn = string_at(
            payload.get("blueprint").expect("a blueprint"),
            "blueprintArn",
            region,
        );
        assert_well_formed_arn(&arn, region, region);
    }
}

/// A minted ARN identifies ONE record: ids are never repeated, however many
/// blueprints are created back to back (the id carries the store's own sequence
/// number, so even creates inside one nanosecond differ), and a deleted record
/// does not free its id for the next one.
#[tokio::test]
async fn minted_arns_are_unique_per_record() {
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();

    // Enough back-to-back creates that several land inside the same clock tick.
    for i in 0..25usize {
        let created = create(
            &router,
            "unique-arn",
            &json!({
                "blueprintName": format!("unique-arn-bp-{i}"),
                "type": "DOCUMENT",
                "schema": "{}",
            }),
        )
        .await;
        let arn = string_at(&created, "blueprintArn", "unique-arn");
        assert_well_formed_arn(&arn, "us-east-1", "unique-arn");
        assert!(
            !seen.contains(&arn),
            "case {i}: the minted ARN {arn} was already handed out: {seen:?}"
        );
        seen.push(arn);
    }

    // Deleting a record and creating another under the same name still mints a
    // new id: an id is not recycled.
    let victim = seen[0].clone();
    let (status, payload) = call(
        &router,
        "DELETE",
        &format!("/blueprints/{}/", encode_label(&victim)),
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteBlueprint 200, got {status} with {payload:?}"
    );
    let recreated = create(
        &router,
        "unique-arn-recreate",
        &json!({
            "blueprintName": "unique-arn-bp-0",
            "type": "DOCUMENT",
            "schema": "{}",
        }),
    )
    .await;
    let arn = string_at(&recreated, "blueprintArn", "unique-arn-recreate");
    assert!(
        !seen.contains(&arn),
        "recreating a deleted blueprint must mint a new id, got the old {arn}"
    );
}

#[tokio::test]
async fn a_name_already_taken_is_a_conflict() {
    let router = emulator();

    // Names differing in length and in every character class the modeled
    // `^[a-zA-Z0-9-_]+$` pattern allows.
    for name in ["a1", "my-doc-bp", "Blueprint_With_Underscores", "ALLCAPS-9"] {
        let body = json!({
            "blueprintName": name,
            "type": "DOCUMENT",
            "schema": r#"{"class":"invoice"}"#,
        });
        create(&router, name, &body).await;

        // A second create under the same name, with a different schema and type,
        // is still a conflict: the NAME is what collides.
        let second = json!({
            "blueprintName": name,
            "type": "IMAGE",
            "schema": "{}",
        });
        let (status, payload) = call(&router, "PUT", "/blueprints/", &second).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected ConflictException 409, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ConflictException",
            "case {name:?}: wrong error code in {payload:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_blueprint_is_not_found() {
    let router = emulator();

    // Syntactically valid ARNs (the modeled pattern admits each) that name no
    // stored blueprint: ids at the short bound, the long bound, and in between.
    let unknown = [
        "arn:aws:bedrock:us-east-1:000000000000:blueprint/abcdef123456",
        "arn:aws:bedrock:us-east-1:000000000000:blueprint/11111111-2222-3333-4444-555555555555",
        "arn:aws:bedrock:eu-west-1:000000000000:blueprint/bp-0000000001",
    ];
    for arn in unknown {
        let (status, payload) = get_blueprint(&router, arn, &json!({})).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected ResourceNotFoundException 404, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {arn:?}: wrong error code in {payload:?}"
        );

        let uri = format!("/blueprints/{}/", encode_label(arn));
        let (status, payload) = call(
            &router,
            "PUT",
            &uri,
            &json!({ "schema": r#"{"class":"x"}"# }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected UpdateBlueprint 404, got {status} with {payload:?}"
        );

        let (status, payload) = call(&router, "DELETE", &uri, &json!({})).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected DeleteBlueprint 404, got {status} with {payload:?}"
        );
    }
}

#[tokio::test]
async fn update_blueprint_mutates_the_record_and_bumps_last_modified_time() {
    let router = emulator();

    // Updates differing in the members they name and in schema size/shape.
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("schema-only", r#"{"class":"receipt"}"#, None),
        ("schema-and-promote", r#"{"class":"receipt","v":2}"#, Some("LIVE")),
        ("schema-and-demote", "{}", Some("DEVELOPMENT")),
    ];

    for (case, new_schema, new_stage) in cases {
        let created = create(
            &router,
            case,
            &json!({
                "blueprintName": format!("update-{case}"),
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
            }),
        )
        .await;
        let arn = string_at(&created, "blueprintArn", case);
        let created_at = timestamp_at(&created, "creationTime", case);

        // Real elapsed time between the create and the update, so the bumped
        // lastModifiedTime is a genuinely later instant rather than a fabricated one.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let mut body = json!({ "schema": new_schema });
        if let Some(stage) = new_stage {
            body["blueprintStage"] = json!(stage);
        }
        let uri = format!("/blueprints/{}/", encode_label(&arn));
        let (status, payload) = call(&router, "PUT", &uri, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBlueprint 200, got {status} with {payload:?}"
        );
        let updated = payload
            .get("blueprint")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: UpdateBlueprint returned no blueprint"));

        assert_eq!(
            timestamp_at(&updated, "creationTime", case),
            created_at,
            "case {case:?}: an update must not move creationTime: {updated:?}"
        );
        assert!(
            timestamp_at(&updated, "lastModifiedTime", case) > created_at,
            "case {case:?}: expected lastModifiedTime after creationTime {created_at:?}, got {:?}",
            timestamp_at(&updated, "lastModifiedTime", case)
        );

        // The mutation is persisted, not just echoed: the reader sees it.
        let (status, payload) = get_blueprint(&router, &arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBlueprint 200 after update, got {status}"
        );
        let fetched = payload.get("blueprint").expect("a blueprint");
        assert_eq!(
            string_at(fetched, "schema", case),
            *new_schema,
            "case {case:?}: GetBlueprint must report the updated schema: {fetched:?}"
        );
        assert_eq!(
            string_at(fetched, "blueprintStage", case),
            // An update that names no stage leaves the stored one alone, and the create
            // above named none either, so that is the create's default: LIVE.
            new_stage.unwrap_or("LIVE"),
            "case {case:?}: GetBlueprint must report the updated stage: {fetched:?}"
        );
        assert_eq!(
            timestamp_at(fetched, "lastModifiedTime", case),
            timestamp_at(&updated, "lastModifiedTime", case),
            "case {case:?}: the stored lastModifiedTime must be stable across reads: {fetched:?}"
        );
    }
}

#[tokio::test]
async fn delete_blueprint_removes_it_from_reads_and_listings() {
    // Delete the first, middle and last of a set, so removal is not order-sensitive.
    for victim_index in 0..3usize {
        let router = emulator();
        let mut arns = Vec::new();
        for i in 0..3usize {
            let created = create(
                &router,
                &format!("delete-{victim_index}-{i}"),
                &json!({
                    "blueprintName": format!("delete-{victim_index}-{i}"),
                    "type": "DOCUMENT",
                    "schema": r#"{"class":"invoice"}"#,
                }),
            )
            .await;
            arns.push(string_at(&created, "blueprintArn", "delete"));
        }
        let case = format!("victim-{victim_index}");
        let victim = arns[victim_index].clone();

        let uri = format!("/blueprints/{}/", encode_label(&victim));
        let (status, payload) = call(&router, "DELETE", &uri, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBlueprint 200, got {status} with {payload:?}"
        );

        let (status, payload) = get_blueprint(&router, &victim, &json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted blueprint must read 404, got {status} with {payload:?}"
        );

        let (status, payload) = list_blueprints(&router, &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: expected ListBlueprints 200");
        let listed: Vec<String> = payload["blueprints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: blueprints is not a list: {payload:?}"))
            .iter()
            .map(|summary| string_at(summary, "blueprintArn", &case))
            .collect();
        assert!(
            !listed.contains(&victim),
            "case {case:?}: ListBlueprints still reports the deleted {victim}: {listed:?}"
        );
        for survivor in arns.iter().filter(|arn| **arn != victim) {
            assert!(
                listed.contains(survivor),
                "case {case:?}: ListBlueprints dropped the surviving {survivor}: {listed:?}"
            );
        }

        // Deleting it again is a 404: the record is really gone.
        let (status, _) = call(&router, "DELETE", &uri, &json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second DeleteBlueprint must be 404, got {status}"
        );
    }
}

#[tokio::test]
async fn list_blueprints_pages_over_a_stable_order() {
    // The invariant: walking the pages yields every stored blueprint exactly
    // once, in one stable order, and only the final page carries no nextToken.
    // Rows vary the record count and the page size, including page sizes at and
    // above the record count and the modeled `maxResults` minimum of 1.
    let cases: &[(usize, Option<i64>)] = &[
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (5, Some(5)),
        (5, Some(1000)),
        (7, Some(3)),
        (4, None),
    ];

    for (total, page_size) in cases {
        let case = format!("total={total} maxResults={page_size:?}");
        let router = emulator();
        let mut expected = Vec::new();
        for i in 0..*total {
            let created = create(
                &router,
                &case,
                &json!({
                    "blueprintName": format!("page-bp-{i}"),
                    "type": "DOCUMENT",
                    "schema": format!("{{\"class\":\"c{i}\"}}"),
                }),
            )
            .await;
            expected.push(string_at(&created, "blueprintArn", &case));
        }

        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(max) = page_size {
                body["maxResults"] = json!(max);
            }
            if let Some(ref t) = token {
                body["nextToken"] = json!(t);
            }
            let (status, payload) = list_blueprints(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListBlueprints 200, got {status} with {payload:?}"
            );
            let page = payload["blueprints"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: blueprints is not a list: {payload:?}"))
                .clone();
            if let Some(max) = page_size {
                assert!(
                    page.len() as i64 <= *max,
                    "case {case:?}: a page of {} exceeds maxResults {max}",
                    page.len()
                );
            }
            for summary in &page {
                // A list item carries the narrow projection a listing serves — the
                // identity, the version it names and the creation instant — and not the
                // members a `GetBlueprint` read serves.
                assert!(
                    summary.get("creationTime").is_some(),
                    "case {case:?}: summary without creationTime: {summary:?}"
                );
                for wider in ["blueprintStage", "blueprintName", "lastModifiedTime"] {
                    assert!(
                        summary.get(wider).is_none(),
                        "case {case:?}: a list item must not carry {wider:?}: {summary:?}"
                    );
                }
                seen.push(string_at(summary, "blueprintArn", &case));
            }
            pages += 1;
            token = payload
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen.len(),
            *total,
            "case {case:?}: paging yielded {} ARNs for {total} blueprints: {seen:?}",
            seen.len()
        );
        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        sorted_seen.dedup();
        assert_eq!(
            sorted_seen.len(),
            *total,
            "case {case:?}: paging repeated an ARN: {seen:?}"
        );
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_seen, sorted_expected,
            "case {case:?}: paging yielded a different set than was stored"
        );

        // The order is stable: a second full walk yields the same sequence.
        let (_, payload) = list_blueprints(&router, &json!({})).await;
        let full: Vec<String> = payload["blueprints"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|summary| string_at(summary, "blueprintArn", &case))
            .collect();
        assert_eq!(
            full, seen,
            "case {case:?}: the paged order differs from the unpaged order"
        );
    }
}

#[tokio::test]
async fn list_blueprints_honours_its_filters() {
    let router = emulator();

    // Two blueprints per stage, so a stage filter can neither pass everything
    // nor pass nothing by accident.
    let mut by_stage: Vec<(String, String)> = Vec::new();
    for (i, stage) in ["DEVELOPMENT", "LIVE", "DEVELOPMENT", "LIVE"]
        .into_iter()
        .enumerate()
    {
        let created = create(
            &router,
            stage,
            &json!({
                "blueprintName": format!("filter-bp-{i}"),
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
                "blueprintStage": stage,
            }),
        )
        .await;
        by_stage.push((
            stage.to_string(),
            string_at(&created, "blueprintArn", stage),
        ));
    }

    // `blueprintStageFilter` values, from `smithy_query(shape="BlueprintStageFilter")`
    // (`["DEVELOPMENT", "LIVE", "ALL"]`), plus the absent case.
    let stage_cases: &[(Option<&str>, usize)] = &[
        (None, 4),
        (Some("ALL"), 4),
        (Some("DEVELOPMENT"), 2),
        (Some("LIVE"), 2),
    ];
    for (filter, expected_count) in stage_cases {
        let case = format!("blueprintStageFilter={filter:?}");
        let mut body = json!({});
        if let Some(filter) = filter {
            body["blueprintStageFilter"] = json!(filter);
        }
        let (status, payload) = list_blueprints(&router, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        let page = payload["blueprints"].as_array().expect("a list");
        assert_eq!(
            page.len(),
            *expected_count,
            "case {case:?}: expected {expected_count} summaries, got {}: {payload:?}",
            page.len()
        );
        if let Some(expected_stage) = filter.filter(|f| *f != "ALL") {
            // The list item does not carry the stage, so what the filter selected is
            // checked against the ARNs created at that stage.
            let selected: std::collections::BTreeSet<String> = page
                .iter()
                .map(|summary| string_at(summary, "blueprintArn", &case))
                .collect();
            let of_stage: std::collections::BTreeSet<String> = by_stage
                .iter()
                .filter(|(stage, _)| stage == expected_stage)
                .map(|(_, arn)| arn.clone())
                .collect();
            assert_eq!(
                selected, of_stage,
                "case {case:?}: the stage filter selected the wrong blueprints: {payload:?}"
            );
        }
    }

    // `blueprintArn` narrows the listing to one blueprint.
    for (_, arn) in &by_stage {
        let (status, payload) =
            list_blueprints(&router, &json!({ "blueprintArn": arn })).await;
        assert_eq!(status, 200, "case {arn:?}: expected 200, got {status}");
        let page = payload["blueprints"].as_array().expect("a list");
        assert_eq!(
            page.len(),
            1,
            "case {arn:?}: the blueprintArn filter must select exactly one: {payload:?}"
        );
        assert_eq!(
            page[0].get("blueprintArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {arn:?}: the blueprintArn filter selected the wrong record: {payload:?}"
        );
    }

    // `resourceOwner` values, from `smithy_query(shape="ResourceOwner")`
    // (`["SERVICE", "ACCOUNT"]`). Everything stored here was created through
    // CreateBlueprint, so it is ACCOUNT-owned; the SERVICE catalog is empty.
    for (owner, expected_count) in [("ACCOUNT", 4usize), ("SERVICE", 0)] {
        let (status, payload) =
            list_blueprints(&router, &json!({ "resourceOwner": owner })).await;
        assert_eq!(status, 200, "case {owner:?}: expected 200, got {status}");
        assert_eq!(
            payload["blueprints"].as_array().expect("a list").len(),
            expected_count,
            "case {owner:?}: expected {expected_count} summaries, got {payload:?}"
        );
    }

    // A nextToken this service never issued is a bad request, not a page.
    for bogus in ["not-a-token", "zzzz", "00", "6e6f70653a"] {
        let (status, payload) =
            list_blueprints(&router, &json!({ "nextToken": bogus })).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected ValidationException 400, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {bogus:?}: wrong error code in {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_client_token_replay_returns_the_original_blueprint() {
    let router = emulator();

    // Tokens spanning the modeled `ClientToken` length bounds (33..256) and
    // character classes (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`).
    let tokens = [
        "a1234567890123456789012345678901a",
        "Token-With-Hyphens-0123456789012345678901234567890",
        &"z".repeat(200),
    ];

    for (i, token) in tokens.iter().enumerate() {
        let body = json!({
            "blueprintName": format!("idempotent-bp-{i}"),
            "type": "DOCUMENT",
            "schema": r#"{"class":"invoice"}"#,
            "clientToken": token,
        });
        let first = create(&router, token, &body).await;
        let replay = create(&router, token, &body).await;
        assert_eq!(
            first, replay,
            "case {token:?}: a clientToken replay must return the original blueprint"
        );

        // And it created exactly one record, not two.
        let (_, payload) = list_blueprints(
            &router,
            &json!({ "blueprintArn": string_at(&first, "blueprintArn", token) }),
        )
        .await;
        assert_eq!(
            payload["blueprints"].as_array().expect("a list").len(),
            1,
            "case {token:?}: the replay created a second blueprint: {payload:?}"
        );
    }
}

#[tokio::test]
async fn blueprint_versions_snapshot_the_schema_at_the_time_they_are_cut() {
    let router = emulator();

    // A sequence of schemas: each version must keep the schema that was live
    // when it was cut, whatever the live schema later becomes.
    let schemas = [
        r#"{"class":"invoice","v":1}"#,
        r#"{"class":"invoice","v":2,"fields":["total"]}"#,
        "{}",
    ];

    let created = create(
        &router,
        "versioned",
        &json!({
            "blueprintName": "versioned-bp",
            "type": "DOCUMENT",
            "schema": schemas[0],
        }),
    )
    .await;
    let arn = string_at(&created, "blueprintArn", "versioned");
    assert!(
        created.get("blueprintVersion").is_none()
            || created["blueprintVersion"].is_null(),
        "a blueprint with no version cut must report no blueprintVersion: {created:?}"
    );

    let mut cut: Vec<(String, String)> = Vec::new();
    for (i, schema) in schemas.iter().enumerate() {
        let case = format!("version-{}", i + 1);
        if i > 0 {
            // Move the live schema on before cutting the next version.
            let uri = format!("/blueprints/{}/", encode_label(&arn));
            let (status, _) = call(&router, "PUT", &uri, &json!({ "schema": schema })).await;
            assert_eq!(status, 200, "case {case:?}: expected UpdateBlueprint 200");
        }
        let uri = format!("/blueprints/{}/versions/", encode_label(&arn));
        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateBlueprintVersion 201, got {status} with {payload:?}"
        );
        let versioned = payload.get("blueprint").expect("a blueprint");
        let version = string_at(versioned, "blueprintVersion", &case);
        assert_eq!(
            version,
            (i + 1).to_string(),
            "case {case:?}: versions must increase monotonically from 1: {versioned:?}"
        );
        assert_eq!(
            string_at(versioned, "schema", &case),
            *schema,
            "case {case:?}: a version must carry the schema live when it was cut"
        );
        cut.push((version, (*schema).to_string()));
    }

    // Every snapshot is still retrievable, with the schema it was cut from.
    for (version, schema) in &cut {
        let case = format!("read-version-{version}");
        let (status, payload) =
            get_blueprint(&router, &arn, &json!({ "blueprintVersion": version })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBlueprint 200, got {status} with {payload:?}"
        );
        let fetched = payload.get("blueprint").expect("a blueprint");
        assert_eq!(
            string_at(fetched, "schema", &case),
            *schema,
            "case {case:?}: the snapshot schema changed: {fetched:?}"
        );
        assert_eq!(
            string_at(fetched, "blueprintVersion", &case),
            *version,
            "case {case:?}: the wrong version came back: {fetched:?}"
        );
    }

    // A version nothing ever cut does not exist.
    for version in ["4", "99"] {
        let (status, payload) =
            get_blueprint(&router, &arn, &json!({ "blueprintVersion": version })).await;
        assert_eq!(
            status, 404,
            "case {version:?}: an uncut version must read 404, got {status} with {payload:?}"
        );
    }

    // `blueprintVersion` on DeleteBlueprint removes just that snapshot.
    let uri = format!(
        "/blueprints/{}/?blueprintVersion=1",
        encode_label(&arn)
    );
    let (status, payload) = call(&router, "DELETE", &uri, &json!({})).await;
    assert_eq!(
        status, 200,
        "deleting one version must be 200, got {status} with {payload:?}"
    );
    let (status, _) = get_blueprint(&router, &arn, &json!({ "blueprintVersion": "1" })).await;
    assert_eq!(status, 404, "the deleted version must read 404, got {status}");
    let (status, _) = get_blueprint(&router, &arn, &json!({ "blueprintVersion": "2" })).await;
    assert_eq!(status, 200, "a surviving version must still read 200, got {status}");
    let (status, _) = get_blueprint(&router, &arn, &json!({})).await;
    assert_eq!(
        status, 200,
        "the blueprint itself must survive a version delete, got {status}"
    );
}

/// A version number names ONE schema for the life of the record: the next cut's
/// number comes from a monotonic counter, so releasing a snapshot — including
/// the highest one — never hands its number out again.
///
/// The rows differ in WHICH snapshot is released (the highest, a middle one, the
/// lowest, and all of them), because deriving the next number from the surviving
/// maximum only misbehaves for some of those.
#[tokio::test]
async fn a_released_version_number_is_never_handed_out_again() {
    let cases: &[(&str, &[usize])] = &[
        ("highest", &[2]),
        ("middle", &[1]),
        ("lowest", &[0]),
        ("all", &[0, 1, 2]),
    ];

    for (case, released) in cases {
        let router = emulator();
        let created = create(
            &router,
            case,
            &json!({
                "blueprintName": format!("version-reuse-{case}"),
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
            }),
        )
        .await;
        let arn = string_at(&created, "blueprintArn", case);
        let versions_uri = format!("/blueprints/{}/versions/", encode_label(&arn));

        let mut handed_out: Vec<u64> = Vec::new();
        for i in 0..3usize {
            let (status, payload) = call(&router, "POST", &versions_uri, &json!({})).await;
            assert_eq!(
                status, 201,
                "case {case:?}: cut {i} must be 201, got {status} with {payload:?}"
            );
            let version = string_at(
                payload.get("blueprint").expect("a blueprint"),
                "blueprintVersion",
                case,
            );
            handed_out.push(
                version
                    .parse()
                    .unwrap_or_else(|e| panic!("case {case:?}: version {version:?}: {e}")),
            );
        }

        for index in *released {
            let version = handed_out[*index];
            let (status, payload) = call(
                &router,
                "DELETE",
                &format!(
                    "/blueprints/{}/?blueprintVersion={version}",
                    encode_label(&arn)
                ),
                &json!({}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: releasing version {version} must be 200, \
                 got {status} with {payload:?}"
            );
        }

        // Whatever was released, the next cut takes a number no earlier cut used.
        let (status, payload) = call(&router, "POST", &versions_uri, &json!({})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the cut after a release must be 201, got {status} with {payload:?}"
        );
        let next = string_at(
            payload.get("blueprint").expect("a blueprint"),
            "blueprintVersion",
            case,
        );
        let next: u64 = next
            .parse()
            .unwrap_or_else(|e| panic!("case {case:?}: version {next:?}: {e}"));
        assert!(
            handed_out.iter().all(|used| *used < next),
            "case {case:?}: version {next} was already handed out (earlier: {handed_out:?})"
        );
    }
}

/// A `CreateBlueprintVersion` client token resolves only against the blueprint
/// the request names: aimed at another blueprint it is a fresh cut on THAT
/// blueprint, never a replay of someone else's snapshot. The rows differ in
/// where the second call points — the same blueprint (a real replay), a
/// different one, and one that does not exist.
#[tokio::test]
async fn a_version_client_token_is_scoped_to_the_blueprint_it_names() {
    let router = emulator();
    let token = "t1234567890123456789012345678901t";

    let mut arns = Vec::new();
    for i in 0..2usize {
        let created = create(
            &router,
            "token-scope",
            &json!({
                "blueprintName": format!("token-scope-bp-{i}"),
                "type": "DOCUMENT",
                "schema": format!(r#"{{"class":"invoice","i":{i}}}"#),
            }),
        )
        .await;
        arns.push(string_at(&created, "blueprintArn", "token-scope"));
    }

    let first_uri = format!("/blueprints/{}/versions/", encode_label(&arns[0]));
    let (status, payload) = call(&router, "POST", &first_uri, &json!({"clientToken": token})).await;
    assert_eq!(
        status, 201,
        "the first cut must be 201, got {status} with {payload:?}"
    );
    let first = payload.get("blueprint").cloned().expect("a blueprint");

    // Same blueprint: a replay, the same version back, no second snapshot.
    let (status, payload) = call(&router, "POST", &first_uri, &json!({"clientToken": token})).await;
    assert_eq!(status, 201, "case \"same-blueprint\": got {status} with {payload:?}");
    let replayed = payload.get("blueprint").expect("a blueprint");
    assert_eq!(
        string_at(replayed, "blueprintVersion", "same-blueprint"),
        string_at(&first, "blueprintVersion", "same-blueprint"),
        "case \"same-blueprint\": a token replay must return the version it cut: {replayed:?}"
    );

    // Another blueprint: a cut on THAT blueprint, not the other one's version.
    let other_uri = format!("/blueprints/{}/versions/", encode_label(&arns[1]));
    let (status, payload) = call(&router, "POST", &other_uri, &json!({"clientToken": token})).await;
    assert_eq!(
        status, 201,
        "case \"other-blueprint\": got {status} with {payload:?}"
    );
    let other = payload.get("blueprint").expect("a blueprint");
    assert_eq!(
        string_at(other, "blueprintArn", "other-blueprint"),
        arns[1],
        "case \"other-blueprint\": a token must not return another blueprint: {other:?}"
    );

    // A blueprint that does not exist: 404, not someone else's version.
    let missing = "arn:aws:bedrock:us-east-1:000000000000:blueprint/abcdefabcdef";
    let (status, payload) = call(
        &router,
        "POST",
        &format!("/blueprints/{}/versions/", encode_label(missing)),
        &json!({"clientToken": token}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unknown-blueprint\": expected 404, got {status} with {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "case \"unknown-blueprint\": wrong error code in {payload:?}"
    );
}

/// `projectFilter` narrows a listing to the blueprints a Data Automation project
/// references, and no project can exist while those operations are still 501 —
/// so every project ARN named here is one that does not exist and the listing
/// answers the modelled `ResourceNotFoundException` (404) rather than an
/// unfiltered page. The rows differ in the project ARN's region, account and
/// resource id, and in whether a stage filter rides along.
#[tokio::test]
async fn list_blueprints_refuses_a_project_filter_no_project_can_satisfy() {
    let router = emulator();

    // Stored blueprints, so an unfiltered listing would have something to return.
    for i in 0..2usize {
        create(
            &router,
            "project-filter",
            &json!({
                "blueprintName": format!("project-filter-bp-{i}"),
                "type": "DOCUMENT",
                "schema": "{}",
            }),
        )
        .await;
    }

    let cases: &[(&str, Value)] = &[
        (
            "plain",
            json!({"projectFilter": {"projectArn": "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/abcdefabcdef"}}),
        ),
        (
            "other-region-and-account",
            json!({"projectFilter": {"projectArn": "arn:aws:bedrock:eu-west-2:123456789012:data-automation-project/0123456789ab"}}),
        ),
        (
            "with-stage-filter",
            json!({
                "projectFilter": {"projectArn": "arn:aws:bedrock:ap-south-1:210987654321:data-automation-project/aaaabbbbccccdddd"},
                "blueprintStageFilter": "ALL",
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, payload) = list_blueprints(&router, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a project filter no project can satisfy must be 404, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert!(
            payload.get("blueprints").is_none(),
            "case {case:?}: the refusal must not carry a page: {payload:?}"
        );
    }

    // Without the filter the same listing still answers normally.
    let (status, payload) = list_blueprints(&router, &json!({})).await;
    assert_eq!(status, 200, "an unfiltered listing must be 200, got {status}");
    assert_eq!(
        payload["blueprints"].as_array().map(Vec::len),
        Some(2),
        "an unfiltered listing must report both stored blueprints: {payload:?}"
    );
}

#[tokio::test]
async fn get_blueprint_honours_the_stage_it_is_asked_for() {
    let router = emulator();

    for stage in ["DEVELOPMENT", "LIVE"] {
        let created = create(
            &router,
            stage,
            &json!({
                "blueprintName": format!("staged-bp-{stage}"),
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
                "blueprintStage": stage,
            }),
        )
        .await;
        let arn = string_at(&created, "blueprintArn", stage);

        let (status, payload) =
            get_blueprint(&router, &arn, &json!({ "blueprintStage": stage })).await;
        assert_eq!(
            status, 200,
            "case {stage:?}: the blueprint's own stage must read 200, got {status} with {payload:?}"
        );

        let other = if stage == "DEVELOPMENT" { "LIVE" } else { "DEVELOPMENT" };
        let (status, payload) =
            get_blueprint(&router, &arn, &json!({ "blueprintStage": other })).await;
        assert_eq!(
            status, 404,
            "case {stage:?}: asking for stage {other:?} must be 404, got {status} with {payload:?}"
        );
    }
}

/// The store is bounded: past its quota a create is refused with the modeled
/// `ServiceQuotaExceededException` (HTTP 402) instead of growing the state
/// without end, and the refusal leaves the store exactly at the quota.
#[tokio::test]
async fn the_blueprint_store_is_bounded_by_its_quota() {
    let router = emulator();
    let quota = state::BLUEPRINT_QUOTA;

    for i in 0..quota {
        let (status, payload) = call(
            &router,
            "PUT",
            "/blueprints/",
            &json!({
                "blueprintName": format!("quota-bp-{i}"),
                "type": "DOCUMENT",
                "schema": "{}",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {i}: a create below the quota of {quota} must be 201, got {status} with {payload:?}"
        );
    }

    // Every create past the quota is refused, however it differs from the ones
    // that fit: a distinct name, a different type, a client token.
    let over_quota: &[(&str, Value)] = &[
        (
            "plain",
            json!({"blueprintName": "quota-bp-over", "type": "DOCUMENT", "schema": "{}"}),
        ),
        (
            "other-type",
            json!({"blueprintName": "quota-bp-over-2", "type": "VIDEO", "schema": r#"{"a":1}"#}),
        ),
        (
            "with-client-token",
            json!({
                "blueprintName": "quota-bp-over-3",
                "type": "IMAGE",
                "schema": "{}",
                "clientToken": "q1234567890123456789012345678901q",
            }),
        ),
    ];
    for (case, body) in over_quota {
        let (status, payload) = call(&router, "PUT", "/blueprints/", body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a create past the quota must be ServiceQuotaExceededException 402, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ServiceQuotaExceededException",
            "case {case:?}: wrong error code in {payload:?}"
        );
    }

    // The refusals stored nothing: the listing is still exactly the quota, and a
    // page is never larger than the modeled maximum.
    let mut seen = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(ref t) = token {
            body["nextToken"] = json!(t);
        }
        let (status, payload) = list_blueprints(&router, &body).await;
        assert_eq!(status, 200, "expected ListBlueprints 200, got {status}");
        let page = payload["blueprints"].as_array().expect("a list");
        assert!(
            page.len() as i64 <= i64::from(state::BLUEPRINT_PAGE_LIMIT),
            "a page of {} exceeds the server page limit {}",
            page.len(),
            state::BLUEPRINT_PAGE_LIMIT
        );
        seen += page.len();
        token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, quota,
        "the store must hold exactly the quota of {quota} blueprints, found {seen}"
    );

    // Freeing a record frees the quota slot with it.
    let (_, payload) = list_blueprints(&router, &json!({"maxResults": 1})).await;
    let victim = string_at(&payload["blueprints"][0], "blueprintArn", "quota-release");
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/blueprints/{}/", encode_label(&victim)),
        &json!({}),
    )
    .await;
    assert_eq!(status, 200, "expected DeleteBlueprint 200, got {status}");
    let (status, payload) = call(
        &router,
        "PUT",
        "/blueprints/",
        &json!({"blueprintName": "quota-bp-after-delete", "type": "DOCUMENT", "schema": "{}"}),
    )
    .await;
    assert_eq!(
        status, 201,
        "a create must succeed once a slot is freed, got {status} with {payload:?}"
    );
}

/// A blueprint's snapshot list is bounded the same way: each cut copies the
/// whole live schema, so past the per-blueprint version quota the cut is refused
/// with the modeled `ServiceQuotaExceededException` (HTTP 402).
#[tokio::test]
async fn blueprint_versions_are_bounded_by_their_quota() {
    let router = emulator();
    let quota = state::BLUEPRINT_VERSION_QUOTA;

    let created = create(
        &router,
        "version-quota",
        &json!({
            "blueprintName": "version-quota-bp",
            "type": "DOCUMENT",
            "schema": r#"{"class":"invoice"}"#,
        }),
    )
    .await;
    let arn = string_at(&created, "blueprintArn", "version-quota");
    let uri = format!("/blueprints/{}/versions/", encode_label(&arn));

    for i in 0..quota {
        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(
            status, 201,
            "case {i}: a cut below the quota of {quota} must be 201, got {status} with {payload:?}"
        );
    }

    // Past the quota, with and without a client token.
    for (case, body) in [
        ("plain", json!({})),
        (
            "with-client-token",
            json!({"clientToken": "v1234567890123456789012345678901v"}),
        ),
    ] {
        let (status, payload) = call(&router, "POST", &uri, &body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a cut past the quota must be ServiceQuotaExceededException 402, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ServiceQuotaExceededException",
            "case {case:?}: wrong error code in {payload:?}"
        );
    }

    // Deleting a snapshot frees a slot; version numbers still only increase.
    let delete_uri = format!("/blueprints/{}/?blueprintVersion=1", encode_label(&arn));
    let (status, _) = call(&router, "DELETE", &delete_uri, &json!({})).await;
    assert_eq!(status, 200, "expected the version delete to be 200, got {status}");
    let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
    assert_eq!(
        status, 201,
        "a cut must succeed once a slot is freed, got {status} with {payload:?}"
    );
    assert_eq!(
        string_at(
            payload.get("blueprint").expect("a blueprint"),
            "blueprintVersion",
            "version-quota-reuse"
        ),
        (quota + 1).to_string(),
        "version numbers must keep increasing after a delete: {payload:?}"
    );
}

/// Counts the store holds: (create tokens, version tokens, live snapshots, live
/// blueprints).
fn store_counts(engine: &engine::InMemoryEngine) -> (usize, usize, usize, usize) {
    let state = engine.state::<ServiceState>();
    let store = state.lock();
    let snapshots: usize = store
        .blueprints
        .values()
        .map(|record| record.versions.len())
        .sum();
    (
        store.create_idempotency.len(),
        store.version_idempotency.len(),
        snapshots,
        store.blueprints.len(),
    )
}

/// The invariant behind the store's bound: an idempotency token entry never
/// outlives the resource it resolves to, so the token maps are bounded by the
/// live records and snapshots and no request sequence grows them without end.
///
/// The rows differ in HOW the resource is released — a versioned delete that
/// frees one snapshot while the blueprint lives on, and a whole-blueprint delete
/// that frees the record and everything under it — and the cycle repeats, so a
/// loop that keeps the live counts flat must keep the token counts flat too.
#[tokio::test]
async fn an_idempotency_token_never_outlives_what_it_points_at() {
    let (router, engine) = emulator_with_engine();

    for round in 0..4usize {
        // `ClientToken` is modeled len 33..256, pattern
        // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`: a letter plus 32 digits fits.
        let create_token = format!("c{round:032}");
        let version_token = format!("v{round:032}");
        let case = format!("round-{round}");

        let created = create(
            &router,
            &case,
            &json!({
                "blueprintName": "token-lifetime-bp",
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
                "clientToken": create_token,
            }),
        )
        .await;
        let arn = string_at(&created, "blueprintArn", &case);

        let versions_uri = format!("/blueprints/{}/versions/", encode_label(&arn));
        let (status, payload) = call(
            &router,
            "POST",
            &versions_uri,
            &json!({ "clientToken": version_token }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateBlueprintVersion 201, got {status} with {payload:?}"
        );
        let version = string_at(
            payload.get("blueprint").expect("a blueprint"),
            "blueprintVersion",
            &case,
        );

        let (creates, versions, snapshots, records) = store_counts(&engine);
        assert!(
            creates <= records && versions <= snapshots,
            "case {case:?}: tokens must not outnumber what they point at: \
             {creates} create tokens / {records} blueprints, \
             {versions} version tokens / {snapshots} snapshots"
        );

        // A versioned delete frees one snapshot: its token goes with it, even
        // though the blueprint itself lives on.
        let (status, payload) = call(
            &router,
            "DELETE",
            &format!(
                "/blueprints/{}/?blueprintVersion={version}",
                encode_label(&arn)
            ),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the versioned DeleteBlueprint to be 200, \
             got {status} with {payload:?}"
        );
        let (_, versions, snapshots, _) = store_counts(&engine);
        assert_eq!(
            (versions, snapshots),
            (0, 0),
            "case {case:?}: a released snapshot must release its token: \
             {versions} version tokens left over {snapshots} snapshots"
        );

        // A whole-blueprint delete frees the record: its create token goes too.
        let (status, payload) = call(
            &router,
            "DELETE",
            &format!("/blueprints/{}/", encode_label(&arn)),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBlueprint 200, got {status} with {payload:?}"
        );
        let (creates, versions, snapshots, records) = store_counts(&engine);
        assert_eq!(
            (creates, versions, snapshots, records),
            (0, 0, 0, 0),
            "case {case:?}: a deleted blueprint must leave nothing behind, found \
             {creates} create tokens, {versions} version tokens, {snapshots} snapshots, \
             {records} records"
        );

        // And the release is observable on the wire: replaying the same
        // clientToken after the delete creates a NEW blueprint rather than
        // replaying the deleted one.
        let replayed = create(
            &router,
            &case,
            &json!({
                "blueprintName": "token-lifetime-bp",
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
                "clientToken": format!("c{round:032}"),
            }),
        )
        .await;
        let replayed_arn = string_at(&replayed, "blueprintArn", &case);
        assert_ne!(
            replayed_arn, arn,
            "case {case:?}: a clientToken whose blueprint was deleted must create a new one"
        );
        let (status, payload) = call(
            &router,
            "DELETE",
            &format!("/blueprints/{}/", encode_label(&replayed_arn)),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBlueprint 200, got {status} with {payload:?}"
        );
        assert_eq!(
            store_counts(&engine),
            (0, 0, 0, 0),
            "case {case:?}: the store must be empty again at the end of the cycle"
        );
    }
}
