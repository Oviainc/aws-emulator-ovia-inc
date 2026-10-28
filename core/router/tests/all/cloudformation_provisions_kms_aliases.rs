// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::KMS::Alias` really creates that alias in
//! KMS, and a stack whose alias names a key KMS resolves to nothing fails and rolls back.
//!
//! The rule under test: **a resource a stack reports as created exists in the service that
//! owns it, created through that service's own API — so the owning service's reads resolve
//! the handle the stack publishes, and a create that service REFUSES fails the resource,
//! which rolls the stack back.** For an `AWS::KMS::Alias` that means `CreateAlias` under the
//! `AliasName` the declaration states (which is what `Ref` returns), bound to the
//! `TargetKeyId` the declaration resolves to, so `DescribeKey` with the alias name or the
//! alias ARN answers the target key's metadata — and a `TargetKeyId` KMS answers
//! `NotFoundException` for leaves the stack `ROLLBACK_COMPLETE` rather than
//! `CREATE_COMPLETE`.
//!
//! Two crates have to agree for that to hold (CloudFormation makes the calls, KMS holds the
//! alias and resolves it on every operation that takes a `KeyId`), so the suite lives here
//! rather than in either of them.
//!
//! The rows differ along what the rule generalises over: which intrinsic the declaration
//! names the target key with (`Ref`, which is the key id, and `Fn::GetAtt … Arn`), whether
//! the alias is declared beside its key or against a key created outside the stack, how deep
//! the alias name's path is, which identifier form KMS is then asked about the key by (the
//! bare `alias/<name>`, the alias ARN, and the key's own id), and — for the refusal rows —
//! which shape of unresolvable target the declaration carries, or which property of the
//! declaration KMS could not be asked for at all.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked, except the key's identity, which is KMS's own to mint.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack's key, its alias
/// and both of their ARNs belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due — twice over, since a rolled-back create moves through
/// `ROLLBACK_IN_PROGRESS` before `ROLLBACK_COMPLETE` falls due. Neither a key nor an alias
/// has a transient status of its own.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_500);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one KMS operation over its awsJson1_1 wire.
async fn kms(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header("authorization", authorization("kms"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = kms(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an awsJson refusal, which KMS carries as the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// One output of a settled stack.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {outputs:?}"))
        .clone()
}

/// Create a stack, wait past its transitions, and answer its `DescribeStacks` body with the
/// status it settled in.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> (String, String) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    let settled = opt_element(&body, "StackStatus").unwrap_or_default();
    (settled, body)
}

/// Create a stack that must reach `CREATE_COMPLETE`, and answer its `DescribeStacks` body.
async fn create_completing(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (settled, body) = create_and_settle(router, case, stack_name, template).await;
    assert_eq!(
        settled, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to complete, got {body}"
    );
    body
}

/// Update a stack, wait past its transitions, and answer its `DescribeStacks` body with the
/// status it settled in.
async fn update_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> (String, String) {
    let (status, body) = cloudformation(
        router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    let settled = opt_element(&body, "StackStatus").unwrap_or_default();
    (settled, body)
}

/// Update a stack that must reach `UPDATE_COMPLETE`, and answer the `DescribeStacks` body.
async fn update_completing(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (settled, body) = update_and_settle(router, case, stack_name, template).await;
    assert_eq!(
        settled, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {body}"
    );
    body
}

/// The alias ARN KMS holds an alias of this account and Region under
/// (`arn:aws:kms:<region>:<account>:alias/<name>`, the form `AliasListEntry$AliasArn`
/// carries), built from the alias name a declaration states.
fn alias_arn(alias_name: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:{alias_name}",
        engine::emulator_account_id()
    )
}

/// A template declaring a key and an alias for it, with the alias naming the key through
/// `target` — the intrinsic the row chooses — and publishing the alias's own `Ref`.
fn key_and_alias_template(alias_name: &str, target: Value) -> String {
    json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {"Description": "the aliased key"}},
            "Alias": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": alias_name, "TargetKeyId": target},
            },
        },
        "Outputs": {
            "AliasRef": {"Value": {"Ref": "Alias"}},
            "KeyRef": {"Value": {"Ref": "Key"}},
            "KeyArn": {"Value": {"Fn::GetAtt": ["Key", "Arn"]}},
        },
    })
    .to_string()
}

/// A template declaring ONE alias against a key the stack does not create, which is how a
/// row points an alias at a key some other caller made — and at one that does not exist.
fn alias_only_template(alias_name: &str, target: &str) -> String {
    json!({
        "Resources": {
            "Alias": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": alias_name, "TargetKeyId": target},
            },
        },
        "Outputs": {"AliasRef": {"Value": {"Ref": "Alias"}}},
    })
    .to_string()
}

/// One row of the alias-creation table: how the declaration names its target key, and the
/// alias name it asks for.
struct AliasRow {
    case: &'static str,
    stack_name: &'static str,
    alias_name: &'static str,
    /// The intrinsic the alias's `TargetKeyId` is declared as, over the key the same
    /// template declares.
    target: Value,
}

/// A stack that completes has really created its declared alias in KMS, bound to the key the
/// declaration names — so every identifier form of the alias resolves to that key.
#[tokio::test]
async fn a_stack_creates_its_declared_kms_alias_in_kms_bound_to_the_declared_key() {
    let router = build_router();
    let rows = vec![
        AliasRow {
            case: "an alias naming its key by Ref, which is the key id",
            stack_name: "kms-alias-ref-stack",
            alias_name: "alias/by-ref",
            target: json!({"Ref": "Key"}),
        },
        AliasRow {
            case: "an alias naming its key by the Arn attribute",
            stack_name: "kms-alias-arn-stack",
            alias_name: "alias/by-arn",
            target: json!({"Fn::GetAtt": ["Key", "Arn"]}),
        },
        AliasRow {
            case: "an alias naming its key by the KeyId attribute",
            stack_name: "kms-alias-keyid-stack",
            alias_name: "alias/by-key-id",
            target: json!({"Fn::GetAtt": ["Key", "KeyId"]}),
        },
        AliasRow {
            // "It can contain only alphanumeric characters, forward slashes (/),
            // underscores (_), and dashes (-)" (`CreateAliasRequest$AliasName`), so a path
            // of several segments is one alias name, not several.
            case: "an alias whose name carries a path of its own",
            stack_name: "kms-alias-path-stack",
            alias_name: "alias/team/payments/ledger_key-1",
            target: json!({"Ref": "Key"}),
        },
    ];

    for row in &rows {
        let case = row.case;
        let body = create_completing(
            &router,
            case,
            row.stack_name,
            &key_and_alias_template(row.alias_name, row.target.clone()),
        )
        .await;
        let outputs = outputs_of(&body);
        let key_id = output(&outputs, case, "KeyRef");
        let key_arn = output(&outputs, case, "KeyArn");

        // "Ref returns the alias name": the handle the stack publishes is the name the
        // declaration asked for, not a CloudFormation `<stack>-<logical id>-<suffix>` name.
        assert_eq!(
            output(&outputs, case, "AliasRef"),
            row.alias_name,
            "case {case:?}: expected the declared alias name as the alias's Ref, got {outputs:?}"
        );

        // KMS resolves the alias — by either identifier form an alias is addressed in — to
        // the very key the stack created.
        for (form, identifier) in [
            ("the alias name", row.alias_name.to_string()),
            ("the alias ARN", alias_arn(row.alias_name)),
        ] {
            let described =
                kms_ok(&router, case, "DescribeKey", json!({"KeyId": identifier})).await;
            let metadata = &described["KeyMetadata"];
            assert_eq!(
                (metadata["KeyId"].as_str(), metadata["Arn"].as_str()),
                (Some(key_id.as_str()), Some(key_arn.as_str())),
                "case {case:?}: asked by {form}, KMS had to resolve the alias to the \
                 stack's key, got {described}"
            );
        }

        // And the alias is one KMS LISTS for that key, with the ARN it built for it: the
        // alias is a real entity of the account and Region, not an identifier DescribeKey
        // happens to accept.
        let listed = kms_ok(&router, case, "ListAliases", json!({"KeyId": key_arn})).await;
        let held: Vec<(String, String, String)> = listed["Aliases"]
            .as_array()
            .map(|aliases| {
                aliases
                    .iter()
                    .filter_map(|alias| {
                        Some((
                            alias["AliasName"].as_str()?.to_string(),
                            alias["AliasArn"].as_str()?.to_string(),
                            alias["TargetKeyId"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            held.as_slice(),
            &[(
                row.alias_name.to_string(),
                alias_arn(row.alias_name),
                key_id.clone(),
            )][..],
            "case {case:?}: expected KMS to list the stack's alias against its key, got \
             {listed}"
        );

        // A cryptographic operation addressed by the alias reaches the same key, which is
        // what the alias exists for.
        let encrypted = kms_ok(
            &router,
            case,
            "Encrypt",
            json!({"KeyId": row.alias_name, "Plaintext": "c2VhbGVkIHVuZGVyIHRoZSBhbGlhcw=="}),
        )
        .await;
        assert_eq!(
            encrypted["KeyId"].as_str(),
            Some(key_arn.as_str()),
            "case {case:?}: expected the alias to encrypt under the stack's key, got \
             {encrypted}"
        );
    }
}

/// An alias the stack points at a key ANOTHER caller created is created just the same: the
/// target is whatever KMS resolves, not only a key of the same template.
#[tokio::test]
async fn a_stack_creates_an_alias_for_a_key_created_outside_it() {
    let router = build_router();
    let case = "an alias for a key a client created before the stack";
    let created = kms_ok(&router, case, "CreateKey", json!({})).await;
    let key_id = created["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let alias_name = "alias/adopted-key";

    let body = create_completing(
        &router,
        case,
        "kms-alias-adopted-stack",
        &alias_only_template(alias_name, &key_arn),
    )
    .await;
    assert_eq!(
        output(&outputs_of(&body), case, "AliasRef"),
        alias_name,
        "case {case:?}: expected the declared alias name as the alias's Ref"
    );
    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": alias_name})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyId"].as_str(),
        Some(key_id.as_str()),
        "case {case:?}: expected the alias on the client's own key, got {described}"
    );
}

/// A stack whose alias names a target KMS resolves to no key does NOT complete: the create
/// of that resource is refused, so the stack fails and rolls back — and the alias the
/// template named exists nowhere afterwards.
///
/// The rows differ along the shape of the unresolvable target: a key id of KMS's own shape
/// that no key carries, a key ARN of this account and Region for such an id, and an
/// identifier that is no key identifier at all.
#[tokio::test]
async fn a_stack_whose_alias_targets_a_missing_key_rolls_back() {
    let router = build_router();
    // A key id KMS would mint (a version-4 UUID) that no key of this store carries.
    let absent_key_id = "bc436485-5092-42b8-92a3-0aa8b93536dc";
    // (case, stack name, alias name, the target the declaration carries)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "an alias targeting a key id no key carries",
            "kms-alias-missing-id-stack",
            "alias/dangling-by-id",
            absent_key_id.to_string(),
        ),
        (
            "an alias targeting a key ARN no key carries",
            "kms-alias-missing-arn-stack",
            "alias/dangling-by-arn",
            format!(
                "arn:aws:kms:{REGION}:{}:key/{absent_key_id}",
                engine::emulator_account_id()
            ),
        ),
        (
            "an alias targeting an identifier that is no key identifier at all",
            "kms-alias-unparsable-target-stack",
            "alias/dangling-by-nonsense",
            "not-a-key-identifier".to_string(),
        ),
    ];

    for (case, stack_name, alias_name, target) in &rows {
        let (settled, body) = create_and_settle(
            &router,
            case,
            stack_name,
            &alias_only_template(alias_name, target),
        )
        .await;
        assert_eq!(
            settled, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the refused alias to roll the stack back, got {body}"
        );
        // The reason names the resource that failed, so a client knows which declaration
        // the deployment broke on.
        let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Alias"),
            "case {case:?}: expected the failure to name the alias resource, got {reason:?}"
        );
        // Nothing was left behind: KMS holds no alias of that name. (Every client refusal
        // of this service answers HTTP 400 whatever `@httpError` the model puts on the
        // shape, which that crate's `client_error_status` suite is what pins.)
        let (status, answered) = kms(&router, "DescribeKey", &json!({"KeyId": alias_name})).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "NotFoundException"),
            "case {case:?}: a rolled-back stack must leave no alias behind, got {answered}"
        );
    }
}

/// An update that re-points an alias is applied to the alias the stack already has, through
/// the KMS call that owns that change (`UpdateAlias`) — the alias keeps its name, and it
/// resolves to the key the new declaration names. Deleting the stack deletes the alias and
/// leaves the key it pointed at alone.
#[tokio::test]
async fn an_update_repoints_the_alias_and_the_delete_removes_it() {
    let router = build_router();
    let case = "an alias moved from the stack's key onto another key";
    let stack_name = "kms-alias-updated-stack";
    let alias_name = "alias/repointed";

    let body = create_completing(
        &router,
        case,
        stack_name,
        &key_and_alias_template(alias_name, json!({"Ref": "Key"})),
    )
    .await;
    let outputs = outputs_of(&body);
    let first_key_id = output(&outputs, case, "KeyRef");
    let first_key_arn = output(&outputs, case, "KeyArn");

    // A second key for the alias to move to, created the way any client would.
    let second = kms_ok(&router, case, "CreateKey", json!({})).await;
    let second_key_id = second["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second_key_arn = second["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let repointed = json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {"Description": "the aliased key"}},
            "Alias": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": alias_name, "TargetKeyId": second_key_arn},
            },
        },
        "Outputs": {
            "AliasRef": {"Value": {"Ref": "Alias"}},
            "KeyRef": {"Value": {"Ref": "Key"}},
            "KeyArn": {"Value": {"Fn::GetAtt": ["Key", "Arn"]}},
        },
    })
    .to_string();
    let body = update_completing(&router, case, stack_name, &repointed).await;
    let outputs = outputs_of(&body);
    assert_eq!(
        (
            output(&outputs, case, "AliasRef"),
            output(&outputs, case, "KeyRef"),
        ),
        (alias_name.to_string(), first_key_id.clone()),
        "case {case:?}: an in-place update must keep the alias name and the stack's key"
    );

    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": alias_name})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyId"].as_str(),
        Some(second_key_id.as_str()),
        "case {case:?}: expected the alias on the key the update declares, got {described}"
    );
    // The key it was taken off is untouched: "These operations don't affect the underlying
    // KMS key."
    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": first_key_arn})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyState"].as_str(),
        Some("Enabled"),
        "case {case:?}: re-pointing the alias must leave its old key enabled, got {described}"
    );

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (status, answered) = kms(&router, "DescribeKey", &json!({"KeyId": alias_name})).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "NotFoundException"),
        "case {case:?}: the stack's deletion had to delete its alias, got {answered}"
    );
    // The key the alias had been moved onto was never the stack's, so the deletion leaves
    // it as it is.
    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": second_key_arn})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyState"].as_str(),
        Some("Enabled"),
        "case {case:?}: deleting the alias must leave its target key alone, got {described}"
    );
}

/// A declaration an owning service cannot be asked for is REFUSED rather than deployed with
/// the caller's property dropped: an alias declaring a property the type does not publish
/// fails its create (so the stack rolls back), and an update that stops declaring the
/// `TargetKeyId` an alias cannot exist without fails the update rather than reporting a
/// change nothing applied — with the alias left pointing where it did.
#[tokio::test]
async fn a_declaration_kms_cannot_be_asked_for_is_refused() {
    let router = build_router();

    let case = "an alias declaring a property the type does not publish";
    let unpublished = json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {}},
            "Alias": {
                "Type": "AWS::KMS::Alias",
                "Properties": {
                    "AliasName": "alias/unpublished-property",
                    "TargetKeyId": {"Ref": "Key"},
                    "Description": "no such property of this type",
                },
            },
        },
    })
    .to_string();
    let (settled, body) =
        create_and_settle(&router, case, "kms-alias-unmapped-stack", &unpublished).await;
    assert_eq!(
        settled, "ROLLBACK_COMPLETE",
        "case {case:?}: expected a property nothing carries to KMS to roll the stack back, \
         got {body}"
    );
    let (status, answered) = kms(
        &router,
        "DescribeKey",
        &json!({"KeyId": "alias/unpublished-property"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "NotFoundException"),
        "case {case:?}: a refused create must leave no alias behind, got {answered}"
    );

    let case = "an update that stops declaring the alias's target key";
    let stack_name = "kms-alias-untargeted-stack";
    let alias_name = "alias/keeps-its-target";
    let body = create_completing(
        &router,
        case,
        stack_name,
        &key_and_alias_template(alias_name, json!({"Ref": "Key"})),
    )
    .await;
    let key_id = output(&outputs_of(&body), case, "KeyRef");

    let untargeted = json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {"Description": "the aliased key"}},
            "Alias": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": alias_name},
            },
        },
    })
    .to_string();
    let (settled, body) = update_and_settle(&router, case, stack_name, &untargeted).await;
    assert_eq!(
        settled, "UPDATE_ROLLBACK_COMPLETE",
        "case {case:?}: an update KMS cannot be asked for must fail and roll the stack back \
         rather than complete, got {body}"
    );
    assert!(
        opt_element(&body, "StackStatusReason")
            .unwrap_or_default()
            .contains("Alias"),
        "case {case:?}: expected the failure to name the alias resource, got {body}"
    );
    // The alias is exactly where the deployment that created it left it.
    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": alias_name})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyId"].as_str(),
        Some(key_id.as_str()),
        "case {case:?}: a refused update must leave the alias on its key, got {described}"
    );
}

/// Drift detection on a stack's aliases is read off KMS: an alias somebody deleted out of
/// band is reported `DELETED`, while one nothing touched is `IN_SYNC`.
#[tokio::test]
async fn drift_detection_reports_an_alias_deleted_out_of_band() {
    let router = build_router();
    let case = "one alias deleted behind the stack's back, one untouched";
    let stack_name = "kms-alias-drift-stack";
    let template = json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {}},
            "Drifted": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": "alias/drifted", "TargetKeyId": {"Ref": "Key"}},
            },
            "Untouched": {
                "Type": "AWS::KMS::Alias",
                "Properties": {"AliasName": "alias/untouched", "TargetKeyId": {"Ref": "Key"}},
            },
        },
    })
    .to_string();
    create_completing(&router, case, stack_name, &template).await;

    // A client deletes one of the two aliases through KMS's own API, which the stack knows
    // nothing about.
    kms_ok(
        &router,
        case,
        "DeleteAlias",
        json!({"AliasName": "alias/drifted"}),
    )
    .await;

    let (status, body) =
        cloudformation(&router, "DetectStackDrift", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DetectStackDrift, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (status, body) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with {body}"
    );
    let reported: BTreeMap<String, String> = elements(&body, "LogicalResourceId")
        .into_iter()
        .zip(elements(&body, "StackResourceDriftStatus"))
        .collect();
    for (logical_id, expected) in [("Drifted", "DELETED"), ("Untouched", "IN_SYNC")] {
        assert_eq!(
            reported.get(logical_id).map(String::as_str),
            Some(expected),
            "case {case:?}: expected {logical_id} to be {expected}, got {reported:?}"
        );
    }
}
