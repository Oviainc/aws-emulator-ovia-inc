// SPDX-License-Identifier: CC0-1.0

//! AWS FIS `ListTargetAccountConfigurations`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one FIS states for this operation ("Lists the
//! target account configurations of the specified experiment template"), read as a
//! paginated list over a sub-resource named by its `accountId`:
//!
//! * a page reports the target account configurations the NAMED template holds and
//!   nothing else, each configuration carrying exactly what
//!   `CreateTargetAccountConfiguration` persisted for it and what
//!   `GetTargetAccountConfiguration` reports for it, down to whether `description` is
//!   absent or empty (an explicitly empty one normalized like an omitted one);
//! * a page is at most `maxResults` long, a `nextToken` arrives only with a full page,
//!   and following that token enumerates every configuration exactly once at any page
//!   size — in one and the same order, whatever the page size and however often the list
//!   is read;
//! * a `nextToken` the operation never issued is refused with `ValidationException`
//!   (HTTP 400), and a `maxResults` outside `@range(min: 1, max: 100)` with HTTP 400;
//! * a list naming a template the store does not hold answers `ResourceNotFoundException`
//!   (HTTP 404);
//! * listing changes nothing: the same request answers the same page afterwards.
//!
//! The HTTP harness below is local to this suite, as in the sibling suites: a test
//! harness is not shared state.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ListTargetAccountConfigurationsMaxResults`'s published bounds
/// (`@range(min: 1, max: 100)`), spelled out here rather than imported from the crate so a
/// wrong bound in the model or the handler fails this suite.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 100;

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`).
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The `targetAccountConfigurations` list this answer carries.
    fn configurations(&self, case: &str) -> Vec<Value> {
        self.member("targetAccountConfigurations")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no targetAccountConfigurations list in {:?}",
                    self.body
                )
            })
            .clone()
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(FALLBACK_REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Create an experiment template to hang target account configurations off, and report
/// its minted id.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("list-tac-{label}"),
            "description": format!("list target account configurations suite: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-list-target-account-configurations",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            // A target account configuration belongs to a template that targets MULTIPLE
            // accounts: `accountTargeting` and the configurations a template holds state
            // the same thing between them.
            "experimentOptions": { "accountTargeting": "multi-account" },
            // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
            // documented 1 to 720 minutes.
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            }
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

/// An account id of exactly `len` characters, starting at `seed`: `TargetAccountId` is
/// modelled `@length(12..48)`, `pattern ^[\S]+$`, and AWS account ids are digit strings.
fn account_id_of(seed: u32, len: usize) -> String {
    let base = format!("{seed:012}");
    if len <= base.len() {
        base[..len].to_string()
    } else {
        format!("{base}{}", "7".repeat(len - base.len()))
    }
}

/// One row: the target account configuration one create request persists.
struct Row {
    case: &'static str,
    account_id: String,
    role_arn: String,
    description: Option<String>,
}

/// The configurations the list rule is asserted over.
///
/// They differ along everything that rule generalises over: the digits of the `accountId`
/// the configuration is named by (leading zeros, a repeated figure, an arbitrary one), the
/// shape of the `roleArn`, and whether `description` is absent, empty,
/// plain text, non-ASCII or at `TargetAccountConfigurationDescription`'s published
/// ceiling. Their account ids are deliberately not inserted in sorted order, so the order
/// a page reports cannot be the insertion order by accident.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "middling-account-id-plain-description",
            account_id: "555555555555".to_string(),
            role_arn: "arn:aws:iam::555555555555:role/FisTargetAccount".to_string(),
            description: Some("the staging account".to_string()),
        },
        Row {
            case: "account-id-at-the-floor-no-description",
            account_id: account_id_of(1, ACCOUNT_ID_FLOOR),
            role_arn: "arn:aws:iam::000000000001:role/service-role/fis-target".to_string(),
            description: None,
        },
        Row {
            case: "description-at-the-ceiling",
            account_id: "999999999999".to_string(),
            role_arn: "arn:aws:iam::999999999999:role/fis".to_string(),
            description: Some("d".repeat(DESCRIPTION_CEILING)),
        },
        Row {
            case: "empty-description",
            account_id: "210987654321".to_string(),
            role_arn: "arn:aws-us-gov:iam::210987654321:role/fis-target".to_string(),
            description: Some(String::new()),
        },
        Row {
            case: "non-ascii-description",
            account_id: "444444444444".to_string(),
            role_arn: "arn:aws-cn:iam::444444444444:role/fis-目标账户".to_string(),
            description: Some("説明 — ünïcødé ✅ описание".to_string()),
        },
        Row {
            case: "role-arn-with-a-path",
            account_id: "333333333333".to_string(),
            role_arn: "arn:aws:iam::333333333333:role/path/to/fis-target".to_string(),
            description: Some("an account reached through a role with a path".to_string()),
        },
        Row {
            case: "account-id-with-leading-zeros",
            account_id: account_id_of(2, ACCOUNT_ID_FLOOR),
            role_arn: "arn:aws:iam::000000000002:role/FisTargetAccount".to_string(),
            description: Some("the account whose id carries leading zeros".to_string()),
        },
    ]
}

/// Persist `row` on `template_id` through `CreateTargetAccountConfiguration`.
async fn create_configuration(router: &MultiServiceRouter, template_id: &str, row: &Row) {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(row.role_arn));
    if let Some(description) = &row.description {
        // An absent `description` has to stay absent on the wire.
        body.insert("description".to_string(), json!(description));
    }
    let answer = send(
        router,
        "POST",
        &format!(
            "/experimentTemplates/{template_id}/targetAccountConfigurations/{}",
            row.account_id
        ),
        Some(Value::Object(body)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        row.case, answer.status, answer.body
    );
}

/// Read one configuration back through `GetTargetAccountConfiguration`.
async fn get_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

/// One `ListTargetAccountConfigurations` call.
async fn list(
    router: &MultiServiceRouter,
    template_id: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations{query}"),
        None,
    )
    .await
}

/// Walk every page of `ListTargetAccountConfigurations` at `page_size`, returning the
/// concatenated configurations.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and
/// a `nextToken` only ever arrives with a full page.
async fn walk(
    router: &MultiServiceRouter,
    template_id: &str,
    page_size: Option<i32>,
    bound: usize,
) -> Vec<Value> {
    let case = format!("maxResults={page_size:?}");
    let allowed = page_size
        .map(|size| usize::try_from(size).expect("a positive page size"))
        .unwrap_or(usize::try_from(MAX_RESULTS_CEILING).expect("a positive ceiling"));
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=bound {
        let answer = list(router, template_id, page_size, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer.configurations(&case);
        assert!(
            items.len() <= allowed,
            "case {case:?} page {page}: a page holds at most {allowed} configurations, got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    allowed,
                    "case {case:?} page {page}: a nextToken must only arrive with a full page, \
                     got {} configurations",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case:?}: pagination did not terminate over {bound} configurations");
}

/// The `accountId` of each listed configuration, in the order the pages reported them.
fn account_ids(case: &str, listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|configuration| {
            configuration["accountId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed configuration carries no accountId: {configuration:?}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_page_size_enumerates_every_configuration_of_the_template_exactly_once() {
    let router = emulator();
    let template_id = template(&router, "pagination").await;

    let rows = rows();
    let mut expected = BTreeSet::new();
    for row in &rows {
        create_configuration(&router, &template_id, row).await;
        expected.insert(row.account_id.clone());
    }

    // The rows here are page sizes: the published floor, every size up to one past the
    // collection, the published ceiling, and no `maxResults` at all. Each must enumerate
    // the collection exactly once, in the same order.
    let mut sizes: Vec<Option<i32>> = (MAX_RESULTS_FLOOR
        ..=i32::try_from(rows.len() + 1).expect("the row count fits an i32"))
        .map(Some)
        .collect();
    sizes.push(Some(MAX_RESULTS_CEILING));
    sizes.push(None);

    let mut first_order: Option<Vec<String>> = None;
    for page_size in sizes {
        let case = format!("maxResults={page_size:?}");
        let walked = walk(&router, &template_id, page_size, rows.len() + 1).await;
        let ids = account_ids(&case, &walked);
        let unique: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "case {case:?}: a configuration was served twice: {ids:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: expected every configuration exactly once, got {ids:?}"
        );
        match &first_order {
            None => first_order = Some(ids),
            Some(order) => assert_eq!(
                &ids, order,
                "case {case:?}: expected the same order at every page size, got {ids:?} \
                 after {order:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_listed_configuration_reports_what_the_create_persisted() {
    let router = emulator();
    let template_id = template(&router, "members").await;

    let rows = rows();
    for row in &rows {
        create_configuration(&router, &template_id, row).await;
    }

    let listed = walk(&router, &template_id, Some(2), rows.len() + 1).await;
    for row in &rows {
        let case = row.case;
        let configuration = listed
            .iter()
            .find(|candidate| candidate["accountId"] == json!(row.account_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no listed configuration for account {:?} in {listed:?}",
                    row.account_id
                )
            });
        assert_eq!(
            configuration["roleArn"],
            json!(row.role_arn),
            "case {case:?}: expected the listed configuration to report the roleArn the \
             create carried, got {configuration:?}"
        );
        // An explicitly empty `description` is normalized like an omitted one, so the
        // configuration created with one is listed with no description at all (the rule
        // `target_account_configuration_description.rs` owns).
        match row
            .description
            .as_deref()
            .filter(|description| !description.is_empty())
        {
            Some(description) => assert_eq!(
                configuration["description"],
                json!(description),
                "case {case:?}: expected the listed configuration to report the description \
                 the create carried ({} characters), got {configuration:?}",
                description.chars().count()
            ),
            None => assert!(
                configuration.get("description").is_none_or(Value::is_null),
                "case {case:?}: expected NO description on a configuration created without \
                 one, got {configuration:?}"
            ),
        }

        // The round trip: whatever the list reports is what the reader of that one
        // configuration reports.
        let read = get_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetTargetAccountConfiguration, got {} \
             with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("targetAccountConfiguration"),
            configuration,
            "case {case:?}: expected the listed configuration to equal the one \
             GetTargetAccountConfiguration reports, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_list_reports_only_the_named_templates_configurations_and_changes_nothing() {
    let router = emulator();
    let first = template(&router, "scope-first").await;
    let second = template(&router, "scope-second").await;

    // Each template gets a DIFFERENT subset of the rows, so a handler that ignored the
    // template it was asked about would report an account the other template owns.
    let rows = rows();
    let (mine, theirs) = rows.split_at(3);
    for row in mine {
        create_configuration(&router, &first, row).await;
    }
    for row in theirs {
        create_configuration(&router, &second, row).await;
    }

    for (case, template_id, owned) in [
        ("first-template", &first, mine),
        ("second-template", &second, theirs),
    ] {
        let expected: BTreeSet<String> = owned.iter().map(|row| row.account_id.clone()).collect();
        let before = walk(&router, template_id, Some(1), rows.len() + 1).await;
        let listed: BTreeSet<String> = account_ids(case, &before).into_iter().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected only that template's configurations, got {listed:?}"
        );

        // A read must not change what a later caller sees.
        let after = walk(&router, template_id, Some(1), rows.len() + 1).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected listing twice to answer the same page, got {after:?} \
             after {before:?}"
        );
    }

    // A template holding no configuration at all answers an empty page and no token.
    let empty_template = template(&router, "scope-empty").await;
    let empty = list(&router, &empty_template, None, None).await;
    assert_eq!(
        empty.status, 200,
        "case empty-template: expected HTTP 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        empty.configurations("empty-template").is_empty(),
        "case empty-template: expected no configurations, got {:?}",
        empty.body
    );
    assert_eq!(
        empty.next_token(),
        None,
        "case empty-template: expected no nextToken, got {:?}",
        empty.body
    );
}

#[tokio::test]
async fn a_list_of_a_template_the_store_does_not_hold_is_not_found() {
    let router = emulator();
    let existing = template(&router, "not-found").await;
    create_configuration(&router, &existing, &rows()[0]).await;

    // Each row names no template this store holds: a well-formed id of the right shape
    // (deliberately far past the ids this store has minted), one of the OTHER collection,
    // a value that is no minted id at all, and the existing id with a character appended.
    for (case, template_id) in [
        ("unknown-template-id", "EXT999999999999".to_string()),
        ("an-experiment-id", "EXP999999999999".to_string()),
        ("not-an-id", "nonsense".to_string()),
        ("the-existing-id-with-a-suffix", format!("{existing}-x")),
    ] {
        let answer = list(&router, &template_id, None, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_token_this_operation_never_issued_and_a_max_results_out_of_range_are_refused() {
    let router = emulator();
    let template_id = template(&router, "validation").await;
    let rows = rows();
    for row in &rows {
        create_configuration(&router, &template_id, row).await;
    }

    // A token the operation did issue is accepted — the rows below are refused for being
    // tokens it never issued, not for being tokens at all.
    let first_page = list(&router, &template_id, Some(1), None).await;
    let issued = first_page
        .next_token()
        .unwrap_or_else(|| panic!("a full page over {} rows carries a nextToken", rows.len()));
    let resumed = list(&router, &template_id, Some(1), Some(&issued)).await;
    assert_eq!(
        resumed.status, 200,
        "case issued-token: expected HTTP 200, got {} with {:?}",
        resumed.status, resumed.body
    );

    // Each row is a `nextToken` this operation never issued: a plain word, a bare account
    // id (what the token carries, without the marker this operation puts in front of it),
    // a minted id of either other collection, the marker with no account id behind it,
    // the marker with a value one character under and one over `TargetAccountId`'s
    // published bounds, and the issued token with its marker mangled.
    let marker = issued
        .strip_suffix(
            issued
                .rsplit('.')
                .next()
                .expect("the issued token carries a separator"),
        )
        .expect("the issued token ends with the account id");
    for (case, bad_token) in [
        ("a-plain-word", "nonsense".to_string()),
        ("a-bare-account-id", "555555555555".to_string()),
        ("an-experiment-template-id", "EXT000000000001".to_string()),
        ("an-experiment-id", "EXP000000000001".to_string()),
        ("the-marker-alone", marker.to_string()),
        (
            "the-marker-with-an-account-id-under-the-floor",
            format!("{marker}{}", account_id_of(1, ACCOUNT_ID_FLOOR - 1)),
        ),
        (
            "the-marker-with-an-account-id-over-the-ceiling",
            format!("{marker}{}", account_id_of(1, ACCOUNT_ID_CEILING + 1)),
        ),
        (
            "the-issued-token-without-its-marker",
            issued[marker.len()..].to_string(),
        ),
        (
            "the-issued-token-with-a-mangled-marker",
            format!("x{issued}"),
        ),
    ] {
        let answer = list(&router, &template_id, None, Some(&bad_token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({bad_token:?}): expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({bad_token:?}): expected ValidationException, got {:?}",
            answer.body
        );
    }

    // `maxResults` on each published bound, and one past each of them.
    for (case, max_results, expected_status) in [
        ("at-the-floor", MAX_RESULTS_FLOOR, 200),
        ("at-the-ceiling", MAX_RESULTS_CEILING, 200),
        ("below-the-floor", MAX_RESULTS_FLOOR - 1, 400),
        ("above-the-ceiling", MAX_RESULTS_CEILING + 1, 400),
    ] {
        let answer = list(&router, &template_id, Some(max_results), None).await;
        assert_eq!(
            answer.status, expected_status,
            "case maxResults-{case}: expected HTTP {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
