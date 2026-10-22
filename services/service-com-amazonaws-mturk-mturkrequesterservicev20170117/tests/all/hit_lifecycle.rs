// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the HIT lifecycle: `CreateHIT`, `GetHIT`, `ListHITs`,
//! `UpdateExpirationForHIT`, `DeleteHIT` and `GetAccountBalance`, driven over the
//! wire (`POST /` with `X-Amz-Target: MTurkRequesterServiceV20170117.<Operation>`,
//! awsJson1_1) through the same `MultiServiceRouter` the production router serves
//! from.
//!
//! Every test states a RULE and asserts it over a SET of inputs; the request the
//! issue wrote down is one row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The `HITStatus` / `HITReviewStatus` catalogs are not re-typed here: these are the
/// model-derived constants the scaffold generated into `src/ops/types/shared_types.rs`,
/// so a table built from them cannot drift from the model.
use service_com_amazonaws_mturk_mturkrequesterservicev20170117::ops::types::{
    H_I_T_REVIEW_STATUS_VALUES, H_I_T_STATUS_VALUES,
};
use service_com_amazonaws_mturk_mturkrequesterservicev20170117::state::{
    ComAmazonawsMturkMturkrequesterservicev20170117State, DEFAULT_MAX_ASSIGNMENTS,
    DEFAULT_MAX_RESULTS, UNIQUE_REQUEST_TOKEN_TTL,
};

/// A router carrying only this service, over one shared engine — what
/// `core/router/src/lib.rs` builds, narrowed to the service under test. Request
/// authentication is off so the suite needs no signing keys; routing still goes
/// through the SigV4 credential scope on the `authorization` header below.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// As [`router`], plus the shared engine it was built from, so a test can reach the
/// store directly — to move a HIT's expiration into the past without sleeping, or to
/// prove `GetAccountBalance` reports what is persisted rather than a literal.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false).register_service(
        service_com_amazonaws_mturk_mturkrequesterservicev20170117::EmulatedService::new(
            engine.clone(),
        ),
    );
    (router, engine)
}

/// Call one operation and return `(status, body)`. The body is `Value::Null` when the
/// response carried no bytes.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("MTurkRequesterServiceV20170117.{operation}"),
        )
        // The routing key every front door reads: the 4th field of the SigV4
        // credential scope is this service's signing name, `mturk-requester`.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/mturk-requester/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Whether an error body names `exception`. The awsJson `__type` is either the bare
/// shape name or the namespaced shape id, so the suffix is what is matched.
fn is_error(body: &Value, exception: &str) -> bool {
    body.get("__type")
        .and_then(Value::as_str)
        .is_some_and(|found| found.ends_with(exception))
}

/// Whether `id` satisfies `EntityId`: `@pattern ^[A-Z0-9]+$`, `@length(1, 64)`.
fn is_entity_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

/// The `HIT` out of a `CreateHIT` / `GetHIT` response.
fn hit(body: &Value) -> &Value {
    body.get("HIT")
        .unwrap_or_else(|| panic!("response carries no HIT: {body}"))
}

/// A string member of a JSON object, or `""`.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A numeric member of a JSON object, as `f64`.
fn number(value: &Value, member: &str) -> f64 {
    value.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// The `CreateHIT` input members that are also `HIT` members (from `smithy_query` on
/// `CreateHITRequest` and `HIT`): whatever the caller supplies for one of these, the
/// stored HIT reports back unchanged. The rest of `CreateHIT`'s input is either
/// consumed (`LifetimeInSeconds`, `UniqueRequestToken`) or belongs to a slice this
/// change does not implement (the review policies, `HITLayoutParameters`).
const ECHOED_MEMBERS: &[&str] = &[
    "Title",
    "Description",
    "Question",
    "Keywords",
    "Reward",
    "MaxAssignments",
    "AssignmentDurationInSeconds",
    "AutoApprovalDelayInSeconds",
    "RequesterAnnotation",
    "QualificationRequirements",
    "HITLayoutId",
];

/// Assert the rule a freshly created HIT satisfies, whatever it was created from:
/// every member supplied is reported back, the two ids are `EntityId`s, the HIT is
/// `Assignable` / `NotReviewed`, `Expiration - CreationTime == LifetimeInSeconds`,
/// and all of `MaxAssignments` (default 1) are available with none pending or done.
fn assert_fresh_hit(case: &str, created: &Value, request: &Value) {
    for member in ECHOED_MEMBERS {
        if let Some(supplied) = request.get(member) {
            assert_eq!(
                created.get(member),
                Some(supplied),
                "case {case:?}: HIT.{member} must report what CreateHIT was given \
                 ({supplied}), got {:?}",
                created.get(member)
            );
        }
    }

    let hit_id = text(created, "HITId");
    assert!(
        is_entity_id(&hit_id),
        "case {case:?}: HITId must match EntityId ^[A-Z0-9]+$ (len 1..64), got {hit_id:?}"
    );
    let hit_type_id = text(created, "HITTypeId");
    assert!(
        is_entity_id(&hit_type_id),
        "case {case:?}: HITTypeId must match EntityId ^[A-Z0-9]+$ (len 1..64), got \
         {hit_type_id:?}"
    );

    let status = text(created, "HITStatus");
    assert_eq!(
        status, "Assignable",
        "case {case:?}: a HIT with assignments available and lifetime left is Assignable, \
         got {status:?}"
    );
    assert!(
        H_I_T_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: HITStatus {status:?} is not one of {H_I_T_STATUS_VALUES:?}"
    );
    let review_status = text(created, "HITReviewStatus");
    assert_eq!(
        review_status, "NotReviewed",
        "case {case:?}: a HIT nobody has reviewed is NotReviewed, got {review_status:?}"
    );
    assert!(
        H_I_T_REVIEW_STATUS_VALUES.contains(&review_status.as_str()),
        "case {case:?}: HITReviewStatus {review_status:?} is not one of \
         {H_I_T_REVIEW_STATUS_VALUES:?}"
    );

    let max_assignments = request
        .get("MaxAssignments")
        .and_then(Value::as_f64)
        .unwrap_or(f64::from(DEFAULT_MAX_ASSIGNMENTS));
    assert_eq!(
        number(created, "MaxAssignments"),
        max_assignments,
        "case {case:?}: MaxAssignments defaults to {DEFAULT_MAX_ASSIGNMENTS} when absent"
    );
    assert_eq!(
        number(created, "NumberOfAssignmentsAvailable"),
        max_assignments,
        "case {case:?}: every assignment of a new HIT is available"
    );
    assert_eq!(
        number(created, "NumberOfAssignmentsPending"),
        0.0,
        "case {case:?}: a new HIT has no pending assignment"
    );
    assert_eq!(
        number(created, "NumberOfAssignmentsCompleted"),
        0.0,
        "case {case:?}: a new HIT has no completed assignment"
    );

    let lifetime = request
        .get("LifetimeInSeconds")
        .and_then(Value::as_f64)
        .expect("the table supplies LifetimeInSeconds");
    let lived = number(created, "Expiration") - number(created, "CreationTime");
    assert!(
        (lived - lifetime).abs() < 0.001,
        "case {case:?}: Expiration must be CreationTime + LifetimeInSeconds ({lifetime}s), \
         got {lived}s"
    );
}

/// One row of the lifecycle table: a case name and the `CreateHIT` request.
struct Row {
    case: &'static str,
    request: Value,
}

/// A question payload. `CreateHIT` documents "Either a Question parameter or a
/// HITLayoutId parameter must be provided", and an `ExternalQuestion` is the smallest
/// form that satisfies it.
fn external_question(url: &str) -> String {
    format!(
        "<ExternalQuestion xmlns=\"http://mechanicalturk.amazonaws.com/\
         AWSMechanicalTurkDataSchemas/2006-07-14/ExternalQuestion.xsd\">\
         <ExternalURL>{url}</ExternalURL><FrameHeight>400</FrameHeight></ExternalQuestion>"
    )
}

/// The lifecycle table. Rows differ along what the rules generalise over: the number
/// of assignments (absent, one, several, the 10 that MTurk prices differently), the
/// `Reward` form the `^[0-9]+(\.)?[0-9]{0,2}$` pattern allows (two decimals, none, a
/// bare trailing dot), the lifetime and assignment duration, whether the work is
/// described by a `Question` or by a `HITLayoutId`, and the number of
/// `QualificationRequirements` (zero, one, two — including a `Locale` comparison).
fn lifecycle_rows() -> Vec<Row> {
    vec![
        Row {
            // The case the issue wrote down, plus the Question its own documented
            // constraint requires.
            case: "the issue's request: two assignments, 0.05 reward",
            request: json!({
                "Title": "Test HIT",
                "Description": "demo",
                "Reward": "0.05",
                "LifetimeInSeconds": 3600,
                "AssignmentDurationInSeconds": 600,
                "MaxAssignments": 2,
                "Question": external_question("https://example.invalid/task"),
            }),
        },
        Row {
            case: "no MaxAssignments, so it defaults to one",
            request: json!({
                "Title": "Transcribe a receipt",
                "Description": "Type the total from the photo.",
                "Reward": "1",
                "LifetimeInSeconds": 30,
                "AssignmentDurationInSeconds": 30,
                "Question": external_question("https://example.invalid/receipt"),
            }),
        },
        Row {
            case: "a layout instead of a question, with annotation and keywords",
            request: json!({
                "Title": "Label an image",
                "Description": "Pick every box that contains a bicycle.",
                "Reward": "2.",
                "LifetimeInSeconds": 86_400,
                "AssignmentDurationInSeconds": 1_200,
                "MaxAssignments": 10,
                "AutoApprovalDelayInSeconds": 172_800,
                "Keywords": "image, labelling, bicycle",
                "RequesterAnnotation": "batch-17/item-3",
                "HITLayoutId": "3ABCDEF0123456789",
                "HITLayoutParameters": [{ "Name": "image_url", "Value": "https://example.invalid/1.png" }],
            }),
        },
        Row {
            case: "one qualification requirement",
            request: json!({
                "Title": "Rate a translation",
                "Description": "Score the translation from 1 to 5.",
                "Reward": "0.5",
                "LifetimeInSeconds": 7_200,
                "AssignmentDurationInSeconds": 900,
                "MaxAssignments": 3,
                "Question": external_question("https://example.invalid/translation"),
                "QualificationRequirements": [{
                    "QualificationTypeId": "000000000000000000L0",
                    "Comparator": "GreaterThanOrEqualTo",
                    "IntegerValues": [95],
                    "ActionsGuarded": "DiscoverPreviewAndAccept",
                }],
            }),
        },
        Row {
            case: "two qualification requirements, one of them a locale",
            request: json!({
                "Title": "Moderate a comment",
                "Description": "Flag the comment if it breaks the rules.",
                "Reward": "0.10",
                "LifetimeInSeconds": 1_209_600,
                "AssignmentDurationInSeconds": 300,
                "MaxAssignments": 5,
                "Question": external_question("https://example.invalid/moderate"),
                "QualificationRequirements": [
                    {
                        "QualificationTypeId": "00000000000000000071",
                        "Comparator": "In",
                        "LocaleValues": [{ "Country": "US", "Subdivision": "WA" }, { "Country": "GB" }],
                    },
                    {
                        "QualificationTypeId": "000000000000000000L0",
                        "Comparator": "Exists",
                        "RequiredToPreview": true,
                    },
                ],
            }),
        },
    ]
}

/// The whole lifecycle the issue names, over every row of the table: create, read it
/// back, see it in the list, expire it, see the expiry, dispose of it, and find it
/// gone from both readers.
#[tokio::test]
async fn hit_lifecycle_over_the_wire() {
    for row in lifecycle_rows() {
        let case = row.case;
        let router = router();

        let (status, body) = call(&router, "CreateHIT", &row.request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        let created = hit(&body).clone();
        assert_fresh_hit(case, &created, &row.request);
        let hit_id = text(&created, "HITId");

        // GetHIT reads the HIT back out of the store: the same structure, not a second
        // rendering of the request.
        let (status, body) = call(&router, "GetHIT", &json!({ "HITId": hit_id })).await;
        assert_eq!(status, 200, "case {case:?}: GetHIT answered {status}: {body}");
        assert_eq!(
            hit(&body),
            &created,
            "case {case:?}: GetHIT must return exactly the HIT CreateHIT stored"
        );

        // ListHITs reports it too.
        let (status, body) = call(&router, "ListHITs", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListHITs answered {status}: {body}");
        assert_eq!(
            body.get("HITs"),
            Some(&json!([created.clone()])),
            "case {case:?}: the one HIT created must be the one HIT listed"
        );
        assert_eq!(
            number(&body, "NumResults"),
            1.0,
            "case {case:?}: NumResults counts the HITs on this page"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: no NextToken when nothing remains after the page"
        );

        // Expiring a HIT is "update its expiration to a time in the past".
        let expire_at = now_secs() - 60;
        let (status, body) = call(
            &router,
            "UpdateExpirationForHIT",
            &json!({ "HITId": hit_id, "ExpireAt": expire_at }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateExpirationForHIT answered {status}: {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: UpdateExpirationForHIT has an empty output body"
        );

        let (status, body) = call(&router, "GetHIT", &json!({ "HITId": hit_id })).await;
        assert_eq!(status, 200, "case {case:?}: GetHIT answered {status}: {body}");
        let expired = hit(&body);
        assert_eq!(
            number(expired, "Expiration"),
            expire_at as f64,
            "case {case:?}: the stored Expiration must be the ExpireAt supplied"
        );
        assert_eq!(
            text(expired, "HITStatus"),
            "Reviewable",
            "case {case:?}: a HIT past its expiration is Reviewable, got {:?}",
            text(expired, "HITStatus")
        );
        assert_eq!(
            number(expired, "NumberOfAssignmentsAvailable"),
            0.0,
            "case {case:?}: an expired HIT offers no assignment"
        );
        assert_eq!(
            text(expired, "HITId"),
            hit_id,
            "case {case:?}: expiring a HIT does not change its id"
        );
        assert_eq!(
            number(expired, "CreationTime"),
            number(&created, "CreationTime"),
            "case {case:?}: expiring a HIT does not change when it was created"
        );

        // Disposing of a Reviewable HIT succeeds and is observable by both readers.
        let (status, body) = call(&router, "DeleteHIT", &json!({ "HITId": hit_id })).await;
        assert_eq!(status, 200, "case {case:?}: DeleteHIT answered {status}: {body}");
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: DeleteHIT has an empty output body"
        );

        let (status, body) = call(&router, "GetHIT", &json!({ "HITId": hit_id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetHIT on a disposed HIT is a 400 RequestError, got {status}: {body}"
        );
        assert!(
            is_error(&body, "RequestError"),
            "case {case:?}: GetHIT on a disposed HIT must be a RequestError, got {body}"
        );

        let (status, body) = call(&router, "ListHITs", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListHITs answered {status}: {body}");
        assert_eq!(
            body.get("HITs"),
            Some(&json!([])),
            "case {case:?}: a disposed HIT is not returned by ListHITs"
        );
        assert_eq!(
            number(&body, "NumResults"),
            0.0,
            "case {case:?}: NumResults is zero on an empty page"
        );
    }
}

/// Seconds since the epoch, as an awsJson1_1 timestamp.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_secs(),
    )
    .expect("the epoch seconds fit in an i64")
}

/// The `CreateHIT` request the pagination and id tests reuse, with `Title` varied so
/// the HITs are distinguishable.
fn simple_hit(title: &str) -> Value {
    json!({
        "Title": title,
        "Description": "A task.",
        "Reward": "0.01",
        "LifetimeInSeconds": 3600,
        "AssignmentDurationInSeconds": 600,
        "Question": external_question("https://example.invalid/task"),
    })
}

/// RULE: `ListHITs` returns the HITs newest first, cut into pages of at most
/// `MaxResults`, and walking the `NextToken`s visits every HIT exactly once.
#[tokio::test]
async fn list_hits_pages_without_dropping_or_duplicating() {
    // The sizes span a page that is shorter than, equal to and longer than the store,
    // plus the published bounds of `MaxResults` (`@range(1, 100)`).
    for hit_count in [0_usize, 1, 2, 5, 7] {
        for page_size in [None, Some(1), Some(2), Some(3), Some(100)] {
            let case = format!("{hit_count} HITs, MaxResults={page_size:?}");
            let router = router();

            let mut created_newest_first: Vec<Value> = Vec::new();
            for index in 0..hit_count {
                let (status, body) =
                    call(&router, "CreateHIT", &simple_hit(&format!("HIT {index}"))).await;
                assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
                created_newest_first.insert(0, hit(&body).clone());
            }

            let mut seen: Vec<Value> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({});
                if let Some(max) = page_size {
                    request["MaxResults"] = json!(max);
                }
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let (status, body) = call(&router, "ListHITs", &request).await;
                assert_eq!(status, 200, "case {case:?}: ListHITs answered {status}: {body}");
                let page = body
                    .get("HITs")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("case {case:?}: ListHITs returned no HITs member"))
                    .clone();
                let expected_page_size =
                    page_size.map_or(DEFAULT_MAX_RESULTS, |max| max as usize);
                assert!(
                    page.len() <= expected_page_size,
                    "case {case:?}: a page holds at most MaxResults items, got {}",
                    page.len()
                );
                assert_eq!(
                    number(&body, "NumResults"),
                    page.len() as f64,
                    "case {case:?}: NumResults counts the items on this page"
                );
                seen.extend(page);
                next_token = body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                assert_eq!(
                    next_token.is_some(),
                    seen.len() < hit_count,
                    "case {case:?}: a NextToken is returned exactly while HITs remain \
                     (seen {} of {hit_count})",
                    seen.len()
                );
                if let Some(token) = &next_token {
                    assert!(
                        !token.is_empty() && token.len() <= 255,
                        "case {case:?}: a NextToken must satisfy PaginationToken's \
                         @length(1, 255), got {} characters",
                        token.len()
                    );
                }
                pages += 1;
                assert!(
                    pages <= hit_count + 1,
                    "case {case:?}: pagination must terminate; {pages} pages for {hit_count} HITs"
                );
                if next_token.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen, created_newest_first,
                "case {case:?}: the pages concatenated must be every HIT, newest first"
            );
        }
    }
}

/// RULE: `MaxResults` outside the modelled `@range(1, 100)` is refused at the
/// boundary; the bounds themselves are accepted.
#[tokio::test]
async fn list_hits_honours_the_published_max_results_bounds() {
    let router = router();
    let (status, body) = call(&router, "CreateHIT", &simple_hit("only")).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");

    for (max_results, accepted) in [(0, false), (1, true), (100, true), (101, false)] {
        let (status, body) = call(&router, "ListHITs", &json!({ "MaxResults": max_results })).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: inside @range(1, 100), got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case MaxResults={max_results}: outside @range(1, 100) must be refused, got \
                 {status}: {body}"
            );
        }
    }
}

/// RULE: a `NextToken` this service did not mint is refused as a `RequestError`
/// rather than silently restarting the listing.
#[tokio::test]
async fn list_hits_refuses_a_token_it_did_not_mint() {
    let router = router();
    let (status, body) = call(&router, "CreateHIT", &simple_hit("only")).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");

    for token in ["x", "not-a-token", "6d7475726b", "00", "ZZZZ"] {
        let (status, body) = call(&router, "ListHITs", &json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case token={token:?}: a foreign token is a 400 RequestError, got {status}: {body}"
        );
        assert!(
            is_error(&body, "RequestError"),
            "case token={token:?}: expected RequestError, got {body}"
        );
    }
}

/// RULE: the `HITTypeId` is a function of the HIT-TYPE parameters only — the title,
/// description, keywords, reward, assignment duration, auto-approval delay and
/// qualification requirements — so two HITs described identically share one, and a
/// HIT differing in any one of them gets another.
#[tokio::test]
async fn hit_type_id_is_a_function_of_the_hit_type_parameters() {
    let router = router();

    let base = json!({
        "Title": "Describe a photo",
        "Description": "Write one sentence about the photo.",
        "Keywords": "photo, writing",
        "Reward": "0.25",
        "LifetimeInSeconds": 3600,
        "AssignmentDurationInSeconds": 600,
        "AutoApprovalDelayInSeconds": 86_400,
        "MaxAssignments": 1,
        "Question": external_question("https://example.invalid/photo"),
        "QualificationRequirements": [{
            "QualificationTypeId": "000000000000000000L0",
            "Comparator": "GreaterThan",
            "IntegerValues": [50],
        }],
    });

    let (status, body) = call(&router, "CreateHIT", &base).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");
    let base_type_id = text(hit(&body), "HITTypeId");
    let base_hit_id = text(hit(&body), "HITId");

    // Same type parameters again: one HITTypeId, two HITs.
    let (status, body) = call(&router, "CreateHIT", &base).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");
    assert_eq!(
        text(hit(&body), "HITTypeId"),
        base_type_id,
        "case \"identical type parameters\": the HITTypeId must be reused"
    );
    assert_ne!(
        text(hit(&body), "HITId"),
        base_hit_id,
        "case \"identical type parameters\": each HIT still gets its own HITId"
    );

    // Members that do NOT describe the type: the HITTypeId must not move.
    for (case, member, value) in [
        ("MaxAssignments", "MaxAssignments", json!(7)),
        ("LifetimeInSeconds", "LifetimeInSeconds", json!(60)),
        (
            "Question",
            "Question",
            json!(external_question("https://example.invalid/other")),
        ),
        ("RequesterAnnotation", "RequesterAnnotation", json!("batch-2")),
    ] {
        let mut request = base.clone();
        request[member] = value;
        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        assert_eq!(
            text(hit(&body), "HITTypeId"),
            base_type_id,
            "case {case:?}: {member} does not describe the HIT type, so the HITTypeId must \
             not change"
        );
    }

    // Each member that DOES describe the type, varied one at a time.
    for (case, member, value) in [
        ("Title", "Title", json!("Describe a photograph")),
        ("Description", "Description", json!("Write two sentences.")),
        ("Keywords", "Keywords", json!("photo, writing, caption")),
        ("Reward", "Reward", json!("0.26")),
        (
            "AssignmentDurationInSeconds",
            "AssignmentDurationInSeconds",
            json!(601),
        ),
        (
            "AutoApprovalDelayInSeconds",
            "AutoApprovalDelayInSeconds",
            json!(86_401),
        ),
        (
            "QualificationRequirements",
            "QualificationRequirements",
            json!([{
                "QualificationTypeId": "000000000000000000L0",
                "Comparator": "GreaterThan",
                "IntegerValues": [51],
            }]),
        ),
    ] {
        let mut request = base.clone();
        request[member] = value;
        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        assert_ne!(
            text(hit(&body), "HITTypeId"),
            base_type_id,
            "case {case:?}: {member} describes the HIT type, so the HITTypeId must change"
        );
    }
}

/// RULE: a repeated `UniqueRequestToken` never creates a second HIT — the model
/// documents it as "a AWS.MechanicalTurk.HitAlreadyExists error with a message
/// containing the HITId" — while distinct tokens create distinct HITs.
#[tokio::test]
async fn a_repeated_unique_request_token_creates_no_second_hit() {
    for token in ["t", "retry-after-timeout", &"z".repeat(64)] {
        let case = format!("token={token:?}");
        let router = router();

        let mut request = simple_hit("Idempotent");
        request["UniqueRequestToken"] = json!(token);

        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        let hit_id = text(hit(&body), "HITId");

        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a replayed UniqueRequestToken is refused, got {status}: {body}"
        );
        assert!(
            is_error(&body, "RequestError"),
            "case {case:?}: expected RequestError, got {body}"
        );
        assert_eq!(
            text(&body, "TurkErrorCode"),
            "AWS.MechanicalTurk.HitAlreadyExists",
            "case {case:?}: the model names the code for a replayed token"
        );
        assert!(
            text(&body, "Message").contains(&hit_id),
            "case {case:?}: the message must contain the HITId {hit_id}, got {body}"
        );

        // A different token is a different request, so it creates another HIT. (Kept
        // inside `IdempotencyToken`'s `@length(1, 64)`, which the row at the bound
        // would otherwise exceed.)
        let other_token = if token.len() >= 64 {
            token[1..].to_string()
        } else {
            format!("{token}-2")
        };
        request["UniqueRequestToken"] = json!(other_token);
        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        assert_ne!(
            text(hit(&body), "HITId"),
            hit_id,
            "case {case:?}: a fresh token creates a fresh HIT"
        );

        let (status, body) = call(&router, "ListHITs", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListHITs answered {status}: {body}");
        assert_eq!(
            number(&body, "NumResults"),
            2.0,
            "case {case:?}: the replay must not have created a third HIT: {body}"
        );
    }
}

/// RULE: `DeleteHIT` disposes of a HIT only "in the Reviewable state"; anything else
/// — including an id the store does not hold, which is what a second delete sees — is
/// a `RequestError`.
#[tokio::test]
async fn delete_hit_requires_a_reviewable_hit() {
    let router = router();

    let (status, body) = call(&router, "CreateHIT", &simple_hit("Still assignable")).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");
    let hit_id = text(hit(&body), "HITId");

    // Assignable: its lifetime has not elapsed, so it cannot be disposed of.
    let (status, body) = call(&router, "DeleteHIT", &json!({ "HITId": hit_id })).await;
    assert_eq!(
        status, 400,
        "case \"Assignable\": DeleteHIT on a live HIT is refused, got {status}: {body}"
    );
    assert!(
        is_error(&body, "RequestError"),
        "case \"Assignable\": expected RequestError, got {body}"
    );

    // Still there, and untouched by the refusal.
    let (status, body) = call(&router, "GetHIT", &json!({ "HITId": hit_id })).await;
    assert_eq!(status, 200, "case \"Assignable\": GetHIT answered {status}: {body}");
    assert_eq!(
        text(hit(&body), "HITStatus"),
        "Assignable",
        "case \"Assignable\": a refused delete leaves the status alone"
    );

    // Expired: now Reviewable, so it can be disposed of.
    let (status, body) = call(
        &router,
        "UpdateExpirationForHIT",
        &json!({ "HITId": hit_id, "ExpireAt": now_secs() - 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"Reviewable\": UpdateExpirationForHIT answered {status}: {body}"
    );
    let (status, body) = call(&router, "DeleteHIT", &json!({ "HITId": hit_id })).await;
    assert_eq!(
        status, 200,
        "case \"Reviewable\": DeleteHIT answered {status}: {body}"
    );

    // Gone: a second delete, and any unknown id, is a RequestError rather than a panic
    // or a silent success.
    for unknown in [hit_id.as_str(), "ZZZZZZZZ", "0"] {
        let (status, body) = call(&router, "DeleteHIT", &json!({ "HITId": unknown })).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: DeleteHIT on an unknown HIT is a 400 RequestError, got \
             {status}: {body}"
        );
        assert!(
            is_error(&body, "RequestError"),
            "case {unknown:?}: expected RequestError, got {body}"
        );
    }
}

/// RULE: `UpdateExpirationForHIT` moves the stored `Expiration` to whatever `ExpireAt`
/// says, and the HIT's status follows from it: past means expired (`Reviewable`, no
/// assignment available), future means the remaining assignments are offered again.
#[tokio::test]
async fn update_expiration_moves_the_expiration_in_both_directions() {
    let router = router();
    let mut request = simple_hit("Extendable");
    request["MaxAssignments"] = json!(4);
    let (status, body) = call(&router, "CreateHIT", &request).await;
    assert_eq!(status, 200, "CreateHIT answered {status}: {body}");
    let hit_id = text(hit(&body), "HITId");

    let now = now_secs();
    for (case, expire_at, status_wanted, available_wanted) in [
        ("an hour ago", now - 3_600, "Reviewable", 0.0),
        ("a second ago", now - 1, "Reviewable", 0.0),
        ("in a day", now + 86_400, "Assignable", 4.0),
        ("in a year", now + 31_536_000, "Assignable", 4.0),
        ("the epoch", 0, "Reviewable", 0.0),
    ] {
        let (status, body) = call(
            &router,
            "UpdateExpirationForHIT",
            &json!({ "HITId": hit_id, "ExpireAt": expire_at }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateExpirationForHIT answered {status}: {body}"
        );

        let (status, body) = call(&router, "GetHIT", &json!({ "HITId": hit_id })).await;
        assert_eq!(status, 200, "case {case:?}: GetHIT answered {status}: {body}");
        let found = hit(&body);
        assert_eq!(
            number(found, "Expiration"),
            expire_at as f64,
            "case {case:?}: GetHIT must show the Expiration that was set"
        );
        assert_eq!(
            text(found, "HITStatus"),
            status_wanted,
            "case {case:?}: expected HITStatus {status_wanted}, got {:?}",
            text(found, "HITStatus")
        );
        assert_eq!(
            number(found, "NumberOfAssignmentsAvailable"),
            available_wanted,
            "case {case:?}: expected {available_wanted} assignments available"
        );
    }

    // An id the store does not hold cannot be updated.
    let (status, body) = call(
        &router,
        "UpdateExpirationForHIT",
        &json!({ "HITId": "ZZZZZZZZ", "ExpireAt": now }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"unknown id\": expected a 400 RequestError, got {status}: {body}"
    );
    assert!(
        is_error(&body, "RequestError"),
        "case \"unknown id\": expected RequestError, got {body}"
    );
}

/// RULE: the expiry of a HIT is owed by the CLOCK, not by a call — a HIT whose
/// `Expiration` has passed reads as `Reviewable` with no assignment available even
/// though nothing was called in between.
#[tokio::test]
async fn expiry_is_due_from_the_clock_alone() {
    let (router, engine) = router_with_engine();

    let mut ids = Vec::new();
    for index in 0..3 {
        let (status, body) = call(&router, "CreateHIT", &simple_hit(&format!("HIT {index}"))).await;
        assert_eq!(status, 200, "CreateHIT answered {status}: {body}");
        ids.push(text(hit(&body), "HITId"));
    }

    // Move two of the three expirations into the past in the store itself — the state
    // a HIT reaches when its lifetime elapses with nobody watching. No sleeping: the
    // transition is owed at an instant, not after a wait.
    let state = engine.state::<ComAmazonawsMturkMturkrequesterservicev20170117State>();
    for id in &ids[..2] {
        state
            .lock()
            .hits
            .get_mut(id)
            .unwrap_or_else(|| panic!("the store holds {id}"))
            .hit
            .expiration = Some(::aws_smithy_types::DateTime::from_secs(now_secs() - 5));
    }

    for (index, id) in ids.iter().enumerate() {
        let expired = index < 2;
        let case = format!("HIT {index}, expired={expired}");
        let (status, body) = call(&router, "GetHIT", &json!({ "HITId": id })).await;
        assert_eq!(status, 200, "case {case:?}: GetHIT answered {status}: {body}");
        let found = hit(&body);
        let status_wanted = if expired { "Reviewable" } else { "Assignable" };
        assert_eq!(
            text(found, "HITStatus"),
            status_wanted,
            "case {case:?}: expected HITStatus {status_wanted}, got {:?}",
            text(found, "HITStatus")
        );
        assert_eq!(
            number(found, "NumberOfAssignmentsAvailable"),
            if expired { 0.0 } else { 1.0 },
            "case {case:?}: an expired HIT offers no assignment"
        );
    }

    // ListHITs reports the same statuses GetHIT does.
    let (status, body) = call(&router, "ListHITs", &json!({})).await;
    assert_eq!(status, 200, "ListHITs answered {status}: {body}");
    let listed = body
        .get("HITs")
        .and_then(Value::as_array)
        .expect("ListHITs returns HITs")
        .clone();
    assert_eq!(listed.len(), 3, "every HIT is still listed: {body}");
    for entry in &listed {
        let id = text(entry, "HITId");
        let expired = ids[..2].contains(&id);
        let status_wanted = if expired { "Reviewable" } else { "Assignable" };
        assert_eq!(
            text(entry, "HITStatus"),
            status_wanted,
            "case {id:?}: ListHITs must report the same status GetHIT does"
        );
    }

    // And the clock-owed transition is what makes the expired ones disposable.
    for id in &ids[..2] {
        let (status, body) = call(&router, "DeleteHIT", &json!({ "HITId": id })).await;
        assert_eq!(
            status, 200,
            "case {id:?}: a HIT the clock expired is disposable, got {status}: {body}"
        );
    }
}

/// RULE: `CreateHIT` refuses a request the model cannot refuse on its own — no work
/// for a Worker to do, or a count the HIT could never satisfy — with a `RequestError`,
/// and stores nothing for it.
#[tokio::test]
async fn create_hit_refuses_what_it_cannot_store() {
    let router = router();

    let refusals = vec![
        (
            "neither Question nor HITLayoutId",
            json!({
                "Title": "Nothing to do",
                "Description": "No question and no layout.",
                "Reward": "0.01",
                "LifetimeInSeconds": 3600,
                "AssignmentDurationInSeconds": 600,
            }),
        ),
        (
            "MaxAssignments of zero",
            json!({
                "Title": "Nobody can take it",
                "Description": "Zero assignments.",
                "Reward": "0.01",
                "LifetimeInSeconds": 3600,
                "AssignmentDurationInSeconds": 600,
                "MaxAssignments": 0,
                "Question": external_question("https://example.invalid/task"),
            }),
        ),
        (
            "negative MaxAssignments",
            json!({
                "Title": "Nobody can take it",
                "Description": "Negative assignments.",
                "Reward": "0.01",
                "LifetimeInSeconds": 3600,
                "AssignmentDurationInSeconds": 600,
                "MaxAssignments": -1,
                "Question": external_question("https://example.invalid/task"),
            }),
        ),
        (
            "LifetimeInSeconds of zero",
            json!({
                "Title": "Expired on arrival",
                "Description": "No lifetime.",
                "Reward": "0.01",
                "LifetimeInSeconds": 0,
                "AssignmentDurationInSeconds": 600,
                "Question": external_question("https://example.invalid/task"),
            }),
        ),
        (
            "negative AssignmentDurationInSeconds",
            json!({
                "Title": "No time to work",
                "Description": "Negative duration.",
                "Reward": "0.01",
                "LifetimeInSeconds": 3600,
                "AssignmentDurationInSeconds": -600,
                "Question": external_question("https://example.invalid/task"),
            }),
        ),
    ];

    for (case, request) in refusals {
        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 RequestError, got {status}: {body}"
        );
        assert!(
            is_error(&body, "RequestError"),
            "case {case:?}: expected RequestError, got {body}"
        );
    }

    // A `Reward` outside `CurrencyAmount`'s `^[0-9]+(\.)?[0-9]{0,2}$` is refused at the
    // boundary, before this handler runs.
    for reward in ["", "free", "0.005", "-1", "1,00"] {
        let mut request = simple_hit("Bad reward");
        request["Reward"] = json!(reward);
        let (status, body) = call(&router, "CreateHIT", &request).await;
        assert_eq!(
            status, 400,
            "case Reward={reward:?}: outside the CurrencyAmount pattern, got {status}: {body}"
        );
    }

    // Nothing was stored for any of them.
    let (status, body) = call(&router, "ListHITs", &json!({})).await;
    assert_eq!(status, 200, "ListHITs answered {status}: {body}");
    assert_eq!(
        body.get("HITs"),
        Some(&json!([])),
        "a refused CreateHIT must store nothing: {body}"
    );
}

/// RULE: `GetAccountBalance` reports the account held in this service's store, so it
/// changes with the store and is identical across calls that do not change it.
#[tokio::test]
async fn get_account_balance_reports_the_persisted_account() {
    let (router, engine) = router_with_engine();

    let (status, first) = call(&router, "GetAccountBalance", &json!({})).await;
    assert_eq!(status, 200, "GetAccountBalance answered {status}: {first}");
    let seeded_available = text(&first, "AvailableBalance");
    let seeded_on_hold = text(&first, "OnHoldBalance");
    for (member, value) in [
        ("AvailableBalance", &seeded_available),
        ("OnHoldBalance", &seeded_on_hold),
    ] {
        assert!(
            is_currency_amount(value),
            "{member} must match CurrencyAmount's ^[0-9]+(\\.)?[0-9]{{0,2}}$, got {value:?}"
        );
    }

    // Reading the balance is a read: it neither credits nor debits the account.
    let (status, again) = call(&router, "GetAccountBalance", &json!({})).await;
    assert_eq!(status, 200, "GetAccountBalance answered {status}: {again}");
    assert_eq!(
        again, first,
        "reporting the balance must not change it: {first} then {again}"
    );

    // And it is READ from the store, not computed in the handler: change the stored
    // account and the next call reports the change.
    let state = engine.state::<ComAmazonawsMturkMturkrequesterservicev20170117State>();
    for (available, on_hold) in [("0", "0.00"), ("12.34", "5.6"), ("999999.99", "100")] {
        {
            let mut store = state.lock();
            store.account.available_balance = available.to_string();
            store.account.on_hold_balance = on_hold.to_string();
        }
        let (status, body) = call(&router, "GetAccountBalance", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {available:?}/{on_hold:?}: GetAccountBalance answered {status}: {body}"
        );
        assert_eq!(
            body,
            json!({ "AvailableBalance": available, "OnHoldBalance": on_hold }),
            "case {available:?}/{on_hold:?}: the balance must come from the store"
        );
    }
}

/// Whether `value` matches `CurrencyAmount`'s `^[0-9]+(\.)?[0-9]{0,2}$`.
fn is_currency_amount(value: &str) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    !whole.is_empty()
        && whole.chars().all(|c| c.is_ascii_digit())
        && fraction.len() <= 2
        && fraction.chars().all(|c| c.is_ascii_digit())
}

/// RULE: disposing of a HIT releases everything only that HIT held — its interned HIT
/// type (unless another HIT is still of that type) and its idempotency record — so a
/// create/dispose loop cannot grow the store.
#[tokio::test]
async fn disposing_a_hit_releases_what_only_it_held() {
    let (router, engine) = router_with_engine();
    let state = engine.state::<ComAmazonawsMturkMturkrequesterservicev20170117State>();

    // Two HITs described identically: one interned type, two idempotency records.
    let mut first = simple_hit("Shared type");
    first["UniqueRequestToken"] = json!("token-a");
    let mut second = simple_hit("Shared type");
    second["UniqueRequestToken"] = json!("token-b");

    let mut ids = Vec::new();
    for (case, request) in [("first", &first), ("second", &second)] {
        let (status, body) = call(&router, "CreateHIT", request).await;
        assert_eq!(status, 200, "case {case:?}: CreateHIT answered {status}: {body}");
        ids.push(text(hit(&body), "HITId"));
    }
    assert_eq!(
        state.lock().hit_types.len(),
        1,
        "two identically described HITs share one interned HIT type"
    );

    // Dispose of the first. Its type is still in use by the second, so it stays.
    expire_and_delete(&router, &ids[0]).await;
    assert_eq!(
        state.lock().hit_types.len(),
        1,
        "case \"one HIT left of the type\": the type is still referenced, so it stays"
    );
    // Its idempotency record is gone: the token can no longer replay a disposed HIT, so
    // it creates a new one.
    let (status, body) = call(&router, "CreateHIT", &first).await;
    assert_eq!(
        status, 200,
        "case \"token of a disposed HIT\": the record is released, got {status}: {body}"
    );
    let replacement = text(hit(&body), "HITId");
    assert_ne!(
        replacement, ids[0],
        "case \"token of a disposed HIT\": a fresh HIT, not the disposed one"
    );

    // Dispose of every HIT of that type and the type goes with the last one.
    for id in [ids[1].clone(), replacement] {
        expire_and_delete(&router, &id).await;
    }
    assert_eq!(
        state.lock().hit_types.len(),
        0,
        "case \"no HIT left of the type\": the interned type is released with the last HIT"
    );
    assert_eq!(
        state.lock().hits.len(),
        0,
        "every HIT was disposed of"
    );
}

/// Expire `hit_id` and dispose of it, asserting both calls succeed.
async fn expire_and_delete(router: &MultiServiceRouter, hit_id: &str) {
    let (status, body) = call(
        router,
        "UpdateExpirationForHIT",
        &json!({ "HITId": hit_id, "ExpireAt": now_secs() - 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {hit_id:?}: UpdateExpirationForHIT answered {status}: {body}"
    );
    let (status, body) = call(router, "DeleteHIT", &json!({ "HITId": hit_id })).await;
    assert_eq!(status, 200, "case {hit_id:?}: DeleteHIT answered {status}: {body}");
}

/// RULE: a `UniqueRequestToken` stops replaying once its documented window has passed
/// — "the unique token expires after 24 hours. Subsequent calls using the same
/// UniqueRequestToken made after the 24 hour limit could create duplicate HITs" — so
/// the records cannot accumulate forever.
#[tokio::test]
async fn a_unique_request_token_stops_replaying_after_its_documented_window() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = engine.state::<ComAmazonawsMturkMturkrequesterservicev20170117State>();
    let now = SystemTime::now();

    // Recorded at instants on either side of the window, read at `now`.
    let inside = UNIQUE_REQUEST_TOKEN_TTL / 2;
    let outside = UNIQUE_REQUEST_TOKEN_TTL + UNIQUE_REQUEST_TOKEN_TTL / 2;
    for (case, age, replays) in [
        ("just recorded", std::time::Duration::ZERO, true),
        ("half the window ago", inside, true),
        ("exactly the window ago", UNIQUE_REQUEST_TOKEN_TTL, false),
        ("past the window", outside, false),
    ] {
        let mut store = state.lock();
        let recorded_at = now
            .checked_sub(age)
            .unwrap_or_else(|| panic!("case {case:?}: {age:?} before now is representable"));
        store.remember_request_token(Some("replayed"), "0000000000000001", recorded_at);
        assert_eq!(
            store.replayed_hit_id(Some("replayed"), now).is_some(),
            replays,
            "case {case:?}: a record {age:?} old must{} replay",
            if replays { "" } else { " not" }
        );
        store.forget_request_tokens_for("0000000000000001");
    }
}
