// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the order in which `GetMicrovmImage` decides what to answer,
//! driven over the same router the emulator boots.
//!
//! RULE: the read decides ACCESS BEFORE EXISTENCE, and the two questions are asked at
//! different scopes. ACCESS is the ACCOUNT's: an `imageIdentifier` outside this account's
//! own image namespace is refused with the modelled authorization error (HTTP 403
//! `AccessDeniedException`) whether or not it names a resource, so the read never reports
//! absence for an identifier the caller may not read and cannot be used to probe AWS's
//! own catalogue or another account's images. EXISTENCE is the REGION's: an identifier of
//! this account's own namespace gets an answer about the record — the record while this
//! region holds it, and the modelled not-found (HTTP 404 `ResourceNotFoundException`)
//! when it does not, which is the one answer a name nothing was created under, a record
//! the account deleted and an image minted through another region's endpoint all get.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, and the
//! AWS-owned namespace's own rows are `managed_namespace_authorization`'s.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, managed_base_image_arn, MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN, REGION,
};

/// `MicrovmImageIdentifier` is `@length(1..256)`
/// (`smithy_query(query="operation",operation="GetMicrovmImage")`), so an identifier of
/// exactly 256 characters is still one the service reads rather than one it rejects on
/// its own bound.
const MAX_IDENTIFIER_LENGTH: usize = 256;

/// `GET /2025-09-09/microvm-images/{imageIdentifier}`, for an identifier every character
/// of which is a legal path character.
async fn get_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

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

/// `DELETE /2025-09-09/microvm-images/{imageIdentifier}`.
async fn delete_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// The sentence AWS words this refusal with, spelled out here rather than read from the
/// crate so a change to the wording fails this suite: fixed text `User: ` and ` is not
/// authorized to perform this operation`, with the one variable part the identity that
/// signed the request — the account's own IAM user, which `sts:GetCallerIdentity`
/// reports as `arn:aws:iam::<account>:user/emulator`.
fn expected_denial_sentence() -> String {
    format!("User: arn:aws:iam::{ACCOUNT}:user/emulator is not authorized to perform this operation")
}

/// The refusal every denied row must carry: the modelled status, code and sentence.
fn assert_denied(status: u16, body: &Value, case: &str) {
    assert_eq!(status, 403, "case {case}: expected 403, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_denial_sentence().as_str()),
        "case {case}: expected the denial sentence naming the caller, got {:?}",
        body["message"]
    );
}

/// The absence every own-account row that names no record of this region must carry.
fn assert_absent(status: u16, body: &Value, identifier: &str, case: &str) {
    assert_eq!(status, 404, "case {case}: expected 404, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(image_not_found_message(identifier).as_str()),
        "case {case}: expected the read path's own message for {identifier}, got {:?}",
        body["message"]
    );
}

/// RULE: every well-formed `imageIdentifier` OUTSIDE this account's own image namespace
/// is denied 403 `AccessDeniedException`, whichever other namespace it names and whether
/// or not it names a resource that exists — while an identifier of this account's own
/// namespace that this region holds no record of is reported absent (404), the answer
/// that says nothing about any other account's resources.
#[tokio::test]
async fn a_read_of_an_identifier_this_account_was_never_granted_is_denied() {
    let router = emulator();

    // An image this account DOES hold, so the denied rows are denied for the identifier
    // they name rather than for an empty store.
    let held = "held-image";
    let (status, created) = create_image(&router, held, json!({})).await;
    assert_eq!(status, 201, "create the held image: got {status} {created}");
    let held_arn = expected_image_arn(held);

    // An identifier at the published maximum length: the ARN form padded out to the
    // bound `MicrovmImageIdentifier` declares.
    let prefix = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm-image:");
    let longest = format!(
        "{prefix}{}",
        "x".repeat(MAX_IDENTIFIER_LENGTH - prefix.len())
    );
    assert_eq!(
        longest.len(),
        MAX_IDENTIFIER_LENGTH,
        "the longest row must sit exactly on the published bound, got {}",
        longest.len()
    );

    // Denied rows differ in WHICH namespace outside this account's own the identifier
    // names: AWS's own catalogue (an entry that exists, and one that does not), another
    // account's, another partition's, another vendor's, another resource type's, and this
    // account's own resource type spelled in a form `MicrovmImageArn`'s pattern does not
    // admit (`microvm-image/<name>` rather than `microvm-image:<name>`).
    let mut denied: Vec<(String, String)> = MANAGED_BASE_IMAGE_IDS
        .iter()
        .map(|image_id| {
            (
                format!("an AWS-owned image the catalogue publishes {image_id:?}"),
                managed_base_image_arn(REGION, image_id),
            )
        })
        .collect();
    denied.extend([
        (
            "an AWS-owned ARN the catalogue does not publish".to_string(),
            managed_base_image_arn(REGION, "no-such-base-image"),
        ),
        (
            "another account's ARN for a name this one holds".to_string(),
            format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:{held}"),
        ),
        (
            "another partition's ARN for a name this account holds".to_string(),
            format!("arn:aws-us-gov:lambda:{REGION}:{ACCOUNT}:microvm-image:{held}"),
        ),
        (
            "another vendor's ARN".to_string(),
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:microvm-image:{held}"),
        ),
        (
            "another resource type of this account".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{held}"),
        ),
    ]);

    let mut refusals: Vec<(String, Value)> = Vec::new();
    for (case, identifier) in &denied {
        let (status, body) = get_image(&router, identifier).await;
        assert_denied(status, &body, &format!("{case} ({identifier})"));
        refusals.push((case.clone(), body));
    }

    // Absent rows are this account's OWN namespace with no record of them in this
    // region: a name nothing was created under, one at the identifier length bound, and
    // a name this account holds in ANOTHER region.
    let absent: Vec<(String, String)> = vec![
        (
            "this account's namespace, a name nothing was created under".to_string(),
            expected_image_arn("never-created"),
        ),
        (
            "this account's namespace, at the identifier length bound".to_string(),
            longest,
        ),
        (
            "this account's namespace in another region, a name it holds".to_string(),
            format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:{held}"),
        ),
    ];
    for (case, identifier) in &absent {
        let (status, body) = get_image(&router, identifier).await;
        assert_absent(status, &body, identifier, &format!("{case} ({identifier})"));
    }

    // The refusal says NOTHING about existence: the answer for an AWS-owned image the
    // catalogue publishes — a resource that unambiguously exists — is byte for byte the
    // answer for an identifier of another account's namespace that names nothing.
    let (first_case, first) = refusals
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the rows are not empty"));
    for (case, body) in &refusals {
        assert_eq!(
            body, &first,
            "case {case:?}: the denial must be indistinguishable from {first_case:?}'s, got \
             {body}"
        );
    }

    // The positive control: this account's own image is still served through the same
    // path, so the rows above are refused by the verdict and not by a dead path — and the
    // refusals changed nothing a later caller sees.
    let (status, image) = get_image(&router, &held_arn).await;
    assert_eq!(
        status, 200,
        "this account's own image is served, got {status} {image}"
    );
    assert_eq!(
        image["imageArn"].as_str(),
        Some(held_arn.as_str()),
        "the read must report the stored record, got {image}"
    );
}

/// RULE: an identifier naming a record this account created is one it is granted, so the
/// read answers about EXISTENCE for it: the stored record while it is there, and 404
/// `ResourceNotFoundException` built from the identifier the request carried once the
/// deletion this account asked for has removed it — the signal a cleanup poll waits for,
/// and it stays that answer however often it is asked.
#[tokio::test]
async fn a_read_of_this_accounts_own_record_reports_it_gone_once_the_delete_is_due() {
    let router = emulator();

    // Rows differ in what the image holds and in which form the delete addressed it,
    // because neither changes what the read answers for the account's own identifier.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        ("required members only", "plain", json!({}), true),
        (
            "tagged and fully specified, deleted by name",
            "fully-specified",
            json!({
                "description": "deletable",
                "environmentVariables": {"STAGE": "prod"},
                "tags": {"team": "platform"},
            }),
            false,
        ),
        (
            "logging explicitly disabled",
            "logging-off",
            json!({"logging": {"disabled": {}}}),
            true,
        ),
    ];

    for (case, name, extra, delete_by_arn) in rows {
        let (status, created) = create_image(&router, name, extra).await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {created}");
        let arn = expected_image_arn(name);

        // While the record is there the read serves it — including the transient state
        // the delete puts it in, which is what tells a poll the delete is in flight.
        let (status, before) = get_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: read got {status} {before}");

        let identifier = match delete_by_arn {
            true => arn.as_str(),
            false => name,
        };
        let (status, deleted) = delete_image(&router, identifier).await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {deleted}");
        let (status, during) = get_image(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the record is still there while the delete is in flight, got \
             {status} {during}"
        );
        assert_eq!(
            during["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the read reports the stored state, got {:?}",
            during["state"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;

        // The record is gone, and it was this account's: the read reports the absence,
        // with the read path's own message built from the identifier the request
        // carried — and a second poll gets exactly the same answer.
        for poll in 1..=2 {
            let (status, gone) = get_image(&router, &arn).await;
            assert_eq!(
                status, 404,
                "case {case:?} poll {poll}: expected 404, got {status} {gone}"
            );
            assert_eq!(
                gone["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {case:?} poll {poll}: expected ResourceNotFoundException, got {gone}"
            );
            assert_eq!(
                gone["message"].as_str(),
                Some(image_not_found_message(&arn).as_str()),
                "case {case:?} poll {poll}: expected the read path's own message for {arn}, got \
                 {:?}",
                gone["message"]
            );
        }

        // The name is released: a replacement created under it is served again through
        // the very identifier that was answering the not-found.
        let (status, replacement) = create_image(&router, name, json!({})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the name must be reusable, got {status} {replacement}"
        );
        let (status, read) = get_image(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacement reads back through the same identifier, got {status} \
             {read}"
        );
        assert_eq!(
            read["createdAt"].as_f64(),
            replacement["createdAt"].as_f64(),
            "case {case:?}: the read reports the replacement's own record, got {read}"
        );
    }
}

/// RULE: the verdict is on the identifier, not on its spelling — the resource field of an
/// ARN carries its type and id separated by `:` or `/`, and `MicrovmImageArn`'s own
/// pattern (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`) admits only
/// the `:` form, so a value spelling this resource type with a `/` names no image of this
/// account and is denied whichever name follows it.
#[tokio::test]
async fn the_denial_does_not_depend_on_which_resource_form_the_arn_uses() {
    let router = emulator();

    let held = "held-image";
    let (status, created) = create_image(&router, held, json!({})).await;
    assert_eq!(status, 201, "create the held image: got {status} {created}");

    // Rows are the same account, region and resource type in both resource forms, for a
    // name nothing was created under and for one this account DOES hold: neither is a
    // record the account holds under that identifier, so both are denied. The `/` is
    // percent-encoded because it is not a legal character in a single path segment.
    let rows: Vec<(&str, String)> = vec![
        (
            "the `type/id` form, a name nothing was created under",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm-image%2Fnever-created"),
        ),
        (
            "the `type/id` form, a name this account holds",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm-image%2F{held}"),
        ),
    ];

    for (case, path_identifier) in rows {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{path_identifier}"),
            None,
        )
        .await;
        assert_denied(status, &body, &format!("{case} ({path_identifier})"));
    }
}
