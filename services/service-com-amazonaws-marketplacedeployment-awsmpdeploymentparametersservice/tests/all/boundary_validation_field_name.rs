// SPDX-License-Identifier: CC0-1.0

//! The BODY this service answers a refused request with: every member the modelled error
//! declares.
//!
//! The rule under test: a failure answered as a shape this model declares carries every
//! member that shape marks `@required`. The client-input error here is
//! `ValidationException`, whose `message` AND `fieldName` are both `@required`
//! (`smithy_query(query="shape",shape="ValidationException")`), so each refusal stating
//! that shape names the member it is about beside the sentence — whichever LAYER of the
//! service refused the request, so one logical failure has one body:
//!
//! * the modelled constraint walk at the boundary (a `@pattern`, a `@length`, a
//!   `@required` member absent), which answers before any handler runs;
//! * a handler's own refusal of a rule the model states only in prose (a `catalog` naming
//!   no catalog, a tag key in the namespace AWS reserves).
//!
//! `fieldName` is the member path the sentence's first clause states, so the two halves of
//! one body name the same position: a member of the input by its own lowerCamel name, a
//! member of a nested shape by its trail from the input, and a value inside a collection
//! at the position the report gives it.
//!
//! What is NOT this shape, and is asserted to carry no `fieldName`: the tagging
//! front-end's resource-ARN parse, which answers `BadRequestException` — a shape the model
//! does not declare (the `resource_arn_validation` suite owns that rule) — and the front
//! door's 403 `ForbiddenException`.
//!
//! The rows span what the rule generalises over: which constraint class was broken, which
//! layer refused, whether the member travels in the URI, the body or the query string,
//! whether it is a member of the input or of a nested shape, whether it is `@sensitive`
//! (whose value is still not echoed), how many constraints one request broke, and which
//! operation answered.
//!
//! Every case drives the real wire path through the harness the `deployment_parameters`
//! suite owns, so what is asserted is what a signed SDK request observes.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::RESERVED_TAG_KEY_PREFIX;

use crate::deployment_parameters::{
    call, emulator, error_code, field_name, message, pct, provision, put, tag, untag, CATALOG,
    SECRET_MAX_LEN,
};

/// The status the modelled shape declares (`@httpError 400`).
const EXPECTED_STATUS: u16 = 400;
/// The wire error code it carries, written out rather than read from the crate so the
/// assertion is not the implementation compared with itself.
const EXPECTED_CODE: &str = "ValidationException";
/// What `support::field_name` reports for a body carrying no `fieldName` at all.
const ABSENT: &str = "<no fieldName>";

/// The agreement and product every write below targets.
const AGREEMENT: &str = "agmt-field-name";
const PRODUCT: &str = "prod-field-name";

/// The member path the first clause of `reported` states — `… at '<path>' failed to
/// satisfy constraint: …` — or `None` for a sentence that states no clause.
///
/// Read out of the message so a row asserts the two halves of one body against each
/// other and not just against a literal.
fn clause_member(reported: &str) -> Option<String> {
    let after = reported.split_once(" at '")?.1;
    let (path, rest) = after.split_once('\'')?;
    rest.starts_with(" failed to satisfy constraint: ")
        .then(|| path.to_string())
}

/// A `PutDeploymentParameter` body naming `agreement_id`, with the deployment parameter
/// `member` carries verbatim.
fn body(agreement_id: &str, member: Value) -> Value {
    json!({ "agreementId": agreement_id, "deploymentParameter": member })
}

/// One row: the call to make, and the member its refusal must name.
struct Row {
    case: &'static str,
    catalog: String,
    product_id: String,
    body: Value,
    field: String,
    /// How many clauses the sentence must state, which is how many constraints the
    /// request broke.
    clauses: usize,
}

impl Row {
    /// One row whose refusal states a single clause.
    fn new(case: &'static str, catalog: &str, product_id: &str, body: Value, field: &str) -> Self {
        Self {
            case,
            catalog: catalog.to_string(),
            product_id: product_id.to_string(),
            body,
            field: field.to_string(),
            clauses: 1,
        }
    }

    /// The same row, stating `clauses` clauses.
    fn stating(mut self, clauses: usize) -> Self {
        self.clauses = clauses;
        self
    }
}

#[tokio::test]
async fn every_boundary_refusal_names_the_member_it_is_about() {
    let (router, engine) = emulator();
    // A target the account holds, so no row below is refused for naming no product or
    // agreement instead of for the constraint it breaks.
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);

    let over_long = "c".repeat(65);
    let over_long_id = "p".repeat(65);
    let secret = "s".to_string();

    let rows: Vec<Row> = vec![
        // A `@pattern` on a member bound to the URI.
        Row::new(
            "a catalog carrying a character its pattern excludes",
            "AWSMarketplace1",
            PRODUCT,
            body(AGREEMENT, json!({ "name": "N", "secretString": secret })),
            "catalog",
        ),
        // A `@length` maximum on the same member: the class differs, the rule does not.
        Row::new(
            "a catalog past its modelled maximum",
            &over_long,
            PRODUCT,
            body(AGREEMENT, json!({ "name": "N", "secretString": secret })),
            "catalog",
        ),
        Row::new(
            "a product id carrying a bang",
            CATALOG,
            "prod!held",
            body(AGREEMENT, json!({ "name": "N", "secretString": secret })),
            "productId",
        ),
        Row::new(
            "a product id past its modelled maximum",
            CATALOG,
            &over_long_id,
            body(AGREEMENT, json!({ "name": "N", "secretString": secret })),
            "productId",
        ),
        // A member that travels in the BODY rather than in the URI.
        Row::new(
            "an agreement id carrying a space",
            CATALOG,
            PRODUCT,
            body("agmt held", json!({ "name": "N", "secretString": secret })),
            "agreementId",
        ),
        // A member of a NESTED shape, named by its trail from the input.
        Row::new(
            "a parameter name carrying a colon",
            CATALOG,
            PRODUCT,
            body(AGREEMENT, json!({ "name": "Db:Password", "secretString": secret })),
            "deploymentParameter.name",
        ),
        // The one `@sensitive` member: its value is not echoed, and the member is still
        // named.
        Row::new(
            "a secret shorter than its modelled minimum",
            CATALOG,
            PRODUCT,
            body(AGREEMENT, json!({ "name": "N", "secretString": "" })),
            "deploymentParameter.secretString",
        ),
        Row::new(
            "an idempotency token shorter than its modelled minimum",
            CATALOG,
            PRODUCT,
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": secret },
                "clientToken": "too-short",
            }),
            "clientToken",
        ),
        Row::new(
            "an idempotency token of the right length carrying a bang",
            CATALOG,
            PRODUCT,
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": secret },
                "clientToken": "0123456789abcdef0123456789abcde!",
            }),
            "clientToken",
        ),
        // A value inside a COLLECTION, named at the position the report gives it.
        Row::new(
            "a tag value outside the character class the constrained map states",
            CATALOG,
            PRODUCT,
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": secret },
                "tags": { "owner": "bad value" },
            }),
            "tags.owner",
        ),
        // A `tags` map past its modelled entry COUNT is deliberately absent: that
        // limit is the tagging front end's, answered with its own unmodelled identity
        // and no member at all (`tag_count_front_end` owns that rule), so it is not a
        // refusal this body states.
        // A `@required` member absent, which is found while READING the request.
        Row::new(
            "a required member of the input absent",
            CATALOG,
            PRODUCT,
            json!({ "deploymentParameter": { "name": "N", "secretString": secret } }),
            "agreementId",
        ),
        Row::new(
            "a required structure member absent",
            CATALOG,
            PRODUCT,
            json!({ "agreementId": AGREEMENT }),
            "deploymentParameter",
        ),
        Row::new(
            "a required member of a nested shape absent",
            CATALOG,
            PRODUCT,
            body(AGREEMENT, json!({ "name": "N" })),
            "deploymentParameter.secretString",
        ),
        // Several constraints broken at once: the body names the member the FIRST clause
        // is about, and the sentence counts them all.
        Row::new(
            "two members outside their constraints at once",
            "AWSMarketplace1",
            PRODUCT,
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": secret },
                "clientToken": "too-short",
            }),
            "catalog",
        )
        .stating(2),
    ];

    for Row {
        case,
        catalog,
        product_id,
        body,
        field,
        clauses,
    } in rows
    {
        let (status, answer) = put(&router, &catalog, &product_id, &body).await;
        assert_eq!(
            status, EXPECTED_STATUS,
            "case {case:?}: expected HTTP {EXPECTED_STATUS}, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            EXPECTED_CODE,
            "case {case:?}: expected {EXPECTED_CODE}, got {answer:?}"
        );
        let reported = message(&answer);
        let expected_envelope = match clauses {
            1 => "1 validation error detected: ".to_string(),
            n => format!("{n} validation errors detected: "),
        };
        assert!(
            reported.starts_with(&expected_envelope),
            "case {case:?}: expected {expected_envelope:?}, got {reported:?}"
        );
        assert_eq!(
            field_name(&answer),
            field,
            "case {case:?}: expected the refusal to name {field:?}, got {answer:?}"
        );
        // The two halves of one body name the same position.
        assert_eq!(
            clause_member(&reported).as_deref(),
            Some(field.as_str()),
            "case {case:?}: expected the first clause to be about {field:?}, got {reported:?}"
        );
    }

    // A `@sensitive` member is named without its value: the member's name is in the body,
    // the secret is not. An EMPTY secret is what breaks `SecretString`'s modelled minimum
    // (`length 1..15000`, `smithy_query(query="shape",shape="SecretString")`); a value
    // past its maximum is the front door's 403 instead.
    let (status, answer) = put(
        &router,
        CATALOG,
        PRODUCT,
        &body(AGREEMENT, json!({ "name": "N", "secretString": "" })),
    )
    .await;
    assert_eq!(
        status, EXPECTED_STATUS,
        "a secret short of its minimum must be refused, got {status} {answer:?}"
    );
    assert!(
        !message(&answer).contains("Value ''"),
        "a @sensitive member's value is not echoed, got {answer:?}"
    );

    // Nothing above was written: every refusal precedes the write.
    let (status, answer) = put(
        &router,
        CATALOG,
        PRODUCT,
        &body(AGREEMENT, json!({ "name": "Accepted", "secretString": "s3cr3t" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a request inside every constraint must still be written, got {status} {answer:?}"
    );
}

#[tokio::test]
async fn a_boundary_refusal_and_a_handler_refusal_answer_the_same_body() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, created) = put(
        &router,
        CATALOG,
        PRODUCT,
        &body(AGREEMENT, json!({ "name": "Present", "secretString": "s3cr3t" })),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}created-by");

    // The BOUNDARY refusal reached through an operation OTHER than the one the table
    // above drives is not reachable at all on this model: `UntagResource$tagKeys` is the
    // only `@required` member that travels outside the URI path, and the framework's own
    // route lookup rejects a route whose required query binding the request does not carry
    // — so a `DELETE /tags/{resourceArn}` with no `tagKeys` names no operation and is the
    // router's routing failure rather than this shape. `TagResource` and
    // `ListTagsForResource` bind no member the model constrains at all
    // (`smithy_query(query="operation",operation="ListTagsForResource,TagResource")`:
    // `resourceArn` and `tags` target unconstrained shapes). So the refusals those three
    // operations CAN answer with this shape are the ones their handlers own, below — and
    // they carry the same two members the boundary's do.

    // The handler-side refusals of the same shape, one per operation that owns one.
    let (status, answer) = put(
        &router,
        "NotTheRealMarketplace",
        PRODUCT,
        &body(AGREEMENT, json!({ "name": "N", "secretString": "s" })),
    )
    .await;
    assert_refusal(
        "PutDeploymentParameter with a catalog that names no catalog",
        status,
        &answer,
        "catalog",
    );

    let (status, answer) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "Reserved", "secretString": "s" },
            "tags": { reserved.clone(): "x" },
        }),
    )
    .await;
    assert_refusal(
        "PutDeploymentParameter carrying a reserved tag key",
        status,
        &answer,
        "tags",
    );

    let (status, answer) = tag(
        &router,
        &arn,
        &std::collections::BTreeMap::from([(reserved.clone(), "x".to_string())]),
    )
    .await;
    assert_refusal(
        "TagResource carrying a reserved tag key",
        status,
        &answer,
        "tags",
    );

    let (status, answer) = untag(&router, &arn, std::slice::from_ref(&reserved)).await;
    assert_refusal(
        "UntagResource naming a reserved tag key",
        status,
        &answer,
        "tagKeys",
    );

    // The refusals that are NOT this shape carry no `fieldName`: a client branches on the
    // code, and these two are not the modelled error at all.
    let (status, answer) = call(&router, "GET", &format!("/tags/{}", pct("not-an-arn")), None).await;
    assert_eq!(
        status, EXPECTED_STATUS,
        "a value this API cannot address must be refused, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "BadRequestException",
        "the ARN parse is the front-end's own refusal, got {answer:?}"
    );
    assert_eq!(
        field_name(&answer),
        ABSENT,
        "a refusal that is not the modelled shape carries no fieldName, got {answer:?}"
    );

    let (status, answer) = put(
        &router,
        CATALOG,
        PRODUCT,
        &body(
            AGREEMENT,
            json!({ "name": "TooBig", "secretString": "s".repeat(SECRET_MAX_LEN + 1) }),
        ),
    )
    .await;
    assert_eq!(
        status, 403,
        "the front door must refuse an oversized sensitive payload, got {status} {answer:?}"
    );
    assert_eq!(
        field_name(&answer),
        ABSENT,
        "the front door's refusal is not the modelled shape, got {answer:?}"
    );
}

/// One refusal of the modelled shape: HTTP 400, the shape's own code, a sentence, and the
/// member `field`.
fn assert_refusal(case: &str, status: u16, answer: &Value, field: &str) {
    assert_eq!(
        status, EXPECTED_STATUS,
        "case {case:?}: expected HTTP {EXPECTED_STATUS}, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        EXPECTED_CODE,
        "case {case:?}: expected {EXPECTED_CODE}, got {answer:?}"
    );
    assert!(
        !message(answer).is_empty() && message(answer) != "<no message>",
        "case {case:?}: the modelled shape requires a message, got {answer:?}"
    );
    assert_eq!(
        field_name(answer),
        field,
        "case {case:?}: expected the refusal to name {field:?}, got {answer:?}"
    );
}
