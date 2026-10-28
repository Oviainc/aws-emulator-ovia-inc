// SPDX-License-Identifier: CC0-1.0

//! `GetEmailIdentityPolicies`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rule under test: a read of an identity's sending-authorization policies
//! reports exactly the map that identity carries — every stored policy name against
//! the document stored under it, byte for byte, and an empty map when it carries none
//! — resolving the identity the way the identity table is keyed; a name the account
//! holds no identity for is refused with the modelled 404; and the read leaves what a
//! later caller sees untouched.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::create_email_identity_policy::{
    MAX_IDENTITY_POLICIES, MAX_POLICY_BYTES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    email_identity_not_found, identity_key, ComAmazonawsSesv2SimpleemailserviceV2State,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        // A shape name may arrive namespaced (`com.amazonaws.sesv2#NotFound`) or with
        // a trailing URL, as the protocol allows.
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Policies` map the response reports, as name against document.
    fn policies(&self) -> BTreeMap<String, String> {
        self.body
            .get("Policies")
            .and_then(Value::as_object)
            .map(|members| {
                members
                    .iter()
                    .map(|(name, document)| {
                        (
                            name.clone(),
                            document.as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the response carries a `Policies` member at all, for the case about an
    /// identity that holds no policy.
    fn reports_a_policy_map(&self) -> bool {
        self.body.get("Policies").is_some_and(Value::is_object)
    }
}

/// The emulated service behind its router, plus the shared engine so a test can read
/// what a handler persisted.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
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

    /// Start verifying an identity, so the account holds it.
    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn delete_identity(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// `CreateEmailIdentityPolicy`: the name is a path label, the document the body.
    async fn create_policy(&self, identity: &str, name: &str, document: &str) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/identities/{identity}/policies/{name}"),
            Some(json!({"Policy": document})),
        )
        .await
    }

    /// The operation under test.
    async fn get_policies(&self, identity: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/identities/{identity}/policies"),
            None,
        )
        .await
    }

    /// `GetEmailIdentity`, which reports the same map among the identity's other
    /// attributes: the two readers answer from one record, so they must agree.
    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// The policies the store itself holds for `identity`, for an assertion that a
    /// read changed nothing.
    fn stored_policies(&self, identity: &str) -> BTreeMap<String, String> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .identities
            .get(&identity_key(identity))
            .map(|record| record.policies.clone())
            .unwrap_or_default()
            .into_iter()
            .collect()
    }
}

/// A sending-authorization policy whose statement names `principal`: an `Effect`, the
/// SES action it authorizes and the principal it authorizes to use the identity.
fn policy_for(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"ses:SendEmail\",\"Principal\":{{\"AWS\":\"{principal}\"}}}}]}}"
    )
}

/// A policy document of exactly `target` bytes, padded inside its `Sid`, for the row
/// that carries a document at the published size ceiling.
fn policy_of_size(target: usize) -> String {
    let prefix = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"";
    let suffix = "\",\"Effect\":\"Allow\",\"Action\":\"ses:SendEmail\",\
                  \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}]}";
    let padding = target.saturating_sub(prefix.len() + suffix.len());
    let document = format!("{prefix}{}{suffix}", "a".repeat(padding));
    assert_eq!(
        document.len(),
        target,
        "case {target:?}: the padded document must be exactly that many bytes, got {}",
        document.len()
    );
    document
}

/// One row of the table: the identity the policies were created on, the spelling the
/// read addresses it with, and the policies it carries.
struct ReadCase {
    case: &'static str,
    created: &'static str,
    spelled: &'static str,
    policies: Vec<(String, String)>,
}

/// The rows differ along what the rule generalises over: how many policies the
/// identity carries (none, one, several, the whole retrievable set), the kind of
/// identity they hang off, the spelling the read addresses it with, and the form and
/// size of the names and documents stored.
fn read_cases() -> Vec<ReadCase> {
    // `PolicyName` is modelled `length 1..64`, so this is a name of exactly 64
    // characters: 60 letters, an underscore and three more admitted characters.
    let long_name = format!("{}_{}", "a".repeat(60), "Z-9");
    vec![
        ReadCase {
            case: "an identity carrying no policy",
            created: "quiet.example",
            spelled: "quiet.example",
            policies: Vec::new(),
        },
        ReadCase {
            case: "an address identity carrying one policy",
            created: "sender@example.com",
            spelled: "sender@example.com",
            policies: vec![(
                "AllowPartner".to_string(),
                policy_for("arn:aws:iam::000000000000:root"),
            )],
        },
        ReadCase {
            case: "a domain identity carrying three policies",
            created: "example.com",
            spelled: "example.com",
            policies: vec![
                ("First".to_string(), policy_for("*")),
                (
                    "second_policy".to_string(),
                    policy_for("arn:aws:iam::000000000000:user/sender"),
                ),
                (
                    "third-policy".to_string(),
                    policy_for("arn:aws:iam::000000000000:root"),
                ),
            ],
        },
        ReadCase {
            case: "a domain addressed in another case, which names the same identity",
            created: "Example.ORG",
            spelled: "example.org",
            policies: vec![("Policy-1".to_string(), policy_for("*"))],
        },
        ReadCase {
            case: "an address identity addressed with its domain in another case",
            created: "ops@example.net",
            spelled: "ops@Example.NET",
            policies: vec![(long_name, policy_for("arn:aws:iam::000000000000:root"))],
        },
        ReadCase {
            case: "a document at the published size ceiling, alongside a short one",
            created: "big.example",
            spelled: "big.example",
            policies: vec![
                ("Big".to_string(), policy_of_size(MAX_POLICY_BYTES)),
                (
                    "Inline".to_string(),
                    "{\"Statement\":{\"Effect\":\"Allow\",\"Action\":\"ses:SendRawEmail\",\
                     \"Principal\":\"*\"}}"
                        .to_string(),
                ),
            ],
        },
        ReadCase {
            case: "an identity carrying the whole retrievable set of policies",
            created: "full.example",
            spelled: "full.example",
            policies: (0..MAX_IDENTITY_POLICIES)
                .map(|index| {
                    (
                        format!("policy_{index}"),
                        policy_for(&format!("arn:aws:iam::000000000000:user/sender{index}")),
                    )
                })
                .collect(),
        },
    ]
}

#[tokio::test]
async fn a_read_reports_exactly_the_policies_the_identity_carries() {
    for row in read_cases() {
        let ReadCase {
            case,
            created,
            spelled,
            policies,
        } = row;
        let ses = Ses::new();
        let start = ses.create_identity(created).await;
        assert_eq!(
            start.status, 200,
            "case {case:?}: the identity must be created first, got {} {:?}",
            start.status, start.body
        );
        for (name, document) in &policies {
            let written = ses.create_policy(created, name, document).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: policy {name:?} must be created first, got {} {:?}",
                written.status, written.body
            );
        }

        let expected: BTreeMap<String, String> = policies.iter().cloned().collect();
        let reply = ses.get_policies(spelled).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            reply.reports_a_policy_map(),
            "case {case:?}: expected a Policies map in the response, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.policies(),
            expected,
            "case {case:?}: expected exactly the stored policies, got {:?}",
            reply.policies()
        );

        // The identity's other reader answers from the same record, so the two must
        // report one map.
        let alongside = ses.get_identity(spelled).await.policies();
        assert_eq!(
            alongside,
            expected,
            "case {case:?}: GetEmailIdentity must report the same map, got {alongside:?}"
        );
    }
}

#[tokio::test]
async fn a_read_leaves_what_a_later_caller_sees_untouched() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;
    let stored: Vec<(&str, String)> = vec![
        ("First", policy_for("*")),
        ("Second", policy_for("arn:aws:iam::000000000000:root")),
    ];
    for (name, document) in &stored {
        ses.create_policy("example.com", name, document).await;
    }
    let expected: BTreeMap<String, String> = stored
        .iter()
        .map(|(name, document)| ((*name).to_string(), document.clone()))
        .collect();

    // Reading repeatedly answers the same map, and the record behind it is the one it
    // was before the first read.
    for attempt in 1..=3 {
        let reply = ses.get_policies("example.com").await;
        assert_eq!(
            reply.policies(),
            expected,
            "case \"read {attempt}\": expected the same map every time, got {:?}",
            reply.policies()
        );
    }
    let after = ses.stored_policies("example.com");
    assert_eq!(
        after, expected,
        "case \"three reads\": the stored documents are unchanged, got {after:?}"
    );
}

#[tokio::test]
async fn an_identity_the_account_does_not_hold_is_not_found() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;
    ses.create_policy("example.com", "First", &policy_for("*"))
        .await;

    // The rule is about the identity the policies hang off, so it holds for every name
    // the account holds no identity for — a name it never held, an address under a
    // domain identity it does hold (a different identity), a subdomain of one, and a
    // name whose identity has been deleted, which takes the policies with it.
    ses.create_identity("gone.example").await;
    ses.create_policy("gone.example", "Doomed", &policy_for("*"))
        .await;
    let removed = ses.delete_identity("gone.example").await;
    assert_eq!(
        removed.status, 200,
        "case \"a deleted identity\": the delete must succeed first, got {} {:?}",
        removed.status, removed.body
    );

    for case in [
        "absent.example",
        "nobody@example.net",
        "mail.example.com",
        "sender@example.com",
        "gone.example",
    ] {
        let reply = ses.get_policies(case).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            email_identity_not_found(case),
            "case {case:?}: expected the shared not-found sentence, got {:?}",
            reply.message()
        );
        assert!(
            reply.policies().is_empty(),
            "case {case:?}: a refusal reports no policies, got {:?}",
            reply.policies()
        );
    }

    // The identity the account does hold still reports its own policy, so none of the
    // refusals touched it.
    let held = ses.get_policies("example.com").await;
    assert_eq!(
        held.policies().keys().cloned().collect::<Vec<String>>(),
        vec!["First".to_string()],
        "case \"the held identity\": expected its policy to survive, got {:?}",
        held.policies()
    );
}

#[tokio::test]
async fn an_identity_created_again_after_a_delete_carries_no_policy() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;
    ses.create_policy("example.com", "First", &policy_for("*"))
        .await;
    ses.delete_identity("example.com").await;
    let again = ses.create_identity("example.com").await;
    assert_eq!(
        again.status, 200,
        "case \"re-created identity\": the create must succeed, got {} {:?}",
        again.status, again.body
    );

    let reply = ses.get_policies("example.com").await;
    assert_eq!(
        reply.status, 200,
        "case \"re-created identity\": expected 200, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.policies().is_empty(),
        "case \"re-created identity\": expected no policy, got {:?}",
        reply.policies()
    );
}
