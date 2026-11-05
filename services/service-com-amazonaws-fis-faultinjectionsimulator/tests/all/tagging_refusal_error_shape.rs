// SPDX-License-Identifier: CC0-1.0

//! WHICH error shape a refusal of FIS names on the wire, and which one it may not.
//!
//! RULE: `restJson1` resolves an error shape from the `x-amzn-errortype` header, so the
//! shape a refusal names there (and in the body's `__type` beside it) is the shape the
//! OPERATION answers that refusal with. FIS's three ARN-addressed tag operations declare
//! NO error at all — `smithy_query(query="operation",
//! operation="TagResource,UntagResource,ListTagsForResource",
//! model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`
//! reports `errors: []` for each of the three — so no modelled error shape of this service
//! is theirs to answer, and each refusal that surface answers carries the shape the tagging
//! front door states for it: an ARN this service could not have ISSUED is a bad request
//! (`BadRequestException`, HTTP 400, "Invalid input resource arn") and an ARN it CAN address
//! that names no stored record is a generic miss (`NotFoundException`, HTTP 404, "Resource
//! was not found"). An operation whose model DOES declare a not-found error keeps that
//! modelled shape: `GetExperimentTemplate` and `GetExperiment` bind
//! `ResourceNotFoundException` at `@httpError 404`, and they are asked for a resource of
//! one named kind rather than for whatever an ARN addresses.
//!
//! The rows differ along everything the rule generalises over:
//!
//! * WHICH operation of the tagging surface serves the call — the write, the removal and
//!   the read, so a client cannot see the three disagree about one ARN;
//! * which refusal CLASS the ARN earns, and WHICH FIELD of the ARN puts it in that class
//!   (its syntax, its vendor, its resource type, its id's prefix, width, alphabet or
//!   emptiness — the whole `unaddressable_arns` table) or which kind of record it names and
//!   no longer holds (`unminted_arns`, plus a template this suite creates and deletes,
//!   which is the state a tag call meets after a delete);
//! * whether the shape is read from the HEADER or from the BODY, which must agree;
//! * and, in the contrast table, whether the operation's own model declares a not-found
//!   error, which is what decides between the modelled shape and the surface's own.
//!
//! The ARN tables, the resource helpers and the one-call harness come from the tag suites
//! (`resource_tagging`, `tagging_response_envelope`), so what is asserted here is the
//! envelope of the very same calls whose persisted effect those suites assert.

use serde_json::json;

use crate::experiment_lifecycle::{emulator, send, FALLBACK_REGION};
use crate::resource_tagging::{
    new_template, template_id_shaped, unaddressable_arns, unminted_arns, ID_SUFFIX_LEN,
};
use crate::tagging_response_envelope::{
    call, operations, Envelope, INVALID_RESOURCE_ARN, NOT_FOUND,
};

/// The shape the tagging front door answers for a `resourceArn` this service could not have
/// issued.
const BAD_REQUEST: &str = "BadRequestException";

/// The shape the tagging front door answers for an ARN of this service naming no record.
const NOT_FOUND_SHAPE: &str = "NotFoundException";

/// The shape an operation whose model DECLARES a not-found error answers.
const MODELLED_NOT_FOUND: &str = "ResourceNotFoundException";

/// One refusal class of the tagging surface: the status it travels with, the shape it names
/// and the sentence it carries.
struct Class {
    status: u16,
    shape: &'static str,
    message: &'static str,
}

/// The bad-request class: the ARN layer refuses the value before any lookup.
const UNADDRESSABLE: Class = Class {
    status: 400,
    shape: BAD_REQUEST,
    message: INVALID_RESOURCE_ARN,
};

/// The miss class: the ARN is one of this service's and names no stored record.
const NO_RECORD: Class = Class {
    status: 404,
    shape: NOT_FOUND_SHAPE,
    message: NOT_FOUND,
};

/// Assert one answer against the class its row belongs to: the status, the shape named in
/// the header a `restJson1` client resolves it from, the shape named in the body beside it,
/// and the sentence.
fn assert_class(answer: &Envelope, class: &Class, case: &str) {
    assert_eq!(
        answer.status, class.status,
        "case {case:?}: expected HTTP {}, got {} with {:?}",
        class.status, answer.status, answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(class.shape),
        "case {case:?}: expected the error-type header to name {:?}, got {:?}",
        class.shape,
        answer.error_type
    );
    assert_eq!(
        answer.code(),
        class.shape,
        "case {case:?}: expected the body to name {:?}, got {:?}",
        class.shape,
        answer.body
    );
    assert_eq!(
        answer.message(),
        class.message,
        "case {case:?}: expected the sentence {:?}, got {:?}",
        class.message,
        answer.message()
    );
}

/// RULE: every operation of the tagging surface names the tagging front door's own shape
/// for the class the ARN falls in — never a shape its own model does not declare for it.
#[tokio::test]
async fn the_tagging_surface_names_its_own_error_shape_for_every_refusal_class() {
    for operation in operations() {
        let router = emulator();

        // A template this case creates and then deletes: the ARN stays one this service
        // issued and the record behind it is gone, which is the state a tag call meets
        // after a delete.
        let (deleted_id, deleted_arn) = new_template(&router, operation.as_str(), &json!({})).await;
        let deleted = send(
            &router,
            "DELETE",
            &format!("/experimentTemplates/{deleted_id}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
            deleted.status, deleted.body
        );

        let mut rows: Vec<(&Class, String, String)> = Vec::new();
        for (reason, arn) in unaddressable_arns() {
            rows.push((&UNADDRESSABLE, reason.to_string(), arn));
        }
        for (reason, arn) in unminted_arns() {
            rows.push((&NO_RECORD, reason.to_string(), arn));
        }
        rows.push((
            &NO_RECORD,
            "a template that has since been deleted".to_string(),
            deleted_arn,
        ));

        for (class, reason, arn) in rows {
            let case = format!("{} / {reason} / {arn}", operation.as_str());
            let answer = operation.call_on(&router, &arn).await;
            assert_class(&answer, class, &case);
        }
    }
}

/// RULE (the other side of it): an operation whose model DECLARES a not-found error keeps
/// answering that modelled shape, so what the tagging surface answers is decided by its own
/// empty error list and not by a service-wide rename of the miss.
#[tokio::test]
async fn an_operation_that_models_a_not_found_error_keeps_the_modelled_shape() {
    let router = emulator();

    // Ids of the form this service mints that no call ever minted — the same records the
    // tagging rows above address by ARN, reached here by the typed id each reader takes.
    let template_id = template_id_shaped(&"Z".repeat(ID_SUFFIX_LEN));
    let experiment_id = format!("EXP{}", "Z".repeat(ID_SUFFIX_LEN));

    // The readers that bind `ResourceNotFoundException` at `@httpError 404`
    // (`smithy_query(query="operation", operation="GetExperimentTemplate,GetExperiment")`),
    // reached with an id naming no record of their own kind.
    let rows = [
        (
            "GetExperimentTemplate",
            format!("/experimentTemplates/{template_id}"),
        ),
        ("GetExperiment", format!("/experiments/{experiment_id}")),
    ];

    for (operation, uri) in rows {
        let answer = call(&router, "GET", &uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(MODELLED_NOT_FOUND),
            "case {operation:?}: expected the error-type header to name {MODELLED_NOT_FOUND}, \
             got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.code(),
            MODELLED_NOT_FOUND,
            "case {operation:?}: expected the body to name {MODELLED_NOT_FOUND}, got {:?}",
            answer.body
        );
        // The modelled refusal was asked for a resource of ONE named kind, so it words the
        // miss after that kind rather than with the tagging surface's generic sentence.
        assert_ne!(
            answer.message(),
            NOT_FOUND,
            "case {operation:?}: expected a kind-specific sentence, got the tagging \
             surface's generic one"
        );
    }
}

/// Both refusal classes of the tagging surface answer DIFFERENT codes, so a client that
/// branches on the header tells "this ARN is not one of ours" from "that resource is not
/// there" — asserted through every operation of the surface, on one ARN of each class, and
/// beside a call on a LIVE resource, which names no shape at all because the header belongs
/// to a failure alone.
#[tokio::test]
async fn the_two_refusal_classes_answer_different_codes() {
    let router = emulator();
    let (_, live_arn) = new_template(&router, "two refusal classes", &json!({})).await;
    let (unaddressable_reason, unaddressable_arn) = unaddressable_arns()
        .into_iter()
        .next()
        .expect("the bad-request table carries rows");
    let (missing_reason, missing_arn) = unminted_arns()
        .into_iter()
        .next()
        .expect("the miss table carries rows");

    for operation in operations() {
        let refused = operation.call_on(&router, &unaddressable_arn).await;
        let missed = operation.call_on(&router, &missing_arn).await;
        let case = format!(
            "{} / {unaddressable_reason} vs {missing_reason}",
            operation.as_str()
        );
        assert_class(&refused, &UNADDRESSABLE, &case);
        assert_class(&missed, &NO_RECORD, &case);
        assert_ne!(
            refused.error_type, missed.error_type,
            "case {case:?}: the two classes must not share one code, both answered {:?}",
            refused.error_type
        );

        let served = operation.call_on(&router, &live_arn).await;
        assert!(
            served.status < 400,
            "case {case:?}: expected the live resource to be served, got {} with {:?}",
            served.status,
            served.body
        );
        assert_eq!(
            served.error_type, None,
            "case {case:?}: a served call names no error shape, got {:?}",
            served.error_type
        );
    }
}
