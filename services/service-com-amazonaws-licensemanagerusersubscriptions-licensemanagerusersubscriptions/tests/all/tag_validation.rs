// SPDX-License-Identifier: CC0-1.0

//! What this service accepts as a TAG, driven over the real wire path through the
//! router's `dispatch`, so every request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request:
//!
//! * a tag this service is asked to persist has to be a legal AWS tag — a key of
//!   [`MIN_TAG_KEY_CHARS`]..=[`MAX_TAG_KEY_CHARS`] characters, a value of at most
//!   [`MAX_TAG_VALUE_CHARS`] characters, and a key outside the [`RESERVED_TAG_PREFIX`]
//!   namespace AWS keeps for its own system tags — whatever `Tags` map carries it, because
//!   the model's only length trait is on the MAP and so constrains no key or value;
//! * a payload breaking that rule is refused with HTTP 400 `ValidationException` and
//!   NOTHING is written: not the illegal tag, not the legal tags beside it in the same
//!   payload, and not the resource a create would have minted;
//! * the same key rule governs the keys `UntagResource` names, and a refusal there removes
//!   nothing;
//! * the lengths are counted in CHARACTERS, not bytes, so a key of multi-byte characters
//!   is admitted up to the same count as an ASCII one.
//!
//! The harness (router + engine + the account's directory fleet the registrations resolve
//! against) is the one in [`super::identity_provider_subscriptions`]; only the rule is new
//! here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::{
    state::{MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, MIN_TAG_KEY_CHARS, RESERVED_TAG_PREFIX},
    ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState,
};

use super::identity_provider_subscriptions as fixture;

/// A directory the harness puts in the account's fleet, `Active` and resolvable.
const DIRECTORY: &str = "d-0123456789";

/// An EC2 instance of the account's seeded fleet (`state::seed_account_instances`), so an
/// association on it is authorized.
const AUTHORIZED_INSTANCE: &str = "i-0f1e2d3c4b5a69788";

/// The wire error code every refusal in this suite carries, and its status.
const VALIDATION_CODE: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// One tag to try, and whether the rule admits it.
#[derive(Debug, Clone)]
struct TagCase {
    case: String,
    key: String,
    value: String,
    legal: bool,
}

/// The table: one row per way a tag can sit against the rule, differing along the
/// dimension the rule generalises over — the LENGTH of the key and of the value (at each
/// published bound and one past it), the CHARACTER WIDTH they are counted in, and whether
/// the key falls in the reserved namespace (and in which case, and where in the key).
///
/// The bounds come from the AWS tagging reference, which every tagging API shares and
/// which the production constants carry: key 1-128 Unicode characters, value 0-256, and
/// the `aws:` prefix reserved for AWS use. None of these rows is the one the issue cited
/// beyond the three it names.
fn tag_cases() -> Vec<TagCase> {
    let legal = |case: &str, key: String, value: String| TagCase {
        case: case.to_string(),
        key,
        value,
        legal: true,
    };
    let illegal = |case: &str, key: String, value: String| TagCase {
        case: case.to_string(),
        key,
        value,
        legal: false,
    };
    vec![
        // --- admitted ---
        legal(
            "an ordinary key and value",
            "cost-center".to_string(),
            "platform engineering".to_string(),
        ),
        legal(
            "a key at the minimum length and an empty value",
            "k".repeat(MIN_TAG_KEY_CHARS),
            String::new(),
        ),
        legal(
            "a key and a value at their maxima",
            "k".repeat(MAX_TAG_KEY_CHARS),
            "v".repeat(MAX_TAG_VALUE_CHARS),
        ),
        legal(
            "a multi-byte key and value at their maxima, counted in characters",
            "é".repeat(MAX_TAG_KEY_CHARS),
            "λ".repeat(MAX_TAG_VALUE_CHARS),
        ),
        legal(
            "a key that merely STARTS with the reserved letters",
            "awsome-owner".to_string(),
            "someone".to_string(),
        ),
        legal(
            "a key carrying the reserved prefix somewhere other than the start",
            format!("mine-{RESERVED_TAG_PREFIX}thing"),
            "kept".to_string(),
        ),
        // --- refused ---
        illegal("an empty key", String::new(), "x".to_string()),
        illegal(
            "a key one character past the maximum",
            "k".repeat(MAX_TAG_KEY_CHARS + 1),
            "x".to_string(),
        ),
        illegal(
            "a key far past the maximum",
            "k".repeat(MAX_TAG_KEY_CHARS * 8),
            "x".to_string(),
        ),
        illegal(
            "a multi-byte key one character past the maximum",
            "é".repeat(MAX_TAG_KEY_CHARS + 1),
            "x".to_string(),
        ),
        illegal(
            "a value one character past the maximum",
            "sized".to_string(),
            "v".repeat(MAX_TAG_VALUE_CHARS + 1),
        ),
        illegal(
            "a value far past the maximum",
            "sized".to_string(),
            "v".repeat(MAX_TAG_VALUE_CHARS * 8),
        ),
        illegal(
            "a key in the reserved namespace",
            format!("{RESERVED_TAG_PREFIX}created-by"),
            "x".to_string(),
        ),
        illegal(
            "the reserved prefix in upper case",
            format!("{}created-by", RESERVED_TAG_PREFIX.to_uppercase()),
            "x".to_string(),
        ),
        illegal(
            "the reserved prefix and nothing else",
            RESERVED_TAG_PREFIX.to_string(),
            "x".to_string(),
        ),
    ]
}

impl TagCase {
    /// The one-entry `Tags` map this row sends.
    fn tags(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert(self.key.clone(), Value::String(self.value.clone()));
        Value::Object(map)
    }
}

fn settings() -> Value {
    json!({ "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" })
}

fn license_server_settings() -> Value {
    json!({
        "ServerType": "RDS_SAL",
        "ServerSettings": {
            "RdsSalSettings": {
                "RdsSalCredentialsProvider": {
                    "SecretsManagerCredentialsProvider": { "SecretId": "rds-sal-secret" }
                }
            }
        }
    })
}

async fn register(h: &fixture::Harness, body: &Value) -> fixture::Reply {
    fixture::call(h, "/identity-provider/RegisterIdentityProvider", body).await
}

async fn list_tags(h: &fixture::Harness, resource_arn: &str) -> fixture::Reply {
    let path = format!("/tags/{}", fixture::encoded_label(resource_arn));
    fixture::send(h, "GET", &path, None).await
}

async fn tag_resource(h: &fixture::Harness, resource_arn: &str, tags: &Value) -> fixture::Reply {
    let path = format!("/tags/{}", fixture::encoded_label(resource_arn));
    fixture::send(h, "PUT", &path, Some(&json!({ "Tags": tags }))).await
}

/// Percent-encode one `@httpQuery` VALUE.
///
/// [`fixture::encoded_label`] encodes the three characters an ARN can carry that a path
/// segment cannot, which is all a path label needs; a `tagKeys` value is arbitrary text,
/// so everything outside RFC 3986's unreserved set is encoded here — a URI is ASCII, and a
/// key of multi-byte characters would otherwise not be expressible as a request at all.
fn encoded_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(*byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

async fn untag_resource(h: &fixture::Harness, resource_arn: &str, keys: &[&str]) -> fixture::Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded_query_value(key)))
        .collect();
    let path = format!(
        "/tags/{}?{}",
        fixture::encoded_label(resource_arn),
        query.join("&")
    );
    fixture::send(h, "DELETE", &path, None).await
}

/// Register one identity provider for `product`, untagged, and answer its ARN.
async fn provider_arn(h: &fixture::Harness, product: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": fixture::provider_union(DIRECTORY),
            "Product": product,
            "Settings": settings(),
        }),
    )
    .await;
    reply.ok("a registration to tag")["IdentityProviderSummary"]["IdentityProviderArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the registration must carry an ARN: {:?}", reply.body))
        .to_string()
}

/// Assert one refusal: the status and the wire error code a client branches on, naming the
/// case and showing what was actually answered.
fn assert_refused(case: &str, reply: &fixture::Reply) {
    assert_eq!(
        reply.status, VALIDATION_STATUS,
        "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(VALIDATION_CODE),
        "case {case:?}: expected {VALIDATION_CODE}, got {reply:?}"
    );
    assert!(
        reply.message().is_some_and(|message| !message.is_empty()),
        "case {case:?}: a refusal must say what was wrong, got {:?}",
        reply.body
    );
}

/// The tags `ListTagsForResource` reports for `resource_arn`, as a JSON object.
async fn held_tags(
    h: &fixture::Harness,
    resource_arn: &str,
    case: &str,
) -> serde_json::Map<String, Value> {
    let reply = list_tags(h, resource_arn).await;
    reply.ok(case)["Tags"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a map, got {:?}", reply.body))
}

/// RULE: `TagResource` persists a tag only when it is a legal AWS tag; an illegal key or
/// value is refused with HTTP 400 `ValidationException` and nothing is written, which
/// `ListTagsForResource` then confirms.
#[tokio::test]
async fn tag_resource_admits_exactly_the_legal_tags() {
    for row in tag_cases() {
        let h = fixture::harness();
        let arn = provider_arn(&h, fixture::PRODUCTS[0]).await;
        let reply = tag_resource(&h, &arn, &row.tags()).await;
        if row.legal {
            reply.ok(&row.case);
        } else {
            assert_refused(&row.case, &reply);
        }
        // Either way the answer is checked against what a later read reports, so an
        // accepted tag is PERSISTED and a refused one is nowhere.
        let held = held_tags(&h, &arn, &row.case).await;

        if row.legal {
            assert_eq!(
                held.get(&row.key).and_then(Value::as_str),
                Some(row.value.as_str()),
                "case {:?}: the tag written must be reported back, got {held:?}",
                row.case
            );
        } else {
            assert!(
                held.is_empty(),
                "case {:?}: a refused tag must not be stored, got {held:?}",
                row.case
            );
        }
    }
}

/// Which operation is writing the tags, for the surfaces that accept a `Tags` map on a
/// CREATE rather than through `TagResource`.
#[derive(Debug, Clone, Copy)]
enum Surface {
    RegisterIdentityProvider,
    AssociateUser,
    StartProductSubscription,
    CreateLicenseServerEndpoint,
}

impl Surface {
    fn name(self) -> &'static str {
        match self {
            Self::RegisterIdentityProvider => "RegisterIdentityProvider",
            Self::AssociateUser => "AssociateUser",
            Self::StartProductSubscription => "StartProductSubscription",
            Self::CreateLicenseServerEndpoint => "CreateLicenseServerEndpoint",
        }
    }

    /// Create this surface's resource carrying `tags`, answering the reply and, where the
    /// reply is a success, the ARN of the resource the tags were written against.
    async fn create(self, h: &fixture::Harness, tags: &Value) -> (fixture::Reply, Option<String>) {
        let (path, body, arn_path) = match self {
            Self::RegisterIdentityProvider => (
                "/identity-provider/RegisterIdentityProvider",
                json!({
                    "IdentityProvider": fixture::provider_union(DIRECTORY),
                    "Product": fixture::PRODUCTS[0],
                    "Settings": settings(),
                    "Tags": tags,
                }),
                vec!["IdentityProviderSummary", "IdentityProviderArn"],
            ),
            Self::AssociateUser => {
                provider_arn(h, fixture::PRODUCTS[0]).await;
                (
                    "/user/AssociateUser",
                    json!({
                        "Username": "tagged.user",
                        "InstanceId": AUTHORIZED_INSTANCE,
                        "IdentityProvider": fixture::provider_union(DIRECTORY),
                        "Tags": tags,
                    }),
                    vec!["InstanceUserSummary", "InstanceUserArn"],
                )
            }
            Self::StartProductSubscription => {
                provider_arn(h, fixture::PRODUCTS[0]).await;
                (
                    "/user/StartProductSubscription",
                    json!({
                        "Username": "tagged.user",
                        "IdentityProvider": fixture::provider_union(DIRECTORY),
                        "Product": fixture::PRODUCTS[0],
                        "Tags": tags,
                    }),
                    vec!["ProductUserSummary", "ProductUserArn"],
                )
            }
            Self::CreateLicenseServerEndpoint => {
                let provider = provider_arn(h, fixture::PRODUCTS[0]).await;
                (
                    "/license-server/CreateLicenseServerEndpoint",
                    json!({
                        "IdentityProviderArn": provider,
                        "LicenseServerSettings": license_server_settings(),
                        "Tags": tags,
                    }),
                    vec!["LicenseServerEndpointArn"],
                )
            }
        };
        let reply = fixture::call(h, path, &body).await;
        let arn = arn_path
            .iter()
            .try_fold(&reply.body, |value, member| value.get(*member))
            .and_then(Value::as_str)
            .map(str::to_string);
        (reply, arn)
    }
}

const SURFACES: [Surface; 4] = [
    Surface::RegisterIdentityProvider,
    Surface::AssociateUser,
    Surface::StartProductSubscription,
    Surface::CreateLicenseServerEndpoint,
];

/// How many resources of each kind, and how many tag sets, the store holds.
fn stored(h: &fixture::Harness) -> (usize, usize, usize, usize, usize) {
    let store = h.engine.state::<ServiceState>();
    let store = store.lock();
    (
        store.identity_providers.len(),
        store.instance_users.len(),
        store.product_users.len(),
        store.license_server_endpoints.len(),
        store.tags.len(),
    )
}

/// RULE: the rule belongs to the `Tags` map, not to one operation — every operation that
/// accepts tags on a CREATE applies it, refusing an illegal tag with HTTP 400
/// `ValidationException` and minting NO resource, and persisting a legal one where
/// `ListTagsForResource` reports it.
#[tokio::test]
async fn every_operation_that_writes_tags_applies_the_same_rule() {
    for surface in SURFACES {
        for row in tag_cases() {
            let case = format!("{}/{}", surface.name(), row.case);
            let h = fixture::harness();
            let before = stored(&h);
            let (reply, arn) = surface.create(&h, &row.tags()).await;
            // The prerequisite registration the surface created for itself, if any, is in
            // the counts taken after it — so only the row's own write is compared.
            let baseline = if matches!(surface, Surface::RegisterIdentityProvider) {
                before
            } else {
                (before.0 + 1, before.1, before.2, before.3, before.4)
            };

            if row.legal {
                reply.ok(&case);
                let arn = arn.unwrap_or_else(|| {
                    panic!("case {case:?}: a created resource must carry an ARN: {reply:?}")
                });
                let held = held_tags(&h, &arn, &case).await;
                assert_eq!(
                    held.get(&row.key).and_then(Value::as_str),
                    Some(row.value.as_str()),
                    "case {case:?}: the tag the create carried must be reported back, got {held:?}"
                );
            } else {
                assert_refused(&case, &reply);
                let after = stored(&h);
                assert_eq!(
                    after, baseline,
                    "case {case:?}: a refused create must mint nothing and store no tags, \
                     expected {baseline:?}, got {after:?}"
                );
            }
        }
    }
}

/// RULE: a payload is refused WHOLE — one illegal tag beside legal ones writes none of
/// them, and a refusal leaves the tags the resource already carried exactly as they were.
#[tokio::test]
async fn one_illegal_tag_refuses_the_whole_payload() {
    let h = fixture::harness();
    let arn = provider_arn(&h, fixture::PRODUCTS[0]).await;

    tag_resource(&h, &arn, &json!({ "team": "licensing" }))
        .await
        .ok("the tags the resource starts with");

    // Each row pairs a legal tag with one illegal one, varying which rule the illegal tag
    // breaks and whether its key sorts before or after its legal companion's.
    let rows: Vec<(String, String, String)> = vec![
        (
            "a reserved key beside a legal one".to_string(),
            format!("{RESERVED_TAG_PREFIX}created-by"),
            "x".to_string(),
        ),
        (
            "an empty key beside a legal one".to_string(),
            String::new(),
            "x".to_string(),
        ),
        (
            "an over-long key beside a legal one".to_string(),
            "k".repeat(MAX_TAG_KEY_CHARS + 1),
            "x".to_string(),
        ),
        (
            "an over-long value beside a legal one".to_string(),
            "sized".to_string(),
            "v".repeat(MAX_TAG_VALUE_CHARS + 1),
        ),
    ];

    for (case, key, value) in rows {
        let case = case.as_str();
        let mut map = serde_json::Map::new();
        map.insert("env".to_string(), Value::String("test".to_string()));
        map.insert(key, Value::String(value));
        let tags = Value::Object(map);
        let reply = tag_resource(&h, &arn, &tags).await;
        assert_refused(case, &reply);
        let held = held_tags(&h, &arn, case).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: a refused payload must write none of its tags, got {held:?}"
        );
        assert_eq!(
            held.get("team").and_then(Value::as_str),
            Some("licensing"),
            "case {case:?}: the tags already held must survive a refusal, got {held:?}"
        );
    }
}

/// RULE: the key rule governs the keys `UntagResource` names too — a key that is not a
/// tag key at all is refused with HTTP 400 `ValidationException` and removes nothing,
/// while a legal key the resource does not carry stays the no-op `@idempotent` asks for.
#[tokio::test]
async fn untag_resource_applies_the_same_key_rule() {
    let h = fixture::harness();
    let arn = provider_arn(&h, fixture::PRODUCTS[0]).await;
    tag_resource(&h, &arn, &json!({ "team": "licensing", "env": "test" }))
        .await
        .ok("the tags to remove from");

    // The key rows of the table, reused: the rule is the same rule, so the inputs are the
    // same inputs. A row whose key only breaks the VALUE rule carries a legal key, and is
    // not a refusal here.
    for row in tag_cases() {
        let case = format!("UntagResource/{}", row.case);
        let key_is_legal = row.legal || row.value.chars().count() > MAX_TAG_VALUE_CHARS;
        let reply = untag_resource(&h, &arn, &[row.key.as_str()]).await;
        if key_is_legal {
            reply.ok(&case);
        } else {
            assert_refused(&case, &reply);
        }
        let held = held_tags(&h, &arn, &case).await;
        assert_eq!(
            held.len(),
            2,
            "case {case:?}: no row of this table names a key the resource carries, so \
             nothing may be removed, got {held:?}"
        );
    }

    // The rule is not a blanket refusal: a legal key the resource DOES carry is removed.
    untag_resource(&h, &arn, &["env"])
        .await
        .ok("removing a key the resource carries");
    let held = held_tags(&h, &arn, "after the removal").await;
    assert_eq!(
        held.keys().collect::<Vec<_>>(),
        vec!["team"],
        "UntagResource must remove exactly the key it named, got {held:?}"
    );
}
