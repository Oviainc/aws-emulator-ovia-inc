// SPDX-License-Identifier: CC0-1.0

//! `GetEmailAddressInsights`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all exercised.
//!
//! The rule these cases pin: the call reports one `EmailAddressInsightsConfidenceVerdict`
//! per documented mailbox evaluation of the address in the request — its syntax, the
//! DNS records of its domain, whether the mailbox exists, whether it is a role
//! address, whether its domain is a throwaway mailbox service and whether the input
//! reads as random text — each computed from the value itself and from what the
//! account's own records already say about that domain and that mailbox, with
//! `IsValid` the weakest of them once the three evaluations whose HIGH value counts
//! against an address are inverted; and the report is a read, so it moves nothing a
//! later caller sees.
//!
//! The table below spans what the rule generalises over: the ways a value falls
//! outside the address form, the length bound and a value past it, a domain no DNS
//! zone can cover, role mailboxes spelled three ways, a throwaway service addressed
//! directly and under a subdomain, character mixes that read as generated and as
//! typed, and the case the domain is spelled in. The second suite varies the account's
//! records instead, which is the other half of what each verdict is computed from.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `EmailAddressInsightsConfidenceVerdict` accepts on the wire
/// (`smithy_query(query="shape",shape="EmailAddressInsightsConfidenceVerdict")`
/// lists `LOW`, `MEDIUM` and `HIGH`), so a case can show every verdict reported is
/// one of them.
const CONFIDENCE_VERDICT_VALUES: [&str; 3] = ["LOW", "MEDIUM", "HIGH"];

/// The longest an address member of this model may be: `InsightsEmailAddress` is a
/// `length 1..320` string.
const MAX_EMAIL_ADDRESS_CHARS: usize = 320;

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

/// The seven verdicts one report carries, each as the `ConfidenceVerdict` string the
/// response spells, with an absent member reported as an empty string.
#[derive(Debug, PartialEq, Eq)]
struct Verdicts {
    is_valid: String,
    has_valid_syntax: String,
    has_valid_dns_records: String,
    mailbox_exists: String,
    is_role_address: String,
    is_disposable: String,
    is_random_input: String,
}

impl Verdicts {
    /// The verdicts as the reply reports them.
    fn of(reply: &Reply) -> Self {
        let validation = reply
            .body
            .get("MailboxValidation")
            .cloned()
            .unwrap_or(Value::Null);
        let evaluations = validation
            .get("Evaluations")
            .cloned()
            .unwrap_or(Value::Null);
        let confidence = |holder: &Value, member: &str| {
            holder
                .get(member)
                .and_then(|verdict| verdict.get("ConfidenceVerdict"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            is_valid: confidence(&validation, "IsValid"),
            has_valid_syntax: confidence(&evaluations, "HasValidSyntax"),
            has_valid_dns_records: confidence(&evaluations, "HasValidDnsRecords"),
            mailbox_exists: confidence(&evaluations, "MailboxExists"),
            is_role_address: confidence(&evaluations, "IsRoleAddress"),
            is_disposable: confidence(&evaluations, "IsDisposable"),
            is_random_input: confidence(&evaluations, "IsRandomInput"),
        }
    }

    /// The expected verdicts of one case, written in the order the model declares the
    /// evaluations in.
    fn expected(
        is_valid: &str,
        has_valid_syntax: &str,
        has_valid_dns_records: &str,
        mailbox_exists: &str,
        is_role_address: &str,
        is_disposable: &str,
        is_random_input: &str,
    ) -> Self {
        Self {
            is_valid: is_valid.to_string(),
            has_valid_syntax: has_valid_syntax.to_string(),
            has_valid_dns_records: has_valid_dns_records.to_string(),
            mailbox_exists: mailbox_exists.to_string(),
            is_role_address: is_role_address.to_string(),
            is_disposable: is_disposable.to_string(),
            is_random_input: is_random_input.to_string(),
        }
    }

    /// Every verdict this report carries, for a case that checks the catalog they
    /// come from.
    fn all(&self) -> [&str; 7] {
        [
            self.is_valid.as_str(),
            self.has_valid_syntax.as_str(),
            self.has_valid_dns_records.as_str(),
            self.mailbox_exists.as_str(),
            self.is_role_address.as_str(),
            self.is_disposable.as_str(),
            self.is_random_input.as_str(),
        ]
    }
}

/// The emulated service behind its router, plus the shared engine so a case can set
/// up the records a verdict is computed from.
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
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// The insights report for `address`.
    async fn insights(&self, address: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/email-address-insights",
            Some(json!({ "EmailAddress": address })),
        )
        .await
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn suppress(&self, address: &str, reason: &str) -> Reply {
        self.call(
            "PUT",
            "/v2/email/suppression/addresses",
            Some(json!({ "EmailAddress": address, "Reason": reason })),
        )
        .await
    }

    /// The identities the account holds, as the listing reports them.
    async fn identities(&self) -> Value {
        self.call("GET", "/v2/email/identities", None).await.body
    }

    /// The account report, so a case can show a read moved nothing about the account
    /// either.
    async fn account(&self) -> Value {
        self.call("GET", "/v2/email/account", None).await.body
    }

    /// A suppression-list write is refused while the account is in the sandbox, which
    /// is not what these cases are about.
    fn grant_production_access(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().account.production_access_enabled = true;
    }

    /// Bring every identity's verification transition into the past, so a case does
    /// not race the span the clock owes it.
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

    /// Create `identity` and bring its verification into the past, so the report
    /// reads a verified identity.
    async fn verified_identity(&self, identity: &str) {
        let created = self.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "expected the identity create for {identity:?} to succeed, got {} {:?}",
            created.status, created.body
        );
        self.settle_identities();
    }
}

/// One graded value and the verdicts the rule gives it, in an account holding no
/// records at all: no identity says what any domain publishes and no suppression
/// entry says anything about any mailbox, so the DNS and mailbox evaluations rest on
/// the value alone.
struct Row {
    name: &'static str,
    address: String,
    expected: Verdicts,
}

/// A local part of `length` repetitions of one letter, for the rows that sit on the
/// length bound.
fn repeated_local_part(length: usize) -> String {
    "a".repeat(length)
}

fn rows() -> Vec<Row> {
    let domain = "@example.com";
    vec![
        // A typed personal address: nothing is wrong with it, and nothing here can
        // confirm its domain's records or its mailbox.
        Row {
            name: "typed-personal-address",
            address: format!("john.doe{domain}"),
            expected: Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW"),
        },
        // Four ways a value falls outside the address form: no `@` at all, a second
        // `@`, an empty atom in the local part, and a single character.
        Row {
            name: "no-at-sign",
            address: "not-an-email".to_string(),
            expected: Verdicts::expected("LOW", "LOW", "LOW", "LOW", "LOW", "LOW", "LOW"),
        },
        Row {
            name: "two-at-signs",
            address: format!("user@{domain}"),
            expected: Verdicts::expected("LOW", "LOW", "LOW", "LOW", "LOW", "LOW", "LOW"),
        },
        Row {
            name: "doubled-dot-in-the-local-part",
            address: format!("john..doe{domain}"),
            expected: Verdicts::expected("LOW", "LOW", "LOW", "LOW", "LOW", "LOW", "LOW"),
        },
        Row {
            name: "a-single-character",
            address: "a".to_string(),
            expected: Verdicts::expected("LOW", "LOW", "LOW", "LOW", "LOW", "LOW", "LOW"),
        },
        // A domain under a top-level name the DNS root does not delegate: the records
        // can never be published, so no mailbox under it can be reached either.
        Row {
            name: "undelegated-top-level-domain",
            address: "user@nothing.invalid".to_string(),
            expected: Verdicts::expected("LOW", "HIGH", "LOW", "LOW", "LOW", "LOW", "LOW"),
        },
        // The length bound of an address member, and one character past it.
        Row {
            name: "at-the-length-bound",
            address: format!(
                "{}{domain}",
                repeated_local_part(MAX_EMAIL_ADDRESS_CHARS - domain.len())
            ),
            expected: Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW"),
        },
        Row {
            name: "one-past-the-length-bound",
            address: format!(
                "{}{domain}",
                repeated_local_part(MAX_EMAIL_ADDRESS_CHARS - domain.len() + 1)
            ),
            expected: Verdicts::expected(
                "MEDIUM", "MEDIUM", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW",
            ),
        },
        // A domain label outside DNS's preferred name syntax: an address a receiving
        // system may still turn down, which is weaker than well-spelled rather than
        // no address at all.
        Row {
            name: "domain-label-opening-with-a-hyphen",
            address: "user@-example.com".to_string(),
            expected: Verdicts::expected(
                "MEDIUM", "MEDIUM", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW",
            ),
        },
        // Role mailboxes: one named by RFC 2142 that the member's documentation does
        // not cite, the same role carrying an RFC 5233 subaddress, and a role name as
        // one segment of a longer local part.
        Row {
            name: "role-mailbox",
            address: format!("postmaster{domain}"),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "HIGH", "LOW", "LOW"),
        },
        Row {
            name: "role-mailbox-with-a-subaddress",
            address: format!("support+eu{domain}"),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "HIGH", "LOW", "LOW"),
        },
        Row {
            name: "role-name-as-one-segment",
            address: format!("sales.emea{domain}"),
            expected: Verdicts::expected(
                "MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "MEDIUM", "LOW", "LOW",
            ),
        },
        // A throwaway mailbox service addressed directly, and under one of its
        // subdomains.
        Row {
            name: "throwaway-mailbox-service",
            address: "someone@mailinator.com".to_string(),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "LOW", "HIGH", "LOW"),
        },
        Row {
            name: "throwaway-mailbox-service-subdomain",
            address: "x@mail.mailinator.com".to_string(),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "LOW", "HIGH", "LOW"),
        },
        // Character mixes: letters with no vowels and no repeat, letters and digits
        // interleaved, and digits appended to a name, which is not a generated value.
        Row {
            name: "vowel-less-local-part",
            address: format!("xkqjvzwp{domain}"),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "HIGH"),
        },
        Row {
            name: "letters-and-digits-interleaved",
            address: format!("a7f3k9q2zp{domain}"),
            expected: Verdicts::expected("LOW", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "HIGH"),
        },
        Row {
            name: "digits-appended-to-a-name",
            address: format!("jdoe2024{domain}"),
            expected: Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW"),
        },
        // DNS compares a domain without regard to case, and a local part's case is
        // not what makes it a role or a mailbox at a throwaway service.
        Row {
            name: "mixed-case-address",
            address: "John.Doe@EXAMPLE.COM".to_string(),
            expected: Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW"),
        },
    ]
}

#[tokio::test]
async fn each_evaluation_is_computed_from_the_address_itself() {
    let ses = Ses::new();

    for row in rows() {
        let reported = ses.insights(&row.address).await;
        assert_eq!(
            reported.status, 200,
            "case {:?}: expected 200 from the insights report, got {} {:?}",
            row.name, reported.status, reported.body
        );
        let verdicts = Verdicts::of(&reported);
        assert_eq!(
            verdicts, row.expected,
            "case {:?} ({:?}): expected {:?}, got {verdicts:?}",
            row.name,
            row.address.chars().take(32).collect::<String>(),
            row.expected
        );
        for verdict in verdicts.all() {
            assert!(
                CONFIDENCE_VERDICT_VALUES.contains(&verdict),
                "case {:?}: expected every verdict to be one of {CONFIDENCE_VERDICT_VALUES:?}, got {verdict:?}",
                row.name
            );
        }
    }
}

#[tokio::test]
async fn the_dns_and_mailbox_evaluations_are_computed_from_the_accounts_records() {
    let ses = Ses::new();
    ses.grant_production_access();

    // A verified domain identity: the records SES was told to look for in that zone
    // were found, which is what the DNS evaluation rests on.
    ses.verified_identity("insights.example.com").await;
    // A verified mailbox inside it, and one in a domain the account holds no identity
    // for: a mailbox whose owner followed the verification link exists whether or not
    // anything is known about its domain's records.
    ses.verified_identity("person@insights.example.com").await;
    ses.verified_identity("solo@other.example.org").await;
    // Mail to one address came back, and the owner of another reported the message
    // they received.
    for (address, reason) in [
        ("returned@insights.example.com", "BOUNCE"),
        ("reporter@insights.example.com", "COMPLAINT"),
    ] {
        let suppressed = ses.suppress(address, reason).await;
        assert_eq!(
            suppressed.status, 200,
            "expected the {reason} suppression of {address:?} to succeed, got {} {:?}",
            suppressed.status, suppressed.body
        );
    }

    // Each row differs from the table above only in what the account's records say
    // about it: the same well-spelled, personal, non-throwaway address form.
    let rows: Vec<(&str, &str, Verdicts)> = vec![
        (
            "address-in-a-verified-domain",
            "someone@insights.example.com",
            Verdicts::expected("MEDIUM", "HIGH", "HIGH", "MEDIUM", "LOW", "LOW", "LOW"),
        ),
        (
            "verified-mailbox-in-a-verified-domain",
            "person@insights.example.com",
            Verdicts::expected("HIGH", "HIGH", "HIGH", "HIGH", "LOW", "LOW", "LOW"),
        ),
        (
            "verified-mailbox-in-an-unknown-domain",
            "solo@other.example.org",
            Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "HIGH", "LOW", "LOW", "LOW"),
        ),
        (
            "mailbox-suppressed-for-a-bounce",
            "returned@insights.example.com",
            Verdicts::expected("LOW", "HIGH", "HIGH", "LOW", "LOW", "LOW", "LOW"),
        ),
        (
            "mailbox-suppressed-for-a-complaint",
            "reporter@insights.example.com",
            Verdicts::expected("HIGH", "HIGH", "HIGH", "HIGH", "LOW", "LOW", "LOW"),
        ),
        (
            "address-in-a-sibling-domain-the-account-knows-nothing-about",
            "someone@sibling.example.net",
            Verdicts::expected("MEDIUM", "HIGH", "MEDIUM", "MEDIUM", "LOW", "LOW", "LOW"),
        ),
    ];

    for (name, address, expected) in &rows {
        let reported = ses.insights(address).await;
        assert_eq!(
            reported.status, 200,
            "case {name:?}: expected 200 from the insights report, got {} {:?}",
            reported.status, reported.body
        );
        let verdicts = Verdicts::of(&reported);
        assert_eq!(
            verdicts, *expected,
            "case {name:?} ({address:?}): expected {expected:?}, got {verdicts:?}"
        );
    }
}

#[tokio::test]
async fn the_report_moves_nothing_a_later_caller_sees() {
    let ses = Ses::new();
    ses.grant_production_access();
    ses.verified_identity("steady.example.com").await;
    let suppressed = ses
        .suppress("returned@steady.example.com", "BOUNCE")
        .await;
    assert_eq!(
        suppressed.status, 200,
        "expected the suppression to succeed, got {} {:?}",
        suppressed.status, suppressed.body
    );

    let identities_before = ses.identities().await;
    let account_before = ses.account().await;

    for address in [
        "someone@steady.example.com",
        "returned@steady.example.com",
        "not-an-email",
        "postmaster@mailinator.com",
    ] {
        let first = ses.insights(address).await;
        assert_eq!(
            first.status, 200,
            "case {address:?}: expected 200 from the insights report, got {} {:?}",
            first.status, first.body
        );
        for read in ["second", "third"] {
            let again = ses.insights(address).await;
            assert_eq!(
                again.body, first.body,
                "case {address:?} ({read} read): expected the same report as the first, got {:?}",
                again.body
            );
        }
    }

    assert_eq!(
        ses.identities().await,
        identities_before,
        "expected the reads to leave the identity listing alone, got {:?}",
        ses.identities().await
    );
    assert_eq!(
        ses.account().await,
        account_before,
        "expected the reads to leave the account report alone, got {:?}",
        ses.account().await
    );
    // No report added, moved or removed a suppression-list entry either: the address
    // it was asked about is not a resource the call creates.
    let state = ses
        .engine
        .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
    let addresses: Vec<String> = state
        .lock()
        .suppressed_destinations
        .values()
        .map(|entry| entry.email_address.clone())
        .collect();
    assert_eq!(
        addresses,
        vec!["returned@steady.example.com".to_string()],
        "expected the suppression list to hold only the address the test suppressed, got {addresses:?}"
    );
}
