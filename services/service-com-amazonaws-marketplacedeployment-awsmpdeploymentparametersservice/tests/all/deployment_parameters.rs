// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the deployment-parameter slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — so what is asserted is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * a deployment parameter is ONE persisted record keyed on
//!   (catalog, productId, agreementId, deploymentParameter.name), so a repeat
//!   `PutDeploymentParameter` for that identity updates it in place and reports the
//!   same `deploymentParameterId`/`resourceArn`, and every reported field is read
//!   out of the record rather than echoed;
//! * `TagResource` merges onto, and `UntagResource` removes from, that record's
//!   tag map, and `ListTagsForResource` reports exactly what those writes left;
//! * an ARN of this service's vendor naming no stored record is a 404 carrying this
//!   service's own `NotFoundException` code, a value this API cannot address at all a
//!   400 `BadRequestException` (the `resource_arn_validation` suite owns that rule),
//!   and a request outside the modelled constraints a `ValidationException`.
//!
//! The harness below (the router, the signed request, and the per-operation
//! callers) is `pub(crate)` because the sibling suites in this target —
//! `put_deployment_parameter_target`, `validation_messages` and
//! `not_found_error_code` — drive the same wire path and reuse it rather than
//! restating it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    seed_agreements, service_state, ProvisionedAgreement, MAX_CLIENT_TOKENS,
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder and the body accessors are shared by every suite of this
// test binary; ONE copy lives in `tests/all/support.rs` and is re-exported here, so
// the sibling suites that import them from this module keep working.
pub(crate) use crate::support::{error_code, field_name, message, pct, rfc3339};

/// The region every request below signs for; every minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";
/// This service's ARN namespace and sigv4 signing name, from the model's
/// `aws.api#service$arnNamespace` / `aws.auth#sigv4$name`.
pub(crate) const SIGNING_NAME: &str = "aws-marketplace";
/// The one catalog this service serves, which every request below names.
///
/// `PutDeploymentParameterRequest$catalog` is documented as a fixed value
/// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`),
/// and the service reads the set it belongs to from
/// `state::SUPPORTED_CATALOGS` — which this constant is asserted against in the
/// `catalog_allowed_values` suite, so the two cannot drift.
pub(crate) const CATALOG: &str = "AWSMarketplace";
/// The longest `DeploymentParameterName` the model admits (`length 1..400`).
pub(crate) const PARAMETER_NAME_MAX_LEN: usize = 400;
/// The longest `SecretString` the model admits (`length 1..15000`).
pub(crate) const SECRET_MAX_LEN: usize = 15_000;
/// The second parameter name every lifecycle row writes under its own identity,
/// to prove a different name mints a different resource.
const ALTERNATE_NAME: &str = "AlternateParameter";

/// One emulator with an empty store, mounted exactly as the router mounts it,
/// beside the engine that store lives in.
///
/// The engine is handed back so a test can [`provision`] the agreements a write
/// targets: `PutDeploymentParameter` writes against an agreement of the account,
/// and this API models no operation that creates one.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// Record `agreement_id` of `catalog` as covering `product_ids` — the out-of-band
/// provisioning a deployment-parameter write is targeted at.
pub(crate) fn provision(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    agreement_id: &str,
    product_ids: &[&str],
) {
    let agreement = product_ids.iter().fold(
        ProvisionedAgreement::new(catalog, agreement_id),
        |agreement, product_id| agreement.with_product(*product_id),
    );
    assert!(
        seed_agreements(engine, [agreement]).is_some(),
        "the shared engine must hold this service's store"
    );
}

/// Whether this service's store holds the target `(catalog, product_id,
/// agreement_id)`: the agreement the account holds, covering that product.
pub(crate) fn holds_target(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    product_id: &str,
    agreement_id: &str,
) -> bool {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .holds_target(catalog, product_id, agreement_id)
}

/// How many deployment parameters the store holds.
pub(crate) fn stored_parameters(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .parameters
        .len()
}

/// How many targets the store has recorded: one per product an agreement covers,
/// read through the store's own accessor.
pub(crate) fn recorded_targets(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .recorded_targets()
}

/// A request signed for `aws-marketplace` in [`REGION`]: the signal the router
/// routes on and the handlers read the ARN's region from.
pub(crate) fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("deployment-marketplace.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDMPDEPLOYTEST/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

pub(crate) async fn put(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/catalogs/{}/products/{}/deployment-parameters",
            pct(catalog),
            pct(product_id)
        ),
        Some(body),
    )
    .await
}

pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", pct(resource_arn)), None).await
}

pub(crate) async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let body = json!({
        "tags": tags
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect::<serde_json::Map<String, Value>>(),
    });
    call(
        router,
        "POST",
        &format!("/tags/{}", pct(resource_arn)),
        Some(&body),
    )
    .await
}

pub(crate) async fn untag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", pct(resource_arn)),
        None,
    )
    .await
}

/// The `tags` member of a response, as a comparable map. An absent member reads
/// as the empty map.
pub(crate) fn tags_of(body: &Value) -> BTreeMap<String, String> {
    body.get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tag map from `(key, value)` pairs.
pub(crate) fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The ARN this emulator must mint for the deployment parameter `id`.
pub(crate) fn expected_arn(id: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/{id}")
}

/// Whether `id` satisfies the modelled `DeploymentParameterResourceIdentifier`
/// (`length 1..32`, `^dp-[a-zA-Z0-9]+$`).
pub(crate) fn is_parameter_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("dp-") else {
        return false;
    };
    !suffix.is_empty() && id.len() <= 32 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// One row of the lifecycle table: an identity, its content, and the tags its
/// create carries.
struct Lifecycle {
    case: &'static str,
    catalog: String,
    product_id: String,
    agreement_id: String,
    parameter: String,
    secret: String,
    put_tags: BTreeMap<String, String>,
    /// The instant `expirationDate` names, as an epoch second.
    ///
    /// The member is `@timestampFormat("date-time")`, so it reaches the wire as the
    /// RFC 3339 string `support::rfc3339` renders — the form an AWS SDK sends.
    expiration_date: Option<i64>,
}

/// The lifecycle table. The rows differ along everything the rule generalises
/// over: the form of each identity segment (the characters its own modelled
/// pattern admits — `ResourceId` is `^[A-Za-z0-9_/-]+$`,
/// `DeploymentParameterName` `^[a-zA-Z0-9/_+=.@-]+$`), the length of the secret at
/// both modelled bounds, the number of tags the create carries (none, one, a few,
/// and one short of the cap) and whether the create names an `expirationDate`.
///
/// Every row carries the SAME catalog: `catalog` is an allowed-value member (the
/// model documents it as a fixed value, `crate::catalog_allowed_values` pins the
/// rule), so a row varying it would be testing that rule and not this one.
fn lifecycle_table() -> Vec<Lifecycle> {
    // One short of the cap, so the single tag the TagResource step adds lands the
    // record exactly ON the modelled `TagsMap length max 50`.
    let nearly_full: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE - 1)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    vec![
        Lifecycle {
            case: "plain identity, one tag",
            catalog: CATALOG.to_string(),
            product_id: "prod-abc123".to_string(),
            agreement_id: "agmt-abc123".to_string(),
            parameter: "DbPassword".to_string(),
            secret: "s3cr3t".to_string(),
            put_tags: tag_map(&[("env", "dev")]),
            expiration_date: None,
        },
        Lifecycle {
            case: "nested product id, punctuated name, no tags",
            catalog: CATALOG.to_string(),
            product_id: "prod/nested/id-7".to_string(),
            agreement_id: "agmt_1/2-3".to_string(),
            parameter: "api.key+v1=1@host/path_1-2".to_string(),
            secret: "x".to_string(),
            put_tags: BTreeMap::new(),
            expiration_date: Some(1_893_456_000),
        },
        Lifecycle {
            case: "secret at the modelled maximum, several punctuated tags",
            catalog: CATALOG.to_string(),
            product_id: "prod_01/beta-2".to_string(),
            agreement_id: "A".to_string(),
            parameter: "p".to_string(),
            secret: "y".repeat(SECRET_MAX_LEN),
            put_tags: tag_map(&[
                ("a", "1"),
                ("Z", "26"),
                ("mid.key:v", "value/with+chars=1"),
                ("at@sign", "ok_1"),
            ]),
            expiration_date: None,
        },
        Lifecycle {
            case: "name at the modelled maximum, one tag short of the cap",
            catalog: CATALOG.to_string(),
            product_id: "0".to_string(),
            agreement_id: "9".to_string(),
            parameter: "n".repeat(PARAMETER_NAME_MAX_LEN),
            secret: "rotate-me".to_string(),
            put_tags: nearly_full,
            expiration_date: None,
        },
        Lifecycle {
            case: "single-character identity",
            catalog: CATALOG.to_string(),
            product_id: "b".to_string(),
            agreement_id: "c".to_string(),
            parameter: "d".to_string(),
            secret: "e".to_string(),
            put_tags: tag_map(&[("k", "v")]),
            expiration_date: None,
        },
    ]
}

impl Lifecycle {
    /// The request body for this row's create, naming `parameter`/`secret`.
    fn body(&self, parameter: &str, secret: &str, tags: Option<&BTreeMap<String, String>>) -> Value {
        let mut body = json!({
            "agreementId": self.agreement_id,
            "deploymentParameter": { "name": parameter, "secretString": secret },
        });
        let map = body.as_object_mut().expect("an object body");
        if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
            map.insert(
                "tags".to_string(),
                Value::Object(tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
            );
        }
        if let Some(expiration) = self.expiration_date {
            map.insert("expirationDate".to_string(), json!(rfc3339(expiration)));
        }
        body
    }
}

#[tokio::test]
async fn a_deployment_parameter_is_one_record_that_put_tag_untag_and_list_all_answer_from() {
    let (router, engine) = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for row in lifecycle_table() {
        provision(&engine, &row.catalog, &row.agreement_id, &[&row.product_id]);
        let case = row.case;

        // --- create -------------------------------------------------------
        let (status, created) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(&row.parameter, &row.secret, Some(&row.put_tags)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutDeploymentParameter must succeed, got {status} {created:?}"
        );
        let id = created
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id of at most 32 characters, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a freshly minted id, got the already-minted {id:?}"
        );
        let arn = created
            .get("resourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            arn,
            expected_arn(&id),
            "case {case:?}: expected the ARN of the minted id, got {arn:?}"
        );
        assert_eq!(
            created.get("agreementId").and_then(Value::as_str),
            Some(row.agreement_id.as_str()),
            "case {case:?}: expected the stored agreementId, got {created:?}"
        );
        assert_eq!(
            tags_of(&created),
            row.put_tags,
            "case {case:?}: expected the create's tags back from the record, got {created:?}"
        );

        // --- the read that proves the create persisted --------------------
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            row.put_tags,
            "case {case:?}: expected the create-time tags, got {listed:?}"
        );

        // --- merge: one new key, and an overwrite of an existing one ------
        let mut added = tag_map(&[("team", "platform")]);
        if let Some(first) = row.put_tags.keys().next() {
            added.insert(first.clone(), "overwritten".to_string());
        }
        let (status, body) = tag(&router, &arn, &added).await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource must answer 204, got {status} {body:?}"
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: TagResource must answer no members, got {body:?}"
        );
        let mut expected = row.put_tags.clone();
        expected.extend(added.clone());
        assert!(
            expected.len() <= MAX_TAGS_PER_RESOURCE,
            "case {case:?}: the table row must stay inside the modelled cap, got {}",
            expected.len()
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the create-time and tag-time writes merged, got {listed:?}"
        );

        // --- removal, including a key that was never there ----------------
        let removals = vec!["team".to_string(), "never-set".to_string()];
        for attempt in 1..=2 {
            let (status, body) = untag(&router, &arn, &removals).await;
            assert_eq!(
                status, 204,
                "case {case:?}: UntagResource attempt {attempt} must answer 204 (@idempotent), \
                 got {status} {body:?}"
            );
        }
        expected.remove("team");
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the removed key gone and the rest intact, got {listed:?}"
        );

        // --- a repeat Put for the SAME identity updates in place ----------
        let ignored = tag_map(&[("ignored-at-update", "yes")]);
        // A different secret of the same class: still inside `SecretString`'s
        // modelled `length 1..15000`, including for the row already at the maximum.
        let rotated = if row.secret.len() + "rotated-".len() > SECRET_MAX_LEN {
            "z".repeat(row.secret.len())
        } else {
            format!("rotated-{}", row.secret)
        };
        let (status, again) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(&row.parameter, &rotated, Some(&ignored)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat Put must succeed, got {status} {again:?}"
        );
        assert_eq!(
            again.get("deploymentParameterId").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the same deploymentParameterId on update, got {again:?}"
        );
        assert_eq!(
            again.get("resourceArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the same resourceArn on update, got {again:?}"
        );
        assert_eq!(
            tags_of(&again),
            expected,
            "case {case:?}: tags are applied on create only, so an update must report the \
             record's tag map unchanged, got {again:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: an update must not rewrite the tag map, got {listed:?}"
        );

        // --- a DIFFERENT name under the same catalog/product/agreement ----
        let (status, other) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(ALTERNATE_NAME, "another-secret", None),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second parameter name must succeed, got {status} {other:?}"
        );
        let other_id = other
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&other_id) && other_id != id,
            "case {case:?}: a different parameter name must mint a different resource, \
             got {other_id:?} against {id:?}"
        );
        assert!(
            minted.insert(other_id.clone()),
            "case {case:?}: expected a freshly minted id, got {other_id:?}"
        );
        assert_eq!(
            tags_of(&other),
            BTreeMap::new(),
            "case {case:?}: a create carrying no tags must report an empty tag map, got {other:?}"
        );
        // The two records are independent: the first one's tags are untouched.
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: writing another parameter must not touch this record, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_record_is_not_found_on_every_tag_operation() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-present", &["prod-present"]);

    // A record that DOES exist, so a 404 below is about the ARN and not about an
    // empty store.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-present",
        &json!({
            "agreementId": "agmt-present",
            "deploymentParameter": { "name": "Present", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let live = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // `resourceArn` on the tagging operations targets a plain `String` with no
    // pattern, so each of these reaches the handler. Every row is an ARN of THIS
    // service's vendor, so what it observes is the store's answer about the ARN; a
    // value this API cannot address is refused one step earlier as a bad request,
    // which the `resource_arn_validation` suite owns.
    let unknown = vec![
        (
            "a well-formed ARN of an id that was never minted",
            expected_arn("dp-ffffffffffffffff"),
        ),
        (
            "the live ARN in another region",
            live.replace(REGION, "eu-west-1"),
        ),
        (
            "the live ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
        ),
        (
            "an ARN of this vendor naming another resource type",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
        ),
    ];

    for (case, resource_arn) in unknown {
        let (status, body) = list_tags(&router, &resource_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );

        let (status, body) = tag(&router, &resource_arn, &tag_map(&[("k", "v")])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: TagResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );

        let (status, body) = untag(&router, &resource_arn, &["k".to_string()]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: UntagResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );
    }

    // The live record is still readable, so none of the above wrote anything.
    let (status, listed) = list_tags(&router, &live).await;
    assert_eq!(
        status, 200,
        "the live record must still be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        BTreeMap::new(),
        "a refused TagResource must leave the record's tags alone, got {listed:?}"
    );
}

#[tokio::test]
async fn a_put_outside_the_modelled_constraints_is_refused() {
    let (router, engine) = emulator();
    // Every row below is aimed at a target the account holds, so what each one
    // observes is the modelled constraint it breaks and not a missing agreement.
    // The tag-key and tag-value rows need this: the centralized walk does not
    // report map-key constraints, so those two are refused by the handler's own
    // tag check, which runs after the target is resolved.
    provision(&engine, "AWSMarketplace", "a1", &["prod-1"]);
    // The one legal row at the end of this test writes against this target: the
    // shortest product, agreement, name and secret the model admits, under the one
    // catalog it serves.
    provision(&engine, "AWSMarketplace", "a", &["p"]);

    // A `tags` map past the number of entries one resource holds is NOT a row here:
    // that count is the tagging front end's limit and carries its own wire identity,
    // which the `tag_count_front_end` suite pins.

    // Each row breaks exactly one modelled constraint of `PutDeploymentParameter`'s
    // input: `Catalog` (`^[a-zA-Z_-]+$`, len 1..64), `ResourceId`
    // (`^[A-Za-z0-9_/-]+$`, len 1..64), `DeploymentParameterName` (len 1..400),
    // `SecretString`'s MINIMUM (len 1..15000), `TagKey`/`TagValue` or `@required`.
    // `SecretString`'s MAXIMUM is deliberately absent: an oversized `@sensitive`
    // payload is refused one layer earlier, by the front door, with HTTP 403
    // `ForbiddenException` — the `front_door_forbidden` suite pins it. So is
    // `TagsMap`'s entry COUNT: that limit is the tagging front end's and carries its
    // own identity (`tag_count_front_end`).
    let over_long_catalog = "c".repeat(65);
    let over_long_product = "p".repeat(65);

    let refused: Vec<(&str, &str, &str, Value)> = vec![
        (
            "catalog carrying a space and a bang",
            "bad catalog 1!",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "catalog carrying a digit, which its pattern excludes",
            "AWSMarketplace1",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "catalog one character past its maximum",
            &over_long_catalog,
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "product id carrying a bang",
            "AWSMarketplace",
            "prod!1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "product id one character past its maximum",
            "AWSMarketplace",
            &over_long_product,
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "agreementId outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "bad id!", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "agreementId absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "deploymentParameter absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1" }),
        ),
        (
            "parameter name absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "secretString": "s" } }),
        ),
        (
            "parameter name empty",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "", "secretString": "s" } }),
        ),
        (
            "parameter name one character past its maximum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "n".repeat(PARAMETER_NAME_MAX_LEN + 1), "secretString": "s" },
            }),
        ),
        (
            "parameter name outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "bad name!", "secretString": "s" } }),
        ),
        (
            "secretString absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N" } }),
        ),
        (
            "secretString empty",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "" } }),
        ),
        (
            "a tag value one character past its maximum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "k": "v".repeat(TAG_VALUE_MAX_LEN + 1) },
            }),
        ),
        (
            "a tag key outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "bad key!": "v" },
            }),
        ),
        (
            "clientToken shorter than its minimum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "clientToken": "too-short",
            }),
        ),
    ];

    for (case, catalog, product_id, body) in refused {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // A legal boundary row the refusals must not have swallowed: the shortest
    // product, agreement, name and secret the model admits.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "p",
        &json!({ "agreementId": "a", "deploymentParameter": { "name": "n", "secretString": "s" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the shortest legal request must succeed, got {status} {created:?}"
    );
}

#[tokio::test]
async fn a_tag_write_past_the_tagging_limits_is_refused_and_stores_nothing() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-tags", &["prod-tags"]);

    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-tags",
        &json!({
            "agreementId": "agmt-tags",
            "deploymentParameter": { "name": "Tagged", "secretString": "s" },
            "tags": { "env": "dev" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // `TagResource$tags` targets the tag map the model leaves UNCONSTRAINED, so what
    // refuses a write here is AWS's own tagging limits and nothing else: a key must
    // carry at least one character and at most `TAG_KEY_MAX_LEN`, a value at most
    // `TAG_VALUE_MAX_LEN`, and the record may not pass `MAX_TAGS_PER_RESOURCE`. The
    // keys and values those limits ADMIT — spaces, letters outside ASCII, an empty
    // value — are the `unconstrained_tag_characters` suite's rule.
    //
    // The limits differ in WHOSE they are, which is what the expected code per row
    // says: a bound on what a tag IS is this service's own modelled input error,
    // while the COUNT of tags a resource may hold is the tagging front end's, with
    // its own unmodelled identity (`tag_count_front_end` owns that rule).
    let over_the_cap: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|i| (format!("extra-{i:02}"), "v".to_string()))
        .collect();
    let refused: Vec<(&str, BTreeMap<String, String>, &str)> = vec![
        ("an empty key", tag_map(&[("", "v")]), "ValidationException"),
        (
            "a key one character past its maximum",
            [("k".repeat(TAG_KEY_MAX_LEN + 1), "v".to_string())]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a value one character past its maximum",
            [("k".to_string(), "v".repeat(TAG_VALUE_MAX_LEN + 1))]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a merge one entry past the cap",
            over_the_cap,
            "BadRequestException",
        ),
    ];

    for (case, tags, expected_code) in refused {
        let (status, answer) = tag(&router, &arn, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {case:?}: expected {expected_code}, got {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("env", "dev")]),
            "case {case:?}: a refused TagResource must store nothing, got {listed:?}"
        );
    }

    // The boundary cases that DO fit: a key and a value at their maxima, and a
    // merge landing exactly on the cap.
    let at_the_bounds: BTreeMap<String, String> = [
        ("k".repeat(TAG_KEY_MAX_LEN), "v".repeat(TAG_VALUE_MAX_LEN)),
        ("punct".to_string(), "a/b+c=d.e:f@g_h-i".to_string()),
    ]
    .into_iter()
    .collect();
    let (status, answer) = tag(&router, &arn, &at_the_bounds).await;
    assert_eq!(
        status, 204,
        "tags at their modelled maxima must be accepted, got {status} {answer:?}"
    );
    let onto_the_cap: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE - 3)
        .map(|i| (format!("fill-{i:02}"), "v".to_string()))
        .collect();
    let (status, answer) = tag(&router, &arn, &onto_the_cap).await;
    assert_eq!(
        status, 204,
        "a merge landing exactly on the cap must be accepted, got {status} {answer:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed).len(),
        MAX_TAGS_PER_RESOURCE,
        "expected the record to hold exactly the cap, got {listed:?}"
    );
}

#[tokio::test]
async fn a_client_token_identifies_exactly_one_write() {
    let (router, engine) = emulator();
    // Every identity this test aims the token at is a target the account holds, so
    // what the rows below observe is the token rule and not a missing agreement.
    provision(
        &engine,
        "AWSMarketplace",
        "agmt-token",
        &["prod-token", "prod-other"],
    );
    provision(&engine, "AWSMarketplace", "agmt-other", &["prod-token"]);
    // 36 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s len 32..64.
    let token = "0123456789abcdef0123456789abcdef0123";

    let first_body = json!({
        "agreementId": "agmt-token",
        "deploymentParameter": { "name": "Tokened", "secretString": "s" },
        "tags": { "env": "dev" },
        "clientToken": token,
    });
    let (status, created) = put(&router, "AWSMarketplace", "prod-token", &first_body).await;
    assert_eq!(status, 200, "the first Put must succeed, got {created:?}");

    // The same token aimed at the same identity is a REPLAY: the identical output,
    // and no second resource.
    let replay_body = json!({
        "agreementId": "agmt-token",
        "deploymentParameter": { "name": "Tokened", "secretString": "rotated" },
        "tags": { "late": "tag" },
        "clientToken": token,
    });
    let (status, replayed) = put(&router, "AWSMarketplace", "prod-token", &replay_body).await;
    assert_eq!(status, 200, "the replay must succeed, got {replayed:?}");
    assert_eq!(
        replayed, created,
        "a clientToken replay must answer the identical output, got {replayed:?} against {created:?}"
    );

    // The same token aimed at a DIFFERENT parameter conflicts: one token identifies
    // one write.
    let conflicting: Vec<(&str, &str, &str, Value)> = vec![
        (
            "another parameter name",
            "AWSMarketplace",
            "prod-token",
            json!({
                "agreementId": "agmt-token",
                "deploymentParameter": { "name": "Other", "secretString": "s" },
                "clientToken": token,
            }),
        ),
        (
            "another agreement",
            "AWSMarketplace",
            "prod-token",
            json!({
                "agreementId": "agmt-other",
                "deploymentParameter": { "name": "Tokened", "secretString": "s" },
                "clientToken": token,
            }),
        ),
        (
            "another product",
            "AWSMarketplace",
            "prod-other",
            json!({
                "agreementId": "agmt-token",
                "deploymentParameter": { "name": "Tokened", "secretString": "s" },
                "clientToken": token,
            }),
        ),
    ];
    let bound_id = created
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();
    for (case, catalog, product_id, body) in conflicting {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a clientToken must conflict, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            answer.get("resourceId").and_then(Value::as_str),
            Some(bound_id.as_str()),
            "case {case:?}: the conflict must name the parameter the token is bound to, \
             got {answer:?}"
        );
    }

    // A FRESH token for the same identity is the ordinary update-in-place path.
    let other_token = "fedcba9876543210fedcba9876543210fedc";
    let (status, updated) = put(
        &router,
        "AWSMarketplace",
        "prod-token",
        &json!({
            "agreementId": "agmt-token",
            "deploymentParameter": { "name": "Tokened", "secretString": "rotated" },
            "clientToken": other_token,
        }),
    )
    .await;
    assert_eq!(status, 200, "a fresh token must succeed, got {updated:?}");
    assert_eq!(
        updated.get("deploymentParameterId").and_then(Value::as_str),
        Some(bound_id.as_str()),
        "a fresh token for the same identity must still update in place, got {updated:?}"
    );
}

#[tokio::test]
async fn the_client_token_ledger_is_bounded_and_evicts_the_oldest_token() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-ledger", &["prod-ledger"]);

    /// The i-th distinct token: 32 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, the
    /// minimum `ClientToken` length.
    fn token(i: usize) -> String {
        format!("token-{i:026}")
    }
    /// A Put body for `name` under one fixed catalog/product/agreement.
    fn body(name: &str, client_token: &str) -> Value {
        json!({
            "agreementId": "agmt-ledger",
            "deploymentParameter": { "name": name, "secretString": "s" },
            "clientToken": client_token,
        })
    }

    // The ledger grows by one per fresh token, so it is capped with FIFO eviction;
    // past the cap the oldest token stops replaying, which is the same answer the
    // service gives for a token it never saw.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("Ledger", &token(0)),
    )
    .await;
    assert_eq!(status, 200, "the first Put must succeed, got {created:?}");
    let first_id = created
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();

    // Fill the ledger past its cap with fresh tokens aimed at the SAME identity, so
    // nothing new is minted and no other bound is tripped.
    for i in 1..=MAX_CLIENT_TOKENS {
        let (status, answer) = put(
            &router,
            "AWSMarketplace",
            "prod-ledger",
            &body("Ledger", &token(i)),
        )
        .await;
        assert_eq!(
            status, 200,
            "token {i} must update in place, got {status} {answer:?}"
        );
        assert_eq!(
            answer.get("deploymentParameterId").and_then(Value::as_str),
            Some(first_id.as_str()),
            "token {i} must not mint a second resource, got {answer:?}"
        );
    }

    // The oldest token has been evicted, so it no longer binds anything: aiming it
    // at another identity is an ordinary create rather than a conflict.
    let (status, evicted) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("LedgerAfterEviction", &token(0)),
    )
    .await;
    assert_eq!(
        status, 200,
        "an evicted token must no longer conflict, got {status} {evicted:?}"
    );
    let evicted_id = evicted
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_parameter_id(&evicted_id) && evicted_id != first_id,
        "an evicted token must mint the new identity's own resource, got {evicted:?}"
    );

    // A token still inside the cap is still honoured, so eviction is bounded-oldest
    // rather than a lost ledger.
    let (status, answer) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("LedgerStillBound", &token(MAX_CLIENT_TOKENS)),
    )
    .await;
    assert_eq!(
        status, 409,
        "the newest token must still be bound to its write, got {status} {answer:?}"
    );
    assert_eq!(
        answer.get("resourceId").and_then(Value::as_str),
        Some(first_id.as_str()),
        "the conflict must name the parameter the newest token is bound to, got {answer:?}"
    );
}

#[tokio::test]
async fn interleaved_tag_and_untag_calls_track_an_independent_tag_model() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-model", &["prod-model"]);
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-model",
        &json!({
            "agreementId": "agmt-model",
            "deploymentParameter": { "name": "Modelled", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // The invariant: for ANY interleaving of TagResource and UntagResource calls,
    // ListTagsForResource reports the map an independent model (a plain map with
    // insert-overwrites and removes) holds. The sequence is generated by a fixed-seed
    // LCG so the run is bounded and reproducible from this constant alone; nothing is
    // written to disk.
    const SEED: u64 = 0x2026_1021_2308_25A5;
    const ROUNDS: usize = 48;
    let keys: Vec<String> = ["alpha", "beta", "gamma.delta", "e+f", "g=h", "i:j", "k@l", "m_n"]
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    let values = ["0", "1", "two", "three-3", "v.4"];

    let mut rng = SEED;
    let mut next = move || {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (rng >> 33) as usize
    };

    let mut model: BTreeMap<String, String> = BTreeMap::new();
    for round in 0..ROUNDS {
        let count = 1 + next() % 3;
        let chosen: Vec<String> = (0..count)
            .map(|_| keys[next() % keys.len()].clone())
            .collect();
        let is_tag = next() % 2 == 0;
        if is_tag {
            let writes: BTreeMap<String, String> = chosen
                .iter()
                .map(|key| (key.clone(), values[next() % values.len()].to_string()))
                .collect();
            let (status, answer) = tag(&router, &arn, &writes).await;
            assert_eq!(
                status, 204,
                "round {round} TagResource({writes:?}) must answer 204, got {status} {answer:?}"
            );
            model.extend(writes);
        } else {
            let (status, answer) = untag(&router, &arn, &chosen).await;
            assert_eq!(
                status, 204,
                "round {round} UntagResource({chosen:?}) must answer 204, got {status} {answer:?}"
            );
            for key in &chosen {
                model.remove(key);
            }
        }

        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "round {round}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            model,
            "round {round} (seed {SEED:#x}, {}): expected the model's tag map, got {listed:?}",
            if is_tag { "after a tag" } else { "after an untag" }
        );
    }

    // A read never changes what a later reader sees.
    let (_, first) = list_tags(&router, &arn).await;
    let (_, second) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&first),
        tags_of(&second),
        "ListTagsForResource is @readonly: two reads must agree, got {first:?} then {second:?}"
    );
}
