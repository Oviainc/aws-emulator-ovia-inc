// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MicroVM image and instance paths this change fixes:
//! durable deletion, the routed `CreateMicrovmAuthToken`, the created-image
//! projection (its `major.minor` version and its `updatedAt`), and the one
//! `ResourceNotFoundException` envelope every lookup on these paths answers.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` already owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    next_image_version, FIRST_IMAGE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, built_image, call, emulator, expected_image_arn, image_body, run_microvm,
    PAST_THE_SPAN,
};

/// `TokenParts`'s value shape `AuthTokenValue` is `@length(max: 8000)`
/// (`smithy_query(query="shape",shape="TokenParts")`).
const MAX_AUTH_TOKEN_VALUE_LENGTH: usize = 8000;

/// The `authToken` key `CreateMicrovmAuthTokenResponse$authToken` documents: "Use the
/// value at key \"X-aws-proxy-auth\" as the header value when connecting to the
/// MicroVM endpoint."
const AUTH_TOKEN_KEY: &str = "X-aws-proxy-auth";

/// `DELETE /2025-09-09/microvm-images/{imageIdentifier}`.
pub(crate) async fn delete_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// `GET /2025-09-09/microvm-images/{imageIdentifier}`.
pub(crate) async fn get_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// `POST /2025-09-09/microvms/{microvmIdentifier}/auth-token`.
async fn auth_token(
    router: &MultiServiceRouter,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/2025-09-09/microvms/{}/auth-token",
            as_uri_value(identifier)
        ),
        Some(body),
    )
    .await
}

/// Run a MicroVM from a built image and return its id once it is RUNNING.
async fn running_microvm(router: &MultiServiceRouter, image: &str) -> String {
    built_image(router, image).await;
    let (status, run) = run_microvm(router, json!({"imageIdentifier": image})).await;
    assert_eq!(status, 200, "run a MicroVM from {image}: got {run}");
    tokio::time::sleep(PAST_THE_SPAN).await;
    run["microvmId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: `DeleteMicrovmImage` deletes the image for good — the call records it
/// `DELETING` and the removal the clock then owes takes the record and every version
/// it holds, so every later lookup of it misses and its name is free again — and,
/// being documented idempotent, a repeat WHILE the record still exists with that
/// deletion in flight still succeeds and reports the same state. (What a delete
/// addressed at a record that is already GONE answers is the refusal
/// `gone_image_delete_refusal` owns.)
#[tokio::test]
async fn deleting_an_image_is_durable_and_idempotent() {
    let router = emulator();

    // Rows differ in which form the delete addresses the image by and in what the
    // image holds, because neither changes what a delete leaves behind.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        ("by name, required members only", "plain", json!({}), false),
        (
            "by ARN, required members only",
            "plain-by-arn",
            json!({}),
            true,
        ),
        (
            "by name, tagged and fully specified",
            "fully-specified",
            json!({
                "baseImageVersion": "2.3",
                "description": "deletable",
                "egressNetworkConnectors": ["connector-egress"],
                "environmentVariables": {"STAGE": "prod"},
                "tags": {"team": "platform"},
            }),
            false,
        ),
    ];

    for (case, name, extra, by_arn) in rows {
        let (status, created) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(name, extra.clone())),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {created}");

        let arn = expected_image_arn(name);
        let identifier = if by_arn { arn.as_str() } else { name };

        let (status, deleted) = delete_image(&router, identifier).await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {deleted}");
        assert_eq!(
            deleted["imageIdentifier"].as_str(),
            Some(identifier),
            "case {case:?}: the response names the deleted image, got {:?}",
            deleted["imageIdentifier"]
        );
        assert_eq!(
            deleted["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the deletion is asynchronous, so the image is DELETING, got {:?}",
            deleted["state"]
        );

        // The transient state is observable: a caller acting straight after the
        // delete still sees the image, in the state the delete put it in.
        let (status, during) = get_image(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the image reads back while DELETING, got {status} {during}"
        );
        assert_eq!(
            during["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the read reports the stored state, got {:?}",
            during["state"]
        );

        // A repeat while the deletion is in flight succeeds and changes nothing.
        let (status, again) = delete_image(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat delete must succeed, got {status} {again}"
        );
        assert_eq!(
            again["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the repeat reports the same state, got {:?}",
            again["state"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;

        // The delete is DURABLE: neither form resolves the image any more. The forms
        // differ in HOW they stop resolving: the minted ARN stays a well-formed
        // identifier that now names nothing (404), while the name is no longer an id
        // this account holds and is no ARN either, so it addresses nothing this API can
        // name (400) — the separation `image_request_validation` owns.
        for (form, lookup, expected_status, expected_type) in [
            ("name", name, 400, "ValidationException"),
            ("ARN", arn.as_str(), 404, "ResourceNotFoundException"),
        ] {
            let (status, body) = get_image(&router, lookup).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: a deleted image must not read back by {form}, got {status} {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some(expected_type),
                "case {case:?}: expected {expected_type} by {form}, got {body}"
            );
        }
        // A filter is not a lookup: the listing keeps answering, now selecting nothing.
        // (`ListMicrovms` is `@paginated` with `items` `@required`, so a filter value
        // that selects no MicroVM is a terminal empty page, not a refusal.)
        let (status, listed) =
            call(&router, "GET", &format!("/2025-09-09/microvms?imageIdentifier={name}"), None)
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a deleted image filters nothing, got {status} {listed}"
        );
        assert_eq!(
            listed["items"].as_array().cloned(),
            Some(Vec::new()),
            "case {case:?}: expected an empty page, got {listed}"
        );

        // The name is free again, which is what "a subsequent create reusing the name"
        // needs: nothing of the deleted image is left to collide with.
        let (status, recreated) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(name, extra)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the name must be reusable after the delete, got {status} {recreated}"
        );
    }

}

/// RULE: a lookup on these paths that resolves nothing answers 404
/// `ResourceNotFoundException` whose body is a message built from the identifier the
/// request carried — `message` is the shape's only required member, and the optional
/// `resourceType` and `resourceId` are left absent.
#[tokio::test]
async fn every_not_found_carries_only_the_request_derived_message() {
    let router = emulator();
    built_image(&router, "present").await;
    let absent_arn = expected_image_arn("absent");

    // An image this account created and then deleted. Its ARN stays an identifier the
    // account holds a record of, so the image lookup REPORTS THE ABSENCE of that record
    // rather than answering the authorization refusal an identifier of a namespace this
    // account was never granted gets (`managed_namespace_authorization` and
    // `image_read_authorization` own that verdict).
    let gone_arn = built_image(&router, "gone").await;
    let (status, deleted) = delete_image(&router, &gone_arn).await;
    assert_eq!(status, 200, "delete the gone image: got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    // Rows span every operation on these paths that resolves an identifier, both
    // identifier forms, and the two message templates: an image lookup, a MicroVM
    // lookup, and `RunMicrovm`'s active-version resolution. An IMAGE identifier that
    // is neither an id this account holds nor a well-formed ARN is refused instead
    // (400 `ValidationException`), which `image_request_validation` owns, so the image
    // lookup is addressed here by the ARN form.
    let token_body = json!({"expirationInMinutes": 15, "allowedPorts": [{"allPorts": {}}]});
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "get a record this account deleted, by ARN",
            "GET",
            format!("/2025-09-09/microvm-images/{}", as_uri_value(&gone_arn)),
            None,
            format!("MicroVMImage not found for MicroVMImageID: {gone_arn}"),
        ),
        (
            "get an absent MicroVM by id",
            "GET",
            "/2025-09-09/microvms/mvm-00000000deadbeef".to_string(),
            None,
            "MicroVM not found: mvm-00000000deadbeef".to_string(),
        ),
        (
            "terminate an absent MicroVM",
            "DELETE",
            "/2025-09-09/microvms/no-such-microvm".to_string(),
            None,
            "MicroVM not found: no-such-microvm".to_string(),
        ),
        (
            "auth token for an absent MicroVM",
            "POST",
            "/2025-09-09/microvms/no-such-microvm/auth-token".to_string(),
            Some(token_body.clone()),
            "MicroVM not found: no-such-microvm".to_string(),
        ),
        (
            "run from an image that does not exist",
            "POST",
            "/2025-09-09/microvms".to_string(),
            Some(json!({"imageIdentifier": absent_arn.clone()})),
            format!("No active version found for MicroVM image {absent_arn}"),
        ),
        (
            "run a version a present image does not carry",
            "POST",
            "/2025-09-09/microvms".to_string(),
            Some(json!({"imageIdentifier": "present", "imageVersion": "9.9"})),
            format!(
                "MicroVM image {} has no version 9.9",
                expected_image_arn("present")
            ),
        ),
    ];

    for (case, method, uri, body, expected_message) in rows {
        let (status, response) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the request-derived message, got {:?}",
            response["message"]
        );
        assert!(
            response.get("resourceType").is_none(),
            "case {case:?}: resourceType must be absent, got {:?}",
            response.get("resourceType")
        );
        assert!(
            response.get("resourceId").is_none(),
            "case {case:?}: resourceId must be absent, got {:?}",
            response.get("resourceId")
        );
    }
}

/// RULE: a MicroVM image version is named in this service's `major.minor` form — the
/// first build of an image is `1.0` and the next build of the same image takes the next
/// minor of the highest version already minted — and that string is what the create
/// reports and what a version-addressed call takes back.
#[tokio::test]
async fn image_versions_are_minted_in_major_minor_form() {
    // The minting rule over a set of version sets. Rows differ in how many versions
    // exist, in their ordering, and in their magnitude.
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("no version yet", vec![], "1.0"),
        ("the first build", vec!["1.0"], "1.1"),
        ("minors already past the first", vec!["1.0", "1.1"], "1.2"),
        (
            "the highest version is not the last listed",
            vec!["1.3", "1.0", "1.2"],
            "1.4",
        ),
        ("a minor past single digits", vec!["1.9", "1.10"], "1.11"),
        ("a later major", vec!["1.0", "2.4"], "2.5"),
        ("a version in no recognised form is ignored", vec!["latest"], "1.0"),
    ];
    for (case, existing, expected) in rows {
        let minted = next_image_version(existing.iter().copied());
        assert_eq!(
            minted, expected,
            "case {case:?}: from {existing:?} expected {expected}, got {minted}"
        );
    }

    // And over the wire: the create reports the first version in that form, and the
    // version it reports is the one a version-addressed call resolves.
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("versioned", json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create: got {status} {created}");
    assert_eq!(
        created["imageVersion"].as_str(),
        Some("1.0"),
        "the first version must be minted in major.minor form, got {:?}",
        created["imageVersion"]
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, image) = get_image(&router, "versioned").await;
    assert_eq!(status, 200, "get: got {status} {image}");
    assert_eq!(
        image["latestActiveImageVersion"].as_str(),
        created["imageVersion"].as_str(),
        "the built version is the one the create reported, got {:?}",
        image["latestActiveImageVersion"]
    );
    let (status, run) = run_microvm(
        &router,
        json!({"imageIdentifier": "versioned", "imageVersion": created["imageVersion"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the reported version must address the version it named, got {status} {run}"
    );
}

/// RULE: the create response is the stored image record projected in full — every
/// modelled member the request carried comes back, a member it omitted whose value the
/// service supplies comes back RESOLVED (the base image's version; the AWS-managed
/// egress connector), and `updatedAt` is stamped with the record's last change, so a
/// later read reports it and it moves on when the record changes.
#[tokio::test]
async fn the_created_image_projection_carries_every_modelled_member() {
    let router = emulator();

    // The default a request that names no `egressNetworkConnectors` gets, in the
    // region the request is signed for.
    let default_connector = format!(
        "arn:aws:lambda:{}:aws:network-connector:aws-network-connector:INTERNET_EGRESS",
        crate::microvm_lifecycle::REGION
    );

    // The version a request that pins no `baseImageVersion` gets: the `major.minor`
    // initial version (`state::FIRST_IMAGE_VERSION`), rather than a literal of this
    // suite.
    let resolved_base_version = json!(FIRST_IMAGE_VERSION);

    // Rows differ in which of the two resolved members the request carries, because
    // each one is resolved, stored and projected independently of the other.
    let rows: Vec<(&str, &str, Value, Value, Value)> = vec![
        (
            "neither member named",
            "bare",
            json!({}),
            resolved_base_version.clone(),
            json!([default_connector.clone()]),
        ),
        (
            "a base image version only",
            "based",
            json!({"baseImageVersion": "7.2"}),
            json!("7.2"),
            json!([default_connector.clone()]),
        ),
        (
            "egress connectors only",
            "connected",
            json!({"egressNetworkConnectors": [default_connector.clone()]}),
            resolved_base_version.clone(),
            json!([default_connector.clone()]),
        ),
        (
            "both, with several connectors",
            "based-and-connected",
            json!({
                "baseImageVersion": "12.34",
                "egressNetworkConnectors": ["connector-a", "connector-b"],
            }),
            json!("12.34"),
            json!(["connector-a", "connector-b"]),
        ),
    ];

    let mut created_rows: Vec<(&str, String, Value)> = Vec::new();
    for (case, name, extra, expected_base_version, expected_connectors) in rows {
        let request = image_body(name, extra);
        let (status, created) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(request.clone()),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: got {status} {created}");

        assert_eq!(
            created["baseImageVersion"],
            expected_base_version,
            "case {case:?}: expected the resolved base image version, got {:?}",
            created["baseImageVersion"]
        );
        assert_eq!(
            created["egressNetworkConnectors"],
            expected_connectors,
            "case {case:?}: expected the stored egress connectors, got {:?}",
            created["egressNetworkConnectors"]
        );
        assert_eq!(
            created["imageVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: expected the first version, got {:?}",
            created["imageVersion"]
        );
        let created_at = created["createdAt"].as_f64();
        let updated_at = created["updatedAt"].as_f64();
        assert!(
            created_at.is_some() && updated_at == created_at,
            "case {case:?}: the create is the record's first update, got createdAt {:?} updatedAt {:?}",
            created["createdAt"],
            created["updatedAt"]
        );
        created_rows.push((case, name.to_string(), created));
    }

    tokio::time::sleep(PAST_THE_SPAN).await;

    for (case, name, created) in &created_rows {
        let (status, image) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: get got {status} {image}");
        assert_eq!(
            image["createdAt"].as_f64(),
            created["createdAt"].as_f64(),
            "case {case:?}: createdAt never changes, got {:?}",
            image["createdAt"]
        );
        let updated_at = image["updatedAt"].as_f64();
        assert!(
            updated_at.is_some(),
            "case {case:?}: the read must report updatedAt, got {image}"
        );
        assert!(
            updated_at >= created["updatedAt"].as_f64(),
            "case {case:?}: the completed build moved updatedAt on, got {:?} after {:?}",
            image["updatedAt"],
            created["updatedAt"]
        );
        // A second read reports the same stamp: a read is not a change.
        let (status, again) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: second get got {status} {again}");
        assert_eq!(
            again["updatedAt"].as_f64(),
            updated_at,
            "case {case:?}: reading must not move updatedAt, got {:?}",
            again["updatedAt"]
        );
    }
}

/// RULE: `CreateMicrovmAuthToken` is a modelled operation on
/// `POST /2025-09-09/microvms/{microvmIdentifier}/auth-token`: for a MicroVM that can
/// still be connected to it answers 200 with a freshly minted token at the documented
/// `X-aws-proxy-auth` key, whatever form the requested ports take.
#[tokio::test]
async fn an_auth_token_is_minted_for_every_form_of_allowed_ports() {
    let router = emulator();
    let microvm_id = running_microvm(&router, "tokenable").await;
    let microvm_arn = format!(
        "arn:aws:lambda:{}:000000000000:microvm:{microvm_id}",
        crate::microvm_lifecycle::REGION
    );

    // Rows span each arm of the `PortSpecification` union (`port`, `range`,
    // `allPorts`), the published bounds of `PortNumber` (@range 1..65535), several
    // entries at once, both identifier forms, and the published minimum of
    // `expirationInMinutes` (@range min 1).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a single port at the published minimum",
            microvm_id.as_str(),
            json!({"expirationInMinutes": 1, "allowedPorts": [{"port": 1}]}),
        ),
        (
            "a single port at the published maximum",
            microvm_id.as_str(),
            json!({"expirationInMinutes": 60, "allowedPorts": [{"port": 65535}]}),
        ),
        (
            "a port range",
            microvm_id.as_str(),
            json!({
                "expirationInMinutes": 15,
                "allowedPorts": [{"range": {"startPort": 8080, "endPort": 8090}}],
            }),
        ),
        (
            "a range of one port",
            microvm_id.as_str(),
            json!({
                "expirationInMinutes": 15,
                "allowedPorts": [{"range": {"startPort": 443, "endPort": 443}}],
            }),
        ),
        (
            "every port",
            microvm_id.as_str(),
            json!({"expirationInMinutes": 15, "allowedPorts": [{"allPorts": {}}]}),
        ),
        (
            "several specifications at once, addressed by ARN",
            microvm_arn.as_str(),
            json!({
                "expirationInMinutes": 1_440,
                "allowedPorts": [
                    {"port": 80},
                    {"range": {"startPort": 3000, "endPort": 3010}},
                    {"allPorts": {}},
                ],
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, identifier, body) in rows {
        let (status, response) = auth_token(&router, identifier, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        let token = response["authToken"][AUTH_TOKEN_KEY].as_str();
        let Some(token) = token else {
            panic!("case {case:?}: expected a token at {AUTH_TOKEN_KEY}, got {response}");
        };
        assert!(
            !token.is_empty() && token.len() <= MAX_AUTH_TOKEN_VALUE_LENGTH,
            "case {case:?}: the token must be a non-empty AuthTokenValue, got {} characters",
            token.len()
        );
        assert!(
            !minted.iter().any(|earlier| earlier == token),
            "case {case:?}: every call mints a FRESH token, got one seen before"
        );
        minted.push(token.to_string());
    }
}

/// RULE: `CreateMicrovmAuthToken` refuses what it cannot grant — a request outside the
/// model's own bounds or whose range is not a range is 400 `ValidationException`, and a
/// MicroVM that accepts no further connection is the modelled 409 conflict.
#[tokio::test]
async fn an_auth_token_request_is_refused_where_it_grants_nothing() {
    let router = emulator();
    let microvm_id = running_microvm(&router, "refusable").await;

    // Rows span each bound the model publishes for this input and the one rule it
    // cannot express (a range's start must not pass its end).
    let rows: Vec<(&str, Value)> = vec![
        (
            "a port below the published minimum",
            json!({"expirationInMinutes": 15, "allowedPorts": [{"port": 0}]}),
        ),
        (
            "a port past the published maximum",
            json!({"expirationInMinutes": 15, "allowedPorts": [{"port": 65_536}]}),
        ),
        (
            "an endpoint of a range past the published maximum",
            json!({
                "expirationInMinutes": 15,
                "allowedPorts": [{"range": {"startPort": 1, "endPort": 65_536}}],
            }),
        ),
        (
            "an inverted range",
            json!({
                "expirationInMinutes": 15,
                "allowedPorts": [{"range": {"startPort": 90, "endPort": 80}}],
            }),
        ),
        (
            "no allowed ports at all",
            json!({"expirationInMinutes": 15, "allowedPorts": []}),
        ),
        (
            "an expiration below the published minimum",
            json!({"expirationInMinutes": 0, "allowedPorts": [{"allPorts": {}}]}),
        ),
        (
            "an expiration that is absent",
            json!({"allowedPorts": [{"allPorts": {}}]}),
        ),
        (
            "allowed ports that are absent",
            json!({"expirationInMinutes": 15}),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = auth_token(&router, &microvm_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
    }

    // A MicroVM on its way out accepts no connection, so there is nothing for a token
    // to grant access to.
    let (status, terminated) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "terminate: got {status} {terminated}");
    for (case, wait) in [
        ("while TERMINATING", false),
        ("once TERMINATED", true),
    ] {
        if wait {
            tokio::time::sleep(PAST_THE_SPAN).await;
        }
        let (status, response) = auth_token(
            &router,
            &microvm_id,
            json!({"expirationInMinutes": 15, "allowedPorts": [{"allPorts": {}}]}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the modelled conflict, got {status} {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {response}"
        );
    }
}
