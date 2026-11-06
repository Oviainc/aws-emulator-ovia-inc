// SPDX-License-Identifier: CC0-1.0

//! The account report, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all
//! exercised. The rule under test is that `GetAccount` reports the account
//! attributes this service persists and the sending limits it keeps, computed from
//! the store rather than fixed in the handler: each row persists a different
//! account state and the report is asserted against that state, member by member.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    AccountDetails, DashboardAttributes, GuardianAttributes, ReviewDetails,
    SuppressionConditionThreshold, SuppressionConfidenceThreshold, SuppressionValidationAttributes,
    FEATURE_STATUS_VALUES, MAIL_TYPE_VALUES, PRICING_PLAN_VALUES, REVIEW_STATUS_VALUES,
    SUPPRESSION_LIST_REASON_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    AccountState, ComAmazonawsSesv2SimpleemailserviceV2State, SendQuotaState, SentMessage,
    ENFORCEMENT_STATUS_HEALTHY, FEATURE_DISABLED, FEATURE_ENABLED, PRICING_PLAN_NONE,
    SEND_QUOTA_WINDOW,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `GeneralEnforcementStatus` values `GetAccount` documents for
/// `EnforcementStatus`: "HEALTHY", "PROBATION" and "SHUTDOWN" (the model types the
/// member as a plain string, so the catalog is the member's documentation —
/// `smithy_query(query="doc",shape="GetAccountResponse",member="EnforcementStatus")`).
const ENFORCEMENT_STATUS_VALUES: &[&str] = &["HEALTHY", "PROBATION", "SHUTDOWN"];

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A boolean member of the body, absent meaning the modelled default `false`.
    fn flag(&self, member: &str) -> bool {
        self.body
            .get(member)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member reached by its path through the nested structures, or `Null`.
    fn at(&self, path: &[&str]) -> Value {
        let mut here = &self.body;
        for step in path {
            match here.get(step) {
                Some(next) => here = next,
                None => return Value::Null,
            }
        }
        here.clone()
    }

    /// A string at `path`, or `None` when the member is absent.
    fn text_at(&self, path: &[&str]) -> Option<String> {
        self.at(path).as_str().map(str::to_string)
    }

    /// The numbers a `SendQuota` member carries, or `None` when absent.
    fn number_at(&self, path: &[&str]) -> Option<f64> {
        self.at(path).as_f64()
    }

    /// The strings of a list member at `path`, or `None` when the list is absent.
    fn strings_at(&self, path: &[&str]) -> Option<Vec<String>> {
        self.at(path).as_array().map(|rows| {
            rows.iter()
                .map(|row| row.as_str().unwrap_or_default().to_string())
                .collect()
        })
    }
}

/// The emulated service behind its router, plus the shared engine so a test can
/// persist the account state a case is about and read what a handler kept.
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
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// The account report.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    /// Persist the account state and the sending limits a case is about.
    fn persist(&self, account: &AccountState, quota: &SendQuotaState) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.account = account.clone();
        store.send_counters = *quota;
    }

    /// Bring every identity's verification transition into the past, then read the
    /// account back so the transition the clock owes is materialised.
    async fn settle_identities(&self) {
        {
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
        let reply = self.account().await;
        assert_eq!(
            reply.status, 200,
            "case settle: expected 200 from the account report, got {} {:?}",
            reply.status, reply.body
        );
    }

    /// Append a message to the outbound log as if it had been accepted at
    /// `sent_at`, so a case can place recipients outside the quota's window.
    fn log_message(&self, message_id: &str, recipients: usize, sent_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.sent_messages.push(SentMessage {
            message_id: message_id.to_string(),
            from: Some("sender@quota.example.com".to_string()),
            to: (0..recipients)
                .map(|index| format!("rcpt-{index}@example.org"))
                .collect(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: None,
            body: None,
            body_html: None,
            configuration_set_name: None,
            reply_to: Vec::new(),
            feedback_forwarding: None,
            email_tags: Vec::new(),
            headers: Vec::new(),
            sent_at,
        });
    }
}

/// One row of the account table: the state to persist, and the name the
/// assertions report it under.
struct AccountCase {
    name: &'static str,
    account: AccountState,
    quota: SendQuotaState,
}

/// Assert the report mirrors the persisted account state, member by member.
fn assert_mirrors(case: &str, account: &AccountState, quota: &SendQuotaState, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the account report, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.flag("SendingEnabled"),
        account.sending_enabled,
        "case {case:?}: expected SendingEnabled {}, got {:?}",
        account.sending_enabled,
        reply.body
    );
    assert_eq!(
        reply.flag("DedicatedIpAutoWarmupEnabled"),
        account.dedicated_ip_auto_warmup_enabled,
        "case {case:?}: expected DedicatedIpAutoWarmupEnabled {}, got {:?}",
        account.dedicated_ip_auto_warmup_enabled,
        reply.body
    );
    assert_eq!(
        reply.flag("ProductionAccessEnabled"),
        account.production_access_enabled,
        "case {case:?}: expected ProductionAccessEnabled {}, got {:?}",
        account.production_access_enabled,
        reply.body
    );
    assert_eq!(
        reply.text("EnforcementStatus"),
        account.enforcement_status,
        "case {case:?}: expected EnforcementStatus {:?}, got {:?}",
        account.enforcement_status,
        reply.body
    );

    // The suppression attributes: the reason set as persisted, and the validation
    // attributes only once something has set them.
    assert_eq!(
        reply.strings_at(&["SuppressionAttributes", "SuppressedReasons"]),
        Some(account.suppressed_reasons.clone()),
        "case {case:?}: expected SuppressedReasons {:?}, got {:?}",
        account.suppressed_reasons,
        reply.body
    );
    let validation = account.suppression_validation.as_ref().map(|validation| {
        (
            validation.condition_threshold.condition_threshold_enabled.clone(),
            validation
                .condition_threshold
                .overall_confidence_threshold
                .as_ref()
                .map(|threshold| threshold.confidence_verdict_threshold.clone()),
        )
    });
    let reported_validation = reply
        .text_at(&[
            "SuppressionAttributes",
            "ValidationAttributes",
            "ConditionThreshold",
            "ConditionThresholdEnabled",
        ])
        .map(|enabled| {
            (
                enabled,
                reply.text_at(&[
                    "SuppressionAttributes",
                    "ValidationAttributes",
                    "ConditionThreshold",
                    "OverallConfidenceThreshold",
                    "ConfidenceVerdictThreshold",
                ]),
            )
        });
    assert_eq!(
        reported_validation, validation,
        "case {case:?}: expected the validation attributes {validation:?}, got {reported_validation:?}"
    );

    // The VDM attributes: the account's status, and each nested feature only once
    // something has set it.
    assert_eq!(
        reply.text_at(&["VdmAttributes", "VdmEnabled"]),
        Some(account.vdm_enabled.clone()),
        "case {case:?}: expected VdmEnabled {:?}, got {:?}",
        account.vdm_enabled,
        reply.body
    );
    let engagement_metrics = account
        .vdm_dashboard_attributes
        .as_ref()
        .and_then(|dashboard| dashboard.engagement_metrics.clone());
    assert_eq!(
        reply.text_at(&["VdmAttributes", "DashboardAttributes", "EngagementMetrics"]),
        engagement_metrics,
        "case {case:?}: expected EngagementMetrics {engagement_metrics:?}, got {:?}",
        reply.body
    );
    let optimized_shared_delivery = account
        .vdm_guardian_attributes
        .as_ref()
        .and_then(|guardian| guardian.optimized_shared_delivery.clone());
    assert_eq!(
        reply.text_at(&[
            "VdmAttributes",
            "GuardianAttributes",
            "OptimizedSharedDelivery"
        ]),
        optimized_shared_delivery,
        "case {case:?}: expected OptimizedSharedDelivery {optimized_shared_delivery:?}, got {:?}",
        reply.body
    );

    // The pricing attributes: the active plan, and the next one only while a
    // change is scheduled.
    assert_eq!(
        reply.text_at(&["PricingAttributes", "CurrentPlan"]),
        Some(account.pricing_plan.clone()),
        "case {case:?}: expected CurrentPlan {:?}, got {:?}",
        account.pricing_plan,
        reply.body
    );
    assert_eq!(
        reply.text_at(&["PricingAttributes", "NextPlan"]),
        account.next_pricing_plan.clone(),
        "case {case:?}: expected NextPlan {:?}, got {:?}",
        account.next_pricing_plan,
        reply.body
    );

    // The account details, reported only once the account has supplied them.
    assert_eq!(
        reply.text_at(&["Details", "MailType"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.mail_type.clone()),
        "case {case:?}: expected the details' MailType to be reported as persisted, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text_at(&["Details", "WebsiteURL"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.website_url.clone()),
        "case {case:?}: expected the details' WebsiteURL to be reported as persisted, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text_at(&["Details", "ContactLanguage"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.contact_language.clone()),
        "case {case:?}: expected the details' ContactLanguage to be reported as persisted, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.strings_at(&["Details", "AdditionalContactEmailAddresses"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.additional_contact_email_addresses.clone()),
        "case {case:?}: expected the details' additional contacts to be reported as persisted, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text_at(&["Details", "ReviewDetails", "Status"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.review_details.as_ref())
            .and_then(|review| review.status.clone()),
        "case {case:?}: expected the review status to be reported as persisted, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text_at(&["Details", "ReviewDetails", "CaseId"]),
        account
            .details
            .as_ref()
            .and_then(|details| details.review_details.as_ref())
            .and_then(|review| review.case_id.clone()),
        "case {case:?}: expected the review case id to be reported as persisted, got {:?}",
        reply.body
    );

    // The sending limits, as the account holds them.
    assert_eq!(
        reply.number_at(&["SendQuota", "Max24HourSend"]),
        Some(quota.max_24h),
        "case {case:?}: expected Max24HourSend {}, got {:?}",
        quota.max_24h,
        reply.body
    );
    assert_eq!(
        reply.number_at(&["SendQuota", "MaxSendRate"]),
        Some(quota.max_send_rate),
        "case {case:?}: expected MaxSendRate {}, got {:?}",
        quota.max_send_rate,
        reply.body
    );
}

#[tokio::test]
async fn the_account_report_mirrors_every_persisted_account_attribute() {
    // Each row differs from the others along what the rule generalises over: each
    // switch both ways, each enum-valued member across its catalog, the suppression
    // reason set at every size it can take, the optional nested members present and
    // absent, and the sending limits a sandbox and a production account hold.
    let cases = vec![
        AccountCase {
            name: "fresh",
            account: AccountState::default(),
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "sending-paused-and-warmup-off",
            account: AccountState {
                sending_enabled: false,
                dedicated_ip_auto_warmup_enabled: false,
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "production-access-under-probation",
            account: AccountState {
                production_access_enabled: true,
                enforcement_status: ENFORCEMENT_STATUS_VALUES[1].to_string(),
                suppressed_reasons: vec![SUPPRESSION_LIST_REASON_VALUES[0].to_string()],
                ..AccountState::default()
            },
            // The raised limits of an account out of the sandbox: the report reads
            // them off the account rather than off the sandbox default.
            quota: SendQuotaState {
                sent_last_24h: 0.0,
                max_24h: 50_000.0,
                max_send_rate: 14.0,
            },
        },
        AccountCase {
            name: "shutdown-with-no-suppression-reasons",
            account: AccountState {
                enforcement_status: ENFORCEMENT_STATUS_VALUES[2].to_string(),
                suppressed_reasons: Vec::new(),
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "vdm-enabled-with-both-features-set",
            account: AccountState {
                vdm_enabled: FEATURE_ENABLED.to_string(),
                vdm_dashboard_attributes: Some(DashboardAttributes {
                    engagement_metrics: Some(FEATURE_ENABLED.to_string()),
                }),
                vdm_guardian_attributes: Some(GuardianAttributes {
                    optimized_shared_delivery: Some(FEATURE_DISABLED.to_string()),
                }),
                suppressed_reasons: vec![SUPPRESSION_LIST_REASON_VALUES[1].to_string()],
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "pricing-change-scheduled",
            account: AccountState {
                pricing_plan: "ESSENTIALS".to_string(),
                next_pricing_plan: Some("PRO".to_string()),
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "pricing-enterprise",
            account: AccountState {
                pricing_plan: "ENTERPRISE".to_string(),
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
        AccountCase {
            name: "details-and-suppression-validation-supplied",
            account: AccountState {
                details: Some(AccountDetails {
                    mail_type: Some(MAIL_TYPE_VALUES[1].to_string()),
                    website_url: Some("https://details.example.com/ses".to_string()),
                    contact_language: Some("JA".to_string()),
                    use_case_description: None,
                    additional_contact_email_addresses: Some(vec![
                        "ops@details.example.com".to_string(),
                        "billing@details.example.com".to_string(),
                    ]),
                    review_details: Some(ReviewDetails {
                        status: Some(REVIEW_STATUS_VALUES[3].to_string()),
                        case_id: Some("case-00000000".to_string()),
                    }),
                }),
                suppression_validation: Some(SuppressionValidationAttributes {
                    condition_threshold: SuppressionConditionThreshold {
                        condition_threshold_enabled: FEATURE_ENABLED.to_string(),
                        overall_confidence_threshold: Some(SuppressionConfidenceThreshold {
                            confidence_verdict_threshold: "HIGH".to_string(),
                        }),
                    },
                }),
                ..AccountState::default()
            },
            quota: SendQuotaState::default(),
        },
    ];

    // Every plan the model lists is covered by the rows above, so no value of the
    // catalog goes untested.
    for plan in PRICING_PLAN_VALUES {
        assert!(
            cases
                .iter()
                .any(|case| case.account.pricing_plan == *plan
                    || case.account.next_pricing_plan.as_deref() == Some(*plan)),
            "case catalog: the table must cover the pricing plan {plan:?}"
        );
    }

    for case in &cases {
        // A fresh emulator per row: the account record is one per account and
        // Region, so the rows must not see each other's state.
        let ses = Ses::new();
        ses.persist(&case.account, &case.quota);
        let reply = ses.account().await;
        assert_mirrors(case.name, &case.account, &case.quota, &reply);
    }
}

#[tokio::test]
async fn an_unconfigured_account_reports_the_attributes_a_new_account_has() {
    let ses = Ses::new();
    let reply = ses.account().await;
    let account = AccountState::default();
    assert_mirrors("unconfigured", &account, &SendQuotaState::default(), &reply);

    // The account nobody has configured: sending and dedicated-IP warm-up on, the
    // account in the sandbox, its reputation unenforced, the suppression list on
    // for both documented reasons, VDM off and no pricing plan active.
    assert_eq!(
        (
            reply.flag("SendingEnabled"),
            reply.flag("DedicatedIpAutoWarmupEnabled"),
            reply.flag("ProductionAccessEnabled"),
            reply.text("EnforcementStatus"),
        ),
        (
            true,
            true,
            false,
            ENFORCEMENT_STATUS_HEALTHY.to_string()
        ),
        "case unconfigured: expected a new account's switches and reputation status, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.strings_at(&["SuppressionAttributes", "SuppressedReasons"]),
        Some(
            SUPPRESSION_LIST_REASON_VALUES
                .iter()
                .map(|reason| (*reason).to_string())
                .collect::<Vec<_>>()
        ),
        "case unconfigured: expected the account-level suppression list on for every documented reason, got {:?}",
        reply.body
    );
    assert_eq!(
        (
            reply.text_at(&["VdmAttributes", "VdmEnabled"]),
            reply.text_at(&["PricingAttributes", "CurrentPlan"]),
        ),
        (
            Some(FEATURE_DISABLED.to_string()),
            Some(PRICING_PLAN_NONE.to_string())
        ),
        "case unconfigured: expected VDM off and no plan active, got {:?}",
        reply.body
    );

    // Every enum-valued member the report answers with carries a value its model
    // shape declares.
    for (member, value, catalog) in [
        (
            "EnforcementStatus",
            reply.text("EnforcementStatus"),
            ENFORCEMENT_STATUS_VALUES,
        ),
        (
            "VdmEnabled",
            reply
                .text_at(&["VdmAttributes", "VdmEnabled"])
                .unwrap_or_default(),
            FEATURE_STATUS_VALUES,
        ),
        (
            "CurrentPlan",
            reply
                .text_at(&["PricingAttributes", "CurrentPlan"])
                .unwrap_or_default(),
            PRICING_PLAN_VALUES,
        ),
    ] {
        assert!(
            catalog.contains(&value.as_str()),
            "case {member:?}: expected one of {catalog:?}, got {value:?}"
        );
    }
    for reason in reply
        .strings_at(&["SuppressionAttributes", "SuppressedReasons"])
        .unwrap_or_default()
    {
        assert!(
            SUPPRESSION_LIST_REASON_VALUES.contains(&reason.as_str()),
            "case SuppressedReasons: expected one of {SUPPRESSION_LIST_REASON_VALUES:?}, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn the_reported_send_quota_counts_the_recipients_charged_in_the_trailing_window() {
    let ses = Ses::new();
    assert_eq!(
        ses.create_identity("quota.example.com").await.status,
        200,
        "expected the create to succeed"
    );
    ses.settle_identities().await;

    let before = ses.account().await;
    assert_eq!(
        before.number_at(&["SendQuota", "SentLast24Hours"]),
        Some(0.0),
        "case no-sends: expected nothing charged against the quota yet, got {:?}",
        before.body
    );

    // Each recipient of an accepted message counts as one message against the
    // account's 24-hour quota, whichever destination field carries it.
    let rows: &[(&str, Value, f64)] = &[
        (
            "one-to",
            json!({"ToAddresses": ["a@example.org"]}),
            1.0,
        ),
        (
            "two-to",
            json!({"ToAddresses": ["b@example.org", "c@example.org"]}),
            2.0,
        ),
        (
            "to-and-cc",
            json!({"ToAddresses": ["d@example.org"], "CcAddresses": ["e@example.org"]}),
            2.0,
        ),
        (
            "to-cc-and-bcc",
            json!({
                "ToAddresses": ["f@example.org"],
                "CcAddresses": ["g@example.org"],
                "BccAddresses": ["h@example.org", "i@example.org"]
            }),
            4.0,
        ),
    ];

    let mut charged = 0.0;
    for (case, destination, recipients) in rows {
        let sent = ses
            .send(json!({
                "FromEmailAddress": "sender@quota.example.com",
                "Destination": destination,
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}}
            }))
            .await;
        assert_eq!(
            sent.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            sent.status, sent.body
        );
        charged += recipients;
        let reply = ses.account().await;
        assert_eq!(
            reply.number_at(&["SendQuota", "SentLast24Hours"]),
            Some(charged),
            "case {case:?}: expected {charged} recipients charged against the quota, got {:?}",
            reply.body
        );
    }

    // A message accepted before the window opened is no longer charged, so the
    // report drops it rather than keeping a tally that only grows.
    let charged_before_expiry = charged;
    ses.log_message(
        "outside-the-window",
        7,
        SystemTime::now()
            .checked_sub(SEND_QUOTA_WINDOW + Duration::from_secs(60))
            .expect("an instant a day ago"),
    );
    let reply = ses.account().await;
    assert_eq!(
        reply.number_at(&["SendQuota", "SentLast24Hours"]),
        Some(charged_before_expiry),
        "case outside-the-window: expected recipients older than {SEND_QUOTA_WINDOW:?} not to be charged, got {:?}",
        reply.body
    );

    // One inside the window is charged, wherever it came from.
    ses.log_message("inside-the-window", 3, SystemTime::now());
    let reply = ses.account().await;
    assert_eq!(
        reply.number_at(&["SendQuota", "SentLast24Hours"]),
        Some(charged_before_expiry + 3.0),
        "case inside-the-window: expected the newly logged recipients to be charged, got {:?}",
        reply.body
    );
}
