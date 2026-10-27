// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteMicrovmImage` answers once the image it addresses is
//! REALLY gone, driven over the same router the emulator boots.
//!
//! RULE: a call addressed at a resource identifier that resolves to no record is refused
//! with the modelled not-found (HTTP 404 `ResourceNotFoundException`) whatever the verb,
//! so the documented idempotence of the delete covers only the window in which the record
//! still exists with a deletion in flight — which is what lets a client tell "the delete I
//! issued is in flight" from "there is nothing here", and keeps a delete of an identifier
//! nothing was ever created under from being reported as a deletion.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    delete_image_not_found_message, invalid_arn_format_message,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    call, emulator, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN, REGION,
};

/// `POST /2025-09-09/microvm-images`.
async fn create_image(router: &MultiServiceRouter, name: &str, extra: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await
}

/// RULE: once the clock has made the deletion due the record is gone, so the SECOND
/// delete of it is refused with HTTP 404 `ResourceNotFoundException` carrying the delete
/// path's own message built from the identifier the request's URI label carried — while a
/// delete repeated WHILE the record still exists is the documented idempotent success.
#[tokio::test]
async fn a_delete_of_a_record_that_is_gone_is_refused() {
    let router = emulator();

    // Rows differ in what the image holds and in how long the caller waits before the
    // repeat, because neither changes the verdict: only whether the record is still there
    // does.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("required members only", "plain", json!({})),
        (
            "tagged and fully specified",
            "fully-specified",
            json!({
                "description": "deletable",
                "egressNetworkConnectors": ["connector-egress"],
                "environmentVariables": {"STAGE": "prod"},
                "tags": {"team": "platform"},
            }),
        ),
        (
            "logging explicitly disabled",
            "logging-off",
            json!({"logging": {"disabled": {}}}),
        ),
    ];

    for (case, name, extra) in rows {
        let (status, created) = create_image(&router, name, extra).await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {created}");
        let arn = expected_image_arn(name);

        // While the deletion is in flight the record is still there, so the repeat is the
        // documented idempotent success reporting the state the record carries.
        let (status, first) = delete_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {first}");
        assert_eq!(
            first["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the deletion is asynchronous, got {:?}",
            first["state"]
        );
        let (status, again) = delete_image(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeat while the deletion is in flight succeeds, got {status} \
             {again}"
        );
        assert_eq!(
            again["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the repeat reports the same state, got {:?}",
            again["state"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;

        // The record is gone: the identifier resolves to nothing and the delete is
        // refused, with the SAME verdict the read path answers for it.
        let (status, gone) = delete_image(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting a record that is gone is refused, got {status} {gone}"
        );
        assert_eq!(
            gone["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {gone}"
        );
        assert_eq!(
            gone["message"].as_str(),
            Some(delete_image_not_found_message(&arn).as_str()),
            "case {case:?}: expected the delete path's own message for {arn}, got {:?}",
            gone["message"]
        );
        let (status, read) = get_image(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the read misses on the same identifier, got {status} {read}"
        );
    }
}

/// RULE: the verdict comes from the identifier's RESOLUTION, not from anything the caller
/// did before: an identifier this account never held is refused exactly as one whose
/// record the clock removed — a well-formed ARN naming no record is the modelled 404, and
/// a value that is neither an id this account holds nor a well-formed ARN addresses
/// nothing this API can name and is the modelled 400 (the separation
/// `image_request_validation` owns for the read path).
#[tokio::test]
async fn an_identifier_that_resolves_to_no_record_is_refused_however_it_fails() {
    let router = emulator();

    // An image this account DOES hold, so the refused rows are refused for the identifier
    // they name rather than for an empty store.
    let (status, created) = create_image(&router, "held", json!({})).await;
    assert_eq!(status, 201, "create the held image: got {created}");

    // Rows differ in HOW the identifier fails to name a record: a never-minted ARN of
    // this account's namespace, another account's, another region's, another partition's,
    // another resource type's, and values that are no identifier of this API at all.
    let rows: Vec<(&str, String, u16)> = vec![
        ("an ARN nobody minted", expected_image_arn("never-minted"), 404),
        (
            "another account's ARN",
            format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:theirs"),
            404,
        ),
        (
            "another region's ARN for a held name",
            format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:held"),
            404,
        ),
        (
            "another partition's ARN for a held name",
            format!("arn:aws-us-gov:lambda:{REGION}:{ACCOUNT}:microvm-image:held"),
            404,
        ),
        (
            "another resource type",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:held"),
            404,
        ),
        ("a name nothing was created under", "never-created".to_string(), 400),
        ("nothing ARN-shaped at all", "not.an.arn".to_string(), 400),
    ];

    for (case, identifier, expected_status) in rows {
        let (status, body) = delete_image(&router, &identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {identifier:?} expected {expected_status}, got {status} {body}"
        );
        let (expected_type, expected_message) = match expected_status {
            404 => (
                "ResourceNotFoundException",
                delete_image_not_found_message(&identifier),
            ),
            _ => (
                "ValidationException",
                invalid_arn_format_message(&identifier),
            ),
        };
        assert_eq!(
            body["__type"].as_str(),
            Some(expected_type),
            "case {case:?}: expected {expected_type}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the refusal built from the identifier the request \
             carried, got {:?}",
            body["message"]
        );
    }

    // The positive control: the image this account holds still deletes through the same
    // path, so the rows above are refused by the resolution and not by a dead path.
    let (status, body) = delete_image(&router, "held").await;
    assert_eq!(
        status, 200,
        "a held image still deletes, got {status} {body}"
    );
}

/// RULE: the delete releases the name for good — a create reusing it is a NEW resource
/// minted under the same (name-derived) ARN, in the initial state, created at an instant
/// strictly after the deleted one's, and an independent read reports exactly that.
#[tokio::test]
async fn a_deleted_images_name_is_released_for_reuse() {
    let router = emulator();

    // Rows differ in what the replacement asks for, which cannot change the identity it
    // is minted under: the ARN is derived from the name.
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        ("the same request", "reused-plain", json!({}), json!({})),
        (
            "a differently configured replacement",
            "reused-reconfigured",
            json!({"tags": {"team": "platform"}}),
            json!({"description": "the replacement", "resources": [{"minimumMemoryInMiB": 4096}]}),
        ),
    ];

    for (case, name, first_extra, second_extra) in rows {
        let (status, first) = create_image(&router, name, first_extra).await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {first}");
        let arn = expected_image_arn(name);
        let first_created_at = first["createdAt"].as_f64().unwrap_or_default();

        let (status, deleted) = delete_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {deleted}");
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, gone) = delete_image(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the record must be gone before the name is reused, got {status} \
             {gone}"
        );

        let (status, replacement) = create_image(&router, name, second_extra).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the name must be reusable, got {status} {replacement}"
        );
        assert_eq!(
            replacement["imageArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the ARN is derived from the name, so the replacement carries the \
             same one, got {:?}",
            replacement["imageArn"]
        );
        assert_eq!(
            replacement["state"].as_str(),
            Some("CREATING"),
            "case {case:?}: the replacement's build is asynchronous, so it starts CREATING, \
             got {:?}",
            replacement["state"]
        );
        let replacement_created_at = replacement["createdAt"].as_f64().unwrap_or_default();
        assert!(
            replacement_created_at > first_created_at,
            "case {case:?}: the replacement is a new resource, so its createdAt {replacement_created_at} \
             is after the deleted one's {first_created_at}"
        );

        // An independent read confirms both: the response was the stored record.
        let (status, read) = get_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: read got {status} {read}");
        assert_eq!(
            read["createdAt"].as_f64(),
            Some(replacement_created_at),
            "case {case:?}: the read reports the replacement's own createdAt, got {:?}",
            read["createdAt"]
        );
        assert_eq!(
            read["state"].as_str(),
            Some("CREATING"),
            "case {case:?}: the read reports the replacement's state, got {:?}",
            read["state"]
        );
    }
}
