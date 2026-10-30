// SPDX-License-Identifier: CC0-1.0

//! SYSTEM-TAG mutations, end to end over the `awsJson1_1` wire.
//!
//! THE RULE: a tag key in the namespace Amazon Web Services reserves for the tags it
//! applies ITSELF (`state::SYSTEM_TAG_KEY_PREFIX`) names a service-owned tag, which an end
//! user may neither write nor remove — so every operation of this service that takes
//! caller-supplied tags refuses a request naming one with the modelled `ValidationException`
//! (`smithy_query(query="operation",operation="TagResource,UntagResource")` lists it,
//! `errorKind` client, `httpError` 400) carrying `state::SYSTEM_TAG_MUTATION_MESSAGE`, and
//! refuses it BEFORE any state change: the whole request is refused, so a system key named
//! beside ordinary ones stores and removes none of them, and a create that carries one
//! leaves no resource behind. The sentence is FIXED for the error class — it interpolates
//! nothing from the request, naming neither the key nor the resource.
//!
//! The reservation is a PREFIX on the key, matched however the caller spells its case, so a
//! key that merely contains the prefix later on, or starts with the letters and no colon, is
//! the caller's own and is stored, read back and removed like any other.
//!
//! The rows span what the rule generalises over: what FOLLOWS the prefix (nothing, a real
//! AWS service namespace, one character, letters outside ASCII, a key at `TagKey`'s
//! 128-character upper bound), the CASE the prefix is spelled in, the POSITION of the system
//! key in the request (alone, first, last), the OPERATION (both tagging writes and every
//! create that carries `Tags`), and the lookalikes that are NOT reserved. The recorded case
//! (one `TagResource` and one `UntagResource` naming a service-owned key) is two rows of it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes; the harness is the one `ssoadmin_permission_sets.rs` owns. `resource_tags.rs`
//! and `resource_untagging.rs` own the tagging surface's other rules, and
//! `tags_from_creates.rs` the tags a create persists when it is served.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, empty_account_router, error_code, organization_router, router, seeded_instance_arn,
};

/// The provider a customer managed application is created under: the `custom` entry of the
/// published catalog (`state::BUILT_IN_APPLICATION_PROVIDERS`), the one `CreateApplication`
/// serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// An ordinary tag key the fixtures carry, so a refused write can be shown to have left a
/// real tag set alone.
const ORDINARY_KEY: &str = "Owner";

/// One tag key the rule is asserted over: whether it is in the reserved namespace.
struct KeyCase {
    label: &'static str,
    key: String,
    system: bool,
}

/// The rows, built FROM the prefix constant rather than written out, so they cannot drift
/// from the namespace the service reserves.
///
/// The keys stay inside `TagKey`'s own constraints
/// (`smithy_query(query="shape",shape="TagKey")`: `length 1..128`, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which admits the colon the prefix ends with, letters
/// of any script and the separator a padded row uses), so every row reaches the handler
/// rather than the boundary validator.
fn key_cases() -> Vec<KeyCase> {
    let prefix = state::SYSTEM_TAG_KEY_PREFIX;
    let letters = prefix.trim_end_matches(':');
    vec![
        KeyCase {
            label: "the reserved prefix with nothing after it",
            key: prefix.to_string(),
            system: true,
        },
        KeyCase {
            label: "a real AWS service namespace under the prefix",
            key: format!("{prefix}cloudformation:stack-name"),
            system: true,
        },
        KeyCase {
            label: "one character after the prefix",
            key: format!("{prefix}x"),
            system: true,
        },
        KeyCase {
            label: "letters outside ASCII after the prefix",
            key: format!("{prefix}\u{41f}\u{440}\u{43e}\u{435}\u{43a}\u{442}"),
            system: true,
        },
        KeyCase {
            label: "the prefix in upper case",
            key: prefix.to_uppercase(),
            system: true,
        },
        KeyCase {
            label: "the prefix in mixed case, with a segment after it",
            key: "Aws:servicecatalog:provisioningPrincipalArn".to_string(),
            system: true,
        },
        KeyCase {
            label: "a reserved key at TagKey's 128-character upper bound",
            key: format!("{prefix}{}", "k".repeat(128 - prefix.chars().count())),
            system: true,
        },
        KeyCase {
            label: "the reserved letters without the colon",
            key: letters.to_string(),
            system: false,
        },
        KeyCase {
            label: "the reserved letters as the start of a longer word",
            key: format!("{letters}ome-team"),
            system: false,
        },
        KeyCase {
            label: "the prefix later in the key",
            key: format!("my-{prefix}thing"),
            system: false,
        },
        KeyCase {
            label: "the prefix after a separator",
            key: format!(" {prefix}thing"),
            system: false,
        },
        KeyCase {
            label: "a colon-bearing key of the caller's own",
            key: "cost-center:eu".to_string(),
            system: false,
        },
    ]
}

/// The `Message` an error body carries, whichever member the renderer put it in.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The `Tags` `ListTagsForResource` reports for `resource_arn`.
async fn tags_of(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"ResourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200 for {resource_arn}, got \
         {status} {body}"
    );
    body["Tags"].clone()
}

/// Assert that `body` is the refusal this rule answers with, at `status`.
fn assert_system_tag_refusal(case: &str, what: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: {what} must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: {what} expected ValidationException, got {body}"
    );
    assert_eq!(
        message_of(body),
        state::SYSTEM_TAG_MUTATION_MESSAGE,
        "case {case:?}: {what} expected the fixed system-tag sentence, got {body}"
    );
    assert!(
        !message_of(body).contains(state::SYSTEM_TAG_KEY_PREFIX),
        "case {case:?}: {what} must interpolate nothing from the request, got {}",
        message_of(body)
    );
}

/// THE RULE for `TagResource`: a request naming a system key is refused with the fixed
/// sentence and writes nothing — whatever else the same request carries, and wherever the
/// system key stands in it.
#[tokio::test]
async fn tag_resource_refuses_a_system_tag_key_and_writes_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    // One ordinary tag already on the resource, so a refused write can be shown to have
    // left a real tag set exactly as it was.
    let (status, seeded) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": instance_arn,
            "Tags": [{"Key": ORDINARY_KEY, "Value": "platform"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seeding TagResource expected 200, got {status} {seeded}"
    );

    for row in key_cases().into_iter().filter(|row| row.system) {
        // The positions a system key can take in one request: alone, before an ordinary
        // key, and after one.
        let ordinary = json!({"Key": "Tier", "Value": "test"});
        let reserved = json!({"Key": row.key, "Value": "owned"});
        let shapes = [
            ("alone", json!([reserved])),
            ("before an ordinary key", json!([reserved, ordinary])),
            ("after an ordinary key", json!([ordinary, reserved])),
        ];
        for (position, tags) in shapes {
            let case = format!("{} / {position}", row.label);
            let before = tags_of(&router, &case, &instance_arn).await;
            let (status, refused) = call(
                &router,
                "TagResource",
                &json!({"ResourceArn": instance_arn, "Tags": tags}),
            )
            .await;
            assert_system_tag_refusal(&case, "TagResource", status, &refused);

            let after = tags_of(&router, &case, &instance_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused tag write must store nothing, got {after} vs \
                 {before}"
            );
        }
    }
}

/// THE RULE for `UntagResource`: the same guard on the keys a removal names, so a protected
/// tag cannot be removed either — and the ordinary keys of the same request survive with it.
#[tokio::test]
async fn untag_resource_refuses_a_system_tag_key_and_removes_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let (status, seeded) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": instance_arn,
            "Tags": [
                {"Key": ORDINARY_KEY, "Value": "platform"},
                {"Key": "Tier", "Value": "test"},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seeding TagResource expected 200, got {status} {seeded}"
    );

    for row in key_cases().into_iter().filter(|row| row.system) {
        let shapes = [
            ("alone", json!([row.key])),
            ("before an ordinary key", json!([row.key, ORDINARY_KEY])),
            ("after an ordinary key", json!([ORDINARY_KEY, row.key])),
        ];
        for (position, tag_keys) in shapes {
            let case = format!("{} / {position}", row.label);
            let before = tags_of(&router, &case, &instance_arn).await;
            let (status, refused) = call(
                &router,
                "UntagResource",
                &json!({"ResourceArn": instance_arn, "TagKeys": tag_keys}),
            )
            .await;
            assert_system_tag_refusal(&case, "UntagResource", status, &refused);

            let after = tags_of(&router, &case, &instance_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused removal must remove nothing, got {after} vs \
                 {before}"
            );
        }
    }
}

/// The other side of the prefix rule: a key that only LOOKS reserved is the caller's own, so
/// both writes serve it — it is stored, read back and removed like any other tag.
#[tokio::test]
async fn a_key_that_only_looks_reserved_is_served_by_both_writes() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for row in key_cases().into_iter().filter(|row| !row.system) {
        let case = row.label;
        let (status, written) = call(
            &router,
            "TagResource",
            &json!({
                "ResourceArn": instance_arn,
                "Tags": [{"Key": row.key, "Value": "mine"}],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ordinary key must be served, got {status} {written}"
        );
        // The ROUND TRIP: the read reports what the write stored.
        let stored = tags_of(&router, case, &instance_arn).await;
        assert!(
            stored
                .as_array()
                .is_some_and(|tags| tags.contains(&json!({"Key": row.key, "Value": "mine"}))),
            "case {case:?}: the stored tag must be readable back, got {stored}"
        );

        let (status, removed) = call(
            &router,
            "UntagResource",
            &json!({"ResourceArn": instance_arn, "TagKeys": [row.key]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing an ordinary key must be served, got {status} {removed}"
        );
        let left = tags_of(&router, case, &instance_arn).await;
        assert!(
            left.as_array()
                .is_some_and(|tags| !tags.iter().any(|tag| tag["Key"] == json!(row.key))),
            "case {case:?}: the removal must have taken the key away, got {left}"
        );
    }
}

/// The same guard on every CREATE that takes caller-supplied `Tags`: the create is refused
/// with the same fixed sentence and mints NO resource, so a system tag cannot be attached
/// at creation time either.
#[tokio::test]
async fn every_create_that_carries_tags_refuses_a_system_tag_key() {
    // One reserved key per row shape, from the same table.
    for row in key_cases().into_iter().filter(|row| row.system) {
        let tags = json!([
            {"Key": "Tier", "Value": "test"},
            {"Key": row.key, "Value": "owned"},
        ]);

        // The three creates that hold a resource under an instance, on an instance that
        // serves all of them.
        let (router, instance_arn) = organization_router();
        let creates: Vec<(&str, Value, &str, &str)> = vec![
            (
                "CreatePermissionSet",
                json!({"InstanceArn": instance_arn, "Name": "Refused", "Tags": tags}),
                "ListPermissionSets",
                "PermissionSets",
            ),
            (
                "CreateApplication",
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": PROVIDER_CUSTOM,
                    "Name": "refused-application",
                    "Tags": tags,
                }),
                "ListApplications",
                "Applications",
            ),
            (
                "CreateTrustedTokenIssuer",
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "refused-issuer",
                    "TrustedTokenIssuerType": ISSUER_TYPE,
                    "TrustedTokenIssuerConfiguration": {
                        "OidcJwtConfiguration": {
                            "IssuerUrl": "https://issuer.example.com/oidc",
                            "ClaimAttributePath": "email",
                            "IdentityStoreAttributePath": "emails.primary.value",
                            "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                        }
                    },
                    "Tags": tags,
                }),
                "ListTrustedTokenIssuers",
                "TrustedTokenIssuers",
            ),
        ];
        for (operation, body, reader, member) in creates {
            let case = format!("{} / {operation}", row.label);
            let (status, refused) = call(&router, operation, &body).await;
            assert_system_tag_refusal(&case, operation, status, &refused);

            // And it minted nothing: the instance holds no resource of that kind.
            let (status, listed) =
                call(&router, reader, &json!({"InstanceArn": instance_arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {reader} expected 200, got {status} {listed}"
            );
            assert_eq!(
                listed[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: a refused create must mint no resource, got {listed}"
            );
        }

        // And the create of the INSTANCE itself, whose tags only a create can attach: an
        // account that holds none, so the create is the one the operation is documented for.
        let case = format!("{} / CreateInstance", row.label);
        let router = empty_account_router();
        let (status, refused) = call(
            &router,
            "CreateInstance",
            &json!({"Name": "RefusedInstance", "Tags": tags}),
        )
        .await;
        assert_system_tag_refusal(&case, "CreateInstance", status, &refused);
        let (status, listed) = call(&router, "ListInstances", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {listed}"
        );
        assert_eq!(
            listed["Instances"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused create must enable no instance, got {listed}"
        );
    }
}
