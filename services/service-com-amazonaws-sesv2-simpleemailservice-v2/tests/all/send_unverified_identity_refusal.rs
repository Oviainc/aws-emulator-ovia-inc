// SPDX-License-Identifier: CC0-1.0

//! How `SendEmail` reports a sending identity this account has not verified, end to
//! end over HTTP.
//!
//! An SES identity belongs to ONE Region — `CreateEmailIdentity` "starts the process
//! of verifying an email identity … in the current Amazon Web Services Region" — so
//! the refusal of a send whose `FromEmailAddress` failed the verification check names
//! the Region whose identity list it failed in, and the identities that failed it.
//!
//! The rows below span what that rule generalises over: the Region the call is signed
//! for, and the class of the sender — an address of no identity the account holds, an
//! address whose domain identity is still unverified, and two different senders in one
//! Region, which therefore produce two different messages.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
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
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// settle the verification transition the clock owes an identity.
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

    /// One call, signed for `region`: the credential scope is what the handler reads
    /// the Region of the call from.
    async fn call(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{region}.amazonaws.com{path}"))
            .header("host", format!("email.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
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

    async fn create_identity(&self, region: &str, identity: &str) -> Reply {
        self.call(
            region,
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn send(&self, region: &str, from: &str) -> Reply {
        self.call(
            region,
            "POST",
            "/v2/email/outbound-emails",
            Some(json!({
                "FromEmailAddress": from,
                "Destination": { "ToAddresses": ["recipient@example.net"] },
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}}
            })),
        )
        .await
    }

    /// Bring every identity's verification transition into the past, so a case that
    /// sends from an identity the account holds does not race the span the clock owes
    /// it.
    fn settle_identities(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
    }
}

/// The sentence the service answers for a sender that failed the verification check:
/// the Region the call executes in, upper-cased, then the identities that failed.
fn refusal(region: &str, identities: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region {}: \
         {identities}",
        region.to_uppercase()
    )
}

#[tokio::test]
async fn the_refusal_names_the_region_of_the_call_and_the_identity_that_failed() {
    struct Row<'a> {
        case: &'a str,
        /// The Region the call is signed for.
        region: &'a str,
        /// The Region the refusal must name, which is the Region the call is served
        /// in.
        served_in: &'a str,
        /// An identity to create first, for a row about a sender whose identity
        /// exists but is not verified yet.
        created: Option<&'a str>,
        sender: &'a str,
    }
    let rows = [
        Row {
            case: "an address of no identity the account holds",
            region: "us-west-2",
            served_in: "us-west-2",
            created: None,
            sender: "stranger@example.com",
        },
        Row {
            case: "the same address, from another Region",
            region: "eu-central-1",
            served_in: "eu-central-1",
            created: None,
            sender: "stranger@example.com",
        },
        Row {
            case: "a second sender in the same Region",
            region: "us-west-2",
            served_in: "us-west-2",
            created: None,
            sender: "someone-else@example.org",
        },
        Row {
            case: "an address identity whose verification is not through yet",
            region: "ap-southeast-2",
            served_in: "ap-southeast-2",
            created: Some("pending@example.com"),
            sender: "pending@example.com",
        },
        Row {
            case: "an address inside a domain identity that is not verified yet",
            region: "us-east-2",
            served_in: "us-east-2",
            created: Some("example.net"),
            sender: "sender@example.net",
        },
        Row {
            case: "a Region label with no digits",
            region: "sa-east-1",
            served_in: "sa-east-1",
            created: None,
            sender: "stranger@example.com",
        },
    ];

    for row in rows {
        let ses = Ses::new();
        if let Some(identity) = row.created {
            let created = ses.create_identity(row.region, identity).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the identity must be created first, got {} {:?}",
                row.case, created.status, created.body
            );
        }

        let refused = ses.send(row.region, row.sender).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "MessageRejected"),
            "case {:?}: expected a 400 MessageRejected, got {} {:?}",
            row.case,
            refused.status,
            refused.body
        );
        let expected = refusal(row.served_in, row.sender);
        assert_eq!(
            refused.message(),
            expected,
            "case {:?}: expected the message {expected:?}, got {:?}",
            row.case,
            refused.message()
        );
    }
}

#[tokio::test]
async fn a_verified_sender_is_accepted_in_whatever_region_the_call_names() {
    // The refusal above is about the verification check, not about the Region: an
    // identity this account holds, once its verification is through, sends from every
    // Region the call may name.
    for region in ["us-east-1", "us-west-2", "eu-west-3"] {
        let ses = Ses::new();
        let created = ses.create_identity(region, "verified.example").await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the identity must be created first, got {} {:?}",
            created.status, created.body
        );
        ses.settle_identities();

        let sent = ses.send(region, "sender@verified.example").await;
        assert_eq!(
            sent.status, 200,
            "case {region:?}: a verified sender is accepted, got {} {:?}",
            sent.status, sent.body
        );
        assert!(
            sent.body.get("MessageId").and_then(Value::as_str).is_some(),
            "case {region:?}: expected a MessageId, got {:?}",
            sent.body
        );
    }
}
