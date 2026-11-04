// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE a Lambda refusal carries.
//!
//! The rule under test: a Lambda refusal's `Message` is the service's own fixed sentence
//! for that condition, and it interpolates only what AWS's own sentence has a slot for —
//! never the resource the request addressed, the precondition value the caller supplied,
//! or a count the service computed:
//!
//! * a resource that does not exist is refused with ONE canonical sentence,
//!   [`CANONICAL_NOT_FOUND`], with nothing appended to it — whatever the resource kind
//!   (layer version, event source mapping, function URL, a resource addressed by its tags,
//!   or a function carrying no resource-based policy) and whichever operation asked. The
//!   exception is an operation whose OWN resource is the one missing where AWS words that
//!   case itself: a function, an alias, a capacity provider, a code signing configuration,
//!   and a layer version addressed through its policy (`missing_policy_refusal.rs` pins
//!   that last distinction);
//! * a CODE SIGNING CONFIGURATION the operation's own `CodeSigningConfigArn` named keeps
//!   its own sentence, [`CODE_SIGNING_CONFIG_GONE`] around the ARN the request carried,
//!   whichever of the operations on the configuration asked;
//! * an ALIAS that does not exist keeps its own sentence, [`ALIAS_NOT_FOUND_PREFIX`]
//!   followed by the alias ARN built from stored state, so a caller can tell
//!   alias-not-found from function-not-found by the leading phrase, and a CAPACITY
//!   PROVIDER keeps its own sentence naming the provider ARN the same way;
//! * a REMOVAL on a resource-based policy that finds no policy to act on reports the
//!   missing POLICY ([`NO_POLICY_REFUSAL`]) rather than the missing resource, which is
//!   what the READ of that policy reports;
//! * a `RevisionId` precondition that did not hold is refused with one of the TWO
//!   sentences `PreconditionFailedException` documents
//!   (`smithy_query(query="doc", shape="PreconditionFailedException")`): `AddPermission`
//!   and `RemovePermission` name `GetPolicy`, every other operation names
//!   `GetFunction`/`GetAlias`; neither names the supplied id or the resource;
//! * a tag write past the published per-resource tag quota states the quota and nothing of
//!   the request;
//! * a `PublishVersion` whose `CodeSha256` is not the current one reports the supplied and
//!   the current digest under the member's `CodeSHA256` spelling, and says to retry with
//!   the one `$LATEST` carries; a package this emulator holds by reference has no digest
//!   to compare against, so that state gets its own refusal rather than the comparison
//!   with an empty slot.
//!
//! Every row drives real HTTP requests through the router, so what is asserted is the
//! wire body. The rows differ along what the rule generalises over: the resource KIND
//! refused, the OPERATION and method that refused it, how the resource was addressed (bare
//! name, partial ARN, full ARN), the value supplied for a precondition, the tag count on
//! each side of the quota, and the package a digest was computed from — a recorded case is
//! one row of each table.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_mapping, create_request, router_on, Answer, MAPPINGS_URI, QUEUE_SOURCE,
};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the capacity-provider operations are bound
/// (`/2025-11-30/capacity-providers/{CapacityProviderName}`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where the tagging operations are bound (`/2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// The region the shared harness's SigV4 scope names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Lambda's one sentence for a resource that does not exist. It is the WHOLE message:
/// nothing is appended to it, and it names neither the resource kind nor the identifier
/// the request carried.
const CANONICAL_NOT_FOUND: &str = "The resource you requested does not exist.";

/// The lead-in of Lambda's own sentence for an alias that does not exist, which the alias
/// ARN follows.
const ALIAS_NOT_FOUND_PREFIX: &str = "Cannot find alias arn: ";

/// The lead-in of Lambda's sentence for a FUNCTION (or a version of one) that does not
/// exist, which is a different phrase from [`ALIAS_NOT_FOUND_PREFIX`].
const FUNCTION_NOT_FOUND_PREFIX: &str = "Function not found: ";

/// The refusal a REMOVAL on a resource-based policy gets when there is no policy for it
/// to act on, which answers for the missing POLICY rather than for the resource.
const NO_POLICY_REFUSAL: &str = "No policy is associated with the given resource.";

/// The refusal `AddPermission` and `RemovePermission` give a `RevisionId` precondition
/// that did not hold: the first branch of the rule `PreconditionFailedException`
/// documents ("For AddPermission and RemovePermission API operations: Call `GetPolicy` to
/// retrieve the latest RevisionId for your resource").
const POLICY_REVISION_REFUSAL: &str = "The Revision Id provided does not match the latest \
     Revision Id. Call the GetPolicy API to retrieve the latest Revision Id";

/// The refusal every OTHER operation gives the same precondition: the second branch of
/// that rule ("For all other API operations: Call `GetFunction` or `GetAlias`").
const CONFIGURATION_REVISION_REFUSAL: &str = "The Revision Id provided does not match the latest \
     Revision Id. Call the GetFunction/GetAlias API to retrieve the latest Revision Id";

/// The refusal a tag write past the quota gets.
const TAG_QUOTA_REFUSAL: &str = "Number of tags exceeds resource tag limit.";

/// The published per-resource tag quota ("Tag Lambda resources": "Maximum number of tags
/// per resource: 50").
const MAX_TAGS: usize = 50;

/// A UUID of the length `UUIDString` requires that no mapping is ever held under here.
const UNHELD_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// A code-signing-config ARN satisfying the model's `CodeSigningConfigArn` pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) that nothing is held under.
const UNHELD_CODE_SIGNING_CONFIG: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0a1b2c3d4e5f60718";

/// Lambda's sentence for a code signing configuration the request's own
/// `CodeSigningConfigArn` names and this account holds none of: the ARN fills the slot in
/// the middle, which is `(prefix, suffix)` here.
const CODE_SIGNING_CONFIG_GONE: (&str, &str) = (
    "The Lambda code signing configuration ",
    " can not be found.",
);

/// The `message` of a wire error, under either casing the JSON envelopes use.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Deployment package bytes of a given length. The content is irrelevant to the API —
/// only its length and its digest are reported — but it differs per length so two
/// packages never share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    json!(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// reports, so no test reads the digest it is checking out of a response.
fn digest_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// Percent-encode the characters an ARN carries that a query string would otherwise read
/// as structure.
fn encoded(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A")
}

/// Create one function, asserting it was stored.
async fn create_function(router: &MultiServiceRouter, name: &str, code: &Value, publish: bool) {
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": code,
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Create one function holding a 64-byte inline package.
async fn create_zip_function(router: &MultiServiceRouter, name: &str, publish: bool) {
    create_function(router, name, &json!({ "ZipFile": zip_file(&package_bytes(64)) }), publish)
        .await;
}

/// Publish one version of a layer, asserting it was stored.
async fn publish_layer_version(router: &MultiServiceRouter, name: &str, length: usize) {
    let answer = call(
        router,
        "POST",
        &format!("/2018-10-31/layers/{name}/versions"),
        &json!({ "Content": { "ZipFile": zip_file(&package_bytes(length)) } }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishLayerVersion {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every instant the clock already owes into the past — a function's activation, an
/// in-flight update's settling, and a deleted mapping's removal — so the next call sees
/// what waiting would have shown it. Nothing sleeps.
fn make_everything_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
    for mapping in store.event_source_mappings.values_mut() {
        if mapping.removal_due.is_some() {
            mapping.removal_due = Some(past);
        }
    }
}

/// A `Tags` map of `count` entries, each key prefixed with `prefix` so two rows can name
/// disjoint sets.
fn tag_map(prefix: &str, count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("{prefix}{index}"), json!(format!("value-{index}")));
    }
    Value::Object(tags)
}

/// `TagResource` on `resource`.
async fn tag_resource(router: &MultiServiceRouter, resource: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{TAGS_URI}/{resource}"),
        &json!({ "Tags": tags }),
    )
    .await
}

/// How many tags `ListTags` reports for `resource`.
async fn tag_count(router: &MultiServiceRouter, resource: &str) -> usize {
    let answer = call(
        router,
        "GET",
        &format!("{TAGS_URI}/{resource}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListTags on {resource:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Tags")
        .and_then(Value::as_object)
        .map_or(0, Map::len)
}

/// One not-found row: the request that must be refused, and the identifier it addressed
/// the absent resource by — which the refusal may not quote.
struct NotFoundCase {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    identifier: String,
}

#[tokio::test]
async fn every_resource_that_does_not_exist_is_refused_with_the_one_canonical_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One function with no policy and no function URL, one with a published version, one
    // whose only policy statement is then removed, one layer holding a version, one layer
    // whose only version is deleted, and one event source mapping that is deleted and then
    // removed by the clock.
    create_zip_function(&router, "rmr-bare", false).await;
    create_zip_function(&router, "rmr-published", true).await;
    create_zip_function(&router, "rmr-emptied-policy", false).await;
    create_zip_function(&router, "rmr-mapping-owner", false).await;
    publish_layer_version(&router, "rmr-held-layer", 64).await;
    publish_layer_version(&router, "rmr-emptied-layer", 96).await;

    let granted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/rmr-emptied-policy/policy"),
        &json!({
            "StatementId": "only-statement",
            "Action": "lambda:InvokeFunction",
            "Principal": "events.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "AddPermission must succeed, got {} {}",
        granted.status,
        granted.text()
    );
    let emptied = call(
        &router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/rmr-emptied-policy/policy/only-statement"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        emptied.status,
        204,
        "RemovePermission must succeed, got {} {}",
        emptied.status,
        emptied.text()
    );

    let deleted_layer = call(
        &router,
        "DELETE",
        "/2018-10-31/layers/rmr-emptied-layer/versions/1",
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted_layer.status,
        204,
        "DeleteLayerVersion must succeed, got {} {}",
        deleted_layer.status,
        deleted_layer.text()
    );
    // `@idempotent`: deleting the version a second time removes nothing and is still a
    // success, which is why the contract is graded on what the READ then answers.
    let deleted_again = call(
        &router,
        "DELETE",
        "/2018-10-31/layers/rmr-emptied-layer/versions/1",
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted_again.status,
        204,
        "a repeated DeleteLayerVersion stays a success, got {} {}",
        deleted_again.status,
        deleted_again.text()
    );

    let created = create_mapping(
        &router,
        &create_request("rmr-mapping-owner", Some(QUEUE_SOURCE), &json!({})),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "CreateEventSourceMapping must succeed, got {} {}",
        created.status,
        created.text()
    );
    let gone_uuid = created
        .string("UUID")
        .expect("the create answers a UUID the mapping is held under");
    let deleted_mapping = call(
        &router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{gone_uuid}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted_mapping.status,
        202,
        "DeleteEventSourceMapping must be accepted, got {} {}",
        deleted_mapping.status,
        deleted_mapping.text()
    );
    make_everything_due(&engine);

    let unheld_version_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:rmr-ghost-layer:3");
    let unheld_mapping_arn =
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:{UNHELD_UUID}");

    let cases = vec![
        NotFoundCase {
            label: "GetLayerVersion on a layer nothing was ever published under",
            method: "GET",
            uri: "/2018-10-31/layers/rmr-never-published/versions/1".to_string(),
            body: Value::Null,
            identifier: "rmr-never-published".to_string(),
        },
        NotFoundCase {
            label: "GetLayerVersion on a version number the layer never had",
            method: "GET",
            uri: "/2018-10-31/layers/rmr-held-layer/versions/9".to_string(),
            body: Value::Null,
            identifier: "rmr-held-layer".to_string(),
        },
        NotFoundCase {
            label: "GetLayerVersion after the layer's only version was deleted",
            method: "GET",
            uri: "/2018-10-31/layers/rmr-emptied-layer/versions/1".to_string(),
            body: Value::Null,
            identifier: "rmr-emptied-layer".to_string(),
        },
        NotFoundCase {
            label: "GetLayerVersion addressing the layer by its ARN",
            method: "GET",
            uri: format!(
                "/2018-10-31/layers/{}/versions/1",
                encoded(&format!(
                    "arn:aws:lambda:{REGION}:{ACCOUNT}:layer:rmr-never-published"
                ))
            ),
            body: Value::Null,
            identifier: "rmr-never-published".to_string(),
        },
        NotFoundCase {
            label: "GetLayerVersionByArn for a layer-version ARN nothing holds",
            method: "GET",
            uri: format!(
                "/2018-10-31/layers?find=LayerVersion&Arn={}",
                encoded(&unheld_version_arn)
            ),
            body: Value::Null,
            identifier: "rmr-ghost-layer".to_string(),
        },
        NotFoundCase {
            label: "GetEventSourceMapping on a UUID nothing was ever minted under",
            method: "GET",
            uri: format!("{MAPPINGS_URI}/{UNHELD_UUID}"),
            body: Value::Null,
            identifier: UNHELD_UUID.to_string(),
        },
        NotFoundCase {
            label: "GetEventSourceMapping on a mapping the clock removed",
            method: "GET",
            uri: format!("{MAPPINGS_URI}/{gone_uuid}"),
            body: Value::Null,
            identifier: gone_uuid.clone(),
        },
        NotFoundCase {
            label: "DeleteEventSourceMapping on a UUID nothing was ever minted under",
            method: "DELETE",
            uri: format!("{MAPPINGS_URI}/{UNHELD_UUID}"),
            body: Value::Null,
            identifier: UNHELD_UUID.to_string(),
        },
        NotFoundCase {
            label: "GetPolicy on a function that never carried a statement",
            method: "GET",
            uri: format!("{FUNCTIONS_URI}/rmr-bare/policy"),
            body: Value::Null,
            identifier: "rmr-bare".to_string(),
        },
        NotFoundCase {
            label: "GetPolicy on a function whose last statement was removed",
            method: "GET",
            uri: format!("{FUNCTIONS_URI}/rmr-emptied-policy/policy"),
            body: Value::Null,
            identifier: "rmr-emptied-policy".to_string(),
        },
        NotFoundCase {
            label: "GetPolicy on a published version carrying no policy",
            method: "GET",
            uri: format!("{FUNCTIONS_URI}/rmr-published/policy?Qualifier=1"),
            body: Value::Null,
            identifier: "rmr-published".to_string(),
        },
        NotFoundCase {
            label: "GetPolicy addressing the function by its partial ARN",
            method: "GET",
            uri: format!("{FUNCTIONS_URI}/{ACCOUNT}:function:rmr-bare/policy"),
            body: Value::Null,
            identifier: "rmr-bare".to_string(),
        },
        NotFoundCase {
            label: "GetFunctionUrlConfig on a function holding no function URL",
            method: "GET",
            uri: "/2021-10-31/functions/rmr-bare/url".to_string(),
            body: Value::Null,
            identifier: "rmr-bare".to_string(),
        },
        NotFoundCase {
            label: "ListTags on a code signing config ARN nothing holds",
            method: "GET",
            uri: format!("{TAGS_URI}/{UNHELD_CODE_SIGNING_CONFIG}"),
            body: Value::Null,
            identifier: UNHELD_CODE_SIGNING_CONFIG.to_string(),
        },
        NotFoundCase {
            label: "TagResource on an event source mapping ARN nothing holds",
            method: "POST",
            uri: format!("{TAGS_URI}/{unheld_mapping_arn}"),
            body: json!({ "Tags": { "team": "orders" } }),
            identifier: unheld_mapping_arn.clone(),
        },
    ];

    for case in &cases {
        let answer = call(&router, case.method, &case.uri, &case.body).await;
        let reported = message(&answer);
        assert_eq!(
            (answer.status, answer.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {:?}: expected 404 ResourceNotFoundException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, CANONICAL_NOT_FOUND,
            "case {:?}: expected {CANONICAL_NOT_FOUND:?}, got {reported:?}",
            case.label
        );
        assert!(
            !reported.contains(&case.identifier),
            "case {:?}: the sentence is fixed, so it may not quote {:?}, got {reported:?}",
            case.label,
            case.identifier
        );
    }
}

/// One removal row: the call that finds no policy to act on.
struct NoPolicyCase {
    label: &'static str,
    uri: String,
}

#[tokio::test]
async fn a_removal_with_no_policy_to_act_on_reports_the_missing_policy() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "nop-bare", false).await;
    create_zip_function(&router, "nop-emptied", false).await;
    publish_layer_version(&router, "nop-layer", 64).await;

    // One function and one layer version are granted a statement and then stripped of it,
    // so each kind is tested both never-granted and emptied.
    for (uri, body) in [
        (
            format!("{FUNCTIONS_URI}/nop-emptied/policy"),
            json!({
                "StatementId": "only-statement",
                "Action": "lambda:InvokeFunction",
                "Principal": "events.amazonaws.com",
            }),
        ),
        (
            "/2018-10-31/layers/nop-layer/versions/1/policy".to_string(),
            json!({
                "StatementId": "only-statement",
                "Action": "lambda:GetLayerVersion",
                "Principal": ACCOUNT,
            }),
        ),
    ] {
        let granted = call(&router, "POST", &uri, &body).await;
        assert_eq!(
            granted.status,
            201,
            "the grant on {uri:?} must succeed, got {} {}",
            granted.status,
            granted.text()
        );
        let removed = call(&router, "DELETE", &format!("{uri}/only-statement"), &Value::Null).await;
        assert_eq!(
            removed.status,
            204,
            "the removal on {uri:?} must succeed, got {} {}",
            removed.status,
            removed.text()
        );
    }

    let cases = [
        NoPolicyCase {
            label: "RemovePermission on a function that never carried a statement",
            uri: format!("{FUNCTIONS_URI}/nop-bare/policy/never-added"),
        },
        NoPolicyCase {
            label: "RemovePermission on a function whose last statement was removed",
            uri: format!("{FUNCTIONS_URI}/nop-emptied/policy/only-statement"),
        },
        NoPolicyCase {
            label: "RemovePermission naming a statement id nothing ever added",
            uri: format!("{FUNCTIONS_URI}/nop-bare/policy/some-other-id"),
        },
        NoPolicyCase {
            label: "RemoveLayerVersionPermission whose last statement was removed",
            uri: "/2018-10-31/layers/nop-layer/versions/1/policy/only-statement".to_string(),
        },
    ];

    for case in &cases {
        let answer = call(&router, "DELETE", &case.uri, &Value::Null).await;
        let reported = message(&answer);
        assert_eq!(
            (answer.status, answer.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {:?}: expected 404 ResourceNotFoundException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, NO_POLICY_REFUSAL,
            "case {:?}: expected {NO_POLICY_REFUSAL:?}, got {reported:?}",
            case.label
        );
    }

    // The READ of the same function's policy answers for the RESOURCE instead, so the two
    // refusals stay distinct.
    let read = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/nop-emptied/policy"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        (read.status, message(&read)),
        (404, CANONICAL_NOT_FOUND.to_string()),
        "case \"GetPolicy on the emptied policy\": expected 404 {CANONICAL_NOT_FOUND:?}, got {} {}",
        read.status,
        read.text()
    );
}

/// One code-signing-configuration row: the operation that named an ARN nothing is held
/// under, and the ARN it named.
struct CodeSigningConfigCase {
    label: &'static str,
    method: &'static str,
    arn: &'static str,
    body: Value,
}

#[tokio::test]
async fn a_code_signing_configuration_that_does_not_exist_is_refused_by_its_own_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The rows differ in the operation that asked (the read, the update and the delete all
    // address the configuration by `CodeSigningConfigArn`) and in the ARN they named, which
    // is the only variable part of the sentence.
    let other_unheld: &str =
        "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-ffff0000ffff0000f";
    let cases = [
        CodeSigningConfigCase {
            label: "GetCodeSigningConfig for an ARN nothing holds",
            method: "GET",
            arn: UNHELD_CODE_SIGNING_CONFIG,
            body: Value::Null,
        },
        CodeSigningConfigCase {
            label: "DeleteCodeSigningConfig for an ARN nothing holds",
            method: "DELETE",
            arn: UNHELD_CODE_SIGNING_CONFIG,
            body: Value::Null,
        },
        CodeSigningConfigCase {
            label: "UpdateCodeSigningConfig for an ARN nothing holds",
            method: "PUT",
            arn: UNHELD_CODE_SIGNING_CONFIG,
            body: json!({ "Description": "nothing to update" }),
        },
        CodeSigningConfigCase {
            label: "DeleteCodeSigningConfig for a second ARN nothing holds",
            method: "DELETE",
            arn: other_unheld,
            body: Value::Null,
        },
    ];

    for case in &cases {
        let answer = call(
            &router,
            case.method,
            &format!("/2020-04-22/code-signing-configs/{}", case.arn),
            &case.body,
        )
        .await;
        let reported = message(&answer);
        let expected = format!(
            "{}{}{}",
            CODE_SIGNING_CONFIG_GONE.0, case.arn, CODE_SIGNING_CONFIG_GONE.1
        );
        assert_eq!(
            (answer.status, answer.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {:?}: expected 404 ResourceNotFoundException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.label
        );
    }
}

/// One capacity-provider row: how the request addresses a provider nothing holds.
struct CapacityProviderCase {
    label: &'static str,
    method: &'static str,
    name: &'static str,
    /// How the request addresses the provider, built from its name.
    reference: fn(&str) -> String,
    body: Value,
    /// The variable part the refused sentence names, built from the value the request
    /// addressed the provider with: the identifier itself for the operations that echo it,
    /// the ARN it names for the ones that resolve it.
    subject: fn(&str) -> String,
}

/// The capacity-provider ARN a bare provider name names in the Region these rows call.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

#[tokio::test]
async fn a_capacity_provider_that_does_not_exist_is_refused_by_its_own_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Every row is the same two fixed clauses; what the rows differ along is the operation
    // that asked, the provider name, which of the two documented `CapacityProviderName`
    // forms addressed it, and therefore the variable part each operation states: the
    // delete and the version listing echo the identifier the request carried, and the read
    // and the update state the ARN that identifier names (each form recorded per
    // operation — see
    // `crate::state::capacity_provider::capacity_provider_not_found_message`).
    let cases = [
        CapacityProviderCase {
            label: "GetCapacityProvider addressed by a bare name",
            method: "GET",
            name: "cp-msg-absent",
            reference: |name| name.to_string(),
            body: Value::Null,
            subject: provider_arn,
        },
        CapacityProviderCase {
            label: "GetCapacityProvider addressed by the provider ARN",
            method: "GET",
            name: "cp-msg-ghost",
            reference: provider_arn,
            body: Value::Null,
            subject: provider_arn,
        },
        CapacityProviderCase {
            label: "UpdateCapacityProvider addressed by a bare name",
            method: "PUT",
            name: "cp-msg-unknown",
            reference: |name| name.to_string(),
            body: json!({ "Description": "nothing to update" }),
            subject: provider_arn,
        },
        CapacityProviderCase {
            label: "UpdateCapacityProvider addressed by the provider ARN",
            method: "PUT",
            name: "cp-msg-update-ghost",
            reference: provider_arn,
            body: json!({ "Description": "nothing to update" }),
            subject: provider_arn,
        },
        CapacityProviderCase {
            label: "DeleteCapacityProvider addressed by a bare name",
            method: "DELETE",
            name: "cp-msg-never-created",
            reference: |name| name.to_string(),
            body: Value::Null,
            // The delete echoes what it was given, so a bare name is reported as that
            // bare name.
            subject: |name| name.to_string(),
        },
        CapacityProviderCase {
            label: "DeleteCapacityProvider addressed by the provider ARN",
            method: "DELETE",
            name: "cp-msg-delete-ghost",
            reference: provider_arn,
            body: Value::Null,
            subject: provider_arn,
        },
    ];

    for case in &cases {
        let reference = (case.reference)(case.name);
        let answer = call(
            &router,
            case.method,
            &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
            &case.body,
        )
        .await;
        let reported = message(&answer);
        let expected = format!(
            "Capacity provider not found: {}. Check that the ARN is correct and the \
             capacity provider exists in this Region.",
            (case.subject)(case.name)
        );
        assert_eq!(
            (answer.status, answer.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {:?}: expected 404 ResourceNotFoundException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.label
        );
    }
}

/// One alias row: how the function is addressed, and which alias of it is asked about.
struct AliasCase {
    label: &'static str,
    function: &'static str,
    /// How the request addresses the function, built from its name.
    reference: fn(&str) -> String,
    alias: &'static str,
}

#[tokio::test]
async fn an_alias_that_does_not_exist_is_refused_by_the_alias_sentence_naming_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in ["alias-msg-one", "alias-msg-two"] {
        create_zip_function(&router, name, true).await;
    }
    make_everything_due(&engine);

    // The control: an alias that DOES exist is reported rather than refused, so the rows
    // below are about absence and not about the reader being broken.
    let created = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/alias-msg-one/aliases"),
        &json!({ "Name": "held", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateAlias must succeed, got {} {}",
        created.status,
        created.text()
    );
    let held = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/alias-msg-one/aliases/held"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        held.status,
        200,
        "GetAlias of a held alias must answer it, got {} {}",
        held.status,
        held.text()
    );

    // The rows differ in the alias name's shape (one character, a long mixed name, the
    // documented `-`/`_` characters) and in the documented `FunctionName` form the request
    // addresses the function by — the ARN in the refusal comes from stored state, so it is
    // the canonical form whichever form was asked with.
    let cases = [
        AliasCase {
            label: "a short lower-case alias name, function addressed by its bare name",
            function: "alias-msg-one",
            reference: |name| name.to_string(),
            alias: "live",
        },
        AliasCase {
            label: "a single upper-case character, function addressed by its bare name",
            function: "alias-msg-one",
            reference: |name| name.to_string(),
            alias: "X",
        },
        AliasCase {
            label: "a mixed name carrying both documented separators",
            function: "alias-msg-two",
            reference: |name| name.to_string(),
            alias: "Blue-Green_2",
        },
        AliasCase {
            label: "a long alias name, function addressed by its partial ARN",
            function: "alias-msg-two",
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            alias: "a-very-long-alias-name-nobody-ever-created_0123456789",
        },
        AliasCase {
            label: "function addressed by its full ARN",
            function: "alias-msg-one",
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            alias: "ghost",
        },
    ];

    for case in &cases {
        let reference = (case.reference)(case.function);
        let answer = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{reference}/aliases/{}", case.alias),
            &Value::Null,
        )
        .await;
        let reported = message(&answer);
        let expected = format!(
            "{ALIAS_NOT_FOUND_PREFIX}{}:{}",
            function_arn(case.function),
            case.alias
        );
        assert_eq!(
            (answer.status, answer.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {:?}: expected 404 ResourceNotFoundException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.label
        );
    }

    // A missing FUNCTION keeps its own leading phrase, so the two refusals stay tellable
    // apart by the phrase alone.
    let no_function = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/alias-msg-absent/aliases/live"),
        &Value::Null,
    )
    .await;
    let reported = message(&no_function);
    assert_eq!(
        (no_function.status, reported.starts_with(FUNCTION_NOT_FOUND_PREFIX)),
        (404, true),
        "case \"an alias of a function that does not exist\": expected 404 and the \
         function-not-found phrase, got {} {reported:?}",
        no_function.status
    );
    assert!(
        !reported.starts_with(ALIAS_NOT_FOUND_PREFIX),
        "case \"an alias of a function that does not exist\": the alias phrase names the \
         wrong absence, got {reported:?}"
    );
}

/// One precondition row: the request carrying a stale `RevisionId`, and which of the two
/// documented sentences it must be refused with.
struct RevisionCase {
    label: &'static str,
    method: &'static str,
    /// Built from the stale `RevisionId` the row supplies, which some operations carry in
    /// the query string and some in the body.
    uri: fn(&str) -> String,
    body: fn(&str) -> Value,
    /// The `RevisionId` the request supplies, which no resource here is at.
    stale: &'static str,
    expected: &'static str,
}

#[tokio::test]
async fn a_stale_revision_is_refused_by_the_sentence_naming_the_api_that_reports_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "rev-msg-fn", true).await;
    create_zip_function(&router, "rev-msg-bare", false).await;
    publish_layer_version(&router, "rev-msg-layer", 64).await;
    make_everything_due(&engine);

    let granted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/rev-msg-fn/policy"),
        &json!({
            "StatementId": "held-statement",
            "Action": "lambda:InvokeFunction",
            "Principal": "events.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "AddPermission must succeed, got {} {}",
        granted.status,
        granted.text()
    );
    let shared = call(
        &router,
        "POST",
        "/2018-10-31/layers/rev-msg-layer/versions/1/policy",
        &json!({
            "StatementId": "held-statement",
            "Action": "lambda:GetLayerVersion",
            "Principal": ACCOUNT,
        }),
    )
    .await;
    assert_eq!(
        shared.status,
        201,
        "AddLayerVersionPermission must succeed, got {} {}",
        shared.status,
        shared.text()
    );
    let policy_before = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/rev-msg-fn/policy"),
        &Value::Null,
    )
    .await;
    let revision_before = policy_before
        .string("RevisionId")
        .expect("GetPolicy reports the policy's RevisionId");

    // Each row supplies a DIFFERENT value no resource is at — the all-zeros UUID, a
    // well-formed v4, and a value that is no UUID at all — so a sentence that echoed the
    // supplied id could not be equal across them.
    let cases = [
        RevisionCase {
            label: "AddPermission at a revision the policy is not at",
            method: "POST",
            uri: |_| format!("{FUNCTIONS_URI}/rev-msg-fn/policy"),
            body: |stale| {
                json!({
                    "StatementId": "another-statement",
                    "Action": "lambda:InvokeFunction",
                    "Principal": "events.amazonaws.com",
                    "RevisionId": stale,
                })
            },
            stale: "00000000-0000-0000-0000-000000000000",
            expected: POLICY_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "AddPermission on a function that holds no policy at all",
            method: "POST",
            uri: |_| format!("{FUNCTIONS_URI}/rev-msg-bare/policy"),
            body: |stale| {
                json!({
                    "StatementId": "first-statement",
                    "Action": "lambda:InvokeFunction",
                    "Principal": "events.amazonaws.com",
                    "RevisionId": stale,
                })
            },
            stale: "9a503a20-88dc-4656-9c7b-5045d0499220",
            expected: POLICY_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "RemovePermission at a revision the policy is not at",
            method: "DELETE",
            uri: |stale| {
                format!("{FUNCTIONS_URI}/rev-msg-fn/policy/held-statement?RevisionId={stale}")
            },
            body: |_| Value::Null,
            stale: "not-a-uuid-at-all",
            expected: POLICY_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "UpdateFunctionConfiguration at a revision the function is not at",
            method: "PUT",
            uri: |_| format!("{FUNCTIONS_URI}/rev-msg-fn/configuration"),
            body: |stale| json!({ "Description": "refused", "RevisionId": stale }),
            stale: "11111111-2222-3333-4444-555555555555",
            expected: CONFIGURATION_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "UpdateFunctionCode at a revision the function is not at",
            method: "PUT",
            uri: |_| format!("{FUNCTIONS_URI}/rev-msg-fn/code"),
            body: |stale| {
                json!({ "ZipFile": zip_file(&package_bytes(128)), "RevisionId": stale })
            },
            stale: "00000000-0000-0000-0000-000000000000",
            expected: CONFIGURATION_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "PublishVersion at a revision the function is not at",
            method: "POST",
            uri: |_| format!("{FUNCTIONS_URI}/rev-msg-fn/versions"),
            body: |stale| json!({ "RevisionId": stale }),
            stale: "deadbeef-dead-beef-dead-beefdeadbeef",
            expected: CONFIGURATION_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "AddLayerVersionPermission at a revision the policy is not at",
            method: "POST",
            uri: |stale| {
                format!("/2018-10-31/layers/rev-msg-layer/versions/1/policy?RevisionId={stale}")
            },
            body: |_| {
                json!({
                    "StatementId": "another-statement",
                    "Action": "lambda:GetLayerVersion",
                    "Principal": ACCOUNT,
                })
            },
            stale: "00000000-0000-0000-0000-000000000000",
            expected: CONFIGURATION_REVISION_REFUSAL,
        },
        RevisionCase {
            label: "RemoveLayerVersionPermission at a revision the policy is not at",
            method: "DELETE",
            uri: |stale| {
                format!(
                    "/2018-10-31/layers/rev-msg-layer/versions/1/policy/held-statement\
                     ?RevisionId={stale}"
                )
            },
            body: |_| Value::Null,
            stale: "0d0d0d0d-1e1e-2f2f-3a3a-4b4b4b4b4b4b",
            expected: CONFIGURATION_REVISION_REFUSAL,
        },
    ];

    for case in &cases {
        let answer = call(
            &router,
            case.method,
            &(case.uri)(case.stale),
            &(case.body)(case.stale),
        )
        .await;
        let reported = message(&answer);
        assert_eq!(
            (answer.status, answer.error_code().contains("PreconditionFailedException")),
            (412, true),
            "case {:?}: expected 412 PreconditionFailedException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, case.expected,
            "case {:?}: expected {:?}, got {reported:?}",
            case.label, case.expected
        );
        assert!(
            !reported.contains(case.stale),
            "case {:?}: the sentence is fixed, so it may not echo the supplied {:?}, got \
             {reported:?}",
            case.label,
            case.stale
        );
        for quoted in ["rev-msg-fn", "rev-msg-bare", "rev-msg-layer"] {
            assert!(
                !reported.contains(quoted),
                "case {:?}: the sentence is fixed, so it may not name {quoted:?}, got \
                 {reported:?}",
                case.label
            );
        }
    }

    // The two sentences differ, which is what lets a caller tell a policy-revision
    // conflict from a configuration-revision one.
    assert_ne!(
        POLICY_REVISION_REFUSAL, CONFIGURATION_REVISION_REFUSAL,
        "the two documented branches are two different sentences"
    );

    // And every refused call left the policy at the revision it was already at.
    let policy_after = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/rev-msg-fn/policy"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        policy_after.string("RevisionId"),
        Some(revision_before),
        "a refused precondition must change nothing, got {}",
        policy_after.text()
    );
}

/// One tag-quota row: the tags one `TagResource` call carries, whether the merge stays
/// within the quota, and how many tags the resource holds afterwards.
struct TagCase {
    label: &'static str,
    function: &'static str,
    tags: Value,
    accepted: bool,
    held_after: usize,
}

#[tokio::test]
async fn a_tag_write_past_the_quota_states_the_quota_and_nothing_of_the_request() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in ["tag-msg-shot", "tag-msg-step", "tag-msg-far"] {
        create_zip_function(&router, name, false).await;
    }

    // The rows walk both sides of the quota: one call past it, the quota exactly, a call
    // that would add one past it, a call that only REPLACES a key it already holds (no
    // growth, so still accepted), a mixed call whose one new key would push it past, and a
    // call far past the quota.
    let cases = vec![
        TagCase {
            label: "one call carrying one tag more than the quota",
            function: "tag-msg-shot",
            tags: tag_map("a", MAX_TAGS + 1),
            accepted: false,
            held_after: 0,
        },
        TagCase {
            label: "one call carrying exactly the quota",
            function: "tag-msg-step",
            tags: tag_map("b", MAX_TAGS),
            accepted: true,
            held_after: MAX_TAGS,
        },
        TagCase {
            label: "one further new key on a resource already at the quota",
            function: "tag-msg-step",
            tags: json!({ "beyond": "one-too-many" }),
            accepted: false,
            held_after: MAX_TAGS,
        },
        TagCase {
            label: "a key the resource already holds, which replaces rather than adds",
            function: "tag-msg-step",
            tags: json!({ "b0": "replaced" }),
            accepted: true,
            held_after: MAX_TAGS,
        },
        TagCase {
            label: "a held key beside one new key, which would push it past the quota",
            function: "tag-msg-step",
            tags: json!({ "b0": "replaced-again", "beyond": "one-too-many" }),
            accepted: false,
            held_after: MAX_TAGS,
        },
        TagCase {
            label: "one call carrying one less than the quota",
            function: "tag-msg-far",
            tags: tag_map("c", MAX_TAGS - 1),
            accepted: true,
            held_after: MAX_TAGS - 1,
        },
        TagCase {
            label: "one new key reaching the quota exactly",
            function: "tag-msg-far",
            tags: json!({ "at-the-bound": "accepted" }),
            accepted: true,
            held_after: MAX_TAGS,
        },
        TagCase {
            label: "a call carrying ten more keys than the quota admits",
            function: "tag-msg-far",
            tags: tag_map("d", MAX_TAGS + 10),
            accepted: false,
            held_after: MAX_TAGS,
        },
    ];

    for case in &cases {
        let resource = function_arn(case.function);
        let answer = tag_resource(&router, &resource, &case.tags).await;
        if case.accepted {
            assert_eq!(
                answer.status,
                204,
                "case {:?}: expected 204, got {} {}",
                case.label,
                answer.status,
                answer.text()
            );
        } else {
            let reported = message(&answer);
            assert_eq!(
                (answer.status, answer.error_code().contains("InvalidParameterValueException")),
                (400, true),
                "case {:?}: expected 400 InvalidParameterValueException, got {} {}",
                case.label,
                answer.status,
                answer.text()
            );
            assert_eq!(
                reported, TAG_QUOTA_REFUSAL,
                "case {:?}: expected {TAG_QUOTA_REFUSAL:?}, got {reported:?}",
                case.label
            );
            for quoted in [case.function, resource.as_str()] {
                assert!(
                    !reported.contains(quoted),
                    "case {:?}: the sentence states the quota only, so it may not quote \
                     {quoted:?}, got {reported:?}",
                    case.label
                );
            }
        }
        let held = tag_count(&router, &resource).await;
        assert_eq!(
            held, case.held_after,
            "case {:?}: expected {} tags held afterwards, got {held}",
            case.label, case.held_after
        );
    }
}

/// What a row supplies as `PublishVersion`'s `CodeSha256`.
enum Supplied {
    /// The digest of the function's own package, which matches.
    OwnDigest,
    /// The digest of some other bytes, of the given length.
    OtherPackage(usize),
    /// A value that is no digest at all.
    NotADigest(&'static str),
}

/// One `CodeSha256` precondition row: the package the function holds, and the value the
/// publish supplies.
struct ShaCase {
    label: &'static str,
    /// The `Code` member the function is created with; `None` is an inline package of
    /// `package` bytes.
    s3_package: bool,
    package: usize,
    supplied: Supplied,
}

#[tokio::test]
async fn a_mismatched_code_sha256_reports_the_supplied_and_the_current_digest() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The rows differ in the package the function holds (sizes whose digests all differ,
    // and one package this emulator holds no bytes for so it computes no digest at all)
    // and in what the publish supplies (another package's digest, a value that is no
    // digest, and the function's own digest, which is accepted).
    let cases = [
        ShaCase {
            label: "a one-byte package against another package's digest",
            s3_package: false,
            package: 1,
            supplied: Supplied::OtherPackage(64),
        },
        ShaCase {
            label: "a 64-byte package against a one-byte package's digest",
            s3_package: false,
            package: 64,
            supplied: Supplied::OtherPackage(1),
        },
        ShaCase {
            label: "two packages one byte apart in length",
            s3_package: false,
            package: 4096,
            supplied: Supplied::OtherPackage(4095),
        },
        ShaCase {
            label: "a value that is no digest at all",
            s3_package: false,
            package: 64,
            supplied: Supplied::NotADigest("not-the-stored-digest"),
        },
        ShaCase {
            label: "a package this emulator holds no bytes for, so it computes no digest",
            s3_package: true,
            package: 0,
            supplied: Supplied::OtherPackage(64),
        },
        ShaCase {
            label: "the function's own digest, which is the precondition holding",
            s3_package: false,
            package: 256,
            supplied: Supplied::OwnDigest,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("sha-msg-{index}");
        let bytes = package_bytes(case.package);
        let code = if case.s3_package {
            json!({ "S3Bucket": "deployments", "S3Key": "fn.zip" })
        } else {
            json!({ "ZipFile": zip_file(&bytes) })
        };
        create_function(&router, &name, &code, false).await;
        make_everything_due(&engine);

        // What the function's `CodeSha256` is: the digest of the bytes it holds, and
        // nothing when this emulator holds no bytes for its package.
        let current = if case.s3_package {
            None
        } else {
            Some(digest_of(&bytes))
        };
        let supplied = match case.supplied {
            Supplied::OwnDigest => current.clone().unwrap_or_default(),
            Supplied::OtherPackage(length) => digest_of(&package_bytes(length)),
            Supplied::NotADigest(value) => value.to_string(),
        };

        let answer = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({ "CodeSha256": supplied }),
        )
        .await;

        if matches!(case.supplied, Supplied::OwnDigest) {
            assert_eq!(
                answer.status,
                201,
                "case {:?}: expected 201 for a digest that matches, got {} {}",
                case.label,
                answer.status,
                answer.text()
            );
            continue;
        }

        let reported = message(&answer);
        // A function whose package the emulator holds by reference has no digest at all,
        // so there is no current value for the comparison sentence to name: that state
        // gets its own refusal rather than the comparison with an empty slot.
        let expected = match &current {
            Some(current) => format!(
                "CodeSHA256 ({supplied}) is different from current CodeSHA256 in $LATEST \
                 ({current}). Please try again with the CodeSHA256 in $LATEST."
            ),
            None => format!(
                "CodeSHA256 ({supplied}) cannot be matched: this function's deployment \
                 package is held by reference, so no CodeSHA256 is computed for $LATEST."
            ),
        };
        assert_eq!(
            (answer.status, answer.error_code().contains("InvalidParameterValueException")),
            (400, true),
            "case {:?}: expected 400 InvalidParameterValueException, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.label
        );

        // Nothing was published, so the function still has no version 1.
        let versions = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}?Qualifier=1"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            versions.status,
            404,
            "case {:?}: a refused publish must create no version, got {} {}",
            case.label,
            versions.status,
            versions.text()
        );
    }
}
