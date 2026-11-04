// SPDX-License-Identifier: CC0-1.0

//! Associating a DNS resource with a Route 53 Profile, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises
//! the same restJson1 path a real SDK client takes: routing, deserialization,
//! boundary constraint validation, the handler, and error rendering.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `AssociateResourceToProfile` persists an association for any ARN naming a
//!   resource a Profile can hold, reports `ResourceType` as that ARN's own
//!   resource-type segment in the upper-snake spelling the wire uses, and echoes
//!   `Name`, `ProfileId`, `ResourceArn` and `ResourceProperties` back unchanged;
//! * the association is reported `CREATING` until the move to `COMPLETE` the clock
//!   owes falls due, each status with the sentence that belongs to it, and
//!   `GetProfileResourceAssociation` reports the same record;
//! * a DNS Firewall rule group's `priority` is accepted inside the documented
//!   range and refused outside it, with nothing persisted for a refused request;
//! * an ARN naming any other kind of resource is refused;
//! * a `ProfileId` that names nothing is `ResourceNotFoundException`, a repeat of a
//!   (Profile, resource) pair already associated is `ConflictException`, and an
//!   association id that was never minted is `ResourceNotFoundException`.
//!
//! This suite keeps its own harness rather than the Profile suite's, because it
//! also reaches the store directly to bring the clock-owed transition forward
//! instead of waiting for it, which needs the shared engine the other harness does
//! not hand out.

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53profiles_route53profiles::state::{
    service_state, MAX_FIREWALL_PRIORITY, MIN_FIREWALL_PRIORITY,
};
use service_com_amazonaws_route53profiles_route53profiles::EmulatedService;

/// This service's sigv4 signing name, from the model (`OPERATIONS.md`, "Wire protocol").
const SIGNING_NAME: &str = "route53profiles";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents, as `engine` defines it.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// One response, reduced to what the assertions read.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), with any
    /// `namespace#` prefix stripped. `None` on success.
    error: Option<String>,
    body: Value,
}

impl Reply {
    /// The response's top-level `member`, panicking with the whole body when it is
    /// not there — a failure that is otherwise invisible behind `Value::Null`.
    fn member(&self, name: &str) -> &Value {
        let value = &self.body[name];
        assert!(!value.is_null(), "member {name} is absent from {}", self.body);
        value
    }

    /// The `ProfileResourceAssociation` this call reported.
    fn association(&self) -> &Value {
        self.member("ProfileResourceAssociation")
    }

    /// A string field of the reported association.
    fn field(&self, name: &str) -> String {
        self.association()[name]
            .as_str()
            .unwrap_or_else(|| panic!("ProfileResourceAssociation.{name} is not a string in {}", self.body))
            .to_string()
    }

    /// The refusal message, as the modelled error shapes spell it (`Message`).
    fn message(&self) -> String {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_else(|| panic!("no refusal message in {}", self.body))
            .to_string()
    }
}

/// One emulator over one shared engine: every call sees the same store, and the
/// test can read that store to check what a call persisted.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host, Signature=00"
        );
        let payload = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://{SIGNING_NAME}.{REGION}.amazonaws.com{path}"
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error = if status < 400 {
            None
        } else {
            header_error
                .or_else(|| body["__type"].as_str().map(str::to_string))
                .map(|code| {
                    code.rsplit('#')
                        .next()
                        .unwrap_or(code.as_str())
                        .split('/')
                        .next()
                        .unwrap_or(code.as_str())
                        .to_string()
                })
        };
        Reply {
            status,
            error,
            body,
        }
    }

    /// A Profile to associate resources with, reported by its `Id`.
    async fn profile(&self, name: &str) -> String {
        let created = self
            .call(
                "POST",
                "/profile",
                Some(json!({ "Name": name, "ClientToken": format!("tok-{name}") })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateProfile({name}) expected 200, got {} / {:?}",
            created.status, created.body
        );
        created.member("Profile")["Id"]
            .as_str()
            .expect("a created Profile reports its Id")
            .to_string()
    }

    async fn associate(
        &self,
        profile_id: &str,
        resource_arn: &str,
        name: &str,
        properties: Option<&str>,
    ) -> Reply {
        let mut body = json!({
            "ProfileId": profile_id,
            "ResourceArn": resource_arn,
            "Name": name,
        });
        if let Some(properties) = properties {
            body["ResourceProperties"] = json!(properties);
        }
        self.call("POST", "/profileresourceassociation", Some(body))
            .await
    }

    async fn get_association(&self, id: &str) -> Reply {
        self.call("GET", &format!("/profileresourceassociation/{id}"), None)
            .await
    }

    /// How many resource associations the store holds, so a refused request can be
    /// shown to have persisted nothing.
    fn stored_associations(&self) -> usize {
        service_state(&*self.engine)
            .expect("the service's store is mounted on this engine")
            .lock()
            .profile_resource_associations
            .len()
    }

    /// Bring the clock-owed move to `COMPLETE` forward instead of waiting for it:
    /// the due instant is what the reported status is a function of.
    fn settle(&self, id: &str) {
        let state = service_state(&*self.engine).expect("the service's store is mounted");
        let mut store = state.lock();
        let association = store
            .profile_resource_associations
            .get_mut(id)
            .unwrap_or_else(|| panic!("association {id} is not in the store"));
        association.settled_at = Some(Instant::now());
    }
}

/// The resources a Profile can hold, as `(ResourceArn, ResourceType)`: one row per
/// kind named by `ProfileResourceAssociation$ResourceType`'s documentation ("a
/// private hosted zone, or DNS Firewall rule group") and the Resolver resources a
/// Profile carries beside them. The `ResourceType` is the ARN's own resource-type
/// segment, upper-snake.
fn associable_resources() -> Vec<(String, &'static str)> {
    vec![
        (
            "arn:aws:route53:::hostedzone/Z04567891011ABCDEFGHI".to_string(),
            "HOSTEDZONE",
        ),
        (
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-rule/rslvr-rr-9f8e7d6c5b4a3210"),
            "RESOLVER_RULE",
        ),
        (
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-0123456789abcdef"),
            "FIREWALL_RULE_GROUP",
        ),
        (
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-query-log-config/rslvr-rqlc-abcdef0123456789"),
            "RESOLVER_QUERY_LOG_CONFIG",
        ),
        (
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-dnssec-config/rslvr-ds-fedcba9876543210"),
            "RESOLVER_DNSSEC_CONFIG",
        ),
        (
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc-endpoint/vpce-0a1b2c3d4e5f60718"),
            "VPC_ENDPOINT",
        ),
    ]
}

/// Whether `id` is `rpr-` followed by 16 lowercase hex digits — the id shape this
/// emulator mints for a resource association, inside `ResourceId`'s modelled 1..64
/// length.
fn is_minted_id(id: &str) -> bool {
    match id.strip_prefix("rpr-") {
        Some(hex) => hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

#[tokio::test]
async fn an_associated_resource_is_persisted_and_read_back_whole() {
    let em = Emulator::new();

    // The names span the classes the modelled `Name` accepts (`@length(0..64)`,
    // pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`), and the properties span "none"
    // and "a document the request chose". Each row associates to its OWN Profile, so
    // every call is a (Profile, resource) pair the store does not hold yet.
    for (index, (name, properties)) in [
        ("a", None),
        ("my-resource", Some(r#"{"priority":101}"#)),
        ("My_Resource 1", None),
        ("o'brien", Some(r#"{"priority":9900}"#)),
        ("0a", None),
    ]
    .into_iter()
    .enumerate()
    {
        let profile_id = em.profile(&format!("roundtrip{index}")).await;

        for (resource_arn, resource_type) in associable_resources() {
            // Only a DNS Firewall rule group's properties carry a priority.
            let properties = properties.filter(|_| resource_type == "FIREWALL_RULE_GROUP");
            let case = format!("arn={resource_arn} name={name:?} properties={properties:?}");

            let created = em
                .associate(&profile_id, &resource_arn, name, properties)
                .await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected 200, got {} / {:?}",
                created.status, created.body
            );
            let id = created.field("Id");
            assert!(
                is_minted_id(&id),
                "case {case}: expected a minted `rpr-<16 hex>` Id, got {id:?}"
            );
            assert_eq!(
                created.field("Name"),
                name,
                "case {case}: the Name must be the one requested, got {:?}",
                created.association()
            );
            assert_eq!(
                created.field("ProfileId"),
                profile_id,
                "case {case}: unexpected ProfileId in {:?}",
                created.association()
            );
            assert_eq!(
                created.field("ResourceArn"),
                resource_arn,
                "case {case}: the ResourceArn must be the one requested, got {:?}",
                created.association()
            );
            assert_eq!(
                created.field("ResourceType"),
                resource_type,
                "case {case}: unexpected ResourceType in {:?}",
                created.association()
            );
            assert_eq!(
                created.field("OwnerId"),
                ACCOUNT_ID,
                "case {case}: unexpected OwnerId in {:?}",
                created.association()
            );
            assert_eq!(
                created.association()["ResourceProperties"],
                properties.map(Value::from).unwrap_or(Value::Null),
                "case {case}: ResourceProperties must be echoed, got {:?}",
                created.association()
            );
            // The resource still has to reach the Profile's VPCs, so the association
            // is reported in the transient status first.
            assert_eq!(
                created.field("Status"),
                "CREATING",
                "case {case}: a just-created association is CREATING, got {:?}",
                created.association()
            );
            assert_eq!(
                created.field("StatusMessage"),
                "Creating the Profile resource association",
                "case {case}: unexpected StatusMessage in {:?}",
                created.association()
            );
            assert_eq!(
                created.association()["CreationTime"],
                created.association()["ModificationTime"],
                "case {case}: an association nothing has changed was modified when created, \
                 got {:?}",
                created.association()
            );

            // The read-back is the same record, member for member.
            let fetched = em.get_association(&id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: GetProfileResourceAssociation expected 200, got {} / {:?}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.association(),
                created.association(),
                "case {case}: the read-back association differs from the one created"
            );

            // Once the transition the clock owed falls due, every later read reports
            // the settled status — and keeps reporting it.
            em.settle(&id);
            for read in 0..2 {
                let settled = em.get_association(&id).await;
                assert_eq!(
                    settled.field("Status"),
                    "COMPLETE",
                    "case {case}: read {read} after the due instant must be COMPLETE, got {:?}",
                    settled.association()
                );
                assert_eq!(
                    settled.field("StatusMessage"),
                    "Completed creation of Profile resource association",
                    "case {case}: unexpected settled StatusMessage in {:?}",
                    settled.association()
                );
                assert_eq!(
                    settled.field("Id"),
                    id,
                    "case {case}: a read must report the association asked for, got {:?}",
                    settled.association()
                );
            }
        }
    }
}

#[tokio::test]
async fn a_dns_firewall_priority_is_accepted_inside_the_documented_range_and_refused_outside_it() {
    let em = Emulator::new();
    let profile_id = em.profile("priorities").await;
    let rule_group = |index: i64| {
        format!(
            "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-{index:016x}"
        )
    };

    // Both published bounds, a value between them, and one past each bound — the
    // range is `AssociateResourceToProfile$ResourceProperties`' own ("between 100
    // and 9900").
    let accepted = [MIN_FIREWALL_PRIORITY, 150, 5000, MAX_FIREWALL_PRIORITY];
    let refused = [
        MIN_FIREWALL_PRIORITY - 1,
        MAX_FIREWALL_PRIORITY + 1,
        1,
        0,
        -1,
        99_999,
    ];

    for (index, priority) in accepted.into_iter().enumerate() {
        let arn = rule_group(index as i64);
        let properties = format!(r#"{{"priority": {priority}}}"#);
        let case = format!("priority={priority} (accepted)");
        let created = em
            .associate(&profile_id, &arn, &format!("ok-{priority}"), Some(&properties))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} / {:?}",
            created.status, created.body
        );
        let fetched = em.get_association(&created.field("Id")).await;
        assert_eq!(
            fetched.association()["ResourceProperties"],
            Value::from(properties.as_str()),
            "case {case}: the stored ResourceProperties must be the document given, got {:?}",
            fetched.association()
        );
    }

    for (index, priority) in refused.into_iter().enumerate() {
        let arn = rule_group(1_000 + index as i64);
        let properties = format!(r#"{{"priority": {priority}}}"#);
        let case = format!("priority={priority} (refused)");
        let before = em.stored_associations();
        let refusal = em
            .associate(&profile_id, &arn, &format!("bad-{index}"), Some(&properties))
            .await;
        assert_eq!(
            refusal.status, 400,
            "case {case}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert_eq!(
            refusal.error.as_deref(),
            Some("ValidationException"),
            "case {case}: unexpected error code, got {:?}",
            refusal.body
        );
        assert_eq!(
            refusal.message(),
            "[RSLVR-02017] The priority value you provided is reserved. Provide a number \
             between \"100\" and \"9900\".",
            "case {case}: unexpected refusal message"
        );
        assert_eq!(
            em.stored_associations(),
            before,
            "case {case}: a refused request must persist nothing"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_a_resource_a_profile_cannot_hold_is_refused() {
    let em = Emulator::new();
    let profile_id = em.profile("arn-check").await;

    // Each row is well-formed enough to reach the handler (`Arn` carries only a
    // 1..255 length in the model) but names something no Profile holds: another
    // service's resource, a Resolver resource that is not Profile configuration, an
    // ARN with no resource type at all, and a string that is no ARN.
    let unsupported = [
        "arn:aws:s3:::example-bucket".to_string(),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:answer"),
        format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-endpoint/rslvr-in-0123456789abcdef"),
        format!("arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile/rp-0123456789abcdef"),
        format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group"),
        format!("arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/"),
        "not-an-arn-at-all".to_string(),
    ];

    for (index, resource_arn) in unsupported.into_iter().enumerate() {
        let case = format!("arn={resource_arn}");
        let before = em.stored_associations();
        let refusal = em
            .associate(&profile_id, &resource_arn, &format!("no-{index}"), None)
            .await;
        assert_eq!(
            refusal.status, 400,
            "case {case}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert_eq!(
            refusal.error.as_deref(),
            Some("ValidationException"),
            "case {case}: unexpected error code, got {:?}",
            refusal.body
        );
        assert_eq!(
            refusal.message(),
            format!(
                "[RSLVR-05022] The ARN in your request isn't valid:\"{resource_arn}\". Provide a \
                 valid ARN."
            ),
            "case {case}: unexpected refusal message"
        );
        assert_eq!(
            em.stored_associations(),
            before,
            "case {case}: a refused request must persist nothing"
        );
    }
}

#[tokio::test]
async fn an_unknown_profile_a_repeated_pair_and_an_unminted_id_are_refused() {
    let em = Emulator::new();
    let profile_id = em.profile("refusals").await;
    let other_profile_id = em.profile("second").await;
    let arn = format!(
        "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-aaaabbbbccccdddd"
    );

    // A Profile that was never created holds nothing to associate to.
    for unknown in ["rp-00000000000000000000", "rp-doesnotexist00000"] {
        let refusal = em.associate(unknown, &arn, "orphan", None).await;
        assert_eq!(
            refusal.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case profile={unknown}: expected ResourceNotFoundException, got {:?}",
            refusal.body
        );
        assert_eq!(
            em.stored_associations(),
            0,
            "case profile={unknown}: a refused request must persist nothing"
        );
    }

    let created = em.associate(&profile_id, &arn, "first", None).await;
    assert_eq!(
        created.status, 200,
        "the first associate must succeed, got {} / {:?}",
        created.status, created.body
    );
    let id = created.field("Id");

    // The same pair again conflicts with the association that already holds it,
    // whatever name the repeat carries.
    for name in ["first", "renamed"] {
        let repeat = em.associate(&profile_id, &arn, name, None).await;
        assert_eq!(
            repeat.error.as_deref(),
            Some("ConflictException"),
            "case repeat name={name}: expected ConflictException, got {:?}",
            repeat.body
        );
        assert!(
            repeat.message().contains(&id),
            "case repeat name={name}: the refusal must name the association holding the \
             resource ({id}), got {:?}",
            repeat.message()
        );
        assert_eq!(
            em.stored_associations(),
            1,
            "case repeat name={name}: a refused repeat must persist nothing"
        );
    }

    // A different Profile is a different pair, so the same resource is associable
    // again — a Profile is what the resource is associated TO.
    let second = em.associate(&other_profile_id, &arn, "second", None).await;
    assert_eq!(
        second.status, 200,
        "a second Profile may hold the same resource, got {} / {:?}",
        second.status, second.body
    );
    assert_ne!(
        second.field("Id"),
        id,
        "each association is its own record, got {:?}",
        second.association()
    );

    // An id that was never minted names nothing.
    for unminted in ["rpr-0000000000000000", "rpr-doesnotexist"] {
        let refusal = em.get_association(unminted).await;
        assert_eq!(
            refusal.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case id={unminted}: expected ResourceNotFoundException, got {:?}",
            refusal.body
        );
        assert_eq!(
            refusal.message(),
            format!("[RSLVR-05007] Can't find the resource with ID \"{unminted}\"."),
            "case id={unminted}: unexpected refusal message"
        );
    }
}
