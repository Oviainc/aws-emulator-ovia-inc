// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `BatchGetView`.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and POSTs restJson1 requests at it, so
//! nothing is proved about the handler that is not also proved about the serialized
//! request and response an SDK sends and receives.
//!
//! Two rules are under test, each over a table of inputs rather than one request:
//!
//! * a batch read reports every view it was asked for, each exactly as the service's
//!   single-resource reader (`GetView`) reports it, in the order the request named
//!   them; and
//! * it authorizes every ARN in the batch BEFORE reporting anything, so one ARN that
//!   names no view this account holds fails the whole call with `UnauthorizedException`
//!   (HTTP 401) and leaks no detail of the views that did resolve.
//!
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header: a view ARN carries its own Region, so
//! these assertions also pin that a batch is answered from the ARNs and not from the
//! Region the request was signed for.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `crate::state::INDEX_SETTLE_DELAY`, so an index left in `CREATING`
/// or `DELETING` is certain to have settled by the next call.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The published bounds of `BatchGetViewInput$ViewArns`
/// (`smithy_query(query="operation",operation="BatchGetView")` reports
/// `length {min: 1, max: 20}`).
const VIEW_ARNS_MIN: usize = 1;
const VIEW_ARNS_MAX: usize = 20;

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// POST `/{operation}` as a client signed for `region` would.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// `POST /BatchGetView` with the ARNs the caller names, in that order.
    async fn batch_get_view(&self, region: &str, arns: &[&str]) -> Answer {
        self.call(region, "BatchGetView", json!({ "ViewArns": arns }))
            .await
    }
}

/// `CreateIndex` in `region`, asserting success; the index ARN it answered with.
async fn create_index(emulator: &Emulator, region: &str, token: &str) -> String {
    let answer = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": token }))
        .await;
    answer.ok(region)["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string()
}

/// `CreateView` in `region`, asserting success; the view ARN it answered with.
async fn create_view(emulator: &Emulator, region: &str, body: Value) -> String {
    let case = format!("{region} {body}");
    let answer = emulator.call(region, "CreateView", body).await;
    answer.ok(&case)["View"]["ViewArn"]
        .as_str()
        .expect("CreateView answers a View.ViewArn")
        .to_string()
}

/// What `GetView` reports for `arn`: the single-resource reader every batch answer is
/// held against, so no `View` can pass by agreeing with a literal in this file.
async fn view_per_get_view(emulator: &Emulator, region: &str, arn: &str) -> Value {
    let answer = emulator
        .call(region, "GetView", json!({ "ViewArn": arn }))
        .await;
    answer.ok(arn)["View"].clone()
}

/// An ARN in THIS service's namespace, for the emulator's own account.
fn own_arn(region: &str, resource: &str) -> String {
    engine::emulator_arn("resource-explorer-2", region, resource)
}

/// The views of a fixture: two Regions' indexes, and four views whose stored detail
/// differs along every member a `View` carries.
struct Fixture {
    emulator: Emulator,
    /// `us-east-1`: a plain view, one carrying filters, properties and tags, and one
    /// carrying an explicit account scope.
    alpha: String,
    beta: String,
    gamma: String,
    /// `eu-west-1`, so a batch can span Regions.
    delta: String,
}

const REGION_A: &str = "us-east-1";
const REGION_B: &str = "eu-west-1";
/// A Region with no index of its own: a batch signed for it still resolves every ARN,
/// because a view ARN carries the Region the view belongs to.
const REGION_C: &str = "ap-southeast-2";

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        create_index(&emulator, REGION_A, "batch-get-view-a").await;
        create_index(&emulator, REGION_B, "batch-get-view-b").await;
        let account = engine::emulator_account_id();
        let alpha = create_view(&emulator, REGION_A, json!({ "ViewName": "rex-batch-alpha" })).await;
        let beta = create_view(
            &emulator,
            REGION_A,
            json!({
                "ViewName": "rex-batch-beta",
                "Filters": { "FilterString": "service:resource-explorer-2" },
                "IncludedProperties": [{ "Name": "tags" }],
                "Tags": { "team": "platform" },
            }),
        )
        .await;
        let gamma = create_view(
            &emulator,
            REGION_A,
            json!({
                "ViewName": "rex-batch-gamma",
                "Scope": format!("arn:aws:iam::{account}:root"),
            }),
        )
        .await;
        let delta = create_view(&emulator, REGION_B, json!({ "ViewName": "rex-batch-delta" })).await;
        Self {
            emulator,
            alpha,
            beta,
            gamma,
            delta,
        }
    }
}

/// `BatchGetView` answers one `View` per requested ARN, in request order, each identical
/// to what `GetView` reports for the same ARN — and no `Errors`.
///
/// The rows differ along what the rule generalises over: how many ARNs are asked for
/// (one, two, three, every view in the store, and the published maximum in the bounds
/// test below), the ORDER they are named in, the same ARN named twice, views whose
/// stored detail differs (filters, included properties, tags, an explicit scope), ARNs
/// from two Regions in one call, and the Region the call itself is signed for —
/// including one that holds no index at all.
#[tokio::test]
async fn batch_get_view_answers_every_requested_view_in_request_order() {
    let fixture = Fixture::new().await;
    let Fixture {
        ref emulator,
        ref alpha,
        ref beta,
        ref gamma,
        ref delta,
    } = fixture;

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("one view", REGION_A, vec![alpha]),
        ("two views, in request order", REGION_A, vec![beta, alpha]),
        (
            "two views, in the other order",
            REGION_A,
            vec![alpha.as_str(), beta],
        ),
        ("the same ARN twice", REGION_A, vec![alpha, alpha]),
        (
            "a view with filters, properties and tags",
            REGION_A,
            vec![beta],
        ),
        ("a view with an explicit scope", REGION_A, vec![gamma]),
        (
            "two Regions in one call, signed for the second",
            REGION_B,
            vec![alpha.as_str(), delta],
        ),
        (
            "two Regions in one call, signed for a third with no index",
            REGION_C,
            vec![delta.as_str(), alpha],
        ),
        (
            "every view in the store",
            REGION_C,
            vec![gamma.as_str(), delta, alpha, beta],
        ),
    ];

    for (case, region, requested) in cases {
        let answer = emulator.batch_get_view(region, &requested).await;
        let body = answer.ok(case).clone();
        let returned = body["Views"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Views list, got {body}"))
            .clone();
        assert_eq!(
            returned.len(),
            requested.len(),
            "case {case:?}: expected {} views, got {body}",
            requested.len()
        );
        for (position, arn) in requested.iter().enumerate() {
            let expected = view_per_get_view(emulator, region, arn).await;
            assert_eq!(
                returned[position], expected,
                "case {case:?}: view {position} must be the one GetView reports for \
                 {arn:?}, got {}",
                returned[position]
            );
        }
        assert_eq!(
            body["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: every requested ARN resolved, so no ARN failed, got {body}"
        );
    }
}

/// One ARN that names no view this account holds fails the WHOLE batch with
/// `UnauthorizedException` (HTTP 401), wherever in the request it sits.
///
/// The ARN rows span the forms an unholdable ARN takes: another service's resource, a
/// value that is not an ARN, an empty string, this service's OTHER resource kind (an
/// index ARN is a real resource, but it is not a view), a well-formed view ARN whose id
/// was never minted, a view ARN in a Region with no index, another account's view, and —
/// derived from state rather than written down — a view that was deleted, and a view
/// whose Region's index was deleted under it. Each row is tried alone, first, last and
/// in the middle of a batch whose other ARNs are real views.
#[tokio::test]
async fn batch_get_view_refuses_the_whole_batch_for_an_arn_that_names_no_held_view() {
    let fixture = Fixture::new().await;
    let Fixture {
        ref emulator,
        ref alpha,
        ref beta,
        ref gamma,
        ref delta,
    } = fixture;
    let account = engine::emulator_account_id();
    // An account id this emulator is not: 12 digits, and never the caller's own.
    let foreign_account = if account == "999999999999" {
        "111111111111"
    } else {
        "999999999999"
    };
    let index_arn = create_index(emulator, REGION_A, "batch-get-view-a").await;

    // The ARN forms outside this account's views. The namespace rows mirror the shapes
    // `service_common::arn` distinguishes (the one ARN parse this workspace has), which
    // is what `crate::state::is_own_arn` asks about.
    let mut unholdable: Vec<(String, String)> = vec![
        (
            "an SNS topic, another service's resource".to_string(),
            engine::emulator_arn("sns", REGION_A, "rex-batch-foreign"),
        ),
        (
            "a bare name, not an ARN".to_string(),
            "rex-batch-alpha".to_string(),
        ),
        ("an empty string".to_string(), String::new()),
        (
            "this service's index, which is not a view".to_string(),
            index_arn,
        ),
        (
            "a view id never minted".to_string(),
            own_arn(
                REGION_A,
                "view/rex-batch-absent/0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13",
            ),
        ),
        (
            "a view in a Region with no index".to_string(),
            own_arn(
                REGION_C,
                "view/rex-batch-absent/3a6e9c25-1b74-8de0-95f2-8c417b6d2035",
            ),
        ),
        (
            "another account's view".to_string(),
            format!(
                "arn:aws:resource-explorer-2:{REGION_A}:{foreign_account}:\
                 view/rex-batch-other/5c2d8f31-7a46-8b92-ae03-1d49f6520b78"
            ),
        ),
    ];

    // A view that existed and was deleted is no longer held, so it joins the rows.
    emulator
        .call(REGION_A, "DeleteView", json!({ "ViewArn": gamma }))
        .await
        .ok("DeleteView on the third view");
    unholdable.push(("a deleted view".to_string(), gamma.clone()));

    // And so does a view whose Region's index was deleted under it: the index takes its
    // Region's views with it once the deletion settles.
    let index_b = emulator
        .call(REGION_B, "CreateIndex", json!({ "ClientToken": "batch-get-view-b" }))
        .await;
    let index_b = index_b.ok("the second Region's index")["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();
    emulator
        .call(REGION_B, "DeleteIndex", json!({ "Arn": index_b }))
        .await
        .ok("DeleteIndex in the second Region");
    tokio::time::sleep(PAST_SETTLE).await;
    unholdable.push((
        "a view whose Region's index was deleted".to_string(),
        delta.clone(),
    ));

    for (case, arn) in &unholdable {
        let positions: Vec<(&str, Vec<&str>)> = vec![
            ("alone", vec![arn.as_str()]),
            ("first of two", vec![arn.as_str(), alpha]),
            ("last of two", vec![alpha.as_str(), arn]),
            ("in the middle of three", vec![alpha.as_str(), arn, beta]),
        ];
        for (position, requested) in positions {
            let case = format!("{case}, {position}");
            let answer = emulator.batch_get_view(REGION_A, &requested).await;
            answer.failed(&case, 401, "UnauthorizedException");
            assert_eq!(
                answer.error_message(),
                "Unauthorized",
                "case {case:?}: the refusal carries nothing from the request, got {:?}",
                answer.error_message()
            );
            let rendered = answer.body.to_string();
            assert!(
                !rendered.contains(alpha.as_str()) && !rendered.contains("ViewName"),
                "case {case:?}: a refused batch reports no detail of the views that did \
                 resolve, got {rendered}"
            );
        }
    }

    // None of those refusals removed or invented anything: the views that are still
    // held still batch-read, and still agree with GetView.
    let answer = emulator
        .batch_get_view(REGION_A, &[alpha.as_str(), beta])
        .await;
    let body = answer.ok("the surviving views after every refusal").clone();
    for (position, arn) in [alpha, beta].iter().enumerate() {
        let expected = view_per_get_view(emulator, REGION_A, arn).await;
        assert_eq!(
            body["Views"][position], expected,
            "case {arn:?}: a refused batch must leave the store untouched, got {body}"
        );
    }
}

/// The batch's size is bounded by the modelled `ViewArns` length: the minimum and the
/// maximum are served, and one element past either bound is refused with the modelled
/// `ValidationException` (HTTP 400). A request that names NO list at all asks for no
/// details — the member is not `@required` — so it is answered with an empty batch.
#[tokio::test]
async fn batch_get_view_serves_the_modelled_bounds_of_its_arn_list() {
    let emulator = Emulator::new();
    create_index(&emulator, REGION_A, "batch-get-view-bounds").await;
    let mut created = Vec::new();
    for n in 0..VIEW_ARNS_MAX {
        created.push(
            create_view(
                &emulator,
                REGION_A,
                json!({ "ViewName": format!("rex-bound-{n:02}") }),
            )
            .await,
        );
    }

    // The minimum, the maximum, and a size in between.
    for size in [VIEW_ARNS_MIN, 7, VIEW_ARNS_MAX] {
        let requested: Vec<&str> = created[..size].iter().map(String::as_str).collect();
        let answer = emulator.batch_get_view(REGION_A, &requested).await;
        let body = answer.ok(&format!("{size} ARNs")).clone();
        let returned: Vec<&str> = body["Views"]
            .as_array()
            .unwrap_or_else(|| panic!("case {size} ARNs: expected a Views list, got {body}"))
            .iter()
            .filter_map(|view| view["ViewArn"].as_str())
            .collect();
        assert_eq!(
            returned, requested,
            "case {size} ARNs: expected exactly the ARNs asked for, in order, got {body}"
        );
    }

    // One past each bound: an empty list, and one ARN more than the maximum (the extra
    // one is a real view, so only the size is wrong).
    let too_many: Vec<&str> = created
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(created[0].as_str()))
        .collect();
    assert_eq!(
        too_many.len(),
        VIEW_ARNS_MAX + 1,
        "the over-long row must be exactly one past the maximum"
    );
    emulator
        .batch_get_view(REGION_A, &too_many)
        .await
        .failed("one ARN past the maximum", 400, "ValidationException");
    emulator
        .batch_get_view(REGION_A, &[])
        .await
        .failed("one ARN short of the minimum", 400, "ValidationException");

    // No list at all: nothing was asked for, so nothing is reported and nothing fails.
    let absent = emulator.call(REGION_A, "BatchGetView", json!({})).await;
    let body = absent.ok("no ViewArns member").clone();
    assert_eq!(
        body["Views"].as_array().map(Vec::len),
        Some(0),
        "case no ViewArns member: a request that names no view asks for no details, \
         got {body}"
    );
    assert_eq!(
        body["Errors"].as_array().map(Vec::len),
        Some(0),
        "case no ViewArns member: nothing was asked for, so nothing failed, got {body}"
    );
}
