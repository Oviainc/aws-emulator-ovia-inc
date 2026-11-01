// SPDX-License-Identifier: CC0-1.0

//! The REFUSAL PAYLOADS KMS's tagging surface and `RetireGrant` answer with, driven
//! end-to-end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: TrentService.<Operation>`) through the shared [`super::harness`].
//!
//! A refused call is answered by an error CLASS (the body's `__type`), a member carrying
//! the sentence, and the sentence itself; this suite is about those three and not about
//! the status, which `super::client_error_status` owns. Four rules, each asserted over a
//! TABLE rather than one call:
//!
//! * **T1** — a `TagList` naming one tag key twice is refused `TagException` with FIXED
//!   text that names neither the repeated key nor where it repeats, wherever a request
//!   carries tags.
//! * **T2** — a `KeyId` that is no key identifier AWS could have issued is reported as the
//!   invalid key id it is, echoing the value the request sent rather than an ARN
//!   synthesised from it, while a well-formed key id that names no key is reported by the
//!   key ARN it would have had. Beside it, `TagResource`, `UntagResource` and
//!   `ListResourceTags` report an identifier they cannot read as a key or alias ARN the
//!   same way they report any other failed lookup — as the resource not being found,
//!   naming the part of it that could not be read — rather than as an ARN-syntax error.
//! * **T3** — the tag listing refuses a `Marker` it did not issue by naming the token it
//!   rejected, and still resumes on a token it did issue.
//! * **T4** — `RetireGrant` on a grant the store does not hold answers the authorization
//!   failure in the `Message` member, naming the principal that signed the request and the
//!   action, and nothing about the grant.
//!
//! The rows differ along what each rule generalises over: for T1 the position of the
//! repeat, whether the two values agree, the number of tags and the operation the list was
//! sent to; for T2 the FORM of the value (both key-id forms KMS documents, values that
//! only resemble them, the modelled length bounds, a key ARN, each malformed ARN shape)
//! and the Region it names; for T3 what makes a token invalid; for T4 how the request
//! names the grant and which identity signed it.

use serde_json::{json, Value};

use super::harness::{
    assert_empty, call, call_as, create_key, enabled_key, error_code, error_message, iam_user, ok,
    ok_in, principal, refused, router, router_with_identity_services, text, ACCOUNT_KEY, REGION,
};

/// The operations that reach a key's tags, which word a failed `KeyId` lookup their own
/// way (T2) — `smithy_query(query="digest")`: the three whose input is a `KeyId` and a tag
/// member.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "UntagResource", "ListResourceTags"];

/// A tag list as the wire carries it.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(tag_key, tag_value)| json!({"TagKey": tag_key, "TagValue": tag_value}))
            .collect(),
    )
}

/// The request `operation` needs to reach the key `identifier` names, with the members
/// beyond `KeyId` that operation requires.
fn tagging_request(operation: &str, identifier: &str) -> Value {
    match operation {
        "TagResource" => json!({"KeyId": identifier, "Tags": tag_list(&[("owner", "nobody")])}),
        "UntagResource" => json!({"KeyId": identifier, "TagKeys": ["owner"]}),
        _ => json!({"KeyId": identifier}),
    }
}

/// T1: one tag key twice in a request is refused with KMS's fixed sentence, which carries
/// no value from the request — on every operation that accepts a tag list.
#[tokio::test]
async fn a_repeated_tag_key_is_refused_with_fixed_text() {
    let router = router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");
    // A multi-Region PRIMARY key, which is the only key `ReplicateKey` accepts.
    let primary = create_key(&router, &json!({"Description": "primary", "MultiRegion": true})).await;
    let primary_id = text("setup", &primary, "KeyId");

    // Rows: the case, and the tag list sent. They differ in where the repeat sits, in
    // whether the two values agree, in how many tags surround it, and in the tag key
    // itself — the sentence is the same for all of them.
    let lists: Vec<(&str, Value)> = vec![
        ("the same tag key twice", tag_list(&[("env", "one"), ("env", "two")])),
        (
            "the same tag key and value twice",
            tag_list(&[("env", "same"), ("env", "same")]),
        ),
        (
            "a repeat after other tags",
            tag_list(&[("alpha", "1"), ("beta", "2"), ("alpha", "3")]),
        ),
        (
            "a repeat of an empty value",
            tag_list(&[("owner", ""), ("owner", "")]),
        ),
        (
            "a repeat of a long tag key",
            tag_list(&[("k".repeat(128).as_str(), "1"), ("k".repeat(128).as_str(), "2")]),
        ),
    ];

    // Rows: the operation the list is sent to, and the rest of its request.
    let operations: Vec<(&str, Value)> = vec![
        ("TagResource", json!({"KeyId": key_id})),
        ("CreateKey", json!({"Description": "tagged at creation"})),
        (
            "ReplicateKey",
            json!({"KeyId": primary_id, "ReplicaRegion": "eu-west-1"}),
        ),
    ];

    for (list_case, list) in &lists {
        for (operation, rest) in &operations {
            let case = format!("{list_case} on {operation}");
            let mut request = rest.clone();
            request["Tags"] = list.clone();
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "TagException"),
                "case {case}: expected HTTP 400 TagException, got {status} {code}"
            );
            assert_eq!(
                message, "Duplicate tag keys",
                "case {case}: expected KMS's fixed sentence, got {message:?}"
            );
            for tag in list.as_array().unwrap_or(&Vec::new()) {
                let tag_key = text(&case, tag, "TagKey");
                assert!(
                    !message.contains(tag_key.as_str()),
                    "case {case}: the sentence is fixed text, so it must not name the tag \
                     key {tag_key:?}, got {message:?}"
                );
            }
        }
    }
}

/// T2: a `KeyId` that is no key identifier at all is reported as the INVALID KEY ID it is,
/// echoing the value the request gave — no ARN is synthesised from a value AWS would never
/// have issued — while a well-formed key id that simply names no key is reported by the
/// key ARN it would have had. The rule is about the VALUE, so it holds on every operation
/// that resolves a `KeyId`; the tagging operations are the rows here beside one reader of
/// the key-management surface.
#[tokio::test]
async fn a_key_id_that_is_no_key_identifier_is_reported_as_an_invalid_one() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: the case, the identifier, and the message expected. `KeyIdType` is
    // `@length(1, 2048)` with no pattern
    // (`smithy_query(query="shape",shape="KeyIdType")`), so the service — not the
    // boundary validator — judges the FORM, and the rows walk both published lengths, the
    // two key-id forms KMS documents for this member ("Key ID:
    // 1234abcd-12ab-34cd-56ef-1234567890ab", "Key ARN: arn:aws:kms:…:key/…",
    // `smithy_query(query="doc",shape="TagResourceRequest",member="KeyId")`, and
    // `mrk-1234abcd12ab34cd56ef1234567890ab` from
    // `smithy_query(query="doc",shape="ReplicateKeyRequest",member="KeyId")`), and values
    // that only resemble them.
    let rows: Vec<(String, String, String)> = vec![
        (
            "a single-Region key id that names no key".to_string(),
            "11111111-2222-3333-4444-555555555555".to_string(),
            format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/\
                 11111111-2222-3333-4444-555555555555' does not exist"
            ),
        ),
        (
            "a multi-Region key id that names no key".to_string(),
            format!("mrk-{}", "a".repeat(32)),
            format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/mrk-{}' does not exist",
                "a".repeat(32)
            ),
        ),
        (
            "a key ARN of this Region naming no key".to_string(),
            format!("arn:aws:kms:{REGION}:{account}:key/11111111-2222-3333-4444-555555555555"),
            format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/\
                 11111111-2222-3333-4444-555555555555' does not exist"
            ),
        ),
        (
            "hyphenated groups of the wrong lengths".to_string(),
            "111111111111-is-not-a-key-0000-0000".to_string(),
            "Invalid keyId '111111111111-is-not-a-key-0000-0000'".to_string(),
        ),
        (
            "a uuid's groups carrying characters that are not hexadecimal".to_string(),
            "1111111z-2222-3333-4444-555555555555".to_string(),
            "Invalid keyId '1111111z-2222-3333-4444-555555555555'".to_string(),
        ),
        (
            "a uuid with no hyphens".to_string(),
            "1".repeat(32),
            format!("Invalid keyId '{}'", "1".repeat(32)),
        ),
        (
            "the multi-Region prefix with too few hexadecimal characters".to_string(),
            format!("mrk-{}", "a".repeat(31)),
            format!("Invalid keyId 'mrk-{}'", "a".repeat(31)),
        ),
        (
            "a word that is no identifier of any form".to_string(),
            "no-such-key".to_string(),
            "Invalid keyId 'no-such-key'".to_string(),
        ),
        (
            "a value at the modelled minimum length".to_string(),
            "k".to_string(),
            "Invalid keyId 'k'".to_string(),
        ),
        (
            "a value at the modelled maximum length".to_string(),
            "k".repeat(2048),
            format!("Invalid keyId '{}'", "k".repeat(2048)),
        ),
    ];

    // The rows run on the three tagging operations and on one reader of the
    // key-management surface, because the judgement is about the value the request sent.
    let operations: Vec<&str> = TAGGING_OPERATIONS
        .iter()
        .copied()
        .chain(["DescribeKey"])
        .collect();

    for (case, identifier, expected) in rows {
        for operation in &operations {
            let case = format!("{case} on {operation}");
            let request = tagging_request(operation, &identifier);
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NotFoundException"),
                "case {case}: expected HTTP 400 NotFoundException, got {status} {code}"
            );
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
            if expected.starts_with("Invalid keyId") {
                assert!(
                    !message.contains("arn:aws:kms:"),
                    "case {case}: a value that is no key id must not be expanded into an \
                     ARN the request never carried, got {message:?}"
                );
            }
        }
    }
}

/// T2, the other half: an identifier that is an ARN this service cannot read as a key or
/// alias ARN is reported by the tagging operations as a missing resource — never as an
/// ARN-syntax error — naming the part of the identifier that could not be read: the Region
/// it pins the request to when that Region is not the endpoint's, since no endpoint of
/// this one can serve it, and otherwise the identifier itself.
#[tokio::test]
async fn the_tagging_operations_report_an_unreadable_arn_as_a_missing_resource() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: the case, the identifier, and the token the refusal names. They differ in
    // which field of the ARN is unreadable (a missing resource field, an empty key id, a
    // resource type this service does not address, another vendor's namespace, a truncated
    // ARN) and in whether the ARN names this endpoint's Region or another's.
    let rows: Vec<(String, String, String)> = vec![
        (
            "an ARN with no resource field at all".to_string(),
            format!("arn:aws:kms:{REGION}:{account}"),
            format!("arn:aws:kms:{REGION}:{account}"),
        ),
        (
            "a key resource carrying no key id".to_string(),
            format!("arn:aws:kms:{REGION}:{account}:key/"),
            format!("arn:aws:kms:{REGION}:{account}:key/"),
        ),
        (
            "a key resource carrying no key id, in another Region".to_string(),
            format!("arn:aws:kms:eu-west-1:{account}:key/"),
            "eu-west-1".to_string(),
        ),
        (
            "an unreadable resource beside an account segment that names no account"
                .to_string(),
            "arn:aws:kms:ap-south-1:not-an-account:key/".to_string(),
            "ap-south-1".to_string(),
        ),
        (
            "a resource type this service does not address".to_string(),
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
        ),
        (
            "another vendor's ARN".to_string(),
            "arn:aws:s3:::my-bucket".to_string(),
            "arn:aws:s3:::my-bucket".to_string(),
        ),
        (
            "an ARN truncated to its vendor".to_string(),
            "arn:aws:kms".to_string(),
            "arn:aws:kms".to_string(),
        ),
        (
            "nothing but the ARN prefix".to_string(),
            "arn:".to_string(),
            "arn:".to_string(),
        ),
    ];

    for (case, identifier, token) in rows {
        for operation in TAGGING_OPERATIONS {
            let case = format!("{case} on {operation}");
            let request = tagging_request(operation, &identifier);
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NotFoundException"),
                "case {case}: expected HTTP 400 NotFoundException, got {status} {code}"
            );
            assert_eq!(
                message,
                format!("Invalid arn {token}"),
                "case {case}: expected the refusal to name the unreadable part, got \
                 {message:?}"
            );
        }
    }
}

/// T3: the tag listing names the `Marker` it rejected, and a token it did issue still
/// resumes the listing.
#[tokio::test]
async fn the_tag_listing_names_the_marker_it_rejects() {
    let router = router();
    let tags = tag_list(&[("a", "1"), ("b", "2"), ("c", "3")]);
    let metadata = create_key(&router, &json!({"Description": "markers", "Tags": tags})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // A token of ANOTHER listing of this service, well formed for that listing and
    // foreign to this one. The key listing needs a second key to have a page to resume
    // after at all.
    create_key(&router, &json!({"Description": "a second key"})).await;
    let foreign = text(
        "setup",
        &ok(&router, "ListKeys", &json!({"Limit": 1})).await,
        "NextMarker",
    );
    // A token this listing DID mint, for a tag that is then removed — so it resumes
    // nowhere by the time it comes back.
    let stale = text(
        "setup",
        &ok(
            &router,
            "ListResourceTags",
            &json!({"KeyId": key_id, "Limit": 1}),
        )
        .await,
        "NextMarker",
    );
    let removed = ok(
        &router,
        "UntagResource",
        &json!({"KeyId": key_id, "TagKeys": ["a"]}),
    )
    .await;
    assert_empty("setup", "UntagResource", &removed);

    // Rows: what makes the token invalid — text that is no token of this service, hex
    // carrying no listing prefix, a token of another listing, the shortest value
    // `MarkerType`'s `^[\u0020-\u00FF]*$` pattern admits, and a token this listing minted
    // for a tag the key no longer carries.
    let markers: Vec<(&str, String)> = vec![
        ("text that is no token", "not-a-pagination-token".to_string()),
        ("hex carrying no listing prefix", hex::encode("b")),
        ("a token of another listing", foreign),
        ("one space", " ".to_string()),
        ("a token for a tag the key no longer carries", stale),
    ];

    for (case, marker) in markers {
        let (status, code, message) = refused(
            &router,
            REGION,
            "ListResourceTags",
            &json!({"KeyId": key_id, "Marker": marker}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidMarkerException"),
            "case {case}: expected HTTP 400 InvalidMarkerException, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("Invalid NextMarker: {marker}"),
            "case {case}: expected the refusal to name the token it rejected, got {message:?}"
        );
    }

    // The positive control: a token this listing minted for a tag the key still carries
    // resumes after it, so the refusal above is about the token and not about paging.
    let first = ok(
        &router,
        "ListResourceTags",
        &json!({"KeyId": key_id, "Limit": 1}),
    )
    .await;
    let marker = text("resume", &first, "NextMarker");
    let second = ok(
        &router,
        "ListResourceTags",
        &json!({"KeyId": key_id, "Limit": 1, "Marker": marker}),
    )
    .await;
    assert_eq!(
        second["Tags"].as_array().map(Vec::len),
        Some(1),
        "case resume: a token this listing minted must resume it, got {second}"
    );
}

/// T4: `RetireGrant` on a grant the store does not hold answers the authorization failure
/// in the `Message` member — the member name AWS's authorization layer uses, which is not
/// the lowercase `message` of this service's modelled errors — naming the principal that
/// signed the request and the action, and nothing about the grant.
#[tokio::test]
async fn retire_grant_reports_a_grant_it_does_not_hold_in_the_message_member() {
    let router = router_with_identity_services();
    let account = engine::emulator_account_id();

    // A second identity, so the principal in the sentence is read from the signing
    // identity rather than being one constant.
    let (user_key, user_arn) = iam_user(&router, "retirer", None).await;
    let emulator_arn = format!("arn:aws:iam::{account}:user/emulator");

    // One key, and a grant on it that is retired before the table runs — so a row naming
    // it names a grant that was held and is not any more.
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");
    let key_arn = text(
        "setup",
        &ok_in(&router, REGION, "DescribeKey", &json!({"KeyId": key_id})).await["KeyMetadata"],
        "Arn",
    );
    let created = ok(
        &router,
        "CreateGrant",
        &json!({
            "KeyId": key_id,
            "GranteePrincipal": principal("role/Grantee"),
            "RetiringPrincipal": principal("role/Retirer"),
            "Operations": ["Encrypt", "Decrypt", "RetireGrant"],
        }),
    )
    .await;
    let retired_id = text("setup", &created, "GrantId");
    let retired_token = text("setup", &created, "GrantToken");
    let retire = ok(
        &router,
        "RetireGrant",
        &json!({"KeyId": key_arn, "GrantId": retired_id}),
    )
    .await;
    assert_empty("setup", "RetireGrant", &retire);

    // A second grant, revoked rather than retired: the same absence by another route.
    let revoked = ok(
        &router,
        "CreateGrant",
        &json!({
            "KeyId": key_id,
            "GranteePrincipal": principal("role/Grantee"),
            "Operations": ["Encrypt"],
        }),
    )
    .await;
    let revoked_id = text("setup", &revoked, "GrantId");
    ok(
        &router,
        "RevokeGrant",
        &json!({"KeyId": key_id, "GrantId": revoked_id}),
    )
    .await;

    // Rows: the case, the access key the call is signed with, the principal the sentence
    // must name, and the request — which differ in how the grant is identified (grant id
    // and key id, grant id and key ARN, a grant token) and in why the store holds no such
    // grant (never created, retired, revoked).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a well-formed grant id nothing holds",
            ACCOUNT_KEY,
            emulator_arn.as_str(),
            json!({"KeyId": key_arn, "GrantId": "f".repeat(64)}),
        ),
        (
            "a grant already retired, named by the key ARN",
            ACCOUNT_KEY,
            emulator_arn.as_str(),
            json!({"KeyId": key_arn, "GrantId": retired_id}),
        ),
        (
            "a grant already revoked",
            ACCOUNT_KEY,
            emulator_arn.as_str(),
            json!({"KeyId": key_arn, "GrantId": revoked_id}),
        ),
        (
            "the grant token of a retired grant",
            ACCOUNT_KEY,
            emulator_arn.as_str(),
            json!({"GrantToken": retired_token}),
        ),
        (
            "a retired grant, asked for by another identity",
            user_key.as_str(),
            user_arn.as_str(),
            json!({"KeyId": key_arn, "GrantId": retired_id}),
        ),
    ];

    for (case, access_key_id, caller, request) in rows {
        let (status, payload) = call_as(&router, REGION, access_key_id, "RetireGrant", &request)
            .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "AccessDeniedException"),
            "case {case}: expected HTTP 400 AccessDeniedException, got {status} with {payload}"
        );
        let expected = format!(
            "User: {caller} is not authorized to perform: kms:RetireGrant on this resource \
             because the resource does not exist in this Region, no resource-based policies \
             allow access, or a resource-based policy explicitly denies access"
        );
        assert_eq!(
            payload["Message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the sentence in the Message member, got {payload}"
        );
        assert!(
            payload.get("message").is_none(),
            "case {case}: the authorization refusal carries its sentence in Message alone, \
             got {payload}"
        );
        let message = error_message(&payload);
        for secret in [retired_id.as_str(), revoked_id.as_str(), key_arn.as_str()] {
            assert!(
                !message.contains(secret),
                "case {case}: the refusal reports nothing about the resource, so it must \
                 not name {secret:?}, got {message:?}"
            );
        }
    }

    // The positive control: the key's own grants are still readable, so the refusals above
    // are about the grants that are gone and not about the key.
    let listed = ok(&router, "ListGrants", &json!({"KeyId": key_id})).await;
    assert_eq!(
        listed["Grants"].as_array().map(Vec::len),
        Some(0),
        "case the key after both grants are gone: expected an empty listing, got {listed}"
    );
    let (status, _) = call(&router, REGION, "DescribeKey", &json!({"KeyId": key_id})).await;
    assert_eq!(
        status, 200,
        "case the key after both grants are gone: the key itself is still served, got {status}"
    );
}
