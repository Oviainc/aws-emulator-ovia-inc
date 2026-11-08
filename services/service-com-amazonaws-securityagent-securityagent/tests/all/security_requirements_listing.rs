// SPDX-License-Identifier: CC0-1.0

//! The security requirement LISTING path of the SecurityAgent emulator, end to end
//! over the wire: `ListSecurityRequirements` over the requirements a pack holds.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (source: this crate's `OPERATIONS.md` "Wire protocol" section). So
//! nothing here can pass against a handler that answers a canned body: a value must
//! come back out of the shared store through `ListSecurityRequirements` before a case
//! counts.
//!
//! The packs these cases list and the requirements inside them are both written over
//! the wire — `CreateSecurityRequirementPack` and `BatchCreateSecurityRequirements` — so
//! every value a listing reports here came out of the one shared store a handler wrote,
//! and is compared against what the CREATE reported for that record rather than against
//! request text.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: which
//! members of a stored requirement a listed summary reports, which pack's requirements a
//! listing selects, where a page resumes at every page size, what an input the service
//! cannot act on answers, and what a pack delete leaves behind.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    service_state, AWS_MANAGED_PACKS, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator_on, member, ok};

/// The emulator plus the engine behind it, so the last case can see the store itself and
/// not merely what a read of a deleted pack answers.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(Arc::clone(&engine)), engine)
}

/// `CreateSecurityRequirementPack` over the wire, returning the minted `packId`.
///
/// The model gives the create `"http": {"code": 201}`
/// (`smithy_query(query="operation",operation="CreateSecurityRequirementPack")`).
async fn create_pack(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let answer = call(
        router,
        "CreateSecurityRequirementPack",
        &json!({"name": name}),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreateSecurityRequirementPack to answer 201, got {} ({}) \
         with {}",
        answer.status, answer.error_code, answer.body
    );
    member(&answer.body, "packId").unwrap_or_else(|| {
        panic!(
            "case {case:?}: CreateSecurityRequirementPack returned no packId: {}",
            answer.body
        )
    })
}

/// Create `names` as requirements of `pack_id` over the wire, and answer the record the
/// create reported for each, keyed by name.
///
/// `BatchCreateSecurityRequirements` is the operation that WRITES a requirement, and the
/// model gives it `"http": {"code": 201}`
/// (`smithy_query(query="operation",operation="BatchCreateSecurityRequirements")`). Its
/// per-entry `errors` list has to come back empty here: every name is one the pack does
/// not hold yet.
async fn create_requirements(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> BTreeMap<String, Value> {
    if names.is_empty() {
        return BTreeMap::new();
    }
    let entries: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": format!("the {name} requirement"),
                "domain": "Data protection",
                "evaluation": format!("evaluate {name}"),
                "remediation": format!("remediate {name}"),
            })
        })
        .collect();
    let answer = call(
        router,
        "BatchCreateSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirements": entries}),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected BatchCreateSecurityRequirements to answer 201, got {} ({}) \
         with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.body.get("errors").and_then(Value::as_array),
        Some(&Vec::new()),
        "case {case:?}: expected no per-entry errors creating {names:?}, got {}",
        answer.body
    );
    let created = answer
        .body
        .get("securityRequirements")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: BatchCreateSecurityRequirements answered no \
                 securityRequirements list: {}",
                answer.body
            )
        })
        .clone();
    assert_eq!(
        created.len(),
        names.len(),
        "case {case:?}: expected a result per entry, got {created:?}"
    );
    created
        .into_iter()
        .map(|result| {
            let name = member(&result, "name").unwrap_or_else(|| {
                panic!("case {case:?}: a create result carries no name: {result}")
            });
            (name, result)
        })
        .collect()
}

/// One page of `ListSecurityRequirements`: its summaries and its cursor.
struct Page {
    summaries: Vec<Value>,
    next_token: Option<String>,
}

/// `ListSecurityRequirements` for `pack_id` with `maxResults` and `nextToken` as given.
async fn list_page(
    router: &MultiServiceRouter,
    pack_id: &str,
    page_size: Option<i32>,
    token: Option<&str>,
    case: &str,
) -> Page {
    let mut request = json!({"packId": pack_id});
    if let Some(size) = page_size {
        request["maxResults"] = json!(size);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let payload = ok(router, "ListSecurityRequirements", &request, case).await;
    let summaries = payload
        .get("securityRequirementSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListSecurityRequirements answered no \
                 securityRequirementSummaries list: {payload}"
            )
        })
        .clone();
    Page {
        summaries,
        next_token: member(&payload, "nextToken"),
    }
}

/// Every summary `ListSecurityRequirements` reports for `pack_id`, paging to the end
/// with `page_size` (the service's own default when `None`).
///
/// Asserts the pagination invariants on the way: no page is larger than it may be, a
/// page that carries a cursor is a full page, and only the last page carries none.
async fn list_all(
    router: &MultiServiceRouter,
    pack_id: &str,
    page_size: Option<i32>,
    case: &str,
) -> Vec<Value> {
    let ceiling = page_size
        .map(|size| (size as usize).min(MAX_PAGE_SIZE as usize))
        .unwrap_or(MAX_PAGE_SIZE as usize);
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    // One more round than there can be pages, so a cursor that never retires is a
    // failure rather than a hang.
    for _ in 0..(MAX_PAGE_SIZE as usize + 2) {
        let page = list_page(router, pack_id, page_size, token.as_deref(), case).await;
        assert!(
            page.summaries.len() <= ceiling,
            "case {case:?}: expected at most {ceiling} summaries in a page, got {}",
            page.summaries.len()
        );
        if page.next_token.is_some() {
            assert_eq!(
                page.summaries.len(),
                ceiling,
                "case {case:?}: expected a page carrying a nextToken to be full ({ceiling}), \
                 got {}",
                page.summaries.len()
            );
        }
        summaries.extend(page.summaries);
        match page.next_token {
            Some(next) => token = Some(next),
            None => return summaries,
        }
    }
    panic!("case {case:?}: ListSecurityRequirements never stopped handing out cursors");
}

/// A `@timestampFormat("date-time")` member, in epoch seconds.
///
/// The member has to be an RFC 3339 STRING, never an epoch number:
/// `SecurityRequirementSummary$createdAt`/`$updatedAt` both carry
/// `timestampFormat: date-time` (`smithy_query(query="shape",
/// shape="SecurityRequirementSummary")`), which restJson1 renders as a string, and so do
/// the create result's stamps a case compares them against.
fn date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {payload}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as an RFC 3339 date-time string, got {value} in \
             {payload}"
        )
    });
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {name} {text:?} is not a date-time: {e}"))
        .as_secs_f64()
}

/// The requirement name sets one case lists, each a row of the table.
///
/// The rows differ along the COUNT of requirements a pack holds (none, one, a handful,
/// more than a page) and along the FORM of a name (lower case, upper case, digits,
/// dots and dashes, Unicode, a long one, one that is a prefix of another) — the two
/// things the rule generalises over. `SecurityRequirementName` is a plain string with
/// no `@length` and no `@pattern` (`smithy_query(query="shape",
/// shape="SecurityRequirementName")`), so every form here is one the service takes.
fn requirement_name_rows() -> Vec<(&'static str, Vec<String>)> {
    let long_name = "r".repeat(300);
    vec![
        ("a pack holding no requirement", Vec::new()),
        (
            "a pack holding one requirement",
            vec!["encryption-at-rest".to_string()],
        ),
        (
            "names in mixed forms, out of order",
            vec![
                "zz-last".to_string(),
                "ENCRYPTION".to_string(),
                "access.control-1".to_string(),
                "exigence-de-sécurité".to_string(),
                "a".to_string(),
                long_name,
            ],
        ),
        (
            "a name that is a prefix of another",
            vec![
                "logging".to_string(),
                "logging-retention".to_string(),
                "logging-retention-extended".to_string(),
            ],
        ),
        (
            "more requirements than one default page holds",
            (0..(MAX_PAGE_SIZE + 7))
                .map(|index| format!("requirement-{index:04}"))
                .collect(),
        ),
    ]
}

/// RULE: a listing of the requirements inside a pack reports the pack's STORED
/// requirements — each summary carrying that record's own `packId`, `name`,
/// `description` and stamps — in one stable order, however many there are and whatever
/// they are named.
#[tokio::test]
async fn a_listing_reports_every_stored_requirement_of_the_pack() {
    for (case, names) in requirement_name_rows() {
        let (router, _engine) = harness();
        let pack_id = create_pack(&router, "listed pack", case).await;
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
        let created = create_requirements(&router, &pack_id, &borrowed, case).await;

        let summaries = list_all(&router, &pack_id, None, case).await;
        let listed: Vec<String> = summaries
            .iter()
            .map(|summary| {
                member(summary, "name")
                    .unwrap_or_else(|| panic!("case {case:?}: a summary carries no name"))
            })
            .collect();
        let expected: Vec<String> = created.keys().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the pack's requirements in name order, got {listed:?}"
        );

        for summary in &summaries {
            let name = member(summary, "name").expect("a listed summary carries a name");
            let record = created
                .get(&name)
                .unwrap_or_else(|| panic!("case {case:?}: {name} was never created: {summary}"));
            assert_eq!(
                member(summary, "packId").as_deref(),
                Some(pack_id.as_str()),
                "case {case:?}: expected the summary of {name} to carry the pack it was \
                 listed from, got {summary}"
            );
            assert_eq!(
                member(summary, "description"),
                member(record, "description"),
                "case {case:?}: expected the description the create stored for {name}, got \
                 {summary}"
            );
            assert_eq!(
                date_time(summary, "createdAt", case),
                date_time(record, "createdAt", case),
                "case {case:?}: expected the createdAt the create reported for {name}, got \
                 {summary}"
            );
            assert_eq!(
                date_time(summary, "updatedAt", case),
                date_time(record, "updatedAt", case),
                "case {case:?}: expected the updatedAt the create reported for {name}, got \
                 {summary}"
            );
        }
    }
}

/// RULE: the page size bounds ONE page, never what the caller can reach — paging to the
/// end with any page size reports the same requirements in the same order, and a size
/// above the service's ceiling is clamped rather than refused.
#[tokio::test]
async fn every_page_size_walks_the_same_requirements() {
    let names: Vec<String> = (0..7).map(|index| format!("req-{index}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    // Each row is a page size: the smallest one, sizes that do and do not divide the
    // set, the service's ceiling, and one above it (clamped, not refused).
    for page_size in [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(MAX_PAGE_SIZE),
        Some(MAX_PAGE_SIZE + 25),
    ] {
        let case = format!("page size {page_size:?}");
        let (router, _engine) = harness();
        let pack_id = create_pack(&router, "paged pack", &case).await;
        create_requirements(&router, &pack_id, &borrowed, &case).await;

        let listed: Vec<String> = list_all(&router, &pack_id, page_size, &case)
            .await
            .iter()
            .map(|summary| member(summary, "name").expect("a summary carries a name"))
            .collect();
        assert_eq!(
            listed, names,
            "case {case:?}: expected every requirement once, in name order, got {listed:?}"
        );
    }
}

/// RULE: a listing reports the requirements of the pack the request names and of no
/// other pack, even when two packs hold requirements under the same names.
#[tokio::test]
async fn a_listing_reports_only_the_named_packs_requirements() {
    let (router, _engine) = harness();
    let case = "two packs holding same-named requirements";
    let first = create_pack(&router, "first pack", case).await;
    let second = create_pack(&router, "second pack", case).await;
    let shared = ["encryption", "logging"];
    create_requirements(&router, &first, &["first-only", shared[0], shared[1]], case).await;
    create_requirements(&router, &second, &[shared[0], shared[1], "second-only"], case).await;

    for (pack_id, expected) in [
        (&first, vec!["encryption", "first-only", "logging"]),
        (&second, vec!["encryption", "logging", "second-only"]),
    ] {
        let summaries = list_all(&router, pack_id, Some(1), case).await;
        let listed: Vec<String> = summaries
            .iter()
            .map(|summary| member(summary, "name").expect("a summary carries a name"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {pack_id} to list its own requirements, got {listed:?}"
        );
        for summary in &summaries {
            assert_eq!(
                member(summary, "packId").as_deref(),
                Some(pack_id.as_str()),
                "case {case:?}: a summary of another pack leaked into {pack_id}: {summary}"
            );
        }
    }
}

/// RULE: every pack the account holds answers a listing — a pack that holds no
/// requirement answers an empty page and no cursor, never a refusal.
///
/// The rows are the AWS managed catalog a fresh account holds
/// (`service_com_amazonaws_securityagent_securityagent::state::AWS_MANAGED_PACKS`,
/// whose ids this suite does not name one by one) plus a pack created over the wire.
#[tokio::test]
async fn a_pack_holding_no_requirement_answers_an_empty_page() {
    let (router, _engine) = harness();
    let created = create_pack(&router, "empty pack", "an empty created pack").await;
    let mut pack_ids: Vec<String> = AWS_MANAGED_PACKS
        .iter()
        .map(|entry| entry.pack_id.to_string())
        .collect();
    pack_ids.push(created);

    for pack_id in pack_ids {
        let case = format!("the empty pack {pack_id}");
        let page = list_page(&router, &pack_id, None, None, &case).await;
        assert!(
            page.summaries.is_empty(),
            "case {case:?}: expected no summaries, got {:?}",
            page.summaries
        );
        assert_eq!(
            page.next_token, None,
            "case {case:?}: expected no nextToken on a pack holding nothing, got {:?}",
            page.next_token
        );
    }
}

/// RULE: an input the service cannot act on is refused with the modelled error for it —
/// a `packId` the account holds no pack for is the not-found error, and an input that
/// could name no pack or no page is the validation error.
#[tokio::test]
async fn an_input_the_service_cannot_act_on_is_refused() {
    let (router, _engine) = harness();
    let case = "refusals";
    let pack_id = create_pack(&router, "refusing pack", case).await;
    create_requirements(&router, &pack_id, &["only-one", "and-another"], case).await;
    // A cursor this listing minted for ANOTHER pack, to show a resume point is one
    // inside the pack it was taken in: the same name names another requirement
    // elsewhere.
    let other_pack = create_pack(&router, "other pack", case).await;
    create_requirements(&router, &other_pack, &["only-one", "and-another"], case).await;
    let other_pack_token = list_page(&router, &other_pack, Some(1), None, case)
        .await
        .next_token
        .expect("a pack holding two requirements pages at one per page");
    // A cursor minted by the pack LISTING, to show a token from another list is not a
    // resume point in this one.
    let other_list_token = member(
        &ok(
            &router,
            "ListSecurityRequirementPacks",
            &json!({"maxResults": 1}),
            case,
        )
        .await,
        "nextToken",
    )
    .expect("the pack listing pages the seeded AWS managed catalog");

    // Each row: the request, the HTTP status it answers and the error code with it.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a packId the account holds no pack for",
            json!({"packId": "srp-nothing-here"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a packId that is a known pack's id with a suffix",
            json!({"packId": format!("{pack_id}x")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an empty packId",
            json!({"packId": ""}),
            400,
            "ValidationException",
        ),
        (
            "a nextToken this listing did not mint",
            json!({"packId": pack_id.clone(), "nextToken": "not-a-cursor"}),
            400,
            "ValidationException",
        ),
        (
            "a nextToken minted by another listing",
            json!({"packId": pack_id.clone(), "nextToken": other_list_token}),
            400,
            "ValidationException",
        ),
        (
            "a nextToken this listing minted for another pack",
            json!({"packId": pack_id.clone(), "nextToken": other_pack_token}),
            400,
            "ValidationException",
        ),
        (
            "a maxResults of zero",
            json!({"packId": pack_id.clone(), "maxResults": 0}),
            400,
            "ValidationException",
        ),
        (
            "a negative maxResults",
            json!({"packId": pack_id.clone(), "maxResults": -3}),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in rows {
        let answer = call(&router, "ListSecurityRequirements", &request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {case:?}: expected {code}, got {} with {}",
            answer.error_code, answer.body
        );
    }

    // The not-found refusal names the resource class and the id the request carried,
    // the same sentence the pack reads answer for an id the account holds no pack for.
    let missing = "srp-nothing-here";
    let answer = call(
        &router,
        "ListSecurityRequirements",
        &json!({"packId": missing}),
    )
    .await;
    assert_eq!(
        member(&answer.body, "message").as_deref(),
        Some(format!("No security requirement pack found for ID: {missing}").as_str()),
        "case \"the not-found wording\": got {}",
        answer.body
    );
    let read = call(
        &router,
        "GetSecurityRequirementPack",
        &json!({"packId": missing}),
    )
    .await;
    assert_eq!(
        member(&answer.body, "message"),
        member(&read.body, "message"),
        "case \"the not-found wording\": the listing and the pack read must refuse an \
         unknown packId with the same sentence, got {} and {}",
        answer.body,
        read.body
    );
}

/// RULE: a listing is a read — it changes nothing a later caller sees, at any page size
/// and at any point in a walk.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, _engine) = harness();
    let case = "a repeated listing";
    let pack_id = create_pack(&router, "re-read pack", case).await;
    let names: Vec<String> = (0..5).map(|index| format!("req-{index}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    create_requirements(&router, &pack_id, &borrowed, case).await;

    let first = list_all(&router, &pack_id, Some(2), case).await;
    // A half-finished walk: one page taken and its cursor dropped.
    let abandoned = list_page(&router, &pack_id, Some(2), None, case).await;
    assert!(
        abandoned.next_token.is_some(),
        "case {case:?}: expected the first of three pages to carry a cursor"
    );
    let second = list_all(&router, &pack_id, Some(2), case).await;
    let third = list_all(&router, &pack_id, None, case).await;

    assert_eq!(
        first, second,
        "case {case:?}: a second walk must report exactly what the first did"
    );
    assert_eq!(
        first, third,
        "case {case:?}: the page size must not change which requirements are reported"
    );
}

/// RULE: a pack's requirements live inside the pack — deleting the pack takes them with
/// it ("Deletes a customer managed security requirement pack and all its associated
/// security requirements"), and leaves every other pack's requirements where they are.
#[tokio::test]
async fn deleting_a_pack_takes_its_requirements_with_it() {
    let (router, engine) = harness();
    let case = "a deleted pack";
    let doomed = create_pack(&router, "doomed pack", case).await;
    let kept = create_pack(&router, "kept pack", case).await;
    create_requirements(&router, &doomed, &["encryption", "logging"], case).await;
    create_requirements(&router, &kept, &["encryption", "logging"], case).await;

    ok(
        &router,
        "DeleteSecurityRequirementPack",
        &json!({"packId": doomed}),
        case,
    )
    .await;

    let answer = call(
        &router,
        "ListSecurityRequirements",
        &json!({"packId": doomed}),
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected a deleted pack's listing to answer 404, got {} with {}",
        answer.status, answer.body
    );

    let still_there: Vec<String> = list_all(&router, &kept, None, case)
        .await
        .iter()
        .map(|summary| member(summary, "name").expect("a summary carries a name"))
        .collect();
    assert_eq!(
        still_there,
        vec!["encryption".to_string(), "logging".to_string()],
        "case {case:?}: deleting one pack must leave another pack's requirements alone"
    );

    // And the requirements of the deleted pack are gone from the store, not merely
    // unreachable through a pack that no longer resolves.
    let state = service_state(&*engine).expect("the service store is reachable");
    let orphans: Vec<String> = state
        .lock()
        .security_requirements
        .keys()
        .filter(|(pack, _)| *pack == doomed)
        .map(|(_, name)| name.clone())
        .collect();
    assert!(
        orphans.is_empty(),
        "case {case:?}: the deleted pack left requirements behind: {orphans:?}"
    );
}
