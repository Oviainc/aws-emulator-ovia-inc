// SPDX-License-Identifier: CC0-1.0

//! `CreateEmailIdentityPolicy`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised, and reads what was persisted back through
//! `GetEmailIdentity`, which reports the identity's `Policies` map.
//!
//! The rule under test: one accepted call adds the submitted document to the named
//! identity's policy map under the submitted name, unchanged; a name the identity
//! already carries, a name outside the documented character set, a document that is
//! not a policy or is past the documented ceiling, an identity the account does not
//! hold, and a set already at the documented ceiling are each refused with the
//! modelled error for that class and leave the stored map as it was.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::create_email_identity_policy::{
    MAX_IDENTITY_POLICIES, MAX_POLICY_BYTES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, VERIFICATION_SETTLE,
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
        // A shape name may arrive namespaced (`com.amazonaws.sesv2#NotFound`) or
        // with a trailing URL, as the protocol allows.
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

    /// The identity's policy map, as `GetEmailIdentity` reports it.
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
}

/// The emulated service behind its router, plus the shared engine so a test can
/// read what a handler persisted.
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

    /// `CreateEmailIdentityPolicy`: the name is a path label, the document the body.
    async fn create_policy(&self, identity: &str, name: &str, document: &str) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/identities/{identity}/policies/{name}"),
            Some(json!({"Policy": document})),
        )
        .await
    }

    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// The policies the store itself holds for `identity`, for a case whose read
    /// must not depend on a reporting operation.
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

    /// Hold the clock-owed verification transition off, so a case about an identity
    /// that is still `PENDING` cannot race the span.
    fn hold_verification_pending(&self, identity: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.verification_due_at =
            SystemTime::now() + VERIFICATION_SETTLE + Duration::from_secs(3600);
    }
}

/// A sending-authorization policy whose statement names `principal`: an `Effect`,
/// the SES action it authorizes and the principal it authorizes to use the
/// identity.
fn policy_for(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"ses:SendEmail\",\"Principal\":{{\"AWS\":\"{principal}\"}}}}]}}"
    )
}

/// A policy document of exactly `target` bytes, padded inside its `Sid`, for a case
/// that drives the documented size ceiling.
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

#[tokio::test]
async fn an_accepted_create_adds_the_submitted_document_under_the_submitted_name() {
    // The rows differ along what the rule generalises over: the kind of identity
    // the policy hangs off, how the request spells that identity, the form of the
    // policy name, and the form and size of the document — including a document of
    // exactly the published 4 KB ceiling.
    // `PolicyName` is modelled `length 1..64`, so this is a name of exactly 64
    // characters: 60 letters, the underscore, and three more admitted characters.
    let long_name = format!("{}_{}", "a".repeat(60), "Z-9");
    let rows: [(&str, &str, &str, String, String); 8] = [
        (
            "an address identity",
            "sender@example.com",
            "sender@example.com",
            "AllowPartner".to_string(),
            policy_for("arn:aws:iam::000000000000:root"),
        ),
        (
            "a domain identity",
            "example.com",
            "example.com",
            "AllowAnyone".to_string(),
            policy_for("*"),
        ),
        (
            "a domain spelled in another case, which names the same identity",
            "Example.ORG",
            "example.org",
            "Policy-1".to_string(),
            policy_for("arn:aws:iam::000000000000:user/sender"),
        ),
        (
            "a policy name of underscores and digits",
            "ops@example.net",
            "ops@example.net",
            "policy_2_v3".to_string(),
            policy_for("arn:aws:iam::000000000000:root"),
        ),
        (
            "a policy name at the modelled 64-character ceiling",
            "long@example.net",
            "long@example.net",
            long_name.clone(),
            policy_for("*"),
        ),
        (
            "a document whose Statement is the object itself and declares no Version",
            "object.example",
            "object.example",
            "Inline".to_string(),
            "{\"Statement\":{\"Effect\":\"Allow\",\"Action\":\"ses:SendRawEmail\",\
             \"Principal\":\"*\"}}"
                .to_string(),
        ),
        (
            "a document with two statements, a list of actions and a condition",
            "two.example",
            "two.example",
            "Two".to_string(),
            "{\"Version\":\"2008-10-17\",\"Statement\":[\
             {\"Effect\":\"Allow\",\"Action\":[\"ses:SendEmail\",\"ses:SendRawEmail\"],\
             \"Principal\":\"*\",\"Condition\":{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-1\"}}},\
             {\"Effect\":\"Deny\",\"NotAction\":\"ses:SendEmail\",\
             \"NotPrincipal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\"]}}]}"
                .to_string(),
        ),
        (
            "a document of exactly the published 4 KB ceiling",
            "big.example",
            "big.example",
            "Big".to_string(),
            policy_of_size(MAX_POLICY_BYTES),
        ),
    ];

    for (case, created, spelled, name, document) in rows {
        let ses = Ses::new();
        let start = ses.create_identity(created).await;
        assert_eq!(
            start.status, 200,
            "case {case:?}: the identity must be created first, got {} {:?}",
            start.status, start.body
        );

        let reply = ses.create_policy(spelled, &name, &document).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            reply
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the response models no members, got {:?}",
            reply.body
        );

        let read = ses.get_identity(created).await;
        let policies = read.policies();
        assert_eq!(
            policies.get(&name).map(String::as_str),
            Some(document.as_str()),
            "case {case:?}: expected the submitted document back under {name:?}, got {policies:?}"
        );
        assert_eq!(
            policies.len(),
            1,
            "case {case:?}: expected exactly the one created policy, got {policies:?}"
        );
    }
}

#[tokio::test]
async fn policies_accumulate_and_a_name_the_identity_already_carries_is_refused() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    // Several names on ONE identity: each create adds its own entry, and none
    // disturbs the documents already there.
    let created: [(&str, String); 3] = [
        ("First", policy_for("arn:aws:iam::000000000000:root")),
        ("second_policy", policy_for("*")),
        ("third-policy", policy_for("arn:aws:iam::000000000000:user/x")),
    ];
    for (name, document) in &created {
        let reply = ses.create_policy("example.com", name, document).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
    }

    let expected: BTreeMap<String, String> = created
        .iter()
        .map(|(name, document)| ((*name).to_string(), document.clone()))
        .collect();
    let policies = ses.get_identity("example.com").await.policies();
    assert_eq!(
        policies, expected,
        "case \"three policies\": expected every created policy to be reported, got {policies:?}"
    );

    // A create is not an upsert: a name the identity already carries is refused,
    // and the document it already holds stays.
    let replacement = policy_for("arn:aws:iam::000000000000:user/other");
    let again = ses
        .create_policy("example.com", "First", &replacement)
        .await;
    assert_eq!(
        (again.status, again.error_code().as_str()),
        (400, "AlreadyExistsException"),
        "case \"a repeated policy name\": expected 400 AlreadyExistsException, got {} {:?}",
        again.status,
        again.body
    );
    assert!(
        again.message().contains("First"),
        "case \"a repeated policy name\": the refusal names the policy, got {:?}",
        again.message()
    );
    let after = ses.get_identity("example.com").await.policies();
    assert_eq!(
        after, expected,
        "case \"a repeated policy name\": the stored documents are unchanged, got {after:?}"
    );
}

#[tokio::test]
async fn a_policy_for_an_identity_the_account_does_not_hold_is_not_found() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    // The rule is about the identity the policy hangs off, so it holds for every
    // kind of name the account holds no identity for — including a subdomain of a
    // domain it does hold, which is a different identity.
    for case in [
        "absent.example",
        "nobody@example.net",
        "mail.example.com",
        "sender@example.com",
    ] {
        let reply = ses
            .create_policy(case, "Policy", &policy_for("*"))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            ses.stored_policies(case).is_empty(),
            "case {case:?}: nothing is persisted for an identity the account does not hold, got {:?}",
            ses.stored_policies(case)
        );
    }

    // The identity that DOES exist was not touched by any of those calls.
    let policies = ses.get_identity("example.com").await.policies();
    assert!(
        policies.is_empty(),
        "case \"the held identity\": expected no policy on it, got {policies:?}"
    );
}

#[tokio::test]
async fn text_that_is_not_a_sending_authorization_policy_is_refused() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    // A policy document is an IAM resource-based policy, so the rows differ along
    // the clauses of that grammar: the JSON type of the document, whether it parses
    // at all, whether it carries a statement, and whether each statement names an
    // effect, an action and the principal it authorizes.
    let rows: [(&str, &str); 9] = [
        ("text that is not JSON", "let me send"),
        ("a truncated document", "{\"Statement\":"),
        ("a bare number", "5"),
        ("a bare string", "\"policy\""),
        ("an array of statements with no document around them",
         "[{\"Effect\":\"Allow\",\"Action\":\"ses:SendEmail\",\"Principal\":\"*\"}]"),
        ("an empty object", "{}"),
        ("a Statement that is an empty list", "{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
        (
            "a statement naming no principal",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"ses:SendEmail\"}]}",
        ),
        (
            "a statement naming no action",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\"}]}",
        ),
    ];

    for (case, document) in rows {
        let reply = ses.create_policy("example.com", "Policy", document).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            ses.stored_policies("example.com").is_empty(),
            "case {case:?}: a refused document is not persisted, got {:?}",
            ses.stored_policies("example.com")
        );
    }
}

#[tokio::test]
async fn a_document_past_the_published_size_ceiling_is_refused() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    // The published ceiling is 4 KB: the bound itself is accepted, one byte past it
    // is not, and nor is a document far past it.
    for (case, size, expected) in [
        ("one byte under the ceiling", MAX_POLICY_BYTES - 1, 200),
        ("exactly the ceiling", MAX_POLICY_BYTES, 200),
        ("one byte past the ceiling", MAX_POLICY_BYTES + 1, 400),
        ("far past the ceiling", MAX_POLICY_BYTES * 4, 400),
    ] {
        let document = policy_of_size(size);
        let name = format!("Size{size}");
        let reply = ses.create_policy("example.com", &name, &document).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected {expected}, got {} {:?}",
            reply.status, reply.body
        );
        if expected == 400 {
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                reply.body
            );
            assert!(
                !ses.stored_policies("example.com").contains_key(&name),
                "case {case:?}: a refused document is not persisted, got {:?}",
                ses.stored_policies("example.com")
            );
        } else {
            assert_eq!(
                ses.stored_policies("example.com").get(&name),
                Some(&document),
                "case {case:?}: the accepted document is stored verbatim"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_name_outside_the_documented_character_set_is_refused() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    // `PolicyName` "can only include alphanumeric characters, dashes, and
    // underscores", so the rows differ along the character that breaks it:
    // separators, punctuation, a wildcard and the sub-delimiters a path segment
    // admits. The last row carries an encoded space, which the name is refused for
    // whether the framework hands the handler the encoded or the decoded form.
    for case in [
        "policy.one",
        "policy*",
        "policy+one",
        "policy,one",
        "policy=one",
        "policy:one",
        "policy~one",
        "policy%20one",
    ] {
        let reply = ses
            .create_policy("example.com", case, &policy_for("*"))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            ses.stored_policies("example.com").is_empty(),
            "case {case:?}: a refused name is not persisted, got {:?}",
            ses.stored_policies("example.com")
        );
    }

    // The characters the documentation does admit are accepted, so the refusal is
    // about the character set and not about the name's shape.
    for case in ["Policy1", "policy-one", "policy_one", "P"] {
        let reply = ses
            .create_policy("example.com", case, &policy_for("*"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn one_identity_holds_at_most_the_documented_number_of_policies() {
    let ses = Ses::new();
    ses.create_identity("example.com").await;

    for index in 0..MAX_IDENTITY_POLICIES {
        let name = format!("policy_{index}");
        let reply = ses
            .create_policy("example.com", &name, &policy_for("*"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected 200 below the ceiling, got {} {:?}",
            reply.status, reply.body
        );
    }

    let reply = ses
        .create_policy("example.com", "one_too_many", &policy_for("*"))
        .await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "LimitExceededException"),
        "case \"one past the ceiling\": expected 400 LimitExceededException, got {} {:?}",
        reply.status,
        reply.body
    );
    let policies = ses.stored_policies("example.com");
    assert_eq!(
        policies.len(),
        MAX_IDENTITY_POLICIES,
        "case \"one past the ceiling\": the refused create added nothing, got {policies:?}"
    );

    // The ceiling is per identity, not per account: another identity still takes a
    // policy of its own.
    ses.create_identity("other.example").await;
    let reply = ses
        .create_policy("other.example", "policy_0", &policy_for("*"))
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"another identity at the ceiling of the first\": expected 200, got {} {:?}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn an_identity_still_pending_verification_takes_a_policy() {
    let ses = Ses::new();
    ses.create_identity("pending.example").await;
    // The transition the clock owes is held off, so this case cannot race it: what
    // the rule turns on is that the account HOLDS the identity, which it does from
    // the moment verification started.
    ses.hold_verification_pending("pending.example");

    let document = policy_for("arn:aws:iam::000000000000:root");
    let reply = ses
        .create_policy("pending.example", "WhilePending", &document)
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"an identity still pending verification\": expected 200, got {} {:?}",
        reply.status, reply.body
    );

    let read = ses.get_identity("pending.example").await;
    assert_eq!(
        read.body.get("VerificationStatus").and_then(Value::as_str),
        Some("PENDING"),
        "case \"an identity still pending verification\": the identity is still PENDING, got {:?}",
        read.body
    );
    assert_eq!(
        read.policies().get("WhilePending"),
        Some(&document),
        "case \"an identity still pending verification\": expected the document back, got {:?}",
        read.policies()
    );
}
