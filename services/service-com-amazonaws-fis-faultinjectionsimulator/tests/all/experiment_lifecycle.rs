// SPDX-License-Identifier: CC0-1.0

//! The AWS FIS experiment-template -> experiment lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones FIS's experiment surface states:
//!
//! * a created experiment template is PERSISTED, so every later read of it reports the
//!   same record: `GetExperimentTemplate` reports back every member the create request
//!   carried, under a service-minted `id` and `arn`, and `ListExperimentTemplates`'
//!   summary of it agrees with that record;
//! * `clientToken` is an `@idempotencyToken`, so replaying one reports the resource the
//!   first call created instead of creating a second one;
//! * a paginated FIS list cuts a page at `maxResults`, hands back a `nextToken` exactly
//!   when items remain, and resumes where the previous page ended — so walking the pages
//!   at ANY admissible page size enumerates the collection once;
//! * an experiment is minted from the PERSISTED template: it carries that template's
//!   `roleArn`, targets, actions and stop conditions, starts `initiating`, becomes
//!   `running` once the clock says so, RUNS for as long as the longest path through its
//!   actions' durations takes on the emulator's run clock and then reaches the terminal
//!   status its actions earned, and goes `stopping` -> `stopped` with an `endTime` when it
//!   is stopped instead;
//! * a resource id that names no record answers `ResourceNotFoundException` (HTTP 404),
//!   and a request outside a constraint the model or the FIS documentation states answers
//!   `ValidationException` (HTTP 400).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::ops::get_experiment::GetExperimentErrorModeled;
use service_com_amazonaws_fis_faultinjectionsimulator::scaffolded::Scaffolded;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::{MultiServiceRouter, OperationError};

/// The bounds `ListExperimentTemplatesMaxResults` and `ListExperimentsMaxResults` declare
/// (`@range(min: 1, max: 100)`), spelled out here rather than imported from the crate so a
/// wrong bound in `src/state.rs` fails this suite.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 100;

/// `ExperimentTemplateDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// The account id every ARN this emulator mints carries
/// (`::engine::EMULATOR_ACCOUNT_ID`), spelled out so a changed ARN fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The region an ARN names when the request's credential scope offers none usable.
pub(crate) const FALLBACK_REGION: &str = "us-east-1";

/// How long to wait before a transition this service owes the clock must have been
/// materialised. The emulator's span is deliberately non-zero and well under this.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// A wait every template in this suite can carry: inside the 1..720 minutes FIS documents
/// for the `aws:fis:wait` action's `duration`, and long enough that an experiment running
/// it is still running while a case polls it.
pub(crate) const WAIT_DURATION: &str = "PT5M";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis; the region field is what a minted ARN names.
pub(crate) fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The member `name` of this answer's body.
    pub(crate) fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    region: &str,
) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(region))
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

pub(crate) async fn create_template(router: &MultiServiceRouter, body: Value) -> Answer {
    send(
        router,
        "POST",
        "/experimentTemplates",
        Some(body),
        FALLBACK_REGION,
    )
    .await
}

async fn get_template(router: &MultiServiceRouter, id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

async fn delete_template(router: &MultiServiceRouter, id: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/experimentTemplates/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

async fn list_templates(router: &MultiServiceRouter, query: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates{query}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

pub(crate) async fn start_experiment(router: &MultiServiceRouter, body: Value) -> Answer {
    send(router, "POST", "/experiments", Some(body), FALLBACK_REGION).await
}

pub(crate) async fn get_experiment(router: &MultiServiceRouter, id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experiments/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

pub(crate) async fn stop_experiment(router: &MultiServiceRouter, id: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/experiments/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// Poll `GetExperiment` until the experiment reaches a terminal status, or `budget` runs
/// out, and report the experiment as it was last read.
pub(crate) async fn poll_until_final(router: &MultiServiceRouter, id: &str, budget: Duration) -> Value {
    const INTERVAL: Duration = Duration::from_millis(50);
    let deadline = std::time::Instant::now() + budget;
    loop {
        let experiment = get_experiment(router, id).await.member("experiment").clone();
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if ["completed", "failed", "stopped", "cancelled"].contains(&status.as_str())
            || std::time::Instant::now() >= deadline
        {
            return experiment;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

/// Assert that every member `expected` carries appears in `actual` with an equal value.
///
/// This is the round-trip rule, applied to the whole nested request body at once: a
/// member the create request named has to come back from the store unchanged, however
/// deeply it is nested, while a member the response adds on its own (an id, an arn, a
/// timestamp) is allowed.
fn assert_subset(case: &str, path: &str, expected: &Value, actual: &Value) {
    match expected {
        Value::Object(fields) => {
            for (key, value) in fields {
                let nested = actual.get(key).unwrap_or(&Value::Null);
                assert_subset(case, &format!("{path}.{key}"), value, nested);
            }
        }
        Value::Array(items) => {
            let Some(found) = actual.as_array() else {
                panic!("case {case:?}: expected {path} to be an array, got {actual:?}");
            };
            assert_eq!(
                items.len(),
                found.len(),
                "case {case:?}: expected {path} to hold {} items, got {actual:?}",
                items.len()
            );
            for (index, item) in items.iter().enumerate() {
                assert_subset(case, &format!("{path}[{index}]"), item, &found[index]);
            }
        }
        _ => assert_eq!(
            expected, actual,
            "case {case:?}: expected {path} to be {expected:?}, got {actual:?}"
        ),
    }
}

/// `ExperimentTemplateId` / `ExperimentId` as FIS writes them: the resource's prefix
/// followed by a short alphanumeric suffix, within the modelled `@length(0..64)` and
/// `^[\S]+$`. Checked here rather than through the crate's own predicate so a wrong
/// predicate cannot make this suite pass.
fn is_fis_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    !suffix.is_empty()
        && value.len() <= 64
        && suffix
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

/// The `id` of a created template, with the identity rules asserted.
fn template_id(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    let template = answer.member("experimentTemplate");
    let id = template["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {:?}", answer.body));
    assert!(
        is_fis_id(id, "EXT"),
        "case {case:?}: expected a minted experiment-template id, got {id:?}"
    );
    assert_eq!(
        template["arn"].as_str().unwrap_or_default(),
        format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{id}"),
        "case {case:?}: expected the template arn to name the minted id, got {:?}",
        template["arn"]
    );
    id.to_string()
}

/// A `clientToken` no other case in this suite uses.
pub(crate) fn token(label: &str) -> String {
    format!("token-{label}")
}

/// The create-request bodies the round-trip rule is asserted over.
///
/// The rows differ along everything that rule generalises over: how many actions a
/// template has, whether it names targets at all and how those targets select resources
/// (by ARN, by tag, with filters), how many tags it carries (none, one, three), which
/// stop-condition source it uses, whether it carries a log configuration, a report
/// configuration or experiment options, and the size of its description — including
/// `ExperimentTemplateDescription`'s published ceiling of
/// [`DESCRIPTION_CEILING`] characters.
fn template_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal-one-action-no-targets",
            json!({
                "clientToken": token("minimal"),
                "description": "demo",
                "roleArn": "arn:aws:iam::000000000000:role/fis-minimal",
                "stopConditions": [{ "source": "none" }],
                "actions": {
                    "stopInstances": { "actionId": "aws:ec2:stop-instances" }
                }
            }),
        ),
        (
            "alarm-stop-condition-two-actions-three-tags",
            json!({
                "clientToken": token("alarm"),
                "description": "two actions guarded by a CloudWatch alarm",
                "roleArn": "arn:aws:iam::000000000000:role/fis-alarm",
                "stopConditions": [
                    { "source": "none" },
                    {
                        "source": "aws:cloudwatch:alarm",
                        "value": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:cpu-high"
                    }
                ],
                "targets": {
                    "taggedInstances": {
                        "resourceType": "aws:ec2:instance",
                        "resourceTags": { "Stage": "test" },
                        "selectionMode": "COUNT(2)"
                    }
                },
                "actions": {
                    "stopInstances": {
                        "actionId": "aws:ec2:stop-instances",
                        "description": "stop two instances",
                        "parameters": { "startInstancesAfterDuration": "PT2M" },
                        "targets": { "Instances": "taggedInstances" }
                    },
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["stopInstances"]
                    }
                },
                // This row's target is identified by TAG, and this emulator holds no other
                // service's resources to resolve a tag against, so that target resolves to
                // nothing. Under the documented default (`fail`) such a run ends `failed`
                // on the resolution rather than running, which
                // `experiment_options_decide_the_run.rs` asserts; this row is about the
                // round trip of the stored template, so it names the mode that SKIPS the
                // action instead and keeps running.
                "experimentOptions": { "emptyTargetResolutionMode": "skip" },
                "tags": { "Key1": "Val1", "Owner": "chaos", "Stage": "test" }
            }),
        ),
        (
            "targets-by-arn-with-filters-and-log-configuration",
            json!({
                "clientToken": token("filters"),
                "description": "named instances with a filter and S3 logging",
                "roleArn": "arn:aws:iam::000000000000:role/fis-filters",
                "stopConditions": [{ "source": "none" }],
                "targets": {
                    "namedInstances": {
                        "resourceType": "aws:ec2:instance",
                        "resourceArns": [
                            "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"
                        ],
                        "filters": [
                            { "path": "State.Name", "values": ["running", "pending"] }
                        ],
                        "selectionMode": "ALL",
                        "parameters": {}
                    }
                },
                "actions": {
                    "reboot": { "actionId": "aws:ec2:reboot-instances" }
                },
                "logConfiguration": {
                    "s3Configuration": { "bucketName": "fis-logs", "prefix": "runs/" },
                    "logSchemaVersion": 2
                },
                "experimentOptions": {
                    "accountTargeting": "multi-account",
                    "emptyTargetResolutionMode": "skip"
                },
                "tags": { "Key1": "Val1" }
            }),
        ),
        (
            "report-configuration-and-cloudwatch-logs",
            json!({
                "clientToken": token("report"),
                "description": "a template carrying an experiment report configuration",
                "roleArn": "arn:aws:iam::000000000000:role/fis-report",
                "stopConditions": [{ "source": "none" }],
                "actions": {
                    "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT30M" } }
                },
                "logConfiguration": {
                    "cloudWatchLogsConfiguration": {
                        "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis:*"
                    },
                    "logSchemaVersion": 1
                },
                "experimentReportConfiguration": {
                    "outputs": {
                        "s3Configuration": { "bucketName": "fis-reports", "prefix": "r/" }
                    },
                    "dataSources": {
                        "cloudWatchDashboards": [
                            { "dashboardIdentifier": "arn:aws:cloudwatch::000000000000:dashboard/fis" }
                        ]
                    },
                    "preExperimentDuration": "PT1M",
                    "postExperimentDuration": "PT5M"
                }
            }),
        ),
        (
            "description-at-the-published-ceiling",
            json!({
                "clientToken": token("ceiling"),
                "description": "d".repeat(DESCRIPTION_CEILING),
                "roleArn": "arn:aws:iam::000000000000:role/fis-ceiling",
                "stopConditions": [{ "source": "none" }],
                "actions": {
                    "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } }
                }
            }),
        ),
    ]
}

/// The create body of `case`, with the members a response never echoes removed.
fn expected_template(body: &Value) -> Value {
    let mut expected = body.clone();
    // `clientToken` is request-only: it is the idempotency key, not a member of
    // `ExperimentTemplate`.
    expected
        .as_object_mut()
        .expect("a create body is a JSON object")
        .remove("clientToken");
    expected
}

/// The wire shape of each channel a handler in this crate can answer on, pinned here
/// because the store-missing branch is unreachable through the router: a handler that
/// cannot reach its own store must answer a server fault (500 `InternalFailure`), never
/// the scaffold's 501 — which claims the operation was never written — and never a 400,
/// which would blame the caller for the emulator's own failure.
#[test]
fn each_error_channel_renders_with_its_own_status_and_code() {
    let cases: [(&str, Scaffolded<GetExperimentErrorModeled>, u16, &str); 3] = [
        (
            "store unavailable",
            Scaffolded::internal_failure("the fis experiment store is unavailable"),
            500,
            "InternalFailure",
        ),
        (
            "bad request",
            Scaffolded::validation("maxResults is out of range"),
            400,
            "ValidationException",
        ),
        (
            "not written yet",
            Scaffolded::not_implemented("GetExperiment"),
            501,
            "NotImplemented",
        ),
    ];

    for (case, error, expected_status, expected_code) in cases {
        assert_eq!(
            error.http_status(),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {}",
            error.http_status()
        );
        assert_eq!(
            error.shape_name(),
            expected_code,
            "case {case:?}: expected the wire error code {expected_code:?}, got {:?}",
            error.shape_name()
        );
    }
}

#[tokio::test]
async fn a_created_experiment_template_reads_back_exactly_as_it_was_stored() {
    let router = emulator();

    for (case, body) in template_cases() {
        let created = create_template(&router, body.clone()).await;
        let id = template_id(case, &created);
        let expected = expected_template(&body);

        // What the create call returned is what the request asked for…
        assert_subset(
            case,
            "CreateExperimentTemplate.experimentTemplate",
            &expected,
            created.member("experimentTemplate"),
        );

        // …and reading it back reports the identical record, not a re-derived one.
        let fetched = get_template(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetExperimentTemplate, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.member("experimentTemplate"),
            created.member("experimentTemplate"),
            "case {case:?}: GetExperimentTemplate must report the stored record, got {:?}",
            fetched.body
        );

        // Reading twice reports the same record: a read changes nothing.
        let again = get_template(&router, &id).await;
        assert_eq!(
            again.member("experimentTemplate"),
            fetched.member("experimentTemplate"),
            "case {case:?}: two reads of one template disagreed: {:?}",
            again.body
        );

        // Timestamps are present, and a template nobody has updated reports the same
        // instant for both.
        let template = fetched.member("experimentTemplate");
        assert!(
            template["creationTime"].is_number() || template["creationTime"].is_string(),
            "case {case:?}: expected a creationTime, got {:?}",
            template["creationTime"]
        );
        assert_eq!(
            template["lastUpdateTime"], template["creationTime"],
            "case {case:?}: an untouched template must report lastUpdateTime == creationTime, \
             got {:?}",
            template
        );
        assert_eq!(
            template["targetAccountConfigurationsCount"],
            json!(0),
            "case {case:?}: a template created with no target account configurations must \
             report none, got {:?}",
            template["targetAccountConfigurationsCount"]
        );
    }
}

#[tokio::test]
async fn experiment_options_a_request_omits_report_the_documented_defaults() {
    let router = emulator();

    // The rows differ along which option members the request names: none, one, or both.
    // The service fills in the value FIS documents for each member left out, and reports
    // the named one unchanged.
    let cases = [
        (
            "no-options",
            json!(null),
            "single-account",
            "fail",
        ),
        (
            "account-targeting-only",
            json!({ "accountTargeting": "multi-account" }),
            "multi-account",
            "fail",
        ),
        (
            "resolution-mode-only",
            json!({ "emptyTargetResolutionMode": "skip" }),
            "single-account",
            "skip",
        ),
        (
            "both",
            json!({ "accountTargeting": "multi-account", "emptyTargetResolutionMode": "skip" }),
            "multi-account",
            "skip",
        ),
    ];

    for (case, options, expected_targeting, expected_resolution) in cases {
        let mut body = json!({
            "clientToken": token(case),
            "description": format!("experiment options: {case}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-options",
            "stopConditions": [{ "source": "none" }],
            "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
        });
        if !options.is_null() {
            body["experimentOptions"] = options;
        }

        let created = create_template(&router, body).await;
        let id = template_id(case, &created);
        let fetched = get_template(&router, &id).await;
        let options = fetched.member("experimentTemplate")["experimentOptions"].clone();

        assert_eq!(
            options["accountTargeting"].as_str().unwrap_or_default(),
            expected_targeting,
            "case {case:?}: expected accountTargeting {expected_targeting:?}, got {options:?}"
        );
        assert_eq!(
            options["emptyTargetResolutionMode"]
                .as_str()
                .unwrap_or_default(),
            expected_resolution,
            "case {case:?}: expected emptyTargetResolutionMode {expected_resolution:?}, \
             got {options:?}"
        );
    }
}

#[tokio::test]
async fn a_minted_arn_names_the_region_the_request_was_signed_for() {
    // The rule generalises over the signing region, so the rows are regions: the usual
    // two-word form, a three-word one, a region whose number is more than one digit, and
    // a scope whose region field is not a region at all — which must not be allowed to
    // shape an ARN.
    let cases = [
        ("us-east-1", "us-east-1"),
        ("eu-west-1", "eu-west-1"),
        ("ap-southeast-3", "ap-southeast-3"),
        ("us-gov-west-10", "us-gov-west-10"),
        ("not_a_region", FALLBACK_REGION),
    ];

    for (signing_region, expected_region) in cases {
        let router = emulator();
        let created = send(
            &router,
            "POST",
            "/experimentTemplates",
            Some(json!({
                "clientToken": token("region"),
                "description": format!("signed for {signing_region}"),
                "roleArn": "arn:aws:iam::000000000000:role/fis-region",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            })),
            signing_region,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {signing_region:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        let template = created.member("experimentTemplate");
        let id = template["id"].as_str().unwrap_or_default();
        assert_eq!(
            template["arn"].as_str().unwrap_or_default(),
            format!("arn:aws:fis:{expected_region}:{ACCOUNT_ID}:experiment-template/{id}"),
            "case {signing_region:?}: expected the template arn to name {expected_region}, \
             got {:?}",
            template["arn"]
        );

        let started = send(
            &router,
            "POST",
            "/experiments",
            Some(json!({ "clientToken": token("region-exp"), "experimentTemplateId": id })),
            signing_region,
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {signing_region:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
            started.status, started.body
        );
        let experiment = started.member("experiment");
        let experiment_id = experiment["id"].as_str().unwrap_or_default();
        assert_eq!(
            experiment["arn"].as_str().unwrap_or_default(),
            format!("arn:aws:fis:{expected_region}:{ACCOUNT_ID}:experiment/{experiment_id}"),
            "case {signing_region:?}: expected the experiment arn to name {expected_region}, \
             got {:?}",
            experiment["arn"]
        );
    }
}

#[tokio::test]
async fn replaying_a_client_token_reports_the_resource_it_already_created() {
    let router = emulator();

    // The rows differ along the request the token belongs to, so the rule is asserted for
    // more than one template rather than for one special case.
    let mut ids = BTreeSet::new();
    for (case, body) in template_cases() {
        let first = create_template(&router, body.clone()).await;
        let id = template_id(case, &first);
        ids.insert(id.clone());

        let replay = create_template(&router, body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: expected HTTP 200 on the replay, got {} with {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.member("experimentTemplate"),
            first.member("experimentTemplate"),
            "case {case:?}: replaying the clientToken must report the first template, got {:?}",
            replay.body
        );
    }

    // Every distinct token created a distinct template, and the replays added none.
    let listed = list_templates(&router, "").await;
    let listed_ids: BTreeSet<String> = listed
        .member("experimentTemplates")
        .as_array()
        .expect("experimentTemplates is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids, ids,
        "a replayed clientToken must not add a template: expected {ids:?}, got {listed_ids:?}"
    );
}

#[tokio::test]
async fn a_template_summary_agrees_with_the_template_it_summarises() {
    let router = emulator();

    for (case, body) in template_cases() {
        let created = create_template(&router, body).await;
        let id = template_id(case, &created);
        let template = created.member("experimentTemplate").clone();

        let listed = list_templates(&router, "").await;
        let summary = listed
            .member("experimentTemplates")
            .as_array()
            .expect("experimentTemplates is a list")
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {id} is missing from {:?}", listed.body)
            })
            .clone();

        // Every member `ExperimentTemplateSummary` declares is the one on the record.
        for member in [
            "id",
            "arn",
            "description",
            "creationTime",
            "lastUpdateTime",
            "tags",
        ] {
            assert_eq!(
                summary[member], template[member],
                "case {case:?}: summary.{member} must equal the template's, got {summary:?}"
            );
        }
    }
}

/// Walk every page of a paginated FIS list at `page_size`, returning the concatenated
/// items.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and
/// a `nextToken` only ever arrives with a full page.
async fn walk(
    router: &MultiServiceRouter,
    path: &str,
    items_member: &str,
    extra_query: &str,
    page_size: i32,
    bound: usize,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=bound {
        let query = match token.as_deref() {
            None => format!("?maxResults={page_size}{extra_query}"),
            Some(value) => format!("?maxResults={page_size}{extra_query}&nextToken={value}"),
        };
        let answer = send(
            router,
            "GET",
            &format!("{path}{query}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer
            .member(items_member)
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case maxResults={page_size} page {page}: no {items_member} list in {:?}",
                    answer.body
                )
            })
            .clone();
        let allowed = usize::try_from(page_size).expect("a positive page size");
        assert!(
            items.len() <= allowed,
            "case maxResults={page_size} page {page}: a page holds at most {page_size} items, \
             got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    allowed,
                    "case maxResults={page_size} page {page}: a nextToken must only arrive with \
                     a full page, got {} items",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case maxResults={page_size}: pagination did not terminate over {bound} items");
}

#[tokio::test]
async fn walking_the_pages_at_any_page_size_enumerates_every_template_once() {
    let router = emulator();

    let mut expected = BTreeSet::new();
    for (case, body) in template_cases() {
        let created = create_template(&router, body).await;
        expected.insert(template_id(case, &created));
    }

    // The rows are page sizes: the published floor, every size up to one past the
    // collection, and the published ceiling. Each must enumerate the collection exactly
    // once.
    let mut sizes: Vec<i32> = (MAX_RESULTS_FLOOR..=i32::try_from(expected.len() + 1).unwrap())
        .collect();
    sizes.push(MAX_RESULTS_CEILING);
    for page_size in sizes {
        let walked = walk(
            &router,
            "/experimentTemplates",
            "experimentTemplates",
            "",
            page_size,
            expected.len() + 1,
        )
        .await;
        let ids: Vec<String> = walked
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let unique: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "case maxResults={page_size}: a template was served twice: {ids:?}"
        );
        assert_eq!(
            unique, expected,
            "case maxResults={page_size}: expected every template exactly once, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn an_experiment_carries_the_persisted_template_and_runs_to_the_status_its_actions_earn() {
    let router = emulator();

    for (case, body) in template_cases() {
        let created = create_template(&router, body).await;
        let id = template_id(case, &created);
        let template = created.member("experimentTemplate").clone();

        // A template that targets MULTIPLE accounts only runs once it names one: its
        // `accountTargeting` and the target account configurations it holds state the same
        // thing between them, which `account_targeting_agreement.rs` owns. This suite is
        // about what an experiment carries over from its template, so such a row is given
        // the configuration it needs to be runnable.
        if template["experimentOptions"]["accountTargeting"].as_str() == Some("multi-account") {
            let account_id = "919191919191";
            let configured = send(
                &router,
                "POST",
                &format!("/experimentTemplates/{id}/targetAccountConfigurations/{account_id}"),
                Some(json!({
                    "clientToken": token(&format!("configure-{case}")),
                    "roleArn": format!("arn:aws:iam::{account_id}:role/fis-target")
                })),
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: expected HTTP 200 from CreateTargetAccountConfiguration, \
                 got {} with {:?}",
                configured.status, configured.body
            );
        }

        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("start-{case}")),
                "experimentTemplateId": id,
                "tags": { "Run": case }
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
            started.status, started.body
        );
        let experiment = started.member("experiment").clone();
        let experiment_id = experiment["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_fis_id(&experiment_id, "EXP"),
            "case {case:?}: expected a minted experiment id, got {experiment_id:?}"
        );

        // The experiment is the template's, copied: the members FIS carries over are
        // equal to the stored template's own.
        assert_eq!(
            experiment["experimentTemplateId"], template["id"],
            "case {case:?}: expected the experiment to name its template, got {experiment:?}"
        );
        for member in ["roleArn", "targets", "stopConditions"] {
            assert_eq!(
                experiment[member], template[member],
                "case {case:?}: expected experiment.{member} to be the template's, got {:?}",
                experiment[member]
            );
        }
        assert_subset(
            case,
            "StartExperiment.experiment.actions",
            &template["actions"],
            &experiment["actions"],
        );
        assert_eq!(
            experiment["tags"],
            json!({ "Run": case }),
            "case {case:?}: expected the experiment's own tags, got {:?}",
            experiment["tags"]
        );
        // `actionsMode` is the one option StartExperiment chooses; the other two come
        // from the template.
        assert_eq!(
            experiment["experimentOptions"]["actionsMode"]
                .as_str()
                .unwrap_or_default(),
            "run-all",
            "case {case:?}: expected the documented default actionsMode, got {:?}",
            experiment["experimentOptions"]
        );
        for member in ["accountTargeting", "emptyTargetResolutionMode"] {
            assert_eq!(
                experiment["experimentOptions"][member], template["experimentOptions"][member],
                "case {case:?}: expected experimentOptions.{member} to be the template's, \
                 got {:?}",
                experiment["experimentOptions"]
            );
        }

        // The transient status is observable: the move to `running` is owed to the clock,
        // so it has not happened by the time this call returns.
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "initiating",
            "case {case:?}: expected a started experiment to be initiating, got {:?}",
            experiment["state"]
        );
        assert!(
            experiment["endTime"].is_null(),
            "case {case:?}: an experiment that has not ended carries no endTime, got {:?}",
            experiment["endTime"]
        );

        // A read before the transition falls due still reports the transient status.
        let polled = get_experiment(&router, &experiment_id).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: expected HTTP 200 from GetExperiment, got {} with {:?}",
            polled.status, polled.body
        );
        assert_eq!(
            polled.member("experiment")["state"]["status"]
                .as_str()
                .unwrap_or_default(),
            "initiating",
            "case {case:?}: polling must not advance the experiment, got {:?}",
            polled.body
        );

        tokio::time::sleep(PAST_TRANSITION).await;

        // Every started experiment RUNS: whatever its actions are, the move to `running`
        // has fallen due by now and the run lasts longer than this case's wait, so the
        // experiment is running — and nothing but the status has moved.
        let running = get_experiment(&router, &experiment_id).await;
        assert_eq!(
            running.member("experiment")["state"]["status"]
                .as_str()
                .unwrap_or_default(),
            "running",
            "case {case:?}: expected the experiment to be running once the transition fell \
             due, got {:?}",
            running.body
        );
        assert!(
            running.member("experiment")["endTime"].is_null(),
            "case {case:?}: a running experiment has not ended, got {:?}",
            running.member("experiment")["endTime"]
        );
        // Everything else is the record it was: only the status moved.
        for member in ["id", "arn", "experimentTemplateId", "roleArn", "creationTime"] {
            assert_eq!(
                running.member("experiment")[member], experiment[member],
                "case {case:?}: expected experiment.{member} to be unchanged, got {:?}",
                running.member("experiment")[member]
            );
        }
        // This row is done with its experiment: stop it, so the suite does not hold one
        // active-experiment slot per row and no later row can be refused for having
        // reached the account's quota. The terminal status a run EARNS is asserted in
        // `experiment_run.rs`, which lets a compressed run elapse.
        let stopped = stop_experiment(&router, &experiment_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected the stop to be accepted, got {} with {:?}",
            stopped.status, stopped.body
        );
    }
}

#[tokio::test]
async fn stopping_an_experiment_moves_it_to_stopped_with_a_reason_and_an_end_time() {
    let router = emulator();

    // The rows differ along the status the experiment is in when it is stopped: one
    // stopped while still initiating, one stopped after it started running.
    for (case, wait_first) in [("stopped-while-initiating", false), ("stopped-while-running", true)]
    {
        let created = create_template(
            &router,
            json!({
                "clientToken": token(&format!("stop-template-{case}")),
                "description": format!("stop: {case}"),
                "roleArn": "arn:aws:iam::000000000000:role/fis-stop",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
        )
        .await;
        let template_id = template_id(case, &created);

        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("stop-experiment-{case}")),
                "experimentTemplateId": template_id
            }),
        )
        .await;
        let experiment_id = started.member("experiment")["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if wait_first {
            tokio::time::sleep(PAST_TRANSITION).await;
        }

        let stopped = stop_experiment(&router, &experiment_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopExperiment, got {} with {:?}",
            stopped.status, stopped.body
        );
        let state = stopped.member("experiment")["state"].clone();
        assert_eq!(
            state["status"].as_str().unwrap_or_default(),
            "stopping",
            "case {case:?}: expected the stop to be in progress, got {state:?}"
        );
        assert!(
            state["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: a stopped experiment carries a state reason, got {state:?}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;

        let settled = get_experiment(&router, &experiment_id).await;
        let experiment = settled.member("experiment").clone();
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "stopped",
            "case {case:?}: expected the stop to have completed, got {experiment:?}"
        );
        assert!(
            !experiment["endTime"].is_null(),
            "case {case:?}: a stopped experiment carries an endTime, got {experiment:?}"
        );

        // A second stop is REFUSED: the experiment is in a final state, so the call
        // answers 400 `ValidationException` and leaves the record exactly as it was.
        let again = stop_experiment(&router, &experiment_id).await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected HTTP 400 from a stop of a final experiment, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            again.body
        );
        let untouched = get_experiment(&router, &experiment_id).await;
        assert_eq!(
            untouched.member("experiment")["state"]["status"]
                .as_str()
                .unwrap_or_default(),
            "stopped",
            "case {case:?}: a refused stop must leave the experiment stopped, got {:?}",
            untouched.body
        );
        assert_eq!(
            untouched.member("experiment")["endTime"], experiment["endTime"],
            "case {case:?}: a refused stop must not move endTime, got {:?}",
            untouched.member("experiment")["endTime"]
        );
    }
}

#[tokio::test]
async fn list_experiments_reports_the_persisted_experiments_and_filters_by_template() {
    let router = emulator();

    // Two templates, with a different number of experiments started from each, so the
    // filter has something to exclude in both directions.
    let mut per_template: Vec<(String, Vec<String>)> = Vec::new();
    for (template_index, experiment_count) in [(0usize, 2usize), (1, 3)] {
        let case = format!("template-{template_index}");
        let created = create_template(
            &router,
            json!({
                "clientToken": token(&format!("list-template-{template_index}")),
                "description": format!("list: {case}"),
                "roleArn": "arn:aws:iam::000000000000:role/fis-list",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
        )
        .await;
        let id = template_id(&case, &created);
        let mut experiments = Vec::new();
        for run in 0..experiment_count {
            let started = start_experiment(
                &router,
                json!({
                    "clientToken": token(&format!("list-experiment-{template_index}-{run}")),
                    "experimentTemplateId": id
                }),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case:?} run {run}: expected HTTP 200 from StartExperiment, got {} with {:?}",
                started.status, started.body
            );
            experiments.push(
                started.member("experiment")["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        per_template.push((id, experiments));
    }

    let total: usize = per_template
        .iter()
        .map(|(_, experiments)| experiments.len())
        .sum();

    // Unfiltered, every experiment is listed exactly once, at any page size.
    for page_size in MAX_RESULTS_FLOOR..=i32::try_from(total + 1).unwrap() {
        let walked = walk(
            &router,
            "/experiments",
            "experiments",
            "",
            page_size,
            total + 1,
        )
        .await;
        let ids: BTreeSet<String> = walked
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: BTreeSet<String> = per_template
            .iter()
            .flat_map(|(_, experiments)| experiments.iter().cloned())
            .collect();
        assert_eq!(
            ids, expected,
            "case maxResults={page_size}: expected every experiment once, got {ids:?}"
        );
    }

    // Filtered, only the experiments of the named template are listed — at every page
    // size, so the filter is applied before the page is cut.
    for (template, experiments) in &per_template {
        let expected: BTreeSet<String> = experiments.iter().cloned().collect();
        for page_size in MAX_RESULTS_FLOOR..=i32::try_from(experiments.len() + 1).unwrap() {
            let walked = walk(
                &router,
                "/experiments",
                "experiments",
                &format!("&experimentTemplateId={template}"),
                page_size,
                experiments.len() + 1,
            )
            .await;
            let ids: BTreeSet<String> = walked
                .iter()
                .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                ids, expected,
                "case {template}/maxResults={page_size}: expected only that template's \
                 experiments, got {ids:?}"
            );
            for summary in &walked {
                assert_eq!(
                    summary["experimentTemplateId"].as_str().unwrap_or_default(),
                    template,
                    "case {template}: a filtered summary names another template: {summary:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_template_removes_it_from_every_later_read() {
    let router = emulator();

    let mut remaining = BTreeSet::new();
    let mut created_ids = Vec::new();
    for (case, body) in template_cases() {
        let created = create_template(&router, body).await;
        let id = template_id(case, &created);
        remaining.insert(id.clone());
        created_ids.push((case, id, created.member("experimentTemplate").clone()));
    }

    for (case, id, template) in created_ids {
        let deleted = delete_template(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.member("experimentTemplate"),
            &template,
            "case {case:?}: DeleteExperimentTemplate must report the record it removed, got {:?}",
            deleted.body
        );

        let fetched = get_template(&router, &id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected HTTP 404 after the delete, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );

        let deleted_again = delete_template(&router, &id).await;
        assert_eq!(
            deleted_again.status, 404,
            "case {case:?}: deleting a template twice must answer 404, got {} with {:?}",
            deleted_again.status, deleted_again.body
        );

        remaining.remove(&id);
        let listed = list_templates(&router, "").await;
        let listed_ids: BTreeSet<String> = listed
            .member("experimentTemplates")
            .as_array()
            .expect("experimentTemplates is a list")
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed_ids, remaining,
            "case {case:?}: expected the deleted template to be gone from the list, \
             got {listed_ids:?}"
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_record_answers_resource_not_found() {
    let router = emulator();

    // The rows differ along the shape of the id: one of the shape this service mints, one
    // that is not, and one at `ExperimentTemplateId`'s published 64-character ceiling.
    let ids = [
        "EXT000000000001",
        "EXPdoesnotexist",
        "not-an-id",
        &"z".repeat(64),
    ];

    for id in ids {
        for (operation, method, path) in [
            ("GetExperimentTemplate", "GET", "/experimentTemplates"),
            ("DeleteExperimentTemplate", "DELETE", "/experimentTemplates"),
            ("GetExperiment", "GET", "/experiments"),
            ("StopExperiment", "DELETE", "/experiments"),
        ] {
            let answer = send(
                &router,
                method,
                &format!("{path}/{id}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {id:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {id:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
        }
    }

    // StartExperiment resolves its template the same way.
    let started = start_experiment(
        &router,
        json!({ "clientToken": token("missing"), "experimentTemplateId": "EXT000000000099" }),
    )
    .await;
    assert_eq!(
        started.status, 404,
        "case StartExperiment: expected HTTP 404 for an unknown template, got {} with {:?}",
        started.status, started.body
    );
    assert_eq!(
        started.error_code(),
        "ResourceNotFoundException",
        "case StartExperiment: expected ResourceNotFoundException, got {:?}",
        started.body
    );
}

#[tokio::test]
async fn a_request_outside_a_stated_constraint_answers_validation_exception() {
    let router = emulator();

    // A template every pagination row below can be listed against.
    let created = create_template(
        &router,
        json!({
            "clientToken": token("validation"),
            "description": "a template to list",
            "roleArn": "arn:aws:iam::000000000000:role/fis-validation",
            "stopConditions": [{ "source": "none" }],
            "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
        }),
    )
    .await;
    let template = template_id("validation", &created);

    // Each row is a request outside one stated constraint: the two rules FIS documents
    // for a stop condition, the three enum-valued option members, the published bounds on
    // `description` and `maxResults`, and a pagination token this service never issued.
    let bad_creates = [
        (
            "stop-condition-source-not-documented",
            json!({
                "clientToken": token("bad-source"),
                "description": "an undocumented stop condition source",
                "roleArn": "arn:aws:iam::000000000000:role/fis-bad",
                "stopConditions": [{ "source": "aws:cloudwatch:composite-alarm" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
        ),
        (
            "alarm-stop-condition-without-a-value",
            json!({
                "clientToken": token("bad-alarm"),
                "description": "an alarm stop condition with no alarm",
                "roleArn": "arn:aws:iam::000000000000:role/fis-bad",
                "stopConditions": [{ "source": "aws:cloudwatch:alarm" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
        ),
        (
            "account-targeting-outside-the-enum",
            json!({
                "clientToken": token("bad-targeting"),
                "description": "an account targeting value the enum does not admit",
                "roleArn": "arn:aws:iam::000000000000:role/fis-bad",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } },
                "experimentOptions": { "accountTargeting": "every-account" }
            }),
        ),
        (
            "empty-target-resolution-mode-outside-the-enum",
            json!({
                "clientToken": token("bad-resolution"),
                "description": "a resolution mode the enum does not admit",
                "roleArn": "arn:aws:iam::000000000000:role/fis-bad",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } },
                "experimentOptions": { "emptyTargetResolutionMode": "ignore" }
            }),
        ),
        (
            "description-one-past-the-published-ceiling",
            json!({
                "clientToken": token("bad-description"),
                "description": "d".repeat(DESCRIPTION_CEILING + 1),
                "roleArn": "arn:aws:iam::000000000000:role/fis-bad",
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
        ),
    ];

    for (case, body) in bad_creates {
        let answer = create_template(&router, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let bad_start = start_experiment(
        &router,
        json!({
            "clientToken": token("bad-actions-mode"),
            "experimentTemplateId": template,
            "experimentOptions": { "actionsMode": "run-some" }
        }),
    )
    .await;
    assert_eq!(
        bad_start.status, 400,
        "case actions-mode-outside-the-enum: expected HTTP 400, got {} with {:?}",
        bad_start.status, bad_start.body
    );

    // `maxResults` on each published bound, and one past each of them.
    for (case, max_results, expected_status) in [
        ("maxResults at the floor", MAX_RESULTS_FLOOR, 200),
        ("maxResults at the ceiling", MAX_RESULTS_CEILING, 200),
        ("maxResults below the floor", MAX_RESULTS_FLOOR - 1, 400),
        ("maxResults above the ceiling", MAX_RESULTS_CEILING + 1, 400),
    ] {
        for (operation, path, _) in [
            ("ListExperimentTemplates", "/experimentTemplates", ()),
            ("ListExperiments", "/experiments", ()),
        ] {
            let answer = send(
                &router,
                "GET",
                &format!("{path}?maxResults={max_results}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                answer.status, expected_status,
                "case {operation}/{case}: expected HTTP {expected_status}, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }

    // A pagination token this service never issued — including one that is a well-formed
    // id of the OTHER collection.
    for (operation, path, bad_token) in [
        ("ListExperimentTemplates", "/experimentTemplates", "nonsense"),
        (
            "ListExperimentTemplates",
            "/experimentTemplates",
            "EXP000000000001",
        ),
        ("ListExperiments", "/experiments", "nonsense"),
        ("ListExperiments", "/experiments", "EXT000000000001"),
    ] {
        let answer = send(
            &router,
            "GET",
            &format!("{path}?nextToken={bad_token}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {operation}/{bad_token:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {operation}/{bad_token:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}
