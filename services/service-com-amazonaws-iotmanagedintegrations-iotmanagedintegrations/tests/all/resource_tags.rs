// SPDX-License-Identifier: CC0-1.0

//! The ARN-addressed tag plane, end to end over the wire: `TagResource` (POST
//! `/tags/{ResourceArn}`), `ListTagsForResource` (GET) and `UntagResource` (DELETE).
//!
//! RULE: the three operations read and write the ONE tag map of the resource the
//! `ResourceArn` names, resolved through the region the request was signed for. A write
//! ASSIGNS the pairs it carries — a key it names is added or overwritten and every other
//! key keeps its value — a removal drops exactly the keys it names and nothing else, a
//! read reports the stored map (and reports no member at all when the resource carries no
//! tag, `TagsMap` being `@length` 1..50), and an ARN the caller's region holds no resource
//! under answers the modelled `ResourceNotFoundException` (HTTP 404) from all three. The
//! map is the resource's OWN, so a tag written here comes back from the resource's own
//! read and a tag a create stored is reported here.
//!
//! The recorded unit this suite was derived from
//! (`iot-managed-integrations.tag-resource-round-trip-on-managed-thing`) is ONE row of the
//! tables below. The rows differ along what the rule generalises over: the KIND of
//! resource the ARN names (both kinds this service stores, and every kind the
//! `IoTManagedIntegrationsResourceARN` pattern admits on the refusal table), the number of
//! pairs one call carries, the form of a key and of a value (including both modelled
//! length bounds), whether a key is new or already carried, the keys a removal names
//! (carried, absent, all of them), the region the request is signed for and the account
//! the ARN carries.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::MAX_RESOURCE_TAGS;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// Another region's endpoint, for the cases that address one region's resource through
/// another's.
const OTHER_REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error codes the model's `@httpError` shapes are rendered under.
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The `@length` maximum of `TagKey`, in characters
/// (`smithy_query(query="shape",shape="TagsMap")` -> key `TagKey` length 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The `@length` bounds of `TagValue`, in characters
/// (`smithy_query(query="shape",shape="TagsMap")` -> value `TagValue` length 0..256) — so
/// an EMPTY value is a modelled value, not a missing one.
const TAG_VALUE_BOUNDS: (usize, usize) = (0, 256);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }

    /// The tag map the answer reports on `member`, or `None` when it reports none.
    fn tags(&self, member: &str) -> Option<BTreeMap<String, String>> {
        self.member(member).and_then(Value::as_object).map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// `ResourceArn` is a NON-greedy `@httpLabel`, so the ARN fills ONE path segment: every
/// byte outside the unreserved set — the `:` and `/` an ARN is full of — is
/// percent-encoded, which is what an SDK puts on the wire.
fn label(arn: &str) -> String {
    arn.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `TagResource`: `POST /tags/{ResourceArn}` with the pairs in the body's `Tags`.
async fn tag_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> Reply {
    call_in(
        router,
        region,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({"Tags": tags})),
    )
    .await
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`.
async fn list_tags_in(router: &MultiServiceRouter, region: &str, arn: &str) -> Reply {
    call_in(router, region, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// `UntagResource`: `DELETE /tags/{ResourceArn}?tagKeys=…`. `TagKeys` is bound to
/// `@httpQuery("tagKeys")`, so each key is one parameter of that name.
async fn untag_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    keys: &[String],
) -> Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    call_in(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{}", label(arn), query.join("&")),
        None,
    )
    .await
}

/// Create a connector destination in `region` through the wire and answer the
/// `ConnectorDestinationId` it minted.
///
/// `CreateAccountAssociation` resolves its `ConnectorDestinationId` against this service's
/// own connector-destination store, so an association is created against a destination a
/// client really created; a connector destination in turn names a C2C connector
/// (`CloudConnectorId` is `@required`), so the connector comes first.
async fn connector_destination(router: &MultiServiceRouter, region: &str, case: &str) -> String {
    let connector = call_in(
        router,
        region,
        "POST",
        "/cloud-connectors",
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": {"lambda": {"arn": "connector-handler"}},
        })),
    )
    .await;
    assert_eq!(
        connector.status, 201,
        "case {case:?}: seeding a cloud connector expected HTTP 201, got {} with body {:?}",
        connector.status, connector.body
    );
    let connector_id = connector.string("Id").unwrap_or_default().to_string();
    let destination = call_in(
        router,
        region,
        "POST",
        "/connector-destinations",
        Some(json!({
            "CloudConnectorId": connector_id,
            "AuthConfig": {},
            // The service requires the ARN of the secret a destination keeps its
            // credentials in. Inside the `SecretsManagerArn` `@pattern`
            // (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
            // and the `SecretsManagerVersionId` `@length` (32..64).
            "SecretsManager": {
                "arn": format!(
                    "arn:aws:secretsmanager:{region}:{ACCOUNT}:secret:destination-A1b2C3"
                ),
                "versionId": "a".repeat(32),
            },
        })),
    )
    .await;
    assert_eq!(
        destination.status, 201,
        "case {case:?}: seeding a connector destination expected HTTP 201, got {} with \
         body {:?}",
        destination.status, destination.body
    );
    destination.string("Id").unwrap_or_default().to_string()
}

/// The resource kinds this service STORES, which are the kinds an ARN-addressed tagging
/// call can really resolve. The `IoTManagedIntegrationsResourceARN` pattern admits three
/// more (`provisioning-profile`, `ota-task`, `credential-locker`), none of which any
/// implemented operation mints — they are rows of the refusal table instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ManagedThing,
    AccountAssociation,
}

/// Both kinds, so every table below runs over each.
const KINDS: [Kind; 2] = [Kind::ManagedThing, Kind::AccountAssociation];

impl Kind {
    /// The resource-type segment this kind's ARN carries.
    fn segment(self) -> &'static str {
        match self {
            Self::ManagedThing => "managed-thing",
            Self::AccountAssociation => "account-association",
        }
    }

    /// The collection route a resource of this kind is created on and read back from.
    fn collection(self) -> &'static str {
        match self {
            Self::ManagedThing => "/managed-things",
            Self::AccountAssociation => "/account-associations",
        }
    }

    /// A create request for this kind, carrying only its required members plus `tags` when
    /// a case wants the create to store some.
    ///
    /// `reference` is the resource of this service the create REFERS TO, for a kind whose
    /// create resolves one: an account association names the connector destination it is
    /// created against, which has to be one this account holds.
    fn create_body(self, reference: &str, tags: Option<&BTreeMap<String, String>>) -> Value {
        let mut body = match self {
            Self::ManagedThing => json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "zwave:qr:0123456789",
            }),
            Self::AccountAssociation => json!({"ConnectorDestinationId": reference}),
        };
        if let Some(tags) = tags {
            body["Tags"] = json!(tags);
        }
        body
    }

    /// The member a create of this kind answers the new resource's id on. Both kinds
    /// answer the ARN on `Arn`.
    fn id_member(self) -> &'static str {
        match self {
            Self::ManagedThing => "Id",
            Self::AccountAssociation => "AccountAssociationId",
        }
    }

    /// Create one resource of this kind in `region`, answering `(id, arn)`.
    async fn create(
        self,
        router: &MultiServiceRouter,
        region: &str,
        case: &str,
        tags: Option<&BTreeMap<String, String>>,
    ) -> (String, String) {
        // A kind whose create resolves a reference gets one this account really holds: the
        // account association is created against a connector destination seeded through
        // the API in the same region.
        let reference = match self {
            Self::ManagedThing => String::new(),
            Self::AccountAssociation => connector_destination(router, region, case).await,
        };
        let reply = call_in(
            router,
            region,
            "POST",
            self.collection(),
            Some(self.create_body(&reference, tags)),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: expected HTTP 201 creating a {:?}, got {} with body {:?}",
            self, reply.status, reply.body
        );
        let id = reply.string(self.id_member()).unwrap_or_default().to_string();
        let arn = reply.string("Arn").unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:iotmanagedintegrations:{region}:{ACCOUNT}:{}/{id}",
                self.segment()
            ),
            "case {case:?}: expected the {:?} ARN of the generated id, got {arn:?}",
            self
        );
        (id, arn)
    }

    /// The tags the resource's OWN read reports — the other side of the one map the
    /// tagging surface writes.
    async fn read_tags(
        self,
        router: &MultiServiceRouter,
        region: &str,
        case: &str,
        id: &str,
    ) -> Option<BTreeMap<String, String>> {
        let reply = call_in(
            router,
            region,
            "GET",
            &format!("{}/{id}", self.collection()),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 reading the {:?} back, got {} with body {:?}",
            self, reply.status, reply.body
        );
        reply.tags("Tags")
    }
}

/// Assert `reply` is the modelled refusal `status`/`code` and carries a reason.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// Assert both readers of the tag map — `ListTagsForResource` and the resource's own read
/// — report exactly `expected`, which is what makes them ONE map rather than two copies.
async fn assert_tags(
    router: &MultiServiceRouter,
    kind: Kind,
    case: &str,
    id: &str,
    arn: &str,
    expected: &BTreeMap<String, String>,
) {
    let listed = list_tags_in(router, REGION, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with body {:?}",
        listed.status, listed.body
    );
    let reported = listed.tags("tags").or_else(|| listed.tags("Tags"));
    if expected.is_empty() {
        // `TagsMap` is `@length` 1..50, so no tags is NO member rather than an empty map.
        assert!(
            reported.is_none(),
            "case {case:?}: expected ListTagsForResource to report no tags member for an \
             untagged resource, got {reported:?}"
        );
    } else {
        assert_eq!(
            reported.as_ref(),
            Some(expected),
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got \
             {reported:?}"
        );
    }

    let own = kind.read_tags(router, REGION, case, id).await;
    let own_expected = (!expected.is_empty()).then(|| expected.clone());
    assert_eq!(
        own, own_expected,
        "case {case:?}: expected the {kind:?}'s own read to report {own_expected:?} — the \
         same map the tagging surface writes — got {own:?}"
    );
}

/// A tag map from pairs, as the tables below state them.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// RULE: a `TagResource` call ASSIGNS the pairs it carries onto the resource's tag map —
/// adding a key, overwriting one the resource already carries, and leaving every other key
/// alone — and both readers report the stored map. One row per form of write, applied in
/// order to one resource of each kind, so every row also asserts that what the earlier
/// rows wrote is still there.
#[tokio::test]
async fn a_tag_write_assigns_its_pairs_onto_the_resource_map() {
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let longest_value = "v".repeat(TAG_VALUE_BOUNDS.1);
    let writes: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("one pair", map(&[("env", "test")])),
        (
            "several pairs at once",
            map(&[("team", "iot"), ("owner", "platform")]),
        ),
        ("an existing key, overwritten", map(&[("env", "prod")])),
        // `TagValue` is length 0..256, so the empty string is a value a key may carry.
        ("a value at the TagValue minimum", map(&[("note", "")])),
        (
            "a key at the TagKey maximum",
            map(&[(longest_key.as_str(), "at-the-key-bound")]),
        ),
        (
            "a value at the TagValue maximum",
            map(&[("long", longest_value.as_str())]),
        ),
        ("a pair the resource already carries", map(&[("env", "prod")])),
    ];

    for kind in KINDS {
        let router = emulator();
        let (id, arn) = kind.create(&router, REGION, "seed", None).await;
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        assert_tags(&router, kind, "a fresh resource carries no tag", &id, &arn, &expected).await;

        for (name, write) in &writes {
            let case = format!("{kind:?}: {name}");
            let reply = tag_in(&router, REGION, &arn, write).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 from TagResource, got {} with body {:?}",
                reply.status, reply.body
            );
            expected.extend(write.iter().map(|(key, value)| (key.clone(), value.clone())));
            assert_tags(&router, kind, &case, &id, &arn, &expected).await;
        }
    }
}

/// RULE: an `UntagResource` call drops exactly the keys it names and nothing else, and a
/// key the resource does not carry is nothing to remove — so repeating a removal leaves the
/// same map behind.
#[tokio::test]
async fn a_tag_removal_drops_exactly_the_keys_it_names() {
    let seed = map(&[
        ("env", "prod"),
        ("team", "iot"),
        ("owner", "platform"),
        ("note", ""),
    ]);
    let removals: Vec<(&str, Vec<String>)> = vec![
        ("one carried key", vec!["team".to_string()]),
        ("a key the resource does not carry", vec!["absent".to_string()]),
        (
            "several keys, one carried and one not",
            vec!["owner".to_string(), "never-set".to_string()],
        ),
        (
            "every key still carried",
            vec!["env".to_string(), "note".to_string()],
        ),
        (
            "the same keys again",
            vec!["env".to_string(), "note".to_string()],
        ),
    ];

    for kind in KINDS {
        let router = emulator();
        let (id, arn) = kind.create(&router, REGION, "seed", Some(&seed)).await;
        let mut expected = seed.clone();
        assert_tags(
            &router,
            kind,
            &format!("{kind:?}: the tags the create stored"),
            &id,
            &arn,
            &expected,
        )
        .await;

        for (name, keys) in &removals {
            let case = format!("{kind:?}: {name}");
            let reply = untag_in(&router, REGION, &arn, keys).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 from UntagResource, got {} with body {:?}",
                reply.status, reply.body
            );
            for key in keys {
                expected.remove(key);
            }
            assert_tags(&router, kind, &case, &id, &arn, &expected).await;
        }
    }
}

/// RULE: an ARN the caller's region holds no resource under is the modelled
/// `ResourceNotFoundException` (HTTP 404) from all three operations, and the call stores
/// nothing. The rows span every resource type the `IoTManagedIntegrationsResourceARN`
/// pattern admits, an unknown id, another region's and another account's ARN, and a live
/// resource's id under another kind's segment.
#[tokio::test]
async fn an_arn_naming_no_resource_of_the_region_is_not_found() {
    let router = emulator();
    let tagged = map(&[("env", "prod")]);
    let (_, live_arn) = Kind::ManagedThing
        .create(&router, REGION, "the live resource", Some(&tagged))
        .await;
    let live_id = live_arn.rsplit('/').next().unwrap_or_default().to_string();

    let mut cases: Vec<(String, String)> = Vec::new();
    // Every resource type the member's pattern admits, under an id nothing was created
    // under: the two this service stores, and the three no implemented operation mints.
    for segment in [
        "managed-thing",
        "provisioning-profile",
        "ota-task",
        "credential-locker",
        "account-association",
    ] {
        cases.push((
            format!("an unknown {segment}"),
            format!("arn:aws:iotmanagedintegrations:{REGION}:{ACCOUNT}:{segment}/unknown1"),
        ));
    }
    cases.push((
        "a live resource's ARN, naming another region".to_string(),
        format!(
            "arn:aws:iotmanagedintegrations:{OTHER_REGION}:{ACCOUNT}:managed-thing/{live_id}"
        ),
    ));
    cases.push((
        "a live resource's ARN, naming another account".to_string(),
        format!("arn:aws:iotmanagedintegrations:{REGION}:000000000001:managed-thing/{live_id}"),
    ));
    cases.push((
        "a live resource's id under another kind's segment".to_string(),
        format!("arn:aws:iotmanagedintegrations:{REGION}:{ACCOUNT}:ota-task/{live_id}"),
    ));

    for (name, arn) in &cases {
        for (operation, reply) in [
            ("TagResource", tag_in(&router, REGION, arn, &tagged).await),
            ("ListTagsForResource", list_tags_in(&router, REGION, arn).await),
            (
                "UntagResource",
                untag_in(&router, REGION, arn, &["env".to_string()]).await,
            ),
        ] {
            let case = format!("{operation} on {name}");
            assert_refused(&case, &reply, 404, NOT_FOUND);
            assert_eq!(
                reply.string("ResourceId"),
                Some(arn.as_str()),
                "case {case:?}: expected the 404 to report the ARN the request carried on \
                 ResourceId, got {:?}",
                reply.string("ResourceId")
            );
        }
    }

    // None of those calls touched the live resource.
    let listed = list_tags_in(&router, REGION, &live_arn).await;
    assert_eq!(
        listed.tags("tags").as_ref(),
        Some(&tagged),
        "case {:?}: expected the live resource to keep {tagged:?}, got {:?}",
        "the live resource after the refused calls",
        listed.tags("tags")
    );
}

/// RULE: the tag map belongs to the region that created the resource, so one region's
/// endpoint can neither read nor write another region's tags — the same resolution rule
/// every other operation of this service follows.
#[tokio::test]
async fn one_regions_endpoint_cannot_reach_another_regions_tags() {
    for kind in KINDS {
        let router = emulator();
        let case = format!("{kind:?}");
        let tagged = map(&[("env", "prod")]);
        let (id, arn) = kind.create(&router, REGION, &case, Some(&tagged)).await;

        for (operation, reply) in [
            ("TagResource", tag_in(&router, OTHER_REGION, &arn, &map(&[("team", "iot")])).await),
            ("ListTagsForResource", list_tags_in(&router, OTHER_REGION, &arn).await),
            (
                "UntagResource",
                untag_in(&router, OTHER_REGION, &arn, &["env".to_string()]).await,
            ),
        ] {
            assert_refused(
                &format!("{case}: {operation} through another region's endpoint"),
                &reply,
                404,
                NOT_FOUND,
            );
        }

        assert_tags(
            &router,
            kind,
            &format!("{case}: the owning region's view is unchanged"),
            &id,
            &arn,
            &tagged,
        )
        .await;
    }
}

/// RULE: a resource carries at most `MAX_RESOURCE_TAGS` tags — the `@length` maximum of
/// the `TagsMap` both the write and the read target — so a call that would leave it
/// carrying more is refused and stores nothing, while one that only overwrites keys it
/// already carries is accepted at the bound.
#[tokio::test]
async fn a_write_past_the_tags_per_resource_bound_is_refused() {
    for kind in KINDS {
        let router = emulator();
        let case = format!("{kind:?}");
        let (id, arn) = kind.create(&router, REGION, &case, None).await;

        // Exactly at the bound, in one call: the most a resource may carry.
        let full: BTreeMap<String, String> = (0..MAX_RESOURCE_TAGS)
            .map(|index| (format!("key{index}"), format!("value{index}")))
            .collect();
        let accepted = tag_in(&router, REGION, &arn, &full).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 writing {MAX_RESOURCE_TAGS} tags, got {} with \
             body {:?}",
            accepted.status, accepted.body
        );
        assert_tags(&router, kind, &format!("{case}: at the bound"), &id, &arn, &full).await;

        // Re-stating keys the resource already carries does not grow the map, so it is
        // accepted at the bound.
        let mut overwritten = full.clone();
        overwritten.insert("key0".to_string(), "rewritten".to_string());
        let rewrite = tag_in(&router, REGION, &arn, &map(&[("key0", "rewritten")])).await;
        assert_eq!(
            rewrite.status, 200,
            "case {case:?}: expected HTTP 200 overwriting a carried key at the bound, got {} \
             with body {:?}",
            rewrite.status, rewrite.body
        );
        assert_tags(
            &router,
            kind,
            &format!("{case}: an overwrite at the bound"),
            &id,
            &arn,
            &overwritten,
        )
        .await;

        // One key past it is refused, and the stored map is left exactly as it was.
        let refused = tag_in(&router, REGION, &arn, &map(&[("one-too-many", "x")])).await;
        assert_refused(
            &format!("{case}: one key past the bound"),
            &refused,
            400,
            INVALID_REQUEST,
        );
        assert!(
            refused.message().contains(&(MAX_RESOURCE_TAGS + 1).to_string()),
            "case {case:?}: expected the refusal to state the {} tags the request would \
             leave the resource carrying, got {:?}",
            MAX_RESOURCE_TAGS + 1,
            refused.message()
        );
        assert_tags(
            &router,
            kind,
            &format!("{case}: nothing was stored by the refused call"),
            &id,
            &arn,
            &overwritten,
        )
        .await;

        // Freeing a key makes room again, so the bound is on the map and not on the call.
        let freed = untag_in(&router, REGION, &arn, &["key0".to_string()]).await;
        assert_eq!(
            freed.status, 200,
            "case {case:?}: expected HTTP 200 removing a key, got {} with body {:?}",
            freed.status, freed.body
        );
        let accepted_again = tag_in(&router, REGION, &arn, &map(&[("one-too-many", "x")])).await;
        assert_eq!(
            accepted_again.status, 200,
            "case {case:?}: expected HTTP 200 once a key was freed, got {} with body {:?}",
            accepted_again.status, accepted_again.body
        );
        let mut expected = overwritten.clone();
        expected.remove("key0");
        expected.insert("one-too-many".to_string(), "x".to_string());
        assert_tags(
            &router,
            kind,
            &format!("{case}: the freed slot took the new key"),
            &id,
            &arn,
            &expected,
        )
        .await;
    }
}
