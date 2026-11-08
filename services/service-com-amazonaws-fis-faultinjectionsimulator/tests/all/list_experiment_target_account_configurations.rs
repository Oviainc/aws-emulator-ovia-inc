// SPDX-License-Identifier: CC0-1.0

//! AWS FIS `ListExperimentTargetAccountConfigurations`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one FIS states for this operation ("Lists the
//! target account configurations of the specified experiment"), read as a paginated list
//! over a sub-resource named by its `accountId`:
//!
//! * a page reports the target account configurations the NAMED experiment holds — the
//!   snapshot its template carried when `StartExperiment` ran — and nothing else, each
//!   configuration carrying exactly what `GetExperimentTargetAccountConfiguration`
//!   reports for it — an explicitly empty `description` normalized like an omitted one;
//! * the operation models no `maxResults`, so the page size is the service's own: a
//!   `nextToken` arrives only with a full page, and following that token enumerates every
//!   configuration exactly once, in one and the same order, however often the list is
//!   read;
//! * a `nextToken` the operation never issued is refused with `ValidationException`
//!   (HTTP 400);
//! * a list naming an experiment the store does not hold answers
//!   `ResourceNotFoundException` (HTTP 404);
//! * listing changes nothing: the same request answers the same page afterwards, and it
//!   does not advance the experiment's status.
//!
//! The HTTP harness below is local to this suite, as in the sibling suites: a test
//! harness is not shared state.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`, from
/// `smithy_query(query="shape",shape="ExperimentTargetAccountConfiguration")`), spelled
/// out here rather than imported from the crate so a wrong bound in the model or the
/// handler fails this suite.
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// One more configuration than the largest page any paginated list in this model serves:
/// every `maxResults` member of this service is modelled `@range(min: 1, max: 100)`, and
/// `ListExperimentTargetAccountConfigurations` models none at all, so a collection of this
/// size forces at least two pages whatever page size the service picked within that
/// range.
const PAST_THE_LARGEST_PAGE: usize = 101;

/// Comfortably past the span an experiment's `initiating` -> `running` transition falls
/// due in, so a read after it reports the settled status.
const PAST_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);

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
///
/// The template targets MULTIPLE accounts, which is the targeting a target account
/// configuration belongs to: `accountTargeting` and the configurations a template holds
/// state the same thing between them. [`single_account_template`] is the one for a case
/// whose experiment holds no configuration at all.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    create_template(router, label, "multi-account").await
}

/// Create an experiment template that targets a SINGLE account — the documented default —
/// for a case whose experiment holds no target account configuration.
async fn single_account_template(router: &MultiServiceRouter, label: &str) -> String {
    create_template(router, label, "single-account").await
}

async fn create_template(router: &MultiServiceRouter, label: &str, targeting: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("list-etac-{label}"),
            "description": format!("list experiment target account configurations: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-list-experiment-target-accounts",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "experimentOptions": { "accountTargeting": targeting },
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

/// Start an experiment from `template_id` and report its minted id.
async fn start_experiment(router: &MultiServiceRouter, template_id: &str, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": format!("start-{label}"),
            "experimentTemplateId": template_id
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experiment.id in {:?}", answer.body))
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
/// shape of the `roleArn` (a plain role, a role with a path, a role in
/// another partition), and whether `description` is absent, empty, plain text, non-ASCII
/// or at `TargetAccountConfigurationDescription`'s published ceiling. Their account ids
/// are deliberately not created in sorted order, so the order a page reports cannot be
/// the insertion order by accident.
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

/// Persist one configuration on `template_id` through `CreateTargetAccountConfiguration`.
async fn create_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    case: &str,
    account_id: &str,
    role_arn: &str,
    description: Option<&str>,
) {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(description) = description {
        // An absent `description` has to stay absent on the wire.
        body.insert("description".to_string(), json!(description));
    }
    let answer = send(
        router,
        "POST",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(Value::Object(body)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Persist `row` on `template_id`.
async fn create_row(router: &MultiServiceRouter, template_id: &str, row: &Row) {
    create_configuration(
        router,
        template_id,
        row.case,
        &row.account_id,
        &row.role_arn,
        row.description.as_deref(),
    )
    .await;
}

/// Read one of an experiment's configurations back through
/// `GetExperimentTargetAccountConfiguration`.
async fn get_experiment_configuration(
    router: &MultiServiceRouter,
    experiment_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experiments/{experiment_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

async fn get_experiment(router: &MultiServiceRouter, experiment_id: &str) -> Answer {
    send(router, "GET", &format!("/experiments/{experiment_id}"), None).await
}

/// One `ListExperimentTargetAccountConfigurations` call.
async fn list(
    router: &MultiServiceRouter,
    experiment_id: &str,
    next_token: Option<&str>,
) -> Answer {
    let query = next_token
        .map(|token| format!("?nextToken={token}"))
        .unwrap_or_default();
    send(
        router,
        "GET",
        &format!("/experiments/{experiment_id}/targetAccountConfigurations{query}"),
        None,
    )
    .await
}

/// Walk every page of `ListExperimentTargetAccountConfigurations`, returning the
/// concatenated configurations.
///
/// Asserts the token rule as it goes: the operation models no `maxResults`, so every page
/// but the last is the service's own page size, and a `nextToken` only ever arrives with a
/// page that long.
async fn walk(router: &MultiServiceRouter, experiment_id: &str, bound: usize) -> Vec<Value> {
    let case = format!("walk {experiment_id}");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut page_size: Option<usize> = None;
    for page in 0..=bound {
        let answer = list(router, experiment_id, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer.configurations(&case);
        if let Some(size) = page_size {
            assert!(
                items.len() <= size,
                "case {case:?} page {page}: a page holds at most {size} configurations \
                 (the size of the first full page), got {}",
                items.len()
            );
        }
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                match page_size {
                    None => page_size = Some(items.len()),
                    Some(size) => assert_eq!(
                        items.len(),
                        size,
                        "case {case:?} page {page}: a nextToken must only arrive with a full \
                         page of {size}, got {} configurations",
                        items.len()
                    ),
                }
                token = Some(next);
            }
        }
    }
    panic!("case {case:?}: pagination did not terminate over {bound} pages");
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
async fn a_listed_configuration_reports_what_the_experiment_was_started_with() {
    let router = emulator();
    let template_id = template(&router, "members").await;

    let rows = rows();
    for row in &rows {
        create_row(&router, &template_id, row).await;
    }
    let experiment_id = start_experiment(&router, &template_id, "members").await;

    let listed = walk(&router, &experiment_id, rows.len() + 1).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case members: expected every one of the {} configurations the experiment was \
         started with, got {listed:?}",
        rows.len()
    );
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
        let read = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetExperimentTargetAccountConfiguration, \
             got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("targetAccountConfiguration"),
            configuration,
            "case {case:?}: expected the listed configuration to equal the one \
             GetExperimentTargetAccountConfiguration reports, got {:?}",
            read.body
        );
    }

    // Sorted by the `accountId` the configuration is named by, which is the order that
    // makes a resume token a total order over the collection.
    let ids = account_ids("members", &listed);
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case members: expected the pages to report configurations in accountId order, \
         got {ids:?}"
    );
}

#[tokio::test]
async fn pagination_enumerates_every_configuration_of_the_experiment_exactly_once() {
    let router = emulator();
    let template_id = template(&router, "pagination").await;

    // One configuration past the largest page this model's lists serve, so the collection
    // cannot fit in a single page whatever size the service picked.
    let mut expected = BTreeSet::new();
    for index in 0..PAST_THE_LARGEST_PAGE {
        let seed = u32::try_from(index).expect("the index fits a u32");
        // Every key is an account number, which is what a configuration can be named by,
        // so pagination is asserted over the account ids a client can actually configure.
        let account_id = account_id_of(seed, ACCOUNT_ID_FLOOR);
        create_configuration(
            &router,
            &template_id,
            "pagination",
            &account_id,
            &format!("arn:aws:iam::{account_id}:role/fis-pagination"),
            (index % 3 == 0).then_some("configured for pagination"),
        )
        .await;
        expected.insert(account_id);
    }
    assert_eq!(
        expected.len(),
        PAST_THE_LARGEST_PAGE,
        "case pagination: the test built duplicate account ids: {} of {PAST_THE_LARGEST_PAGE}",
        expected.len()
    );

    let experiment_id = start_experiment(&router, &template_id, "pagination").await;

    // More than one page: a single call cannot have carried the whole collection.
    let first = list(&router, &experiment_id, None).await;
    assert_eq!(
        first.status, 200,
        "case pagination: expected HTTP 200, got {} with {:?}",
        first.status, first.body
    );
    let first_page = first.configurations("pagination");
    assert!(
        first_page.len() < PAST_THE_LARGEST_PAGE,
        "case pagination: expected the service to cut the page at its own size, got all {} \
         configurations in one page",
        first_page.len()
    );
    assert!(
        first.next_token().is_some(),
        "case pagination: expected a nextToken while configurations remain, got {:?}",
        first.body
    );

    let walked = walk(&router, &experiment_id, PAST_THE_LARGEST_PAGE + 1).await;
    let ids = account_ids("pagination", &walked);
    let unique: BTreeSet<String> = ids.iter().cloned().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "case pagination: a configuration was served twice: {ids:?}"
    );
    assert_eq!(
        unique, expected,
        "case pagination: expected every configuration exactly once over all pages, got {} \
         distinct account ids",
        unique.len()
    );

    // Walking again reports the same configurations in the same order: a read changes
    // nothing, and the order is the same however often the list is read.
    let again = walk(&router, &experiment_id, PAST_THE_LARGEST_PAGE + 1).await;
    assert_eq!(
        again, walked,
        "case pagination: expected the same pages on a second walk, got {} configurations",
        again.len()
    );
}

#[tokio::test]
async fn a_list_reports_only_the_named_experiments_snapshot_and_changes_nothing() {
    let router = emulator();
    let first_template = template(&router, "scope-first").await;
    let second_template = template(&router, "scope-second").await;

    // Each template gets a DIFFERENT subset of the rows, so a handler that ignored the
    // experiment it was asked about would report an account the other one owns.
    let rows = rows();
    let (mine, theirs) = rows.split_at(3);
    for row in mine {
        create_row(&router, &first_template, row).await;
    }
    for row in theirs {
        create_row(&router, &second_template, row).await;
    }
    let first = start_experiment(&router, &first_template, "scope-first").await;
    let second = start_experiment(&router, &second_template, "scope-second").await;

    for (case, experiment_id, owned) in [
        ("first-experiment", &first, mine),
        ("second-experiment", &second, theirs),
    ] {
        let expected: BTreeSet<String> = owned.iter().map(|row| row.account_id.clone()).collect();
        let before = walk(&router, experiment_id, rows.len() + 1).await;
        let listed: BTreeSet<String> = account_ids(case, &before).into_iter().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected only that experiment's snapshot, got {listed:?}"
        );

        // A read must not change what a later caller sees.
        let after = walk(&router, experiment_id, rows.len() + 1).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected listing twice to answer the same page, got {after:?} \
             after {before:?}"
        );
    }

    // An experiment started from a template holding no configuration answers an empty
    // page and no token.
    let empty_template = single_account_template(&router, "scope-empty").await;
    let empty_experiment = start_experiment(&router, &empty_template, "scope-empty").await;
    let empty = list(&router, &empty_experiment, None).await;
    assert_eq!(
        empty.status, 200,
        "case empty-experiment: expected HTTP 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        empty.configurations("empty-experiment").is_empty(),
        "case empty-experiment: expected no configurations, got {:?}",
        empty.body
    );
    assert_eq!(
        empty.next_token(),
        None,
        "case empty-experiment: expected no nextToken, got {:?}",
        empty.body
    );

    // A configuration added to the template AFTER the start is not part of the
    // experiment's snapshot, and the list reports the snapshot.
    let added_later = "888888888888";
    create_configuration(
        &router,
        &first_template,
        "added-after-the-start",
        added_later,
        "arn:aws:iam::888888888888:role/fis-added-later",
        None,
    )
    .await;
    let after_add = walk(&router, &first, rows.len() + 2).await;
    let listed: BTreeSet<String> = account_ids("added-after-the-start", &after_add)
        .into_iter()
        .collect();
    assert!(
        !listed.contains(added_later),
        "case added-after-the-start: expected the snapshot the experiment was started \
         with, got {listed:?}"
    );
}

#[tokio::test]
async fn a_list_reports_the_same_snapshot_whatever_the_experiments_status_is() {
    let router = emulator();
    let template_id = template(&router, "across-statuses").await;
    create_configuration(
        &router,
        &template_id,
        "across-statuses",
        "666666666666",
        "arn:aws:iam::666666666666:role/fis-across-statuses",
        Some("listed at every status"),
    )
    .await;
    let experiment_id = start_experiment(&router, &template_id, "across-statuses").await;

    let initiating = list(&router, &experiment_id, None).await;
    assert_eq!(
        initiating.status, 200,
        "case initiating: expected HTTP 200, got {} with {:?}",
        initiating.status, initiating.body
    );

    // Listing did not advance the experiment: the transient status is still there for the
    // caller that looks straight after the start.
    let polled = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        polled.member("experiment")["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "initiating",
        "case initiating: listing target account configurations must not advance the \
         experiment, got {:?}",
        polled.body
    );

    tokio::time::sleep(PAST_TRANSITION).await;

    let running = list(&router, &experiment_id, None).await;
    assert_eq!(
        running.configurations("running"),
        initiating.configurations("initiating"),
        "case running: the snapshot must not change when the experiment's status does, \
         got {:?}",
        running.body
    );

    let stopped = send(
        &router,
        "DELETE",
        &format!("/experiments/{experiment_id}"),
        None,
    )
    .await;
    assert_eq!(
        stopped.status, 200,
        "case stopping: expected HTTP 200 from StopExperiment, got {} with {:?}",
        stopped.status, stopped.body
    );
    let after_stop = list(&router, &experiment_id, None).await;
    assert_eq!(
        after_stop.configurations("stopping"),
        initiating.configurations("initiating"),
        "case stopping: a stopped experiment still lists the snapshot it was started \
         with, got {:?}",
        after_stop.body
    );
}

#[tokio::test]
async fn a_list_of_an_experiment_the_store_does_not_hold_is_not_found() {
    let router = emulator();
    let template_id = template(&router, "not-found").await;
    create_row(&router, &template_id, &rows()[0]).await;
    let existing = start_experiment(&router, &template_id, "not-found").await;

    // Each row names no experiment this store holds: a well-formed id of the right shape
    // (deliberately far past the ids this store has minted), the id of the TEMPLATE the
    // experiment was started from, a value that is no minted id at all, and the existing
    // id with a character appended.
    for (case, experiment_id) in [
        ("unknown-experiment-id", "EXP999999999999".to_string()),
        ("an-experiment-template-id", template_id.clone()),
        ("not-an-id", "nonsense".to_string()),
        ("the-existing-id-with-a-suffix", format!("{existing}-x")),
    ] {
        let answer = list(&router, &experiment_id, None).await;
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
async fn a_token_this_operation_never_issued_is_refused() {
    let router = emulator();
    let template_id = template(&router, "validation").await;
    // Enough configurations to force a token out of the service, whatever page size it
    // serves within the published range.
    for index in 0..PAST_THE_LARGEST_PAGE {
        let seed = u32::try_from(index).expect("the index fits a u32");
        create_configuration(
            &router,
            &template_id,
            "validation",
            &account_id_of(seed, ACCOUNT_ID_FLOOR),
            &format!("arn:aws:iam::{seed:012}:role/fis-validation"),
            None,
        )
        .await;
    }
    let experiment_id = start_experiment(&router, &template_id, "validation").await;

    // A token the operation did issue is accepted — the rows below are refused for being
    // tokens it never issued, not for being tokens at all.
    let first_page = list(&router, &experiment_id, None).await;
    let issued = first_page.next_token().unwrap_or_else(|| {
        panic!("a collection of {PAST_THE_LARGEST_PAGE} cannot fit one page: {:?}", first_page.body)
    });
    let resumed = list(&router, &experiment_id, Some(&issued)).await;
    assert_eq!(
        resumed.status, 200,
        "case issued-token: expected HTTP 200, got {} with {:?}",
        resumed.status, resumed.body
    );

    // Each row is a `nextToken` this operation never issued: a plain word, a bare account
    // id (what the token carries, without the marker the service puts in front of it), a
    // minted id of either collection, the marker with no account id behind it, the marker
    // with a value one character under and one over `TargetAccountId`'s published bounds,
    // and the issued token with its marker stripped or mangled.
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
        let answer = list(&router, &experiment_id, Some(&bad_token)).await;
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
}
