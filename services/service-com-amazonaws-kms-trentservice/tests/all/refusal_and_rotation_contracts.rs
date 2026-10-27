// SPDX-License-Identifier: CC0-1.0

//! The refusal and reporting CONTRACTS three families of this service answer on, driven
//! end-to-end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: TrentService.<Operation>`) through the shared [`super::harness`].
//!
//! Four rules, each asserted over a TABLE rather than one call:
//!
//! * **R1** — a member documented as "the Amazon Resource Name (ARN) of an Amazon Web
//!   Services principal" is served only for a value that names an IAM or STS IDENTITY;
//!   anything else is refused `InvalidArnException` (HTTP 400) rather than answered, so a
//!   malformed principal is never indistinguishable from one that holds nothing.
//! * **R2** — a `Marker` the service never issued is refused `InvalidMarkerException`
//!   (HTTP 400) carrying NO message member at all, on every paginated operation.
//! * **R3** — an on-demand rotation is OBSERVABLE: `GetKeyRotationStatus` reports the
//!   instant it was initiated while it is in progress and stops once it completes,
//!   `ListKeyRotations` records it, and a ciphertext sealed before it still decrypts under
//!   the material that sealed it.
//! * **R4** — `UpdatePrimaryRegion` tells the two refusals of a multi-Region key apart:
//!   a key that is not multi-Region is `UnsupportedOperationException` naming the key, and
//!   a Region holding no replica of the set is a validation failure naming the KEY that
//!   has no replica there.

use serde_json::{json, Value};

use super::harness::{
    b64, call, create_key, describe, error_code, error_message, ok, ok_in, refused, router,
    settle_rotation, text, REGION,
};

// ---------------------------------------------------------------------------
// R1: the principal-ARN grammar
// ---------------------------------------------------------------------------

/// Every value a principal member is served for, and every value it is refused for.
///
/// "To specify the retiring principal, use the Amazon Resource Name (ARN) of an Amazon Web
/// Services principal. Valid principals include Amazon Web Services accounts, IAM users,
/// IAM roles, federated users, and assumed role users"
/// (`smithy_query(query="doc",shape="ListRetirableGrantsRequest",member="RetiringPrincipal")`,
/// whose wording `CreateGrantRequest$GranteePrincipal` repeats) — so the forms are the
/// account root, a user, a role, an assumed-role session and a federated user, each
/// GLOBAL (IAM and STS write no Region into an identity ARN) and owned by a twelve-digit
/// account. `PrincipalIdType`'s modelled pattern `^[\w+=,.@:/-]+$` admits every row of the
/// refused half, which is why both operations model `InvalidArnException`; a value the
/// PATTERN already refuses (one carrying a `*`) is the boundary validator's to reject and
/// is not a row here.
///
/// (case, the value, whether it names a principal)
fn principal_rows(account: &str) -> Vec<(&'static str, String, bool)> {
    vec![
        ("the account root", format!("arn:aws:iam::{account}:root"), true),
        ("an IAM user", format!("arn:aws:iam::{account}:user/Alice"), true),
        (
            "an IAM user under a path",
            format!("arn:aws:iam::{account}:user/engineering/Alice"),
            true,
        ),
        ("an IAM role", format!("arn:aws:iam::{account}:role/App"), true),
        (
            "an IAM role under a path",
            format!("arn:aws:iam::{account}:role/service-role/App"),
            true,
        ),
        (
            "an assumed-role session",
            format!("arn:aws:sts::{account}:assumed-role/App/session"),
            true,
        ),
        (
            "a federated user",
            format!("arn:aws:sts::{account}:federated-user/Alice"),
            true,
        ),
        (
            "an identity of another partition",
            format!("arn:aws-cn:iam::{account}:role/App"),
            true,
        ),
        (
            "an ARN with no IAM resource type at all",
            format!("arn:aws:iam::{account}:this-is-not-a-valid-iam-resource-segment"),
            false,
        ),
        (
            "an IAM group, which is no principal",
            format!("arn:aws:iam::{account}:group/Admins"),
            false,
        ),
        (
            "an IAM policy, which is no principal",
            format!("arn:aws:iam::{account}:policy/ReadOnly"),
            false,
        ),
        (
            "a resource of another service",
            "arn:aws:s3:::bucket/key".to_string(),
            false,
        ),
        (
            "a role ARN naming no role",
            format!("arn:aws:iam::{account}:role/"),
            false,
        ),
        (
            "an assumed-role ARN naming no session",
            format!("arn:aws:sts::{account}:assumed-role/App"),
            false,
        ),
        (
            "an identity ARN carrying a Region, which IAM never writes",
            format!("arn:aws:iam:{REGION}:{account}:role/App"),
            false,
        ),
        (
            "an account that is not twelve digits",
            "arn:aws:iam::12345:role/App".to_string(),
            false,
        ),
        (
            "a resource field written with a colon, which IAM never writes",
            format!("arn:aws:iam::{account}:role:App"),
            false,
        ),
        ("a bare account id", account.to_string(), false),
        ("no ARN at all", "role/App".to_string(), false),
    ]
}

/// R1: `ListRetirableGrants` answers only for a value that names a principal, and refuses
/// every other one with `InvalidArnException` instead of an empty page — which a caller
/// cannot tell from a principal that legitimately holds no retirable grants.
#[tokio::test]
async fn list_retirable_grants_reads_the_principal_arn_grammar() {
    let router = router();
    let account = engine::emulator_account_id();

    for (case, principal, names_a_principal) in principal_rows(account) {
        let (status, payload) = call(
            &router,
            REGION,
            "ListRetirableGrants",
            &json!({"RetiringPrincipal": principal}),
        )
        .await;
        if names_a_principal {
            assert_eq!(
                status, 200,
                "case {case:?} ({principal}): expected the listing to be served, got {status} \
                 with {payload}"
            );
            assert_eq!(
                payload["Grants"],
                json!([]),
                "case {case:?} ({principal}): expected no grants, got {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "InvalidArnException"),
                "case {case:?} ({principal}): expected HTTP 400 InvalidArnException, got \
                 {status} with {payload}"
            );
            assert_eq!(
                error_message(&payload),
                format!("ARN is not valid: {principal}"),
                "case {case:?}: the refusal names the value the request gave, got {payload}"
            );
        }
    }
}

/// R1, on the other member the same sentence documents: `CreateGrant`'s grantee and
/// retiring principals are read against the same grammar, so one operation cannot accept
/// a value the other refuses.
#[tokio::test]
async fn create_grant_reads_the_same_principal_arn_grammar() {
    let router = router();
    let account = engine::emulator_account_id();
    let key_id = text("setup", &create_key(&router, &json!({})).await, "KeyId");
    let valid = format!("arn:aws:iam::{account}:role/Grantee");

    for (case, principal, names_a_principal) in principal_rows(account) {
        for member in ["GranteePrincipal", "RetiringPrincipal"] {
            let mut request = json!({"KeyId": key_id, "Operations": ["Decrypt"]});
            request["GranteePrincipal"] = json!(valid);
            request[member] = json!(principal);
            let (status, payload) = call(&router, REGION, "CreateGrant", &request).await;
            if names_a_principal {
                assert_eq!(
                    status, 200,
                    "case {case:?} as {member} ({principal}): expected the grant to be \
                     created, got {status} with {payload}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, "InvalidArnException"),
                    "case {case:?} as {member} ({principal}): expected HTTP 400 \
                     InvalidArnException, got {status} with {payload}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R2: the invalid-marker envelope
// ---------------------------------------------------------------------------

/// R2: a `Marker` the service never issued is refused with the error code alone — the
/// refusal carries no `Message` member — on every paginated operation of this service.
///
/// The rows are each paginated operation (`smithy_query(query="digest")`: the operations
/// whose input carries a `Marker` and whose errors list `InvalidMarkerException`), and the
/// markers differ along what makes one invalid: text that is no token of this service at
/// all, and a well-formed token of ANOTHER listing.
#[tokio::test]
async fn an_invalid_marker_is_refused_with_no_message_at_all() {
    let router = router();
    let created = create_key(&router, &json!({"Description": "markers"})).await;
    let key_id = text("setup", &created, "KeyId");
    ok(
        &router,
        "TagResource",
        &json!({"KeyId": key_id, "Tags": [{"TagKey": "app", "TagValue": "demo"}]}),
    )
    .await;

    // (operation, the rest of the request)
    let operations: Vec<(&str, Value)> = vec![
        ("ListKeys", json!({})),
        ("ListAliases", json!({})),
        ("ListGrants", json!({"KeyId": key_id})),
        (
            "ListRetirableGrants",
            json!({"RetiringPrincipal":
                   format!("arn:aws:iam::{}:role/Retirer", engine::emulator_account_id())}),
        ),
        ("ListResourceTags", json!({"KeyId": key_id})),
        ("ListKeyRotations", json!({"KeyId": key_id})),
        ("DescribeCustomKeyStores", json!({})),
    ];

    // (case, the marker) — `6772616e74732f312f61` is the hex of "grants/1/a", a token of
    // the grant listing, so it is well formed for one operation and foreign to the rest.
    let markers: [(&str, &str); 3] = [
        ("text that is no token", "not-a-real-pagination-token-xyz"),
        ("a token of another listing", "6772616e74732f312f61"),
        ("one space, the shortest value the pattern admits", " "),
    ];

    for (operation, rest) in operations {
        for (marker_case, marker) in markers {
            let case = format!("{operation} from {marker_case}");
            let mut request = rest.clone();
            request["Marker"] = json!(marker);
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidMarkerException"),
                "case {case:?}: expected HTTP 400 InvalidMarkerException, got {status} {code}"
            );
            assert!(
                message.is_empty(),
                "case {case:?}: AWS words this refusal with no message at all, got \
                 {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R3: an on-demand rotation is observable
// ---------------------------------------------------------------------------

/// R3: `RotateKeyOnDemand` records the rotation it performs — `GetKeyRotationStatus`
/// reports the instant it was initiated while it is in progress and stops once the
/// rotation completes, which is the same instant the rotation's material becomes the one
/// the key encrypts under — `ListKeyRotations` records it as an `ON_DEMAND` rotation, and
/// a ciphertext sealed before it still decrypts under the material that sealed it.
///
/// The rows differ in how the key was created and in how many rotations in a row it gets,
/// since the reported start date is the LATEST rotation's.
#[tokio::test]
async fn an_on_demand_rotation_is_reported_while_it_is_in_progress() {
    let router = router();

    let creates: [(&str, Value); 3] = [
        ("the default symmetric encryption key", json!({})),
        (
            "an explicit SYMMETRIC_DEFAULT ENCRYPT_DECRYPT key",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
    ];

    for (described, request) in creates {
        for rotations in [1usize, 3] {
            let case = format!("{described}, {rotations} rotation(s)");
            let created = create_key(&router, &request).await;
            let key_id = text(&case, &created, "KeyId");
            let arn = text(&case, &created, "Arn");
            let first_material = text(&case, &created, "CurrentKeyMaterialId");

            // A key nothing has rotated reports no on-demand rotation at all.
            let before = rotation_status(&router, &case, &key_id).await;
            assert!(
                before["OnDemandRotationStartDate"].is_null(),
                "case {case:?}: a key that has had no on-demand rotation reports no start \
                 date, got {before}"
            );

            // A ciphertext sealed under the key's first material version, which every
            // rotation below must leave decryptable.
            let sealed = ok(
                &router,
                "Encrypt",
                &json!({"KeyId": key_id, "Plaintext": b64(b"before the rotation")}),
            )
            .await;
            let blob = text(&case, &sealed, "CiphertextBlob");

            for round in 1..=rotations {
                let at = format!("{case}, rotation {round}");
                // The material the key encrypts under when this round starts, which the
                // rotation replaces only once it completes.
                let before_round = text(&at, &describe(&router, &key_id).await, "CurrentKeyMaterialId");
                let started_at = seconds_now();
                let answer = ok(&router, "RotateKeyOnDemand", &json!({"KeyId": key_id})).await;
                assert_eq!(
                    answer["KeyId"],
                    json!(arn),
                    "case {at:?}: the rotation answers the key ARN, got {answer}"
                );

                // The rotation is in progress, and the instant it was initiated is the
                // one this call was served at.
                let during = rotation_status(&router, &at, &key_id).await;
                let start = during["OnDemandRotationStartDate"].as_f64().unwrap_or_else(|| {
                    panic!("case {at:?}: expected an OnDemandRotationStartDate in {during}")
                });
                assert!(
                    start >= started_at - 1.0 && start <= seconds_now() + 1.0,
                    "case {at:?}: expected the start date to be the instant the rotation \
                     was initiated (around {started_at}), got {start}"
                );
                assert_eq!(
                    during["KeyId"],
                    json!(arn),
                    "case {at:?}: the status names the key by its ARN, got {during}"
                );

                // While it is in progress the key goes on encrypting under the material
                // it had: the rotation has not landed, so nothing it brings is current.
                let still = describe(&router, &key_id).await;
                assert_eq!(
                    still["CurrentKeyMaterialId"],
                    json!(before_round),
                    "case {at:?}: a rotation in progress leaves the current material \
                     alone, got {still}"
                );

                // Once the clock completes it, "KMS removes this field from the
                // response" — and the material it installed is current from that same
                // instant, so no caller ever sees one without the other.
                settle_rotation(&router, &at, &key_id).await;
                let settled = rotation_status(&router, &at, &key_id).await;
                assert!(
                    settled["OnDemandRotationStartDate"].is_null(),
                    "case {at:?}: a completed on-demand rotation reports no start date, \
                     got {settled}"
                );
                let rotated = describe(&router, &key_id).await;
                assert_ne!(
                    rotated["CurrentKeyMaterialId"],
                    json!(before_round),
                    "case {at:?}: a completed rotation makes new material current, got \
                     {rotated}"
                );
            }

            // The material that sealed the ciphertext still opens it.
            let opened = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
            assert_eq!(
                opened["KeyMaterialId"],
                json!(first_material),
                "case {case:?}: the material that sealed the ciphertext is what opened it, \
                 got {opened}"
            );

            // Every rotation is recorded as an on-demand one, and the record outlives the
            // in-progress report.
            let listed = ok(&router, "ListKeyRotations", &json!({"KeyId": key_id})).await;
            let entries = listed["Rotations"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Rotations list in {listed}"));
            let on_demand = entries
                .iter()
                .filter(|entry| entry["RotationType"] == json!("ON_DEMAND"))
                .count();
            assert_eq!(
                on_demand, rotations,
                "case {case:?}: expected {rotations} ON_DEMAND rotation(s) to be recorded, \
                 got {listed}"
            );
        }
    }
}

/// The `GetKeyRotationStatus` report for `identifier`.
async fn rotation_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> Value {
    let answer = ok(router, "GetKeyRotationStatus", &json!({"KeyId": identifier})).await;
    assert!(
        answer["KeyId"].is_string(),
        "case {case:?}: expected the status to name the key, got {answer}"
    );
    answer
}

/// Now, as the epoch seconds a `DateType` member is rendered as.
fn seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// R4: the two UpdatePrimaryRegion refusals
// ---------------------------------------------------------------------------

/// R4: the two refusals of `UpdatePrimaryRegion` name different operands, which is what
/// proves the rejection is about the key's multi-Region status rather than a blanket deny:
/// a key that is not multi-Region is refused as an unsupported RESOURCE, named by its ARN,
/// and a multi-Region primary key with no replica in the Region asked for is refused as a
/// bad parameter value, naming the KEY that has no replica there.
///
/// The rows differ in the identifier form the request names the key by, in the Region
/// asked for (one that holds no key of the set, and the one that already holds the primary
/// key), and in the kind of key.
#[tokio::test]
async fn update_primary_region_tells_its_two_refusals_apart() {
    let router = router();

    let single = create_key(&router, &json!({"Description": "single"})).await;
    let single_id = text("setup", &single, "KeyId");
    let single_arn = text("setup", &single, "Arn");
    let multi = create_key(&router, &json!({"MultiRegion": true})).await;
    let multi_id = text("setup", &multi, "KeyId");
    let multi_arn = text("setup", &multi, "Arn");

    // (case, the identifier, the Region asked for, expected code, expected message)
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "a single-Region key, named by its key id",
            &single_id,
            "us-west-2",
            "UnsupportedOperationException",
            format!("{single_arn} is not a multi-region key."),
        ),
        (
            "a single-Region key, named by its ARN",
            &single_arn,
            "eu-west-1",
            "UnsupportedOperationException",
            format!("{single_arn} is not a multi-region key."),
        ),
        (
            "a multi-Region primary key, a Region holding no replica of the set",
            &multi_id,
            "us-west-2",
            "ValidationException",
            format!("{multi_id} does not have a replica key in the specified Region."),
        ),
        (
            "a multi-Region primary key, named by its ARN",
            &multi_arn,
            "eu-west-1",
            "ValidationException",
            format!("{multi_id} does not have a replica key in the specified Region."),
        ),
        (
            "a multi-Region primary key, the Region that already holds the primary key",
            &multi_id,
            REGION,
            "ValidationException",
            format!("{multi_id} does not have a replica key in the specified Region."),
        ),
    ];

    for (case, identifier, primary_region, expected_code, expected_message) in rows {
        let (status, code, message) = refused(
            &router,
            REGION,
            "UpdatePrimaryRegion",
            &json!({"KeyId": identifier, "PrimaryRegion": primary_region}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }

    // The refusals persisted nothing: the key set's primary Region is where it was.
    let described = ok_in(&router, REGION, "DescribeKey", &json!({"KeyId": multi_id})).await
        ["KeyMetadata"]
        .clone();
    assert_eq!(
        described["MultiRegionConfiguration"]["PrimaryKey"]["Region"],
        json!(REGION),
        "a refused promotion must leave the primary Region where it was, got {described}"
    );
}
