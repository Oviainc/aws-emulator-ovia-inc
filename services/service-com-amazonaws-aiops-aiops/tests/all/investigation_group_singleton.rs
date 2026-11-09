// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rules this service enforces ITSELF — the ones the
//! Smithy model cannot express, so no boundary walk applies them.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! One rule per test, each asserted over a SET of inputs rather than one case:
//!
//! 1. An account holds at most [`MAX_INVESTIGATION_GROUPS_PER_REGION`] investigation
//!    group in each Region; a create in a Region that already holds that many is
//!    refused with `ServiceQuotaExceededException` (402), worded as the service words
//!    that error class, and stores nothing. The check and the insert happen under one
//!    lock, so concurrent creates yield exactly one winner.
//! 2. A group's encryption configuration is always reported: omitting it on the
//!    create selects the Amazon Web Services owned key, and that is what a read
//!    reports.
//! 3. A group is addressed by its ARN alone; every other identifier the modelled
//!    pattern admits — the group's own name included — resolves to nothing, on every
//!    operation that takes one. (The full table lives in
//!    `investigation_group_identifier_forms.rs`.)
//! 4. A resource policy document must be a JSON policy document; one that is not is
//!    refused and not attached.
//! 5. A `crossAccountConfigurations` entry must name an account other than the
//!    investigation account.
//! 6. A successful mutation advances `lastModifiedAt`; a read never changes it.
//! 7. A tag write may not take a resource past the published per-resource tag
//!    maximum, and a refused one applies no tag.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES;
use service_com_amazonaws_aiops_aiops::state::{
    AWS_OWNED_KEY_TYPE, MAX_INVESTIGATION_GROUPS_PER_REGION, MAX_TAGS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries,
/// and the account an investigation group belongs to.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account, for the cross-account rows.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The region a case signs for when it only needs one.
const REGION: &str = "us-west-2";

/// The error code the RESOURCE operations report an unresolvable identifier with:
/// this service's own modelled `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the TAGGING operations report an unresolvable `resourceArn` with:
/// the tagging frontend's own `NotFoundException` — see
/// `state::TAGGING_RESOURCE_NOT_FOUND` for the rule and its source.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// Regions the per-region rows span: the recorded one, two other commercial regions
/// and one whose name is longer and carries more fields.
const REGIONS: &[&str] = &["us-west-2", "eu-central-1", "ap-southeast-4", "us-gov-west-1"];

/// The sentence a create refused by the per-account/Region quota carries, as the
/// service words that error class: fixed text that states no count, no Region and
/// nothing the request sent.
const QUOTA_EXCEEDED_MESSAGE: &str = "Cannot create another investigation group.";

/// The sentence a `PutInvestigationGroupPolicy` carrying something that is not a policy
/// document is refused with, as the service words that error class: fixed text that
/// states no member path and no excerpt of the document.
const INVALID_POLICY_DOCUMENT_MESSAGE: &str = "The specified resource policy document is invalid.";

/// The sentence a cross-account source role that names no account other than the
/// investigation account is refused with: fixed text around the investigation account
/// and the ARN the request sent.
fn source_role_sentence(source_role_arn: &str) -> String {
    format!(
        "Source role ARN cannot belong to the investigation account {ACCOUNT}. \
         Found ARN: {source_role_arn}"
    )
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
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
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
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

/// [`call_in`] in [`REGION`], for a call that addresses no single resource.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// The Region a call addressing `identifier` is signed for.
///
/// This service is regional (`aiops.<region>.<dns-suffix>`), so an endpoint serves only
/// its own Region's groups and a caller reaches a group through the Region its ARN
/// names. An identifier that is not a group ARN names no Region, so those rows sign for
/// [`REGION`] — and still resolve nothing, which is what they assert.
fn region_of(identifier: &str) -> String {
    identifier
        .strip_prefix("arn:")
        .map(|rest| rest.split(':').collect::<Vec<_>>())
        .filter(|fields| fields.len() > 3 && fields[1] == "aiops")
        .map_or_else(|| REGION.to_string(), |fields| fields[2].to_string())
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is
/// what an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/investigationGroups", Some(body)).await
}

/// Create a group in `region` that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = create_in(router, region, body).await;
    assert_eq!(
        status, 201,
        "create {body} in {region}: expected 201, got {status} / {created:?}"
    );
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string();
    assert!(
        arn.starts_with(&format!("arn:aws:aiops:{region}:{ACCOUNT}:investigation-group/")),
        "create {body} in {region}: expected a {region} ARN, got {arn:?}"
    );
    arn
}

/// A minimal create body carrying `name`.
fn minimal(name: &str) -> Value {
    json!({ "name": name, "roleArn": ROLE_ARN })
}

async fn get_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(identifier)),
        None,
    )
    .await
}

/// Read a group back, asserting the read itself succeeded.
async fn get_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> Value {
    let (status, body) = get_in(router, region, identifier).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {body:?}"
    );
    body
}

async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    patch: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(identifier)),
        Some(patch),
    )
    .await
}

async fn put_policy(
    router: &MultiServiceRouter,
    identifier: &str,
    document: &str,
) -> (u16, Value) {
    call_in(
        router,
        &region_of(identifier),
        "POST",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        Some(&json!({ "policy": document })),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call_in(
        router,
        &region_of(identifier),
        "GET",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        None,
    )
    .await
}

/// The names and ARNs `ListInvestigationGroups` reports in `region`.
async fn listed(router: &MultiServiceRouter, region: &str) -> Vec<(String, String)> {
    let (status, body) = call_in(router, region, "GET", "/investigationGroups", None).await;
    assert_eq!(
        status, 200,
        "listing {region}: expected 200, got {status} / {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "listing {region}: a region holds at most {MAX_INVESTIGATION_GROUPS_PER_REGION} group, \
         so there is never a next page, got {body:?}"
    );
    body.get("investigationGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("listing {region}: no investigationGroups in {body:?}"))
        .iter()
        .map(|item| {
            (
                item.get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// A tag map body from pairs.
fn tags_body(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// `count` distinct tag pairs satisfying the modelled `TagKey`/`TagValue` constraints.
fn tag_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|n| (format!("key{n:03}"), format!("value{n:03}")))
        .collect()
}

async fn tag(router: &MultiServiceRouter, identifier: &str, tags: &Value) -> (u16, Value) {
    call_in(
        router,
        &region_of(identifier),
        "POST",
        &format!("/tags/{}", label(identifier)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// The tag set `ListTagsForResource` reports for `identifier`.
async fn stored_tags(router: &MultiServiceRouter, identifier: &str) -> BTreeMap<String, String> {
    let (status, body) = call_in(
        router,
        &region_of(identifier),
        "GET",
        &format!("/tags/{}", label(identifier)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading tags back: expected 200, got {status} / {body:?}"
    );
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("ListTagsForResource must report a tag set, got {body:?}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// Creating an investigation group is a one-time setup task per Region: an account
/// holds at most [`MAX_INVESTIGATION_GROUPS_PER_REGION`] group there, a create
/// attempted while that many exist is refused with `ServiceQuotaExceededException`
/// (402) and stores nothing, and the Region's one slot frees again on a delete.
///
/// Rows differ along the REGION (each has its own independent slot) and along what the
/// refused create asked for: another name, the same name, and a fuller member set.
#[tokio::test]
async fn an_account_holds_one_investigation_group_per_region() {
    let router = emulator();

    let mut first_arns: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        // Up to the quota, every create is accepted.
        let mut accepted = Vec::new();
        for slot in 0..MAX_INVESTIGATION_GROUPS_PER_REGION {
            accepted.push(create_ok(&router, region, &minimal(&format!("shared-name-{slot}"))).await);
        }
        let first = accepted
            .last()
            .unwrap_or_else(|| panic!("{region}: the quota is at least one group"))
            .clone();

        // Past it, every create is refused — whatever it asks for.
        let refused = [
            ("another name", minimal("second-group")),
            ("the name the first one took", minimal("shared-name-0")),
            (
                "a fuller member set",
                json!({
                    "name": "third-group",
                    "roleArn": ROLE_ARN,
                    "retentionInDays": 30,
                    "isCloudTrailEventHistoryEnabled": false,
                    "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY",
                                                 "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/k" },
                    "tags": { "Team": "ops" },
                }),
            ),
        ];
        for (why, body) in &refused {
            let case = format!("{region}: a second create with {why}");
            let (status, answer) = create_in(&router, region, body).await;
            assert_eq!(
                status, 402,
                "case {case:?}: expected 402, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
            );
            assert_eq!(
                message(&answer),
                QUOTA_EXCEEDED_MESSAGE,
                "case {case:?}: expected {QUOTA_EXCEEDED_MESSAGE:?}, got {:?}",
                message(&answer)
            );
            assert!(
                answer.get("arn").is_none(),
                "case {case:?}: a refused create mints no ARN, got {answer:?}"
            );

            // Nothing was stored: the Region still reports exactly the groups it held.
            let entries = listed(&router, region).await;
            assert_eq!(
                entries.len(),
                MAX_INVESTIGATION_GROUPS_PER_REGION,
                "case {case:?}: expected {MAX_INVESTIGATION_GROUPS_PER_REGION} group(s) in \
                 {region}, got {entries:?}"
            );
            let group = get_ok(&router, region, &first, &case).await;
            assert_eq!(
                group.get("retentionInDays").and_then(Value::as_i64),
                Some(90),
                "case {case:?}: the refused create must not have altered the stored group, \
                 got {group:?}"
            );
        }

        first_arns.push((region, first));
    }

    // Each Region's slot is its own: every group created above is still readable in its
    // own Region, and no Region's listing carries another Region's group.
    for (region, arn) in &first_arns {
        let entries = listed(&router, region).await;
        assert_eq!(
            entries.iter().map(|(arn, _)| arn.clone()).collect::<Vec<_>>(),
            vec![arn.clone()],
            "case {region:?}: a Region lists exactly its own group, got {entries:?}"
        );
    }

    // Deleting the Region's group frees the slot for exactly one more create, which
    // gets a NEW ARN.
    for (region, arn) in &first_arns {
        let (status, body) = call_in(
            &router,
            region,
            "DELETE",
            &format!("/investigationGroups/{}", label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: deleting the group must succeed, got {status} / {body:?}"
        );
        let reborn = create_ok(&router, region, &minimal("after-the-delete")).await;
        assert_ne!(
            &reborn, arn,
            "case {region:?}: a recreated group must get a fresh id"
        );
        let (status, answer) = create_in(&router, region, &minimal("one-too-many")).await;
        assert_eq!(
            status, 402,
            "case {region:?}: the freed slot takes exactly one create, got {status} / {answer:?}"
        );
    }
}

/// The quota check and the insert happen under ONE lock, so N creates racing in one
/// Region converge on exactly one winner and N-1 `ServiceQuotaExceededException`
/// losers. Rows differ along the number of racers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_in_one_region_yield_one_winner() {
    for racers in [2usize, 5, 8] {
        let router = Arc::new(emulator());
        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            tasks.push(tokio::spawn(async move {
                create_in(&router, REGION, &minimal(&format!("racer-{racer}"))).await
            }));
        }

        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for task in tasks {
            let (status, body) = task.await.expect("a racer task must not panic");
            match status {
                201 => winners.push(
                    body.get("arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                _ => losers.push((status, error_code(&body))),
            }
        }

        assert_eq!(
            winners.len(),
            MAX_INVESTIGATION_GROUPS_PER_REGION,
            "case {racers} racers: expected exactly \
             {MAX_INVESTIGATION_GROUPS_PER_REGION} winner(s), got {winners:?} / {losers:?}"
        );
        assert_eq!(
            losers.len(),
            racers - MAX_INVESTIGATION_GROUPS_PER_REGION,
            "case {racers} racers: expected the rest to lose, got {winners:?} / {losers:?}"
        );
        for loser in &losers {
            assert_eq!(
                (loser.0, loser.1.as_str()),
                (402, "ServiceQuotaExceededException"),
                "case {racers} racers: a loser must be refused with the quota error, \
                 got {losers:?}"
            );
        }

        // The store agrees with the winners: the Region holds exactly their groups.
        let entries = listed(&router, REGION).await;
        assert_eq!(
            entries.iter().map(|(arn, _)| arn.clone()).collect::<Vec<_>>(),
            winners,
            "case {racers} racers: the Region must hold exactly the winner, got {entries:?}"
        );
    }
}

/// A group's encryption configuration is always reported. Omitting it on the create
/// selects the Amazon Web Services owned key — the `AWS_OWNED_KEY` member of the
/// modelled value set, with no `kmsKeyId` — and a configuration the create supplied is
/// reported as given.
///
/// The value set is the modelled `EncryptionConfigurationType`
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`), surfaced as
/// `ENCRYPTION_CONFIGURATION_TYPE_VALUES`.
#[tokio::test]
async fn an_omitted_encryption_configuration_reads_back_as_the_aws_owned_key() {
    assert!(
        ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&AWS_OWNED_KEY_TYPE),
        "the default key type must be one the model admits, got {AWS_OWNED_KEY_TYPE:?} \
         against {ENCRYPTION_CONFIGURATION_TYPE_VALUES:?}"
    );

    let router = emulator();
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/abcd-1234");

    // Rows: the member absent, each value of the modelled set present, and the
    // customer managed key with its `kmsKeyId`.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no encryptionConfiguration at all",
            minimal("default-key"),
            json!({ "type": AWS_OWNED_KEY_TYPE }),
        ),
        (
            "the AWS owned key, stated",
            json!({ "name": "stated-default", "roleArn": ROLE_ARN,
                    "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE } }),
            json!({ "type": AWS_OWNED_KEY_TYPE }),
        ),
        (
            "a customer managed key",
            json!({ "name": "customer-key", "roleArn": ROLE_ARN,
                    "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY",
                                                 "kmsKeyId": key } }),
            json!({ "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": key }),
        ),
    ];

    for (index, (case, body, expected)) in cases.iter().enumerate() {
        let region = REGIONS[index % REGIONS.len()];
        let arn = create_ok(&router, region, body).await;
        let group = get_ok(&router, region, &arn, case).await;
        let got = group.get("encryptionConfiguration");
        assert_eq!(
            got,
            Some(expected),
            "case {case:?}: expected encryptionConfiguration {expected}, got {got:?} \
             in {group:?}"
        );

        // A read does not change it.
        let again = get_ok(&router, region, &arn, case).await;
        assert_eq!(
            again.get("encryptionConfiguration"),
            Some(expected),
            "case {case:?}: a second read must report the same configuration, got {again:?}"
        );
        // An update that omits the member is an update that names no key, and the
        // member documents what the service does then: the Amazon Web Services owned
        // key encrypts the data. So the group reports that key afterwards, whatever it
        // carried before.
        let (status, answer) = update_in(
            &router,
            region,
            &arn,
            &json!({ "isCloudTrailEventHistoryEnabled": false }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} / {answer:?}"
        );
        let after = get_ok(&router, region, &arn, case).await;
        assert_eq!(
            after.get("encryptionConfiguration"),
            Some(&json!({ "type": AWS_OWNED_KEY_TYPE })),
            "case {case:?}: an update omitting encryptionConfiguration must store the \
             Amazon Web Services owned key, got {after:?}"
        );

        // And an update that DOES name it replaces it.
        let replacement = json!({ "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": key });
        let (status, answer) = update_in(
            &router,
            region,
            &arn,
            &json!({ "encryptionConfiguration": replacement }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: replacing the configuration must succeed, got {status} / {answer:?}"
        );
        let after = get_ok(&router, region, &arn, case).await;
        assert_eq!(
            after.get("encryptionConfiguration"),
            Some(&replacement),
            "case {case:?}: expected the replacement reported back, got {after:?}"
        );
    }
}

/// A group is addressed by its ARN — the one address this service resolves. Every
/// OTHER identifier the modelled `InvestigationGroupIdentifier` pattern admits reaches
/// the handler and resolves nothing: the bare-label alternative is a well-formedness
/// rule, not an address, so the service reports the group absent for it.
///
/// Rows differ along the identifier FORM (the group's own bare id, the label the group
/// carries as its NAME, and a label nothing carries) and, for the ARN form, along which
/// single ARN field is changed; they are asserted against every operation that takes an
/// identifier. The tagging operations' `resourceArn` resolves the same way, so a bare
/// form never addresses a tag set either.
#[tokio::test]
async fn an_identifier_that_is_not_the_groups_arn_resolves_nothing() {
    let router = emulator();
    let name = "addressable-only-by-arn";
    let arn = create_ok(&router, REGION, &minimal(name)).await;
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string();

    let unresolvable = [
        ("the group's own bare id", id.clone()),
        ("the group's own name", name.to_string()),
        ("a label nothing carries", "no-such-group".to_string()),
        ("an id-shaped label nothing carries", "A".repeat(16)),
        (
            "the ARN in another region",
            arn.replace(&format!(":{REGION}:"), ":eu-west-1:"),
        ),
        (
            "the ARN in another account",
            arn.replace(ACCOUNT, FOREIGN_ACCOUNT),
        ),
        (
            "the ARN in another partition",
            arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
        ("the ARN with another id", arn.replace(&id, &"Z".repeat(16))),
    ];

    // Every operation that resolves a group through an identifier, as
    // (name, method, uri template, body, expected error code). The resource
    // operations report this service's modelled `ResourceNotFoundException`; the
    // three tagging operations are served by the tagging frontend, whose not-found
    // answer carries its own `NotFoundException`
    // (`state::TAGGING_RESOURCE_NOT_FOUND`).
    let operations: Vec<(&str, &str, &str, Option<Value>, &str)> = vec![
        (
            "GetInvestigationGroup",
            "GET",
            "/investigationGroups/{}",
            None,
            RESOURCE_NOT_FOUND,
        ),
        (
            "UpdateInvestigationGroup",
            "PATCH",
            "/investigationGroups/{}",
            Some(json!({ "isCloudTrailEventHistoryEnabled": true })),
            RESOURCE_NOT_FOUND,
        ),
        (
            "DeleteInvestigationGroup",
            "DELETE",
            "/investigationGroups/{}",
            None,
            RESOURCE_NOT_FOUND,
        ),
        (
            "GetInvestigationGroupPolicy",
            "GET",
            "/investigationGroups/{}/policy",
            None,
            RESOURCE_NOT_FOUND,
        ),
        (
            "PutInvestigationGroupPolicy",
            "POST",
            "/investigationGroups/{}/policy",
            Some(json!({ "policy": r#"{"Version":"2012-10-17"}"# })),
            RESOURCE_NOT_FOUND,
        ),
        (
            "DeleteInvestigationGroupPolicy",
            "DELETE",
            "/investigationGroups/{}/policy",
            None,
            RESOURCE_NOT_FOUND,
        ),
        ("ListTagsForResource", "GET", "/tags/{}", None, TAGGING_NOT_FOUND),
        (
            "TagResource",
            "POST",
            "/tags/{}",
            Some(json!({ "tags": { "Team": "ops" } })),
            TAGGING_NOT_FOUND,
        ),
        (
            "UntagResource",
            "DELETE",
            "/tags/{}?tagKeys=Team",
            None,
            TAGGING_NOT_FOUND,
        ),
    ];

    for (form, identifier) in &unresolvable {
        for (operation, method, template, body, expected_code) in &operations {
            let case = format!("{operation} addressed by {form}");
            let uri = template.replace("{}", &label(identifier));
            let (status, answer) = call(&router, method, &uri, body.as_ref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                *expected_code,
                "case {case:?}: expected {expected_code}, got {answer:?}"
            );
            assert!(
                !message(&answer).contains(identifier),
                "case {case:?}: the message must not echo the identifier {identifier:?}, \
                 got {answer:?}"
            );
        }
    }

    // The ARN still resolves, so the 404s above are about the identifier form and not
    // about the group having gone missing.
    let group = get_ok(&router, REGION, &arn, "the ARN itself").await;
    assert_eq!(
        group.get("name").and_then(Value::as_str),
        Some(name),
        "the ARN must still resolve the group, got {group:?}"
    );
    let (status, tagged) = tag(&router, &arn, &json!({ "Team": "ops" })).await;
    assert_eq!(
        status, 200,
        "tagging through the ARN must succeed, got {status} / {tagged:?}"
    );
}

/// A resource policy document must be a policy document in JSON format: one that is
/// not is refused with `ValidationException` (400) and nothing is attached, so a later
/// read still reports the policy absent. A document that IS one round-trips byte for
/// byte.
///
/// Rows differ along the shape of the document: non-JSON text, a truncated object, a
/// JSON value that is not an object, and — on the accepted side — the smallest object,
/// a realistic policy, and one carrying the whitespace and Latin-1 bytes the modelled
/// `^[\u0009\u000A\u000D\u0020-\u00FF]+$` admits.
#[tokio::test]
async fn a_resource_policy_must_be_a_json_document() {
    let router = emulator();
    let arn = create_ok(&router, REGION, &minimal("policy-bearer")).await;

    let refused = [
        ("the recorded malformed document", "{not-valid-json:::".to_string()),
        ("a single character", "x".to_string()),
        ("a truncated object", r#"{"Version":"2012-10-17""#.to_string()),
        ("a JSON array", "[]".to_string()),
        ("a JSON string", r#""a policy""#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON null", "null".to_string()),
        ("trailing garbage after an object", r#"{"Version":"2012-10-17"} oops"#.to_string()),
    ];
    for (case, document) in &refused {
        let (status, answer) = put_policy(&router, &arn, document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            message(&answer),
            INVALID_POLICY_DOCUMENT_MESSAGE,
            "case {case:?}: expected {INVALID_POLICY_DOCUMENT_MESSAGE:?}, got {answer:?}"
        );
        assert!(
            !message(&answer).contains(document),
            "case {case:?}: the refusal must not echo the document, got {answer:?}"
        );

        // Nothing was attached.
        let (status, body) = get_policy(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused put must attach nothing, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    let accepted = [
        ("the smallest JSON object", "{}".to_string()),
        (
            "a version-only document",
            r#"{"Version":"2012-10-17"}"#.to_string(),
        ),
        (
            "a realistic alarm policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "aiops.alarms.cloudwatch.amazonaws.com" },
                    "Action": ["aiops:CreateInvestigation", "aiops:CreateInvestigationEvent"],
                    "Resource": "*"
                }]
            })
            .to_string(),
        ),
        (
            "whitespace and Latin-1 inside the document",
            "{\n\t\"Version\": \"2012-10-17\",\r\n\t\"Sid\": \"caf\u{e9}-\u{fc}ber\"\n}".to_string(),
        ),
        (
            "a long document",
            format!("{{\"Sid\":\"{}\"}}", "A".repeat(4096)),
        ),
    ];
    for (case, document) in &accepted {
        let (status, answer) = put_policy(&router, &arn, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {answer:?}"
        );
        let (status, body) = get_policy(&router, &arn).await;
        assert_eq!(
            (status, body.get("policy").and_then(Value::as_str)),
            (200, Some(document.as_str())),
            "case {case:?}: expected the stored document back, got {status} / {body:?}"
        );
    }
}

/// A cross-account configuration must name an account OTHER than the investigation
/// account: a `sourceRoleArn` in the account that owns the group is refused with
/// `ValidationException` (400), worded as the service words that error class — the
/// investigation account and the offending ARN — and the stored configuration is left
/// untouched.
///
/// Rows differ along the operation (create and update), the position of the offending
/// entry in the list, and the list's size — with the foreign-account rows as the
/// positive control.
#[tokio::test]
async fn a_cross_account_source_role_must_name_another_account() {
    let router = emulator();
    let own = |n: u32| format!("arn:aws:iam::{ACCOUNT}:role/Own{n}");
    let foreign = |n: u32| format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/Foreign{n}");
    let entries = |arns: &[String]| -> Value {
        Value::Array(
            arns.iter()
                .map(|arn| json!({ "sourceRoleArn": arn }))
                .collect(),
        )
    };

    // (case, the list, the ARN a refusal must echo — `None` when the list must be
    // accepted; a list with several offending entries is refused at the first of them)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        ("an empty list", entries(&[]), None),
        ("one foreign account", entries(&[foreign(1)]), None),
        (
            "several foreign accounts",
            entries(&[foreign(1), foreign(2), foreign(3)]),
            None,
        ),
        (
            "the investigation account alone",
            entries(&[own(1)]),
            Some(own(1)),
        ),
        (
            "the investigation account first",
            entries(&[own(1), foreign(1)]),
            Some(own(1)),
        ),
        (
            "the investigation account last",
            entries(&[foreign(1), foreign(2), own(1)]),
            Some(own(1)),
        ),
        (
            "two entries in the investigation account",
            entries(&[foreign(1), own(2), own(3)]),
            Some(own(2)),
        ),
    ];

    for (index, (case, list, refused)) in cases.iter().enumerate() {
        // A create carrying the list.
        let region = REGIONS[index % REGIONS.len()];
        let body = json!({
            "name": format!("cross-create-{index}"),
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": list,
        });
        let (status, answer) = create_in(&router, region, &body).await;
        if let Some(offending) = refused {
            assert_eq!(
                status, 400,
                "case {case:?} on create: expected 400, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?} on create: expected ValidationException, got {answer:?}"
            );
            let expected = source_role_sentence(offending);
            assert_eq!(
                message(&answer),
                expected,
                "case {case:?} on create: expected {expected:?}, got {:?}",
                message(&answer)
            );
            let left = listed(&router, region).await;
            assert!(
                left.is_empty(),
                "case {case:?} on create: a refused create stores nothing, got {left:?}"
            );
        } else {
            assert_eq!(
                status, 201,
                "case {case:?} on create: expected 201, got {status} / {answer:?}"
            );
            let arn = answer
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let group = get_ok(&router, region, &arn, case).await;
            let want = list.as_array().filter(|list| !list.is_empty());
            assert_eq!(
                group
                    .get("crossAccountConfigurations")
                    .and_then(Value::as_array),
                want,
                "case {case:?} on create: expected the list stored as given, got {group:?}"
            );
            // Clear the Region's one slot for the next row.
            call_in(
                &router,
                region,
                "DELETE",
                &format!("/investigationGroups/{}", label(&arn)),
                None,
            )
            .await;
        }
    }

    // The same rule on the update path, against a group whose stored list is the
    // positive control.
    let region = REGIONS[0];
    let arn = create_ok(
        &router,
        region,
        &json!({
            "name": "cross-update",
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": entries(&[foreign(9)]),
        }),
    )
    .await;
    let stored = entries(&[foreign(9)]);

    for (case, list, refused) in &cases {
        let (status, answer) = update_in(
            &router,
            region,
            &arn,
            &json!({ "crossAccountConfigurations": list }),
        )
        .await;
        if let Some(offending) = refused {
            assert_eq!(
                status, 400,
                "case {case:?} on update: expected 400, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?} on update: expected ValidationException, got {answer:?}"
            );
            let expected = source_role_sentence(offending);
            assert_eq!(
                message(&answer),
                expected,
                "case {case:?} on update: expected {expected:?}, got {:?}",
                message(&answer)
            );
            let group = get_ok(&router, region, &arn, case).await;
            assert_eq!(
                group.get("crossAccountConfigurations"),
                Some(&stored),
                "case {case:?} on update: a refused update must leave the stored list, \
                 got {group:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?} on update: expected 200, got {status} / {answer:?}"
            );
            // Put the control back for the next row.
            update_in(
                &router,
                region,
                &arn,
                &json!({ "crossAccountConfigurations": stored }),
            )
            .await;
        }
    }
}

/// Every successful mutation re-stamps `lastModifiedAt`, so it is strictly greater
/// after the call than before; `createdAt` never moves, and a read changes neither.
///
/// Rows differ along the mutating operation and the member it changes.
#[tokio::test]
async fn every_mutation_advances_last_modified_at() {
    let router = emulator();
    let arn = create_ok(&router, REGION, &minimal("modified-stamps")).await;

    let created = get_ok(&router, REGION, &arn, "the create").await;
    let created_at = created
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("no createdAt in {created:?}"));
    assert_eq!(
        created.get("lastModifiedAt").and_then(Value::as_i64),
        Some(created_at),
        "an untouched group's lastModifiedAt equals its createdAt, got {created:?}"
    );

    // A read changes nothing.
    let again = get_ok(&router, REGION, &arn, "a second read").await;
    assert_eq!(
        again.get("lastModifiedAt").and_then(Value::as_i64),
        Some(created_at),
        "a read must not advance lastModifiedAt, got {again:?}"
    );

    let mutations: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "UpdateInvestigationGroup replacing roleArn",
            "PATCH",
            format!("/investigationGroups/{}", label(&arn)),
            Some(json!({ "roleArn": "arn:aws:iam::000000000000:role/OtherAIOpsRole" })),
        ),
        (
            "UpdateInvestigationGroup replacing retention-independent members",
            "PATCH",
            format!("/investigationGroups/{}", label(&arn)),
            Some(json!({ "isCloudTrailEventHistoryEnabled": false,
                         "tagKeyBoundaries": ["Team", "Service"] })),
        ),
        (
            "UpdateInvestigationGroup naming no member at all",
            "PATCH",
            format!("/investigationGroups/{}", label(&arn)),
            Some(json!({})),
        ),
        (
            "PutInvestigationGroupPolicy",
            "POST",
            format!("/investigationGroups/{}/policy", label(&arn)),
            Some(json!({ "policy": r#"{"Version":"2012-10-17"}"# })),
        ),
        (
            "DeleteInvestigationGroupPolicy",
            "DELETE",
            format!("/investigationGroups/{}/policy", label(&arn)),
            None,
        ),
    ];

    let mut previous = created_at;
    for (case, method, uri, body) in &mutations {
        let (status, answer) = call(&router, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {answer:?}"
        );

        let after = get_ok(&router, REGION, &arn, case).await;
        let stamp = after
            .get("lastModifiedAt")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no lastModifiedAt in {after:?}"));
        assert!(
            stamp > previous,
            "case {case:?}: expected lastModifiedAt to advance past {previous}, got {stamp}"
        );
        assert!(
            stamp >= created_at,
            "case {case:?}: lastModifiedAt may never precede createdAt {created_at}, got {stamp}"
        );
        assert_eq!(
            after.get("createdAt").and_then(Value::as_i64),
            Some(created_at),
            "case {case:?}: createdAt must not move, got {after:?}"
        );
        assert!(
            after
                .get("lastModifiedBy")
                .and_then(Value::as_str)
                .is_some_and(|by| !by.is_empty()),
            "case {case:?}: a mutation re-stamps lastModifiedBy too, got {after:?}"
        );

        // And the stamp the mutation set is what the NEXT read reports too.
        let repeat = get_ok(&router, REGION, &arn, case).await;
        assert_eq!(
            repeat.get("lastModifiedAt").and_then(Value::as_i64),
            Some(stamp),
            "case {case:?}: a read must report the stored stamp, got {repeat:?}"
        );
        previous = stamp;
    }

    // A REFUSED mutation is not a mutation: it leaves the stamp alone.
    let (status, answer) = update_in(
        &router,
        REGION,
        &arn,
        &json!({ "crossAccountConfigurations": [
            { "sourceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/Own") }
        ] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a same-account cross-account update must be refused, got {status} / {answer:?}"
    );
    let after = get_ok(&router, REGION, &arn, "a refused update").await;
    assert_eq!(
        after.get("lastModifiedAt").and_then(Value::as_i64),
        Some(previous),
        "a refused update must not advance lastModifiedAt, got {after:?}"
    );
}

/// A tag write may not take a resource past the published per-resource tag maximum
/// ("You can associate as many as 50 tags with an investigation group", surfaced as
/// `state::MAX_TAGS`): the refusal is HTTP 400 with the error code
/// `BadRequestException`, and no tag from the refused call is applied.
///
/// Rows differ along the operation that writes the tags (create and `TagResource`) and
/// along the resulting set's size: at the maximum, one past it, far past it, and a
/// write that only replaces values at the maximum.
#[tokio::test]
async fn a_tag_write_past_the_maximum_is_refused_as_a_bad_request() {
    let router = emulator();
    let at_max = tag_pairs(MAX_TAGS);

    // A create at the maximum is accepted; one past it is refused and stores nothing.
    let arn = create_ok(
        &router,
        REGIONS[0],
        &json!({ "name": "tags-at-max", "roleArn": ROLE_ARN, "tags": tags_body(&at_max) }),
    )
    .await;
    assert_eq!(
        stored_tags(&router, &arn).await.len(),
        MAX_TAGS,
        "a create at the maximum must store every tag"
    );

    for (case, count) in [
        ("one past the maximum", MAX_TAGS + 1),
        ("far past the maximum", MAX_TAGS * 3),
    ] {
        let region = REGIONS[1];
        let (status, answer) = create_in(
            &router,
            region,
            &json!({ "name": "tags-past-max", "roleArn": ROLE_ARN,
                     "tags": tags_body(&tag_pairs(count)) }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} on create: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {case:?} on create: expected BadRequestException, got {answer:?}"
        );
        let left = listed(&router, region).await;
        assert!(
            left.is_empty(),
            "case {case:?} on create: a refused create stores nothing, got {left:?}"
        );
    }

    // `TagResource` bounds the set the resource ENDS UP with. The group above is at the
    // maximum already, so one new key is refused while replacing values is not.
    let one_more = [("OneTooMany".to_string(), "no".to_string())];
    let (status, answer) = tag(&router, &arn, &tags_body(&one_more)).await;
    assert_eq!(
        status, 400,
        "case \"one key past the maximum\": expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "BadRequestException",
        "case \"one key past the maximum\": expected BadRequestException, got {answer:?}"
    );
    let after = stored_tags(&router, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case \"one key past the maximum\": a refused tag write applies nothing, got {after:?}"
    );
    assert!(
        !after.contains_key("OneTooMany"),
        "case \"one key past the maximum\": the refused key must not be stored, got {after:?}"
    );

    let replacements: Vec<(String, String)> = at_max
        .iter()
        .take(3)
        .map(|(key, _)| (key.clone(), "replaced".to_string()))
        .collect();
    let (status, answer) = tag(&router, &arn, &tags_body(&replacements)).await;
    assert_eq!(
        status, 200,
        "case \"replacing values at the maximum\": expected 200, got {status} / {answer:?}"
    );
    let after = stored_tags(&router, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case \"replacing values at the maximum\": the set must still hold {MAX_TAGS}, \
         got {after:?}"
    );

    // And a write far past the maximum against an untagged resource is refused too.
    let fresh = create_ok(&router, REGIONS[2], &minimal("tagged-later")).await;
    let (status, answer) = tag(&router, &fresh, &tags_body(&tag_pairs(MAX_TAGS + 1))).await;
    assert_eq!(
        status, 400,
        "case \"one past the maximum in one call\": expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "BadRequestException",
        "case \"one past the maximum in one call\": expected BadRequestException, got {answer:?}"
    );
    assert!(
        stored_tags(&router, &fresh).await.is_empty(),
        "case \"one past the maximum in one call\": no tag may be applied"
    );
}
