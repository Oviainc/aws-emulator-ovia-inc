// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal an operation answers when the resource the request
//! NAMES resolves, but the thing it asked about there — the resource-based policy it
//! reads or edits, the function URL it addresses — is not held.
//!
//! The rule under test: that refusal is `ResourceNotFoundException` at HTTP 404 carrying
//! the resource family's own missing-sub-resource message, NEVER the resource-identifying
//! refusal a request whose own resource does not resolve gets (`Function not found:
//! <arn>`, `Layer version <layer-version-arn> does not exist.`). The two are therefore
//! distinguishable, which is what a client reconciling grants branches on: "this function
//! holds no policy yet" is not "this function does not exist", and the first is the state
//! every add-permission/get-policy bootstrap starts from.
//!
//! For a function's policy READ and its URL that message is one fixed sentence repeating
//! nothing from the request, asserted here exactly; a function-policy REMOVAL with no
//! policy to act on reports the missing POLICY in its own fixed sentence, also repeating
//! nothing from the request. The layer-version family words its own two no-policy states
//! (a version nobody granted anything on, and one whose statements have all been removed),
//! owned by those handlers and their suite, so what the rows below pin for a layer version
//! is the distinction itself rather than the wording.
//!
//! `GetPolicy` is the one reader that does NOT distinguish them, and the rows say so: it is
//! asked for a document, and a pair that resolves to nothing has no document either, so it
//! answers the same fixed sentence and names nothing of the request — which is what the
//! recorded service answers for a function it does not hold (corpus
//! `simparity-wire-aws-v1`, `lambda-snx-traj-policy-side-reads-and-identifier-mistakes`).
//! The distinction for the function-policy family is therefore pinned on the REMOVAL,
//! which answers for the policy it was asked to change.
//!
//! The rows differ along what the rule generalises over: the operation (reading a policy,
//! revoking a statement from one, reading a function URL), the resource kind (a function's
//! unpublished version, a published version, an alias, a layer version), the form the
//! request names it in (a bare name, a `Qualifier` query parameter, a `name:qualifier`),
//! and whether the resource resolves at all. Every row drives real HTTP requests through
//! the router, sharing the harness in `function_lifecycle.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, function_arn, json_call, package_bytes, router_on, zip_file, Answer, ACCOUNT,
    REGION, ROLE,
};

/// The one sentence a refusal carries when the FUNCTION the request named resolves but
/// holds neither the policy nor the URL asked about: `ResourceNotFoundException`'s own
/// wording, repeating nothing from the request. Spelled out here rather than read from
/// `state::RESOURCE_NOT_FOUND_MESSAGE`, so a change to that constant fails this suite.
const MISSING_SUBRESOURCE_MESSAGE: &str = "The resource you requested does not exist.";

/// The one sentence a REMOVAL carries when the resource resolves but holds no policy for
/// it to act on: AWS reports the missing POLICY there, while the READ of that policy
/// reports the missing resource ([`MISSING_SUBRESOURCE_MESSAGE`]). Spelled out here rather
/// than read from `state::NO_POLICY_MESSAGE`, so a change to that constant fails this
/// suite.
const NO_POLICY_MESSAGE: &str = "No policy is associated with the given resource.";

/// The error code every row expects, and the status it is bound to.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The only action a layer-version grant can name: `LayerPermissionAllowedAction` is
/// constrained to the pattern `^lambda:GetLayerVersion$`
/// (`smithy_query(query="shape",shape="LayerPermissionAllowedAction")`).
const LAYER_ACTION: &str = "lambda:GetLayerVersion";

/// The body of a dispatched answer, for a failure message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The `message` of a wire error, however the protocol spells the member.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// The ARN version `version` of a layer of `name` carries.
fn layer_version_arn(name: &str, version: i64) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}:{version}")
}

/// Create one function, with no policy and no function URL on it.
async fn create_plain_function(router: &MultiServiceRouter, name: &str) {
    let answer = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: expected 201 from CreateFunction, got {} {}",
        answer.status,
        body_text(&answer)
    );
}

/// `PublishVersion` on a function, which makes version 1 of a freshly created one.
async fn publish_version(router: &MultiServiceRouter, name: &str) {
    let uri = format!("/2015-03-31/functions/{name}/versions");
    let answer = json_call(router, "POST", &uri, &json!({})).await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: expected 201 from PublishVersion, got {} {}",
        answer.status,
        body_text(&answer)
    );
}

/// `CreateAlias` naming one published version.
async fn create_alias(router: &MultiServiceRouter, name: &str, alias: &str, version: &str) {
    let uri = format!("/2015-03-31/functions/{name}/aliases");
    let answer = json_call(
        router,
        "POST",
        &uri,
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {alias:?}: expected 201 from CreateAlias, got {} {}",
        answer.status,
        body_text(&answer)
    );
}

/// `PublishLayerVersion`, which makes version 1 of a layer nothing has published yet.
async fn publish_layer_version(router: &MultiServiceRouter, layer: &str) {
    let uri = format!("/2018-10-31/layers/{layer}/versions");
    let answer = json_call(
        router,
        "POST",
        &uri,
        &json!({ "Content": { "ZipFile": zip_file(&package_bytes(72)) } }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {layer:?}: expected 201 from PublishLayerVersion, got {} {}",
        answer.status,
        body_text(&answer)
    );
}

/// `GetPolicy` on a function reference, optionally carrying a `Qualifier`.
async fn get_policy(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri = format!("/2015-03-31/functions/{reference}/policy{query}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `AddPermission` on a function reference.
async fn add_permission(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/policy");
    json_call(router, "POST", &uri, body).await
}

/// `RemovePermission` of one statement id on a function reference.
async fn remove_permission(
    router: &MultiServiceRouter,
    reference: &str,
    statement_id: &str,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/policy/{statement_id}");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// `GetLayerVersionPolicy` on a layer reference and a version number.
async fn get_layer_version_policy(
    router: &MultiServiceRouter,
    layer: &str,
    version: i64,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{layer}/versions/{version}/policy");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `AddLayerVersionPermission` on a layer version.
async fn add_layer_version_permission(
    router: &MultiServiceRouter,
    layer: &str,
    version: i64,
    body: &Value,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{layer}/versions/{version}/policy");
    json_call(router, "POST", &uri, body).await
}

/// `RemoveLayerVersionPermission` of one statement id on a layer version.
async fn remove_layer_version_permission(
    router: &MultiServiceRouter,
    layer: &str,
    version: i64,
    statement_id: &str,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{layer}/versions/{version}/policy/{statement_id}");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// Which refusal a row must get, and how tightly this suite pins its wording.
enum Refusal {
    /// The resource resolves and holds neither the policy nor the URL asked about, and the
    /// refusal is the function family's fixed sentence, exactly.
    MissingSubresource,
    /// The resource resolves and holds no policy, and the call is a REMOVAL, so what it
    /// reports missing is the POLICY it was asked to change: AWS's own sentence for that,
    /// [`NO_POLICY_MESSAGE`], which is not the one the READ of that policy answers.
    MissingPolicyOnRemoval,
    /// The resource resolves and holds no policy, and the wording is the layer-version
    /// family's own: the row pins only that it differs from the refusal an absent layer
    /// version gets, which is the distinction the rule is about.
    MissingSubresourceWorded,
    /// The resource itself does not resolve, so the refusal has to identify it.
    Unresolvable,
}

/// One row: a call that must be refused 404, and which refusal it must get.
struct Case {
    name: &'static str,
    method: &'static str,
    uri: String,
    /// The resource family the row belongs to, which pairs a missing-sub-resource row with
    /// the unresolvable row addressing the same kind of resource.
    kind: &'static str,
    /// The text identifying the resource the request named — the ARN of the function,
    /// version, alias or layer version it addressed, or the `<layer>:<n>` a layer refusal
    /// names. A refusal for a resource that does not resolve has to carry it; the function
    /// family's missing-sub-resource refusal must not.
    identifier: String,
    refusal: Refusal,
}

/// The resource families the rows span, each with rows in both groups.
const FUNCTION_POLICY: &str = "a function's resource policy";
const LAYER_POLICY: &str = "a layer version's resource policy";
const FUNCTION_URL: &str = "a function's URL";

/// The names the table's resources carry. `POLICY_FREE` is created, published and
/// aliased, so each of its three qualifiers resolves while holding no policy.
const POLICY_FREE: &str = "policy-free";
const URL_LESS: &str = "url-less";
const LAYER: &str = "shared-layer";
const ALIAS: &str = "live";
const UNKNOWN_FUNCTION: &str = "no-such-function";
const UNKNOWN_LAYER: &str = "no-such-layer";

/// Build the resources the rows address.
async fn seed(router: &MultiServiceRouter) {
    create_plain_function(router, POLICY_FREE).await;
    publish_version(router, POLICY_FREE).await;
    create_alias(router, POLICY_FREE, ALIAS, "1").await;
    create_plain_function(router, URL_LESS).await;
    publish_layer_version(router, LAYER).await;
}

/// The rows, in the two groups the rule distinguishes.
fn cases() -> Vec<Case> {
    let version_arn = format!("{}:1", function_arn(POLICY_FREE));
    let alias_arn = format!("{}:{ALIAS}", function_arn(POLICY_FREE));
    vec![
        // The resource resolves; what is missing is the policy, or the function URL.
        Case {
            name: "GetPolicy on a function that holds no policy",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy"),
            kind: FUNCTION_POLICY,
            identifier: function_arn(POLICY_FREE),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "GetPolicy on a published version that holds no policy",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy?Qualifier=1"),
            kind: FUNCTION_POLICY,
            identifier: version_arn.clone(),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "GetPolicy on a version named in the FunctionName",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}:1/policy"),
            kind: FUNCTION_POLICY,
            identifier: version_arn.clone(),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "GetPolicy on an alias that holds no policy",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy?Qualifier={ALIAS}"),
            kind: FUNCTION_POLICY,
            identifier: alias_arn.clone(),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "RemovePermission on a function that holds no policy",
            method: "DELETE",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy/never-granted"),
            kind: FUNCTION_POLICY,
            identifier: function_arn(POLICY_FREE),
            refusal: Refusal::MissingPolicyOnRemoval,
        },
        Case {
            name: "RemovePermission on a published version that holds no policy",
            method: "DELETE",
            uri: format!(
                "/2015-03-31/functions/{POLICY_FREE}/policy/never-granted?Qualifier=1"
            ),
            kind: FUNCTION_POLICY,
            identifier: version_arn.clone(),
            refusal: Refusal::MissingPolicyOnRemoval,
        },
        Case {
            name: "GetLayerVersionPolicy on a layer version that holds no policy",
            method: "GET",
            uri: format!("/2018-10-31/layers/{LAYER}/versions/1/policy"),
            kind: LAYER_POLICY,
            identifier: layer_version_arn(LAYER, 1),
            refusal: Refusal::MissingSubresourceWorded,
        },
        Case {
            name: "RemoveLayerVersionPermission on a layer version that holds no policy",
            method: "DELETE",
            uri: format!("/2018-10-31/layers/{LAYER}/versions/1/policy/never-granted"),
            kind: LAYER_POLICY,
            identifier: layer_version_arn(LAYER, 1),
            refusal: Refusal::MissingSubresourceWorded,
        },
        Case {
            name: "GetFunctionUrlConfig on a function that holds no URL",
            method: "GET",
            uri: format!("/2021-10-31/functions/{URL_LESS}/url"),
            kind: FUNCTION_URL,
            identifier: function_arn(URL_LESS),
            refusal: Refusal::MissingSubresource,
        },
        // The resource itself does not resolve, so the refusal identifies it — except on
        // the policy READ, which is asked for a document and has none to report either
        // way, so it answers its one fixed sentence and identifies nothing.
        Case {
            name: "GetPolicy on a function that does not exist",
            method: "GET",
            uri: format!("/2015-03-31/functions/{UNKNOWN_FUNCTION}/policy"),
            kind: FUNCTION_POLICY,
            identifier: function_arn(UNKNOWN_FUNCTION),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "GetPolicy on a version that does not exist",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy?Qualifier=9"),
            kind: FUNCTION_POLICY,
            identifier: format!("{}:9", function_arn(POLICY_FREE)),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "GetPolicy on an alias that does not exist",
            method: "GET",
            uri: format!("/2015-03-31/functions/{POLICY_FREE}/policy?Qualifier=ghost"),
            kind: FUNCTION_POLICY,
            identifier: format!("{}:ghost", function_arn(POLICY_FREE)),
            refusal: Refusal::MissingSubresource,
        },
        Case {
            name: "RemovePermission on a function that does not exist",
            method: "DELETE",
            uri: format!("/2015-03-31/functions/{UNKNOWN_FUNCTION}/policy/never-granted"),
            kind: FUNCTION_POLICY,
            identifier: function_arn(UNKNOWN_FUNCTION),
            refusal: Refusal::Unresolvable,
        },
        Case {
            name: "GetLayerVersionPolicy on a layer that does not exist",
            method: "GET",
            uri: format!("/2018-10-31/layers/{UNKNOWN_LAYER}/versions/1/policy"),
            kind: LAYER_POLICY,
            identifier: format!("{UNKNOWN_LAYER}:1"),
            refusal: Refusal::Unresolvable,
        },
        Case {
            name: "GetLayerVersionPolicy on a layer version that does not exist",
            method: "GET",
            uri: format!("/2018-10-31/layers/{LAYER}/versions/9/policy"),
            kind: LAYER_POLICY,
            identifier: format!("{LAYER}:9"),
            refusal: Refusal::Unresolvable,
        },
        Case {
            name: "RemoveLayerVersionPermission on a layer version that does not exist",
            method: "DELETE",
            uri: format!("/2018-10-31/layers/{LAYER}/versions/9/policy/never-granted"),
            kind: LAYER_POLICY,
            identifier: format!("{LAYER}:9"),
            refusal: Refusal::Unresolvable,
        },
        Case {
            name: "GetFunctionUrlConfig on a function that does not exist",
            method: "GET",
            uri: format!("/2021-10-31/functions/{UNKNOWN_FUNCTION}/url"),
            kind: FUNCTION_URL,
            identifier: function_arn(UNKNOWN_FUNCTION),
            refusal: Refusal::Unresolvable,
        },
    ]
}

/// A resource that resolves but holds no policy is refused for its POLICY, and a resource
/// that does not resolve with the refusal that identifies it — so a caller can tell the
/// two apart, in every resource family.
#[tokio::test]
async fn a_missing_policy_is_refused_differently_from_a_missing_resource() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    seed(&router).await;

    // The first refusal each family answers in each group, so the rule itself — the two
    // differ — is asserted per family once every row has run.
    let mut missing: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut unresolvable: BTreeMap<&'static str, String> = BTreeMap::new();

    for case in cases() {
        let answer = json_call(&router, case.method, &case.uri, &Value::Null).await;
        assert_eq!(
            answer.status,
            NOT_FOUND_STATUS,
            "case {:?}: expected HTTP {NOT_FOUND_STATUS}, got {} {}",
            case.name,
            answer.status,
            body_text(&answer)
        );
        assert!(
            answer.error_code().contains(NOT_FOUND_CODE),
            "case {:?}: expected {NOT_FOUND_CODE}, got {}",
            case.name,
            answer.error_code()
        );

        let message = message_of(&answer);
        match case.refusal {
            Refusal::MissingSubresource => {
                assert_eq!(
                    message, MISSING_SUBRESOURCE_MESSAGE,
                    "case {:?}: expected the fixed sentence {MISSING_SUBRESOURCE_MESSAGE:?}, got {message:?}",
                    case.name
                );
                assert!(
                    !message.contains(&case.identifier),
                    "case {:?}: the refusal must repeat nothing from the request, but it names \
                     {:?}: {message:?}",
                    case.name,
                    case.identifier
                );
                missing.entry(case.kind).or_insert(message);
            }
            Refusal::MissingPolicyOnRemoval => {
                assert_eq!(
                    message, NO_POLICY_MESSAGE,
                    "case {:?}: expected the fixed sentence {NO_POLICY_MESSAGE:?}, got {message:?}",
                    case.name
                );
                assert!(
                    !message.contains(&case.identifier),
                    "case {:?}: the refusal must repeat nothing from the request, but it names \
                     {:?}: {message:?}",
                    case.name,
                    case.identifier
                );
                missing.entry(case.kind).or_insert(message);
            }
            Refusal::MissingSubresourceWorded => {
                assert!(
                    !message.is_empty(),
                    "case {:?}: expected a refusal message, got {}",
                    case.name,
                    body_text(&answer)
                );
                missing.entry(case.kind).or_insert(message);
            }
            Refusal::Unresolvable => {
                assert!(
                    message.contains(&case.identifier),
                    "case {:?}: expected a refusal naming {:?}, got {message:?}",
                    case.name,
                    case.identifier
                );
                unresolvable.entry(case.kind).or_insert(message);
            }
        }
    }

    // The rule, per resource family: the refusal for a resource that resolves and holds no
    // policy is not the one a resource that does not resolve gets.
    for (kind, missing_message) in &missing {
        let identifying = unresolvable.get(kind).unwrap_or_else(|| {
            panic!("case {kind:?}: the table must carry an unresolvable row for every family")
        });
        assert_ne!(
            missing_message, identifying,
            "case {kind:?}: a resource that holds no policy must not be refused the way one \
             that does not resolve is, but both answered {missing_message:?}"
        );
    }
}

/// The same function tells the two refusals apart over its own lifetime: it holds no
/// policy, then holds one, then holds none again once the last statement is revoked — and
/// the READ answers the fixed sentence throughout, whether the function exists or not,
/// while the REMOVAL answers for the policy it was asked to change.
#[tokio::test]
async fn a_functions_policy_refusal_follows_whether_it_holds_a_policy() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_plain_function(&router, "reconciled").await;

    let before = get_policy(&router, "reconciled", None).await;
    assert_eq!(
        (before.status, message_of(&before)),
        (NOT_FOUND_STATUS, MISSING_SUBRESOURCE_MESSAGE.to_string()),
        "case {:?}: expected HTTP {NOT_FOUND_STATUS} and the fixed sentence, got {} {}",
        "before the grant",
        before.status,
        body_text(&before)
    );

    // A function that does not exist, read at the same moment: the READ is asked for a
    // document, and there is none either way, so it is the SAME fixed sentence and names
    // nothing of the request. The removal below is what tells the two states apart.
    let unknown = get_policy(&router, UNKNOWN_FUNCTION, None).await;
    assert_eq!(
        (unknown.status, message_of(&unknown)),
        (NOT_FOUND_STATUS, MISSING_SUBRESOURCE_MESSAGE.to_string()),
        "case {:?}: expected HTTP {NOT_FOUND_STATUS} and the fixed sentence, got {} {}",
        "an unknown function at the same moment",
        unknown.status,
        body_text(&unknown)
    );
    assert!(
        !message_of(&unknown).contains(&function_arn(UNKNOWN_FUNCTION)),
        "case {:?}: the sentence is fixed, so it may not name the function, got {}",
        "an unknown function at the same moment",
        body_text(&unknown)
    );

    let granted = add_permission(
        &router,
        "reconciled",
        &json!({
            "StatementId": "ses-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "ses.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "case {:?}: expected 201 from AddPermission, got {} {}",
        "the grant",
        granted.status,
        body_text(&granted)
    );

    let read = get_policy(&router, "reconciled", None).await;
    assert_eq!(
        read.status,
        200,
        "case {:?}: expected 200 once a policy exists, got {} {}",
        "after the grant",
        read.status,
        body_text(&read)
    );
    assert!(
        read.string("Policy")
            .is_some_and(|policy| policy.contains("ses-invoke")),
        "case {:?}: expected the granted statement in the document, got {}",
        "after the grant",
        body_text(&read)
    );

    let revoked = remove_permission(&router, "reconciled", "ses-invoke").await;
    assert_eq!(
        revoked.status,
        204,
        "case {:?}: expected 204 from RemovePermission, got {} {}",
        "the revocation",
        revoked.status,
        body_text(&revoked)
    );

    // The statement was the only one, so the function holds no policy again — and the
    // refusal is the one for a resource that resolves, not the one for a missing function:
    // the READ reports the resource it was asked about, the REMOVAL reports the policy it
    // was asked to change.
    for (case, expected) in [
        ("after the revocation", MISSING_SUBRESOURCE_MESSAGE),
        ("revoking it a second time", NO_POLICY_MESSAGE),
    ] {
        let answer = if case == "after the revocation" {
            get_policy(&router, "reconciled", None).await
        } else {
            remove_permission(&router, "reconciled", "ses-invoke").await
        };
        assert_eq!(
            (answer.status, message_of(&answer)),
            (NOT_FOUND_STATUS, expected.to_string()),
            "case {case:?}: expected HTTP {NOT_FOUND_STATUS} and {expected:?}, got {} {}",
            answer.status,
            body_text(&answer)
        );
        assert!(
            !message_of(&answer).contains(&function_arn("reconciled")),
            "case {case:?}: the refusal must repeat nothing from the request, got {}",
            body_text(&answer)
        );
    }
}

/// The same lifetime on a layer version: no policy, a grant, then no policy again. The
/// layer family words its own no-policy refusals, so what is asserted here is that neither
/// of them is the refusal an ABSENT layer version gets.
#[tokio::test]
async fn a_layer_versions_policy_refusal_follows_whether_it_holds_a_policy() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    publish_layer_version(&router, "reconciled-layer").await;

    // The refusal a version that does not exist gets, read at the same moment, is what
    // every no-policy refusal below has to differ from.
    let absent = get_layer_version_policy(&router, "reconciled-layer", 9).await;
    let absent_message = message_of(&absent);
    assert!(
        absent.status == NOT_FOUND_STATUS && absent_message.contains("reconciled-layer:9"),
        "case {:?}: expected HTTP {NOT_FOUND_STATUS} naming the version, got {} {}",
        "a layer version that does not exist",
        absent.status,
        body_text(&absent)
    );

    let before = get_layer_version_policy(&router, "reconciled-layer", 1).await;
    assert!(
        before.status == NOT_FOUND_STATUS && message_of(&before) != absent_message,
        "case {:?}: expected HTTP {NOT_FOUND_STATUS} refused for the policy rather than as \
         {absent_message:?}, got {} {}",
        "before the grant",
        before.status,
        body_text(&before)
    );

    let granted = add_layer_version_permission(
        &router,
        "reconciled-layer",
        1,
        &json!({
            "StatementId": "share-with-account",
            "Action": LAYER_ACTION,
            "Principal": "123456789012",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "case {:?}: expected 201 from AddLayerVersionPermission, got {} {}",
        "the grant",
        granted.status,
        body_text(&granted)
    );

    let read = get_layer_version_policy(&router, "reconciled-layer", 1).await;
    assert_eq!(
        read.status,
        200,
        "case {:?}: expected 200 once a policy exists, got {} {}",
        "after the grant",
        read.status,
        body_text(&read)
    );

    let revoked =
        remove_layer_version_permission(&router, "reconciled-layer", 1, "share-with-account").await;
    assert_eq!(
        revoked.status,
        204,
        "case {:?}: expected 204 from RemoveLayerVersionPermission, got {} {}",
        "the revocation",
        revoked.status,
        body_text(&revoked)
    );

    let after = get_layer_version_policy(&router, "reconciled-layer", 1).await;
    assert!(
        after.status == NOT_FOUND_STATUS && message_of(&after) != absent_message,
        "case {:?}: expected HTTP {NOT_FOUND_STATUS} refused for the policy rather than as \
         {absent_message:?}, got {} {}",
        "after the revocation",
        after.status,
        body_text(&after)
    );
}
