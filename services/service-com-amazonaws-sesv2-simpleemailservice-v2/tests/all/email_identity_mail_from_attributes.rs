// SPDX-License-Identifier: CC0-1.0

//! `PutEmailIdentityMailFromAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization
//! are all exercised, and the persisted configuration is read back through
//! `GetEmailIdentity`.
//!
//! The rule these cases pin: a custom MAIL FROM domain an identity may take is a
//! domain strictly under the identity's own domain, compared the way DNS compares
//! domain names, and any other domain — the identity's own included, since that is
//! the domain receiving its mail — is refused with a 400 `BadRequestException`
//! quoting the domain asked for and the identity's own, which stores nothing; an accepted request carries the whole configuration, so the
//! identity ends up with exactly the domain and behaviour it names, in the `PENDING`
//! state the MX search starts in, and a request that names no domain disables the
//! configuration. The tables below span what the rule generalises over: both
//! identity kinds and their spellings, subdomains several labels deep, the identity's
//! own domain, the domains that merely resemble one, the values not spelled as a
//! domain name at all and each published DNS length bound, each modelled behaviour
//! and the request that omits it, the repeated write, and each way the identity
//! lookup can miss.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, MAIL_FROM_MX_SETTLE,
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

    /// `MailFromAttributes` as the read reported it, absent when the identity has no
    /// custom MAIL FROM configuration.
    fn mail_from(&self) -> Option<&Value> {
        self.body.get("MailFromAttributes")
    }

    /// One member of the reported `MailFromAttributes`.
    fn mail_from_member(&self, member: &str) -> String {
        self.mail_from()
            .and_then(|attributes| attributes.get(member))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// read what the handler persisted rather than only the read-back response.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
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

    async fn create(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// `PutEmailIdentityMailFromAttributes` with whatever body the case supplies, so
    /// a case can leave either optional member out entirely.
    async fn put_mail_from_body(&self, identity: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/mail-from"),
            Some(body),
        )
        .await
    }

    /// `PutEmailIdentityMailFromAttributes` with the domain supplied, and the
    /// behaviour only when the case names one.
    async fn put_mail_from(&self, identity: &str, domain: &str, behavior: Option<&str>) -> Reply {
        let mut body = json!({ "MailFromDomain": domain });
        if let Some(behavior) = behavior {
            body["BehaviorOnMxFailure"] = json!(behavior);
        }
        self.put_mail_from_body(identity, body).await
    }

    /// The stored MAIL FROM configuration as `(domain, status, behaviour)`, absent
    /// when the identity holds none.
    fn stored_mail_from(&self, identity: &str) -> Option<(String, String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.mail_from.as_ref().map(|mail_from| {
            (
                mail_from.domain.clone(),
                mail_from.status.as_str().to_string(),
                mail_from.behavior_on_mx_failure.clone(),
            )
        })
    }

    /// When the stored configuration's `PENDING` -> `SUCCESS` transition falls due.
    fn mx_lookup_due_at(&self, identity: &str) -> SystemTime {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .identities
            .get(&identity_key(identity))
            .and_then(|record| record.mail_from.as_ref())
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must hold a configuration"))
            .mx_lookup_due_at
    }

    /// Move the clock-owed transitions of `identity` out of reach, so a case reading
    /// the transient state cannot race the spans in which they become due.
    fn hold_transitions_pending(&self, identity: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        let held = SystemTime::now() + MAIL_FROM_MX_SETTLE + Duration::from_secs(3600);
        record.verification_due_at = held;
        if let Some(mail_from) = record.mail_from.as_mut() {
            mail_from.mx_lookup_due_at = held;
        }
    }

    /// Make the MAIL FROM MX search's outcome due, as the clock does once the span
    /// has passed, without sleeping for it.
    fn make_mx_lookup_due(&self, identity: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let mail_from = store
            .identities
            .get_mut(&identity_key(identity))
            .and_then(|record| record.mail_from.as_mut())
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must hold a configuration"));
        mail_from.mx_lookup_due_at = SystemTime::now() - Duration::from_secs(1);
    }
}

/// `(identity, MAIL FROM domain)` pairs the rule accepts: the domain sits strictly
/// under the identity's own, for an address identity and a domain identity alike,
/// however either is spelled.
const WITHIN_THE_IDENTITY: &[(&str, &str)] = &[
    ("example.com", "mail.example.com"),
    ("example.com", "bounce.feedback.example.com"),
    ("example.com", "MAIL.Example.COM"),
    ("Mixed.Case@Example.COM", "mail.example.com"),
    ("sender@example.com", "mail.example.com"),
    ("sender@example.com", "deep.bounce.example.com"),
    ("no-reply@mail.example.net", "bounce.mail.example.net"),
    ("mail.example.org", "out.mail.example.org"),
    ("SUB.Example.ORG", "feedback.sub.example.org"),
];

/// `(identity, MAIL FROM domain)` pairs the rule refuses: a domain that does not sit
/// strictly under the identity's own, however close its spelling comes — the
/// identity's own domain itself, in either spelling, a sibling, a name the
/// identity's own is a suffix of, a name that merely ends with the identity's
/// spelling, a parent of a subdomain identity, a bare public suffix, the empty name,
/// a name with no label before the separating dot, and the values that are not
/// spelled as a domain name at all and so name none: an empty label, a label
/// starting or ending with a hyphen, a value carrying whitespace, and an address
/// rather than a domain.
const OUTSIDE_THE_IDENTITY: &[(&str, &str)] = &[
    ("example.com", "example.com"),
    ("example.com", "Example.COM"),
    ("example.com", "example.net"),
    ("example.com", "notexample.com"),
    ("example.com", "mail.notexample.com"),
    ("example.com", "example.com.attacker.net"),
    ("example.com", "com"),
    ("example.com", ""),
    ("example.com", ".example.com"),
    ("example.com", "mail..example.com"),
    ("example.com", "-mail.example.com"),
    ("example.com", "mail-.example.com"),
    ("example.com", " mail.example.com"),
    ("example.com", "mail.example.com "),
    ("example.com", "ma il.example.com"),
    ("example.com", "sender@example.com"),
    ("sender@example.com", "example.com"),
    ("sender@example.com", "example.org"),
    ("sender@example.com", "mail.example.net"),
    ("no-reply@mail.example.net", "mail.example.net"),
    ("mail.example.org", "mail.example.org"),
    ("mail.example.org", "example.org"),
    ("mail.example.org", "other.mail.example.org.net"),
    ("SUB.Example.ORG", "sub.example.org"),
    ("SUB.Example.ORG", "example.org"),
];

/// Every `BehaviorOnMxFailure` the model declares, plus the request that names none.
///
/// From `smithy_query(query="shape", shape="BehaviorOnMxFailure")`: the enum's
/// values are `USE_DEFAULT_VALUE` and `REJECT_MESSAGE`.
const BEHAVIORS: &[Option<&str>] = &[
    None,
    Some("USE_DEFAULT_VALUE"),
    Some("REJECT_MESSAGE"),
];

/// What the stored configuration reports when the request named no behaviour.
const DEFAULT_BEHAVIOR: &str = "USE_DEFAULT_VALUE";

/// The longest one label of a domain name may be, in characters (RFC 1035 §2.3.4).
const MAX_DOMAIN_LABEL_CHARS: usize = 63;

/// The longest a domain name may be written out, in characters: RFC 1035 §2.3.4
/// limits a name to 255 octets on the wire, of which 253 are the name's own
/// characters.
const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// A domain name of exactly `total` characters sitting strictly under `parent`, made
/// of labels no longer than one label may be.
fn name_of_length(total: usize, parent: &str) -> String {
    let suffix = format!(".{parent}");
    let prefix_len = total - suffix.len();
    let mut prefix = String::new();
    while prefix.len() < prefix_len {
        let remaining = prefix_len - prefix.len();
        if prefix.is_empty() {
            prefix.push_str(&"a".repeat(remaining.min(MAX_DOMAIN_LABEL_CHARS)));
        } else {
            // A dot, then at least one character of the next label.
            let label = (remaining - 1).min(MAX_DOMAIN_LABEL_CHARS);
            prefix.push('.');
            prefix.push_str(&"a".repeat(label));
        }
    }
    format!("{prefix}{suffix}")
}

#[tokio::test]
async fn a_domain_within_the_identity_is_stored_and_reported() {
    for (identity, domain) in WITHIN_THE_IDENTITY {
        for behavior in BEHAVIORS {
            let ses = Ses::new();
            let created = ses.create(identity).await;
            assert_eq!(
                created.status, 200,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the identity to be created, got {} {:?}",
                created.status, created.body
            );

            let put = ses.put_mail_from(identity, domain, *behavior).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected 200, got {} {:?}",
                put.status, put.body
            );
            // The response shape models no members, so the body carries none.
            assert!(
                put.body.as_object().is_none_or(serde_json::Map::is_empty),
                "case ({identity:?}, {domain:?}, {behavior:?}): the response models no members, got {:?}",
                put.body
            );

            let expected_behavior = behavior.unwrap_or(DEFAULT_BEHAVIOR);
            assert_eq!(
                ses.stored_mail_from(identity),
                Some((
                    (*domain).to_string(),
                    "PENDING".to_string(),
                    expected_behavior.to_string()
                )),
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the stored configuration to be the one the request carried, got {:?}",
                ses.stored_mail_from(identity)
            );

            // Held pending, so the read below reports the state the MX search
            // starts in rather than racing the span it settles in.
            ses.hold_transitions_pending(identity);
            let read = ses.get(identity).await;
            assert_eq!(
                read.status, 200,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the identity to be readable, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.mail_from_member("MailFromDomain"),
                *domain,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected MailFromDomain {domain:?}, got {:?}",
                read.mail_from()
            );
            assert_eq!(
                read.mail_from_member("BehaviorOnMxFailure"),
                expected_behavior,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected BehaviorOnMxFailure {expected_behavior:?}, got {:?}",
                read.mail_from()
            );
            assert_eq!(
                read.mail_from_member("MailFromDomainStatus"),
                "PENDING",
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the MX search to start in PENDING, got {:?}",
                read.mail_from()
            );

            // The outcome of the MX search is owed by the clock, not by a request:
            // once it falls due, every read reports it.
            ses.make_mx_lookup_due(identity);
            let settled = ses.get(identity).await;
            assert_eq!(
                settled.mail_from_member("MailFromDomainStatus"),
                "SUCCESS",
                "case ({identity:?}, {domain:?}, {behavior:?}): expected SUCCESS once the MX search fell due, got {:?}",
                settled.mail_from()
            );
            assert_eq!(
                settled.mail_from_member("MailFromDomain"),
                *domain,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the domain to survive the transition, got {:?}",
                settled.mail_from()
            );
        }
    }
}

#[tokio::test]
async fn a_domain_outside_the_identity_is_refused() {
    for (identity, domain) in OUTSIDE_THE_IDENTITY {
        for behavior in BEHAVIORS {
            let ses = Ses::new();
            ses.create(identity).await;

            let put = ses.put_mail_from(identity, domain, *behavior).await;
            assert_eq!(
                put.status, 400,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected 400, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                put.error_code(),
                "BadRequestException",
                "case ({identity:?}, {domain:?}, {behavior:?}): expected BadRequestException, got {:?}",
                put.body
            );
            assert_eq!(
                put.message(),
                format!(
                    "Provided MAIL-FROM domain <{domain}> is not subdomain of the domain of the identity <{}>.",
                    identity
                        .rsplit_once('@')
                        .map_or(*identity, |(_, after)| after)
                ),
                "case ({identity:?}, {domain:?}, {behavior:?}): expected the refusal to quote both domains, got {:?}",
                put.message()
            );
            // A refused request configures nothing.
            assert_eq!(
                ses.stored_mail_from(identity),
                None,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected a refused request to store nothing, got {:?}",
                ses.stored_mail_from(identity)
            );
            let read = ses.get(identity).await;
            assert_eq!(
                read.mail_from(),
                None,
                "case ({identity:?}, {domain:?}, {behavior:?}): expected no MailFromAttributes to be reported, got {:?}",
                read.mail_from()
            );
        }
    }
}

#[tokio::test]
async fn a_refused_domain_leaves_the_configuration_already_held() {
    // The same refusal on an identity that already has a configuration: the request
    // is rejected whole, so what was configured before stays configured.
    for (identity, domain) in OUTSIDE_THE_IDENTITY {
        let ses = Ses::new();
        ses.create(identity).await;
        let held = format!(
            "mail.{}",
            identity.rsplit_once('@').map_or(*identity, |(_, after)| after)
        );
        let configured = ses
            .put_mail_from(identity, &held, Some("REJECT_MESSAGE"))
            .await;
        assert_eq!(
            configured.status, 200,
            "case ({identity:?}, {domain:?}): expected {held:?} to be configurable, got {} {:?}",
            configured.status, configured.body
        );
        let before = ses.stored_mail_from(identity);

        let put = ses.put_mail_from(identity, domain, None).await;
        assert_eq!(
            put.status, 400,
            "case ({identity:?}, {domain:?}): expected 400, got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            ses.stored_mail_from(identity),
            before,
            "case ({identity:?}, {domain:?}): expected the held configuration to be untouched, got {:?} where it was {before:?}",
            ses.stored_mail_from(identity)
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_domain_disables_the_configuration() {
    // The domain is the whole of what the call enables, so a body without it — with
    // or without a behaviour beside it — disables the configuration rather than
    // leaving it as it was.
    for body in [json!({}), json!({ "BehaviorOnMxFailure": "REJECT_MESSAGE" })] {
        for (identity, domain) in WITHIN_THE_IDENTITY {
            let ses = Ses::new();
            ses.create(identity).await;
            ses.put_mail_from(identity, domain, Some("REJECT_MESSAGE"))
                .await;
            assert!(
                ses.stored_mail_from(identity).is_some(),
                "case ({identity:?}, {domain:?}, {body}): expected the configuration to be in place first, got {:?}",
                ses.stored_mail_from(identity)
            );

            let put = ses.put_mail_from_body(identity, body.clone()).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {domain:?}, {body}): expected 200, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                ses.stored_mail_from(identity),
                None,
                "case ({identity:?}, {domain:?}, {body}): expected the configuration to be disabled, got {:?}",
                ses.stored_mail_from(identity)
            );
            let read = ses.get(identity).await;
            assert_eq!(
                read.mail_from(),
                None,
                "case ({identity:?}, {domain:?}, {body}): expected no MailFromAttributes to be reported, got {:?}",
                read.mail_from()
            );

            // Disabling again is the same request answered the same way.
            let again = ses.put_mail_from_body(identity, body.clone()).await;
            assert_eq!(
                again.status, 200,
                "case ({identity:?}, {domain:?}, {body}): expected a second disable to be accepted, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                ses.stored_mail_from(identity),
                None,
                "case ({identity:?}, {domain:?}, {body}): expected the identity to stay unconfigured, got {:?}",
                ses.stored_mail_from(identity)
            );
        }
    }
}

#[tokio::test]
async fn rewriting_the_same_domain_starts_no_new_mx_search() {
    // The MX search belongs to the domain configured, so re-writing the domain
    // already in place leaves its outcome where it stands and only the behaviour
    // beside it changes; naming a different domain is a new search, which starts in
    // PENDING again.
    let cases: &[(&str, &str, &str)] = &[
        ("example.com", "mail.example.com", "bounce.example.com"),
        (
            "sender@example.com",
            "mail.example.com",
            "deep.mail.example.com",
        ),
        (
            "SUB.Example.ORG",
            "mail.sub.example.org",
            "bounce.sub.example.org",
        ),
    ];

    for (identity, first, second) in cases {
        let ses = Ses::new();
        ses.create(identity).await;
        ses.put_mail_from(identity, first, Some("REJECT_MESSAGE"))
            .await;
        ses.make_mx_lookup_due(identity);
        let settled = ses.get(identity).await;
        assert_eq!(
            settled.mail_from_member("MailFromDomainStatus"),
            "SUCCESS",
            "case ({identity:?}, {first:?}): expected the first search to have settled, got {:?}",
            settled.mail_from()
        );
        let due_before = ses.mx_lookup_due_at(identity);

        // The same domain, in a different case and with the other behaviour.
        let rewritten = ses
            .put_mail_from(identity, &first.to_uppercase(), Some("USE_DEFAULT_VALUE"))
            .await;
        assert_eq!(
            rewritten.status, 200,
            "case ({identity:?}, {first:?}): expected the rewrite to be accepted, got {} {:?}",
            rewritten.status, rewritten.body
        );
        assert_eq!(
            ses.stored_mail_from(identity),
            Some((
                first.to_uppercase(),
                "SUCCESS".to_string(),
                "USE_DEFAULT_VALUE".to_string()
            )),
            "case ({identity:?}, {first:?}): expected only the spelling and the behaviour to change, got {:?}",
            ses.stored_mail_from(identity)
        );
        assert_eq!(
            ses.mx_lookup_due_at(identity),
            due_before,
            "case ({identity:?}, {first:?}): expected no new MX search to be started"
        );

        // A different domain: a new search, back in PENDING.
        let moved = ses.put_mail_from(identity, second, None).await;
        assert_eq!(
            moved.status, 200,
            "case ({identity:?}, {second:?}): expected the new domain to be accepted, got {} {:?}",
            moved.status, moved.body
        );
        ses.hold_transitions_pending(identity);
        let read = ses.get(identity).await;
        assert_eq!(
            read.mail_from_member("MailFromDomain"),
            *second,
            "case ({identity:?}, {second:?}): expected the new domain to be reported, got {:?}",
            read.mail_from()
        );
        assert_eq!(
            read.mail_from_member("MailFromDomainStatus"),
            "PENDING",
            "case ({identity:?}, {second:?}): expected a new MX search in PENDING, got {:?}",
            read.mail_from()
        );
        assert_eq!(
            read.mail_from_member("BehaviorOnMxFailure"),
            DEFAULT_BEHAVIOR,
            "case ({identity:?}, {second:?}): expected the default behaviour, got {:?}",
            read.mail_from()
        );
    }
}

#[tokio::test]
async fn the_published_domain_name_bounds_are_enforced() {
    // Each bound DNS publishes for a name under the identity's domain, at its value
    // and one character past it.
    let parent = "example.com";
    let longest_label = format!("{}.{parent}", "a".repeat(MAX_DOMAIN_LABEL_CHARS));
    let over_long_label = format!("{}.{parent}", "a".repeat(MAX_DOMAIN_LABEL_CHARS + 1));
    let longest_name = name_of_length(MAX_DOMAIN_NAME_CHARS, parent);
    let over_long_name = name_of_length(MAX_DOMAIN_NAME_CHARS + 1, parent);
    assert_eq!(
        longest_name.len(),
        MAX_DOMAIN_NAME_CHARS,
        "case the longest name: expected a name of {MAX_DOMAIN_NAME_CHARS} characters, got {}",
        longest_name.len()
    );
    assert_eq!(
        over_long_name.len(),
        MAX_DOMAIN_NAME_CHARS + 1,
        "case one past the longest name: expected {} characters, got {}",
        MAX_DOMAIN_NAME_CHARS + 1,
        over_long_name.len()
    );

    for (domain, accepted) in [
        (&longest_label, true),
        (&over_long_label, false),
        (&longest_name, true),
        (&over_long_name, false),
    ] {
        let ses = Ses::new();
        ses.create(parent).await;
        let put = ses.put_mail_from(parent, domain, None).await;
        if accepted {
            assert_eq!(
                put.status, 200,
                "case ({} characters): expected 200, got {} {:?}",
                domain.len(),
                put.status,
                put.body
            );
            assert_eq!(
                ses.get(parent).await.mail_from_member("MailFromDomain"),
                *domain,
                "case ({} characters): expected the domain to be stored and reported, got {:?}",
                domain.len(),
                ses.stored_mail_from(parent)
            );
        } else {
            assert_eq!(
                put.status, 400,
                "case ({} characters): expected 400, got {} {:?}",
                domain.len(),
                put.status,
                put.body
            );
            assert_eq!(
                put.error_code(),
                "BadRequestException",
                "case ({} characters): expected BadRequestException, got {:?}",
                domain.len(),
                put.body
            );
            assert_eq!(
                ses.stored_mail_from(parent),
                None,
                "case ({} characters): expected a refused request to store nothing, got {:?}",
                domain.len(),
                ses.stored_mail_from(parent)
            );
        }
    }
}

#[tokio::test]
async fn an_unmodelled_behaviour_is_refused() {
    // `BehaviorOnMxFailure` arrives as a plain string, so a value outside the enum
    // is invalid input — and it is refused before anything is stored.
    for value in ["", "reject_message", "RejectMessage", "DISCARD_MESSAGE"] {
        let ses = Ses::new();
        ses.create("example.com").await;
        let put = ses
            .put_mail_from_body(
                "example.com",
                json!({ "MailFromDomain": "mail.example.com", "BehaviorOnMxFailure": value }),
            )
            .await;
        assert_eq!(
            put.status, 400,
            "case {value:?}: expected 400, got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.error_code(),
            "BadRequestException",
            "case {value:?}: expected BadRequestException, got {:?}",
            put.body
        );
        assert_eq!(
            ses.stored_mail_from("example.com"),
            None,
            "case {value:?}: expected a refused request to store nothing, got {:?}",
            ses.stored_mail_from("example.com")
        );
    }
}

#[tokio::test]
async fn an_identity_the_account_does_not_hold_is_refused() {
    let ses = Ses::new();
    ses.create("sender@example.com").await;
    ses.create("example.com").await;
    ses.create("deleted@example.com").await;
    let deleted = ses.delete("deleted@example.com").await;
    assert_eq!(
        deleted.status, 200,
        "expected the identity to be deleted, got {} {:?}",
        deleted.status, deleted.body
    );

    // Each way the lookup misses: never created, created and then deleted, a local
    // part differing only in case (the local part is compared as supplied), and a
    // domain that merely contains a stored one.
    let missing: &[&str] = &[
        "absent@example.com",
        "absent.example.com",
        "deleted@example.com",
        "SENDER@example.com",
        "sub.example.com",
    ];

    for identity in missing {
        let put = ses
            .put_mail_from(identity, "mail.example.com", Some("REJECT_MESSAGE"))
            .await;
        assert_eq!(
            put.status, 404,
            "case {identity:?}: expected 404, got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.error_code(),
            "NotFoundException",
            "case {identity:?}: expected NotFoundException, got {:?}",
            put.body
        );
        assert_eq!(
            put.message(),
            format!("Email identity {identity} does not exist."),
            "case {identity:?}: expected the message to name the identity, got {:?}",
            put.message()
        );
    }

    // A refused write persists nothing: the identities the account does hold are
    // still sending with SES's own MAIL FROM domain.
    for held in ["sender@example.com", "example.com"] {
        assert_eq!(
            ses.stored_mail_from(held),
            None,
            "case {held:?}: expected refused writes to configure nothing, got {:?}",
            ses.stored_mail_from(held)
        );
    }
}
