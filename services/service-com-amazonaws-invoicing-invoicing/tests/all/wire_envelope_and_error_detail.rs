// SPDX-License-Identifier: CC0-1.0

//! The wire envelope and the error detail this service answers with, driven end to end
//! over its wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and a JSON body.
//! Nothing reaches into the state struct, so what these tests see is exactly what an
//! SDK client sees — including the response headers, which is what three of these
//! rules are about.
//!
//! The rules pinned here, each over a table spanning the class rather than one
//! request:
//!
//! 1. The media type a response announces is a property of the PLANE that answers it:
//!    an invoice-unit response announces [`INVOICE_UNIT_MEDIA_TYPE`], a tag-plane
//!    response [`TAG_PLANE_MEDIA_TYPE`] — on a success and on a modelled failure
//!    alike. A refusal of the REQUEST is answered by the input validator in front of
//!    the service and keeps the media type the model's protocol trait declares.
//! 2. A request a modelled member constraint rejects is answered HTTP 400
//!    `ValidationException` carrying `reason` = `fieldValidationFailed` and one
//!    `fieldList` entry per rejected member, naming that member and the constraint it
//!    failed — not `message` alone.
//! 3. A request naming a resource that is not in state is answered HTTP 404
//!    `ResourceNotFoundException` whose `message` is the CONSTANT sentence the shape
//!    documents and whose ARN member carries the ARN that was asked for.
//! 4. Tags are state: what a create or a `TagResource` writes is what
//!    `ListTagsForResource` reports, and `UntagResource` removes exactly the keys it
//!    names.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The media type every recorded response of an INVOICE-UNIT operation announces.
const INVOICE_UNIT_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type every recorded response of a TAG operation announces — the one the
/// model's `awsJson1_0` protocol trait declares.
const TAG_PLANE_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The `message` of a `ResourceNotFoundException`: the sentence the shape's own
/// documentation gives (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), whatever was asked for.
const NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// The `ValidationExceptionReason` a rejected member value is stated with.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `MaxResults` range `ListInvoiceUnits` declares
/// (`smithy_query(query="operation", operation="ListInvoiceUnits")`: range 1..500).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 500);

/// How many tags ONE RESOURCE holds — the per-resource quota the service enforces,
/// which is strictly below `ResourceTagList`'s per-CALL maximum (modelled `length
/// 0..200`, `smithy_query(query="operation", operation="TagResource")`) so that a
/// caller tagging in batches can reach it.
const MAX_RESOURCE_TAGS: usize = 50;

/// The per-CALL maximum of `ResourceTagList`, which is NOT the per-resource quota.
const MAX_TAGS_PER_CALL: usize = 200;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// What one call answered: the status, the media type it announced, and its body.
#[derive(Debug)]
struct Answer {
    status: u16,
    media_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type` on the awsJson protocols), shape name only.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the body, as a string.
    fn member(&self, name: &str) -> Option<&str> {
        self.body.get(name).and_then(Value::as_str)
    }
}

/// One request, returning everything a client can see of the response.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        media_type,
        body: parsed,
    }
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {operation} {body}: expected HTTP 200, got {answer:?}"
    );
    answer
}

/// A `CreateInvoiceUnit` request body: the members every row needs, plus `tags`.
fn create_request(
    name: &str,
    receiver: &str,
    client_token: &str,
    tags: &[(&str, &str)],
) -> Value {
    let mut request = json!({
        "Name": name,
        "InvoiceReceiver": receiver,
        "TaxInheritanceDisabled": false,
        "Rule": {"LinkedAccounts": ["210987654321"]},
        "ClientToken": client_token,
    });
    if !tags.is_empty() {
        request["ResourceTags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    request
}

/// Create one invoice unit and answer with its ARN.
async fn create_unit(router: &MultiServiceRouter, name: &str, tags: &[(&str, &str)]) -> String {
    let created = ok(
        router,
        "CreateInvoiceUnit",
        &create_request(name, "123456789012", &format!("token-{name}"), tags),
    )
    .await;
    created
        .member("InvoiceUnitArn")
        .unwrap_or_else(|| panic!("case create {name}: expected an InvoiceUnitArn, got {created:?}"))
        .to_string()
}

/// The `ResourceTags` of a `ListTagsForResource` answer, as sorted pairs.
fn listed_tags(answer: &Answer) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = answer.body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResourceTags list, got {answer:?}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    tags
}

/// RULE 1, successes: every operation's 200 announces the media type of the plane that
/// answers it — the invoice-unit operations one, the tag operations the other.
#[tokio::test]
async fn a_success_announces_the_media_type_of_its_plane() {
    let router = emulator();
    let arn = create_unit(&router, "MediaTypeUnit", &[("env", "test")]).await;

    // Every implemented operation, with the plane it belongs to. The create above is a
    // row too: it is the same question asked of the operation that mints the resource.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "CreateInvoiceUnit",
            create_request("SecondUnit", "210987654321", "token-second", &[]),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        (
            "GetInvoiceUnit",
            json!({"InvoiceUnitArn": arn}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        ("ListInvoiceUnits", json!({}), INVOICE_UNIT_MEDIA_TYPE),
        (
            "UpdateInvoiceUnit",
            json!({"InvoiceUnitArn": arn, "Description": "updated"}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": arn}),
            TAG_PLANE_MEDIA_TYPE,
        ),
        (
            "TagResource",
            json!({"ResourceArn": arn, "ResourceTags": [{"Key": "owner", "Value": "finance"}]}),
            TAG_PLANE_MEDIA_TYPE,
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn, "ResourceTagKeys": ["owner"]}),
            TAG_PLANE_MEDIA_TYPE,
        ),
        // Last: it removes the resource the rows above read.
        (
            "DeleteInvoiceUnit",
            json!({"InvoiceUnitArn": arn}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
    ];

    for (operation, request, expected) in cases {
        let answer = ok(&router, operation, &request).await;
        assert_eq!(
            answer.media_type.as_deref(),
            Some(expected),
            "case {operation} success: expected media type {expected:?}, got {answer:?}"
        );
    }
}

/// RULE 1, modelled failures: a failure a handler raises announces the same media type
/// its plane's successes do.
#[tokio::test]
async fn a_modelled_failure_announces_the_media_type_of_its_plane() {
    let router = emulator();
    let absent = "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff";

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "GetInvoiceUnit",
            json!({"InvoiceUnitArn": absent}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        (
            "UpdateInvoiceUnit",
            json!({"InvoiceUnitArn": absent, "Description": "nope"}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        (
            "DeleteInvoiceUnit",
            json!({"InvoiceUnitArn": absent}),
            INVOICE_UNIT_MEDIA_TYPE,
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": absent}),
            TAG_PLANE_MEDIA_TYPE,
        ),
        (
            "TagResource",
            json!({"ResourceArn": absent, "ResourceTags": [{"Key": "k", "Value": "v"}]}),
            TAG_PLANE_MEDIA_TYPE,
        ),
        (
            "UntagResource",
            json!({"ResourceArn": absent, "ResourceTagKeys": ["k"]}),
            TAG_PLANE_MEDIA_TYPE,
        ),
    ];

    for (operation, request, expected) in cases {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 404,
            "case {operation} on an absent resource: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.media_type.as_deref(),
            Some(expected),
            "case {operation} modelled failure: expected media type {expected:?}, got {answer:?}"
        );
    }
}

/// RULE 3: a resource that is not in state is reported with the CONSTANT documented
/// message and the ARN that was asked for in its own member — never spliced into the
/// prose, and never under the name the published model spells.
#[tokio::test]
async fn an_absent_resource_reports_the_constant_message_and_the_arn_in_its_own_member() {
    let router = emulator();
    // One unit exists, so these rows are reported absent on their own account rather
    // than because the store is empty.
    create_unit(&router, "PresentUnit", &[]).await;

    // ARNs differing along what the rule generalises over: the id's characters and
    // length, and (for the invoice-unit operations, whose `InvoiceUnitArnString` admits
    // a region) whether the ARN carries one. The tag operations' `TagrisArn` admits
    // only the region-less form, so they are driven with those rows alone.
    let regionless: &[&str] = &[
        "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff",
        "arn:aws:invoicing::000000000000:invoice-unit/does-not-exist-000",
        "arn:aws:invoicing::210987654321:invoice-unit/A_b-9/nested:id",
    ];
    let with_region: &[&str] = &["arn:aws:invoicing:us-east-1:000000000000:invoice-unit/absent"];

    let mut cases: Vec<(&str, &str, Value)> = Vec::new();
    for arn in regionless.iter().chain(with_region) {
        cases.push(("GetInvoiceUnit", arn, json!({"InvoiceUnitArn": arn})));
        cases.push((
            "UpdateInvoiceUnit",
            arn,
            json!({"InvoiceUnitArn": arn, "Description": "nope"}),
        ));
        cases.push(("DeleteInvoiceUnit", arn, json!({"InvoiceUnitArn": arn})));
    }
    for arn in regionless {
        cases.push(("ListTagsForResource", arn, json!({"ResourceArn": arn})));
        cases.push((
            "TagResource",
            arn,
            json!({"ResourceArn": arn, "ResourceTags": [{"Key": "k", "Value": "v"}]}),
        ));
        cases.push((
            "UntagResource",
            arn,
            json!({"ResourceArn": arn, "ResourceTagKeys": ["k"]}),
        ));
    }

    for (operation, arn, request) in cases {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 404,
            "case {operation} on {arn}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {operation} on {arn}: expected a ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.member("message"),
            Some(NOT_FOUND_MESSAGE),
            "case {operation} on {arn}: the message is a constant, got {answer:?}"
        );
        assert_eq!(
            answer.member("resourceId"),
            Some(arn),
            "case {operation} on {arn}: the ARN travels in its own member, got {answer:?}"
        );
    }
}

/// One request a modelled member constraint rejects, and the member path the refusal
/// must name.
#[derive(Debug)]
struct RefusedCase {
    operation: &'static str,
    /// The dotted member path the refusal names: a member in lowerCamel.
    member: &'static str,
    request: Value,
}

/// RULE 2: a rejected member value is stated in the modelled `ValidationException` —
/// `reason` plus a `fieldList` entry naming the member and the constraint it failed.
///
/// The rows differ along the CONSTRAINT class the model declares (a pattern, a length
/// bound, a numeric range, a required member absent), the member's type (string,
/// integer, list) and the operation refusing it, so the rule is asserted over the class
/// rather than over the one recorded request.
#[tokio::test]
async fn a_rejected_member_is_stated_as_a_field_validation_failure() {
    let router = emulator();

    let cases = vec![
        // `AccountIdString` carries `@pattern ^\d{12}$`.
        RefusedCase {
            operation: "CreateInvoiceUnit",
            member: "invoiceReceiver",
            request: create_request("PatternUnit", "not-an-account-id", "tok-a", &[]),
        },
        // `InvoiceUnitName` carries `@length(1..50)`.
        RefusedCase {
            operation: "CreateInvoiceUnit",
            member: "name",
            request: create_request(&"n".repeat(51), "123456789012", "tok-b", &[]),
        },
        // `Name` is `@required`, and absence is a refusal of the same class.
        RefusedCase {
            operation: "CreateInvoiceUnit",
            member: "name",
            request: json!({
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
        },
        // `InvoiceUnitArnString` carries the ARN `@pattern`.
        RefusedCase {
            operation: "GetInvoiceUnit",
            member: "invoiceUnitArn",
            request: json!({"InvoiceUnitArn": "not-an-arn"}),
        },
        // `DescriptionString` carries `@length(0..500)`.
        RefusedCase {
            operation: "UpdateInvoiceUnit",
            member: "description",
            request: json!({
                "InvoiceUnitArn": "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff",
                "Description": "d".repeat(501),
            }),
        },
        // `MaxResultsInteger` carries `@range(1..500)`: one row below the minimum …
        RefusedCase {
            operation: "ListInvoiceUnits",
            member: "maxResults",
            request: json!({"MaxResults": MAX_RESULTS_RANGE.0 - 1}),
        },
        // … and one above the maximum.
        RefusedCase {
            operation: "ListInvoiceUnits",
            member: "maxResults",
            request: json!({"MaxResults": MAX_RESULTS_RANGE.1 + 1}),
        },
        // `TagrisArn` carries `@length(20..2048)` and its own pattern.
        RefusedCase {
            operation: "ListTagsForResource",
            member: "resourceArn",
            request: json!({"ResourceArn": "arn:aws:s3:::b"}),
        },
        // `ResourceTags` is `@required` on `TagResource` …
        RefusedCase {
            operation: "TagResource",
            member: "resourceTags",
            request: json!({"ResourceArn": "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff"}),
        },
        // … and `ResourceTagKeys` on `UntagResource`.
        RefusedCase {
            operation: "UntagResource",
            member: "resourceTagKeys",
            request: json!({"ResourceArn": "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff"}),
        },
    ];

    for case in &cases {
        let answer = call(&router, case.operation, &case.request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.member("reason"),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {answer:?}"
        );
        let field_list = answer.body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {answer:?}"));
        let entry = field_list
            .iter()
            .find(|entry| entry["name"].as_str() == Some(case.member))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a fieldList entry naming the member, got {answer:?}")
            });
        let message = entry["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("Member must"),
            "case {case:?}: the entry must name the constraint the member failed, got {answer:?}"
        );
        assert!(
            answer
                .member("message")
                .is_some_and(|text| text.contains("validation error") && text.contains(case.member)),
            "case {case:?}: the envelope must name the refused member, got {answer:?}"
        );
        // A refusal of the REQUEST is answered in front of the service, in the media
        // type the model's protocol trait declares — on either plane.
        assert_eq!(
            answer.media_type.as_deref(),
            Some(TAG_PLANE_MEDIA_TYPE),
            "case {case:?}: a refusal keeps the model-declared media type, got {answer:?}"
        );
    }

    // The published bounds themselves are accepted: the refusal is of what falls
    // OUTSIDE the range, not of the range's own ends.
    for max_results in [MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1] {
        let answer = ok(&router, "ListInvoiceUnits", &json!({"MaxResults": max_results})).await;
        assert!(
            answer.body["InvoiceUnits"].is_array(),
            "case MaxResults {max_results}: a bound of the range is accepted, got {answer:?}"
        );
    }
}

/// One step of the tag-state table: what it writes, and what the resource holds after.
#[derive(Debug)]
struct TagStep {
    operation: &'static str,
    /// `(key, value)` pairs for a `TagResource`, keys for an `UntagResource`.
    tags: &'static [(&'static str, &'static str)],
    keys: &'static [&'static str],
    /// The whole tag set `ListTagsForResource` must report after this step.
    expected: &'static [(&'static str, &'static str)],
}

/// RULE 4: tags are state. What a create or a `TagResource` writes is what
/// `ListTagsForResource` reports, and `UntagResource` removes exactly the keys it names.
///
/// The rows differ along what the rule generalises over: how many tags one write
/// carries, whether a write adds a key or replaces one already set, removing a key that
/// is set and one that is not, and emptying the set completely.
#[tokio::test]
async fn tag_state_is_what_list_tags_reports() {
    let router = emulator();
    // The create carries tags, so the first row's "after" state is already a read-back
    // of what the create persisted.
    let arn = create_unit(&router, "TaggedUnit", &[("Environment", "Test")]).await;

    let created = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        listed_tags(&created),
        vec![("Environment".to_string(), "Test".to_string())],
        "case tags of a created unit: the create's ResourceTags must be readable back, \
         got {created:?}"
    );

    let steps: &[TagStep] = &[
        // One key added beside the create's.
        TagStep {
            operation: "TagResource",
            tags: &[("owner", "finance")],
            keys: &[],
            expected: &[("Environment", "Test"), ("owner", "finance")],
        },
        // Several keys at once, one of them REPLACING a value already set.
        TagStep {
            operation: "TagResource",
            tags: &[("owner", "treasury"), ("tier", "gold"), ("empty", "")],
            keys: &[],
            expected: &[
                ("Environment", "Test"),
                ("empty", ""),
                ("owner", "treasury"),
                ("tier", "gold"),
            ],
        },
        // A removal takes only the keys it names …
        TagStep {
            operation: "UntagResource",
            tags: &[],
            keys: &["owner", "tier"],
            expected: &[("Environment", "Test"), ("empty", "")],
        },
        // … and a key the resource does not carry is not an error, and removes nothing.
        TagStep {
            operation: "UntagResource",
            tags: &[],
            keys: &["owner", "never-set"],
            expected: &[("Environment", "Test"), ("empty", "")],
        },
        // The set can be emptied completely, which is an empty list and not a 404.
        TagStep {
            operation: "UntagResource",
            tags: &[],
            keys: &["Environment", "empty"],
            expected: &[],
        },
        // And a write after that starts the set again.
        TagStep {
            operation: "TagResource",
            tags: &[("again", "yes")],
            keys: &[],
            expected: &[("again", "yes")],
        },
    ];

    for step in steps {
        let request = if step.operation == "TagResource" {
            json!({
                "ResourceArn": arn,
                "ResourceTags": step
                    .tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect::<Vec<Value>>(),
            })
        } else {
            json!({"ResourceArn": arn, "ResourceTagKeys": step.keys})
        };
        ok(&router, step.operation, &request).await;

        let listed = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
        let want: Vec<(String, String)> = step
            .expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&listed),
            want,
            "case {step:?}: expected exactly {want:?}, got {listed:?}"
        );
    }

    // Tags belong to the resource they were written against: a second unit is untouched
    // by every write above.
    let other = create_unit(&router, "UntaggedUnit", &[]).await;
    let other_tags = ok(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": other}),
    )
    .await;
    assert_eq!(
        listed_tags(&other_tags),
        Vec::<(String, String)>::new(),
        "case tags of another unit: writes against one resource must not reach another, \
         got {other_tags:?}"
    );
}

/// RULE 4, at its bound: a resource holds at most the per-resource tag quota, and the
/// write that would pass it is refused with the modelled quota error rather than
/// applied in part — including a single call that stays within the per-call array
/// maximum but asks for more tags than the resource may hold.
#[tokio::test]
async fn a_tag_write_past_the_modelled_maximum_is_refused() {
    let router = emulator();
    let arn = create_unit(&router, "QuotaUnit", &[]).await;

    let full: Vec<Value> = (0..MAX_RESOURCE_TAGS)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let filled = ok(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "ResourceTags": full}),
    )
    .await;
    assert_eq!(
        filled.status, 200,
        "case {MAX_RESOURCE_TAGS} tags: the modelled maximum is accepted, got {filled:?}"
    );

    // Re-writing a key already set is a replacement, not a step past the maximum.
    let replaced = ok(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "ResourceTags": [{"Key": "key-0", "Value": "other"}]}),
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "case replacing a set key at the maximum: expected HTTP 200, got {replaced:?}"
    );

    // One more DISTINCT key would pass the maximum.
    let refused = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "ResourceTags": [{"Key": "one-too-many", "Value": "v"}]}),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case one key past {MAX_RESOURCE_TAGS}: expected HTTP 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "case one key past {MAX_RESOURCE_TAGS}: expected the modelled quota error, got {refused:?}"
    );

    // The refusal applied nothing: the resource still holds exactly what it held.
    let listed = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    let tags = listed_tags(&listed);
    assert_eq!(
        tags.len(),
        MAX_RESOURCE_TAGS,
        "case after a refused write: the tag set is unchanged, got {} entries",
        tags.len()
    );
    assert!(
        !tags.iter().any(|(key, _)| key == "one-too-many"),
        "case after a refused write: the refused key must not be stored, got {tags:?}"
    );

    // The per-CALL array maximum is not the quota: a single legal call carrying that
    // many tags is still refused, because the RESOURCE may not hold them.
    let fresh = create_unit(&router, "QuotaUnitPerCall", &[]).await;
    let per_call_max: Vec<Value> = (0..MAX_TAGS_PER_CALL)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let refused_batch = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": fresh, "ResourceTags": per_call_max}),
    )
    .await;
    assert_eq!(
        refused_batch.status, 402,
        "case {MAX_TAGS_PER_CALL} tags in one call: expected HTTP 402, got {refused_batch:?}"
    );
}
