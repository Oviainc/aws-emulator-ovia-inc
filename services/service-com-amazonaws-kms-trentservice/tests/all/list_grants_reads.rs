// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READER of a key's grants, `ListGrants`, driven over the
//! wire through the shared [`super::harness`].
//!
//! The sibling [`super::grants`] suite drives this reader from the grant WRITERS —
//! what `CreateGrant` persists, what `RevokeGrant` and `RetireGrant` remove, and how
//! the reader pages. This suite is about the reader's own rule: `ListGrants` reports
//! the grants persisted for the key its `KeyId` RESOLVES to — whatever identifier form
//! names that key, and whatever key state the key is in, since a grant outlives the key
//! being disabled or scheduled for deletion and a reader must still see it — narrowed
//! by EVERY filter the request carries at once ("you can filter the grant list by grant
//! ID, grantee principal, or grantee service principal",
//! `smithy_query(query="doc",shape="ListGrants")`), and it writes nothing, so what one
//! caller lists is what the next one lists.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the identifier
//! form the read uses, in the key's state, in which filters the request carries and in
//! whether they select, in which key the grants were created on, and — for the refusals
//! — in why the identifier resolves to no key of this account and Region, and in why a
//! filter member cannot select at all.
//!
//! A reader answers its grants ordered by grant id, which is [`super::grant_read_projection`]'s
//! rule; every expectation here is therefore the selected grants in that order.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    alias_arn, create_alias, create_key, describe, failure, ok, principal, refused, router, text,
    REGION,
};

/// The grantee of the first two grants every seeded key carries.
const READER_ROLE: &str = "role/Reader";

/// The grantee of the third grant, and the retiring principal of the fourth.
const OTHER_ROLE: &str = "role/Other";

/// The grantee SERVICE principal of the fourth grant. Its form is
/// `ServicePrincipalType`'s modelled `^([A-Za-z0-9\-]+)\.([A-Za-z0-9\-]+)(\.[A-Za-z0-9\-]+)+$`
/// (`smithy_query(query="operation",operation="ListGrants")`).
const GRANTEE_SERVICE: &str = "dynamodb.amazonaws.com";

/// A service principal no seeded grant was created for, so the service-principal filter
/// has something to select nothing with.
const OTHER_SERVICE: &str = "ecs.amazonaws.com";

/// A key and the grants it was seeded with.
struct Seeded {
    key_id: String,
    key_arn: String,
    /// The `KeyMetadata` `CreateKey` answered, so a case can show that reading the
    /// key's grants left the key itself untouched.
    metadata: Value,
    /// The grant ids in creation order: two for [`READER_ROLE`], one for
    /// [`OTHER_ROLE`], and one for [`GRANTEE_SERVICE`]. A read answers them in grant
    /// id order, so an expectation is built with [`ascending`].
    grants: Vec<String>,
}

/// `grants` in the order a reader answers them: by grant id ascending.
fn ascending(grants: &[String]) -> Vec<String> {
    let mut ordered = grants.to_vec();
    ordered.sort();
    ordered
}

/// Create a symmetric encryption key and four grants on it that differ in grantee, in
/// whether the grantee is a service principal, in whether the grant carries a `Name`
/// and a retiring principal, and in which operations they permit.
async fn seed(router: &MultiServiceRouter, case: &str) -> Seeded {
    let metadata = create_key(router, &json!({"Description": "grant reader"})).await;
    let key_id = text(case, &metadata, "KeyId");
    let key_arn = text(case, &metadata, "Arn");
    // "When you specify a GranteeServicePrincipal, you must also specify a SourceArn
    // grant constraint" — the resource the service acts on behalf of.
    let source_arn = format!(
        "arn:aws:dynamodb:{REGION}:{account}:table/Orders",
        account = engine::emulator_account_id()
    );
    let requests = [
        json!({
            "KeyId": key_id,
            "GranteePrincipal": principal(READER_ROLE),
            "Operations": ["Decrypt"],
        }),
        json!({
            "KeyId": key_id,
            "Name": "second-of-the-reader",
            "GranteePrincipal": principal(READER_ROLE),
            "RetiringPrincipal": principal(OTHER_ROLE),
            "Operations": ["Encrypt", "Decrypt"],
        }),
        json!({
            "KeyId": key_id,
            "GranteePrincipal": principal(OTHER_ROLE),
            "Operations": ["DescribeKey", "RetireGrant"],
        }),
        json!({
            "KeyId": key_id,
            "GranteeServicePrincipal": GRANTEE_SERVICE,
            "RetiringPrincipal": principal(OTHER_ROLE),
            "Constraints": {"SourceArn": source_arn},
            "Operations": ["GenerateDataKeyWithoutPlaintext", "Decrypt"],
        }),
    ];

    let mut grants = Vec::new();
    for request in &requests {
        let answer = ok(router, "CreateGrant", request).await;
        grants.push(text(case, &answer, "GrantId"));
    }
    Seeded {
        key_id,
        key_arn,
        metadata,
        grants,
    }
}

/// The grant ids one `ListGrants` answer reports, in the order reported — which is by
/// grant id ascending.
///
/// Every entry is checked to name, by its key ARN, the key the request resolved to —
/// the listing is of that key's grants, however the request named it — and the page is
/// checked to be the whole listing, which every case here asks for.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    key_arn: &str,
    request: &Value,
) -> Vec<String> {
    let answer = ok(router, "ListGrants", request).await;
    let entries = answer["Grants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
        .clone();
    for entry in &entries {
        assert_eq!(
            entry["KeyId"],
            json!(key_arn),
            "case {case}: every entry must name the key it was listed from by its key ARN, \
             got {entry}"
        );
    }
    assert_eq!(
        answer["Truncated"],
        json!(false),
        "case {case}: a page holding every grant must not be truncated, got {answer}"
    );
    assert!(
        answer["NextMarker"].is_null(),
        "case {case}: an untruncated page carries no NextMarker, got {answer}"
    );
    entries
        .iter()
        .map(|entry| text(case, entry, "GrantId"))
        .collect()
}

/// A key's grants are reported for every identifier form that resolves to that key, and
/// reading them changes nothing a later caller sees.
#[tokio::test]
async fn the_reader_reports_a_key_s_grants_for_every_identifier_form() {
    let router = router();
    let seeded = seed(&router, "setup").await;
    let alias = "alias/grant-reader";
    create_alias(&router, "setup", alias, &seeded.key_id).await;

    for (form, identifier) in [
        ("key id", seeded.key_id.clone()),
        ("key ARN", seeded.key_arn.clone()),
        ("alias name", alias.to_string()),
        ("alias ARN", alias_arn(REGION, alias)),
    ] {
        let reported = listed(&router, form, &seeded.key_arn, &json!({"KeyId": identifier})).await;
        assert_eq!(
            reported,
            ascending(&seeded.grants),
            "case read by {form}: expected every grant of the key by ascending grant id {:?}, \
             got {reported:?}",
            ascending(&seeded.grants)
        );
    }

    // Reading is not writing: the same grants again, and the key exactly as `CreateKey`
    // answered it.
    let again = listed(
        &router,
        "repeated read",
        &seeded.key_arn,
        &json!({"KeyId": seeded.key_id}),
    )
    .await;
    assert_eq!(
        again,
        ascending(&seeded.grants),
        "case repeated read: listing a key's grants must not change them, got {again:?}"
    );
    let described = describe(&router, &seeded.key_id).await;
    assert_eq!(
        described, seeded.metadata,
        "case repeated read: listing a key's grants must leave the key untouched, \
         got {described}"
    );
}

/// A key's grants are readable in every key state: a grant outlives the key being
/// disabled or scheduled for deletion, so each state reports exactly what the enabled
/// key did.
#[tokio::test]
async fn the_reader_reports_grants_in_every_key_state() {
    let router = router();
    let seeded = seed(&router, "setup").await;

    // Rows: the state, and the call that puts the key into it (a read for the state the
    // key is already in).
    let states: Vec<(&str, &str, Value)> = vec![
        ("enabled", "DescribeKey", json!({"KeyId": seeded.key_id})),
        ("disabled", "DisableKey", json!({"KeyId": seeded.key_id})),
        (
            "pending deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": seeded.key_id, "PendingWindowInDays": 7}),
        ),
        (
            "deletion cancelled",
            "CancelKeyDeletion",
            json!({"KeyId": seeded.key_id}),
        ),
        ("re-enabled", "EnableKey", json!({"KeyId": seeded.key_id})),
    ];
    for (case, operation, request) in states {
        ok(&router, operation, &request).await;
        let reported = listed(
            &router,
            case,
            &seeded.key_arn,
            &json!({"KeyId": seeded.key_id}),
        )
        .await;
        assert_eq!(
            reported,
            ascending(&seeded.grants),
            "case {case}: every key state must report the key's grants, got {reported:?}"
        );
    }

    // A key still waiting for its imported key material is reported on too, and it
    // carries no grants — an empty listing, not a refusal.
    let pending = create_key(
        &router,
        &json!({"Origin": "EXTERNAL", "Description": "pending import"}),
    )
    .await;
    let pending_id = text("pending import", &pending, "KeyId");
    let pending_arn = text("pending import", &pending, "Arn");
    assert_eq!(
        pending["KeyState"],
        json!("PendingImport"),
        "case pending import: a key created with no material waits for it, got {pending}"
    );
    let reported = listed(
        &router,
        "pending import",
        &pending_arn,
        &json!({"KeyId": pending_id}),
    )
    .await;
    assert!(
        reported.is_empty(),
        "case pending import: expected no grants on a key that carries none, got {reported:?}"
    );
}

/// Every filter the request carries narrows the listing, and they narrow it TOGETHER: a
/// grant is reported only when it satisfies all of them, and a filter the request omits
/// narrows nothing.
#[tokio::test]
async fn every_filter_the_request_carries_narrows_the_listing() {
    let router = router();
    let seeded = seed(&router, "setup").await;
    let grants = seeded.grants.clone();

    // Rows: the case, the members the request carries beside `KeyId`, and the grant ids
    // expected in the order expected.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), ascending(&grants)),
        (
            "the grantee of two grants",
            json!({"GranteePrincipal": principal(READER_ROLE)}),
            ascending(&[grants[0].clone(), grants[1].clone()]),
        ),
        (
            "the grantee of one grant",
            json!({"GranteePrincipal": principal(OTHER_ROLE)}),
            vec![grants[2].clone()],
        ),
        (
            "a grantee holding no grant on this key",
            json!({"GranteePrincipal": principal("role/Nobody")}),
            Vec::new(),
        ),
        (
            "the retiring principal is not the grantee filter",
            json!({"GranteePrincipal": principal(OTHER_ROLE), "GrantId": grants[1]}),
            Vec::new(),
        ),
        (
            "the grantee service principal",
            json!({"GranteeServicePrincipal": GRANTEE_SERVICE}),
            vec![grants[3].clone()],
        ),
        (
            "a service principal holding no grant on this key",
            json!({"GranteeServicePrincipal": OTHER_SERVICE}),
            Vec::new(),
        ),
        (
            "one grant id",
            json!({"GrantId": grants[1]}),
            vec![grants[1].clone()],
        ),
        (
            "a grant id together with its own grantee",
            json!({"GrantId": grants[1], "GranteePrincipal": principal(READER_ROLE)}),
            vec![grants[1].clone()],
        ),
        (
            "a service grant's id together with its service principal",
            json!({"GrantId": grants[3], "GranteeServicePrincipal": GRANTEE_SERVICE}),
            vec![grants[3].clone()],
        ),
        (
            "a user grant's id together with a service principal",
            json!({"GrantId": grants[0], "GranteeServicePrincipal": GRANTEE_SERVICE}),
            Vec::new(),
        ),
        (
            "a well-formed grant id this key does not carry",
            json!({"GrantId": "a".repeat(64)}),
            Vec::new(),
        ),
    ];

    for (case, members, expected) in rows {
        let mut request = json!({"KeyId": seeded.key_id});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member.as_str()] = value.clone();
        }
        let reported = listed(&router, case, &seeded.key_arn, &request).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the grants {expected:?}, got {reported:?}"
        );
    }
}

/// A filter member that cannot select is refused rather than answered with a listing:
/// the two grantee filters are alternatives, and a `GrantId` that is not of the form a
/// grant id takes identifies no grant at all.
#[tokio::test]
async fn a_filter_that_cannot_select_is_refused() {
    let router = router();
    let seeded = seed(&router, "setup").await;

    // Rows: the case, the members the request carries beside `KeyId`, and the error code
    // expected. The `GrantId` rows walk the ways a value can fail to be a grant id —
    // KMS gives a grant 64 LOWERCASE hexadecimal characters — since `GrantIdType`'s
    // modelled `@length(1, 128)` accepts far more than any grant id can be.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "both grantee filters at once",
            json!({
                "GranteePrincipal": principal(READER_ROLE),
                "GranteeServicePrincipal": GRANTEE_SERVICE,
            }),
            "ValidationException",
        ),
        (
            "a grant id that is not hexadecimal",
            json!({"GrantId": "not-a-grant-id"}),
            "InvalidGrantIdException",
        ),
        (
            "a grant id in upper case",
            json!({"GrantId": "A".repeat(64)}),
            "InvalidGrantIdException",
        ),
        (
            "a grant id one character short",
            json!({"GrantId": "b".repeat(63)}),
            "InvalidGrantIdException",
        ),
        (
            "a grant id one character long",
            json!({"GrantId": "c".repeat(65)}),
            "InvalidGrantIdException",
        ),
    ];

    for (case, members, code) in rows {
        let mut request = json!({"KeyId": seeded.key_id});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member.as_str()] = value.clone();
        }
        let (status, actual) = failure(&router, "ListGrants", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case}: expected HTTP 400 {code}, got {status} {actual}"
        );
    }

    // Neither refusal disturbed the listing: every seeded grant is still reported.
    let reported = listed(
        &router,
        "after the refusals",
        &seeded.key_arn,
        &json!({"KeyId": seeded.key_id}),
    )
    .await;
    assert_eq!(
        reported,
        ascending(&seeded.grants),
        "case after the refusals: a refused filter must leave the key's grants as they \
         were, got {reported:?}"
    );
}

/// A grant belongs to one key, so the listing holds the grants of the key the
/// identifier resolved to and no others — and a key carrying none reports an empty
/// listing.
#[tokio::test]
async fn the_listing_holds_only_the_grants_of_the_key_it_names() {
    let router = router();
    let first = seed(&router, "first key").await;
    let second = seed(&router, "second key").await;
    let bare = create_key(&router, &json!({"Description": "no grants at all"})).await;
    let bare_id = text("a key with no grants", &bare, "KeyId");
    let bare_arn = text("a key with no grants", &bare, "Arn");

    let rows: Vec<(&str, &str, &str, Vec<String>)> = vec![
        (
            "the first key",
            &first.key_id,
            &first.key_arn,
            ascending(&first.grants),
        ),
        (
            "the second key",
            &second.key_id,
            &second.key_arn,
            ascending(&second.grants),
        ),
        ("a key with no grants", &bare_id, &bare_arn, Vec::new()),
    ];
    for (case, key_id, key_arn, expected) in rows {
        let reported = listed(&router, case, key_arn, &json!({"KeyId": key_id})).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected only that key's grants {expected:?}, got {reported:?}"
        );
    }

    // Each key minted its own grants, so one key's grant id names no grant of the other
    // — asking for it there selects nothing rather than the grant of its own key.
    for grant_id in &second.grants {
        let reported = listed(
            &router,
            "another key's grant id",
            &first.key_arn,
            &json!({"KeyId": first.key_id, "GrantId": grant_id}),
        )
        .await;
        assert!(
            reported.is_empty(),
            "case another key's grant id: {grant_id} is not a grant of {}, got {reported:?}",
            first.key_arn
        );
    }
}

/// An identifier that resolves to no key of this account and Region is refused, and
/// which refusal it gets follows from WHY it resolves to nothing: an ARN that is no KMS
/// key or alias ARN is `InvalidArnException`, one whose account segment names no account
/// at all is an authorization failure, and a well-formed identifier that simply names no
/// such key is `NotFoundException`.
#[tokio::test]
async fn an_identifier_that_names_no_key_is_refused() {
    let router = router();
    let account = engine::emulator_account_id();
    let seeded = seed(&router, "setup").await;
    let key_id = seeded.key_id.clone();

    // Rows: a well-formed identifier that names no key this endpoint serves. A key or
    // alias ARN pins the lookup to its own Region and account, so this key's own ARN in
    // another Region or another account names no key here either.
    let not_found: Vec<(&str, String)> = vec![
        (
            "an unknown key id",
            "33333333-3333-4333-3333-333333333333".to_string(),
        ),
        (
            "an unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/mrk-{}", "3".repeat(32)),
        ),
        ("an unknown alias", "alias/no-such-grants".to_string()),
        (
            "an unknown alias ARN",
            alias_arn(REGION, "alias/no-such-grants"),
        ),
        (
            "this key's ARN in another Region",
            format!("arn:aws:kms:us-west-2:{account}:key/{key_id}"),
        ),
        (
            "this key's ARN in another account",
            format!("arn:aws:kms:{REGION}:999999999999:key/{key_id}"),
        ),
    ];
    for (case, identifier) in not_found {
        let (status, code) = failure(&router, "ListGrants", &json!({"KeyId": identifier})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {case}: expected HTTP 400 NotFoundException, got {status} {code}"
        );
    }

    // Rows: an identifier that is an ARN but no KMS key or alias ARN at all, which this
    // operation models its own error for.
    let malformed: Vec<(&str, String)> = vec![
        (
            "an unknown resource type",
            format!("arn:aws:kms:{REGION}:{account}:thing/{key_id}"),
        ),
        ("another service's ARN", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an empty key id",
            format!("arn:aws:kms:{REGION}:{account}:key/"),
        ),
        (
            "no Region segment",
            format!("arn:aws:kms::{account}:key/{key_id}"),
        ),
        ("nothing but the prefix", "arn:".to_string()),
    ];
    for (case, identifier) in malformed {
        let (status, code) = failure(&router, "ListGrants", &json!({"KeyId": identifier})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArnException"),
            "case {case}: expected HTTP 400 InvalidArnException, got {status} {code}"
        );
    }

    // An ARN whose account segment is not an account id names no account, so no
    // account's permission can cover the key it names: the refusal is the authorization
    // failure, naming the caller and this operation and nothing about the resource.
    for bad_account in ["notanumericaccount", "11111111111", "00000000000a"] {
        let identifier = format!("arn:aws:kms:{REGION}:{bad_account}:key/{key_id}");
        let (status, code, message) =
            refused(&router, REGION, "ListGrants", &json!({"KeyId": identifier})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "AccessDeniedException"),
            "case {identifier}: expected HTTP 400 AccessDeniedException, got {status} {code}"
        );
        assert_eq!(
            message,
            format!(
                "User: arn:aws:iam::{account}:user/emulator is not authorized to perform: \
                 kms:ListGrants on this resource because the resource does not exist in this \
                 Region, no resource-based policies allow access, or a resource-based policy \
                 explicitly denies access"
            ),
            "case {identifier}: expected the refusal to name the principal and the action, \
             got {message:?}"
        );
    }
}
