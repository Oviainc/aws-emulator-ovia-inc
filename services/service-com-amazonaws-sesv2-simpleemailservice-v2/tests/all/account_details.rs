// SPDX-License-Identifier: CC0-1.0

//! `PutAccountDetails` and the account report it feeds, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all
//! exercised, and what a call persisted is read back through `GetAccount`.
//!
//! The rules these cases pin:
//!
//! * the call stores the whole of the account's details as the request carries
//!   them, so the details every later caller reads are exactly the ones submitted
//!   and a member left out of a later submission is absent afterwards;
//! * a request asking for production access the account does not have is reviewed:
//!   the account stays in the sandbox, reporting that request as the review of the
//!   details just submitted, until the decision the clock owes it falls due, and a
//!   submission made while that review is running is refused;
//! * a value outside a member's modelled value set is refused with the input error
//!   the model binds, and nothing is persisted.
//!
//! The tables span what the rules generalise over: both mail types, both contact
//! languages and the member omitted, the optional members present and absent, the
//! published bounds of the sized members, and the production-access member asked
//! both ways and left out.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    CONTACT_LANGUAGE_VALUES, MAIL_TYPE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    AccountState, ComAmazonawsSesv2SimpleemailserviceV2State, FEATURE_ENABLED,
    REVIEW_STATUS_GRANTED, REVIEW_STATUS_PENDING,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The upper bound of `PutAccountDetailsRequest$UseCaseDescription`'s modelled
/// length, 0..5000
/// (`smithy_query(query="operation",operation="PutAccountDetails")`).
const USE_CASE_DESCRIPTION_MAX: usize = 5_000;

/// The bounds of `PutAccountDetailsRequest$AdditionalContactEmailAddresses`'s
/// modelled length, 1..4 (same query).
const ADDITIONAL_CONTACTS_MIN: usize = 1;
const ADDITIONAL_CONTACTS_MAX: usize = 4;

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

    /// A boolean member of the body, absent meaning the modelled default `false`.
    fn flag(&self, member: &str) -> bool {
        self.body
            .get(member)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The `Details` object the report carries, or `Null` when it carries none.
    fn details(&self) -> Value {
        self.body.get("Details").cloned().unwrap_or(Value::Null)
    }

    /// The `Details` object with its `ReviewDetails` dropped, so a case can compare
    /// the submitted details on their own.
    fn submitted_details(&self) -> Value {
        let mut details = self.details();
        if let Some(object) = details.as_object_mut() {
            object.remove("ReviewDetails");
        }
        details
    }

    /// A string inside `Details.ReviewDetails`, or `None` when absent.
    fn review(&self, member: &str) -> Option<String> {
        self.details()
            .get("ReviewDetails")?
            .get(member)?
            .as_str()
            .map(str::to_string)
    }

    /// This reply's body with `member` dropped, so two reports can be compared on
    /// everything except the one member a case changed.
    fn body_without(&self, member: &str) -> Value {
        let mut body = self.body.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove(member);
        }
        body
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// persist the account state it is about and read what a handler kept.
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

    /// `PutAccountDetails` with whatever body the case supplies, so a case can leave
    /// the optional members out entirely.
    async fn put_details(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/account/details", Some(body))
            .await
    }

    /// The account report.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    /// Persist the account state a case is about.
    fn persist(&self, account: &AccountState) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.account = account.clone();
    }

    /// The details the handler persisted, as the store holds them.
    fn stored_details(&self) -> Option<String> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .account
            .details
            .as_ref()
            .map(|details| format!("{details:?}"))
    }

    /// Bring the decision on the production-access request under review into the
    /// past, so a case does not wait out the span the clock owes it.
    fn bring_the_decision_due(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
        assert!(
            store.account.production_access_review_due_at.is_some(),
            "case decision-due: a production-access request must be under review here"
        );
        store.account.production_access_review_due_at = Some(past);
    }
}

/// One submission: the request body, and the `Details` the report must carry
/// afterwards.
struct DetailsCase {
    name: &'static str,
    body: Value,
    expected: Value,
}

/// A website URL, a mail type and the details that go with them, as rows that
/// differ along every member the request carries: both mail types, both contact
/// languages and the language omitted, the optional members present and absent, and
/// the published bounds of the sized members.
fn details_cases() -> Vec<DetailsCase> {
    let longest_use_case = "e".repeat(USE_CASE_DESCRIPTION_MAX);
    let four_contacts: Vec<String> = (0..ADDITIONAL_CONTACTS_MAX)
        .map(|index| format!("contact-{index}@example.com"))
        .collect();
    let one_contact: Vec<String> = (0..ADDITIONAL_CONTACTS_MIN)
        .map(|index| format!("only-{index}@example.net"))
        .collect();
    vec![
        DetailsCase {
            name: "marketing-required-members-only",
            body: json!({
                "MailType": MAIL_TYPE_VALUES[0],
                "WebsiteURL": "https://marketing.example.com",
            }),
            expected: json!({
                "MailType": MAIL_TYPE_VALUES[0],
                "WebsiteURL": "https://marketing.example.com",
            }),
        },
        DetailsCase {
            name: "transactional-english-with-one-contact",
            body: json!({
                "MailType": MAIL_TYPE_VALUES[1],
                "WebsiteURL": "http://transactional.example.org/signup?ref=1#top",
                "ContactLanguage": CONTACT_LANGUAGE_VALUES[0],
                "UseCaseDescription": "Order confirmations for our storefront.",
                "AdditionalContactEmailAddresses": one_contact,
            }),
            expected: json!({
                "MailType": MAIL_TYPE_VALUES[1],
                "WebsiteURL": "http://transactional.example.org/signup?ref=1#top",
                "ContactLanguage": CONTACT_LANGUAGE_VALUES[0],
                "UseCaseDescription": "Order confirmations for our storefront.",
                "AdditionalContactEmailAddresses": one_contact,
            }),
        },
        DetailsCase {
            name: "marketing-japanese-with-the-most-contacts",
            body: json!({
                "MailType": MAIL_TYPE_VALUES[0],
                "WebsiteURL": "https://marketing.example.co.jp/newsletter",
                "ContactLanguage": CONTACT_LANGUAGE_VALUES[1],
                "UseCaseDescription": longest_use_case,
                "AdditionalContactEmailAddresses": four_contacts,
            }),
            expected: json!({
                "MailType": MAIL_TYPE_VALUES[0],
                "WebsiteURL": "https://marketing.example.co.jp/newsletter",
                "ContactLanguage": CONTACT_LANGUAGE_VALUES[1],
                "UseCaseDescription": longest_use_case,
                "AdditionalContactEmailAddresses": four_contacts,
            }),
        },
        DetailsCase {
            name: "transactional-with-an-empty-use-case-and-the-shortest-url",
            body: json!({
                "MailType": MAIL_TYPE_VALUES[1],
                // One character: the lower bound of `WebsiteURL`'s modelled length,
                // which its pattern accepts as a relative reference.
                "WebsiteURL": "s",
                "UseCaseDescription": "",
            }),
            expected: json!({
                "MailType": MAIL_TYPE_VALUES[1],
                "WebsiteURL": "s",
                "UseCaseDescription": "",
            }),
        },
    ]
}

#[tokio::test]
async fn the_submitted_details_are_what_every_later_read_reports() {
    let cases = details_cases();
    // Every value of both enum-valued members is covered by the rows above, so no
    // value of either catalog goes untested.
    for mail_type in MAIL_TYPE_VALUES {
        assert!(
            cases
                .iter()
                .any(|case| case.body.get("MailType").and_then(Value::as_str) == Some(mail_type)),
            "case catalog: the table must cover the mail type {mail_type:?}"
        );
    }
    for language in CONTACT_LANGUAGE_VALUES {
        assert!(
            cases.iter().any(|case| case
                .body
                .get("ContactLanguage")
                .and_then(Value::as_str)
                == Some(language)),
            "case catalog: the table must cover the contact language {language:?}"
        );
    }

    for case in &cases {
        let name = case.name;
        // A fresh emulator per row: the details are one record per account and
        // Region, so the rows must not see each other's submissions.
        let ses = Ses::new();
        let before = ses.account().await;
        assert_eq!(
            before.details(),
            Value::Null,
            "case {name:?}: an account that has supplied no details must report none, got {:?}",
            before.body
        );

        let written = ses.put_details(case.body.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the submission, got {} {:?}",
            written.status, written.body
        );
        assert!(
            written.body.is_null() || written.body == json!({}),
            "case {name:?}: expected an empty response body, got {:?}",
            written.body
        );

        // Every later caller sees the submission, and a read does not move it: two
        // reports in a row agree with it.
        for read in ["first", "second"] {
            let after = ses.account().await;
            assert_eq!(
                after.status, 200,
                "case {name:?} ({read} read): expected 200 from the account report, got {} {:?}",
                after.status, after.body
            );
            assert_eq!(
                after.submitted_details(),
                case.expected,
                "case {name:?} ({read} read): expected the details {:?}, got {:?}",
                case.expected,
                after.details()
            );
            // None of these rows asks for production access, so the account is left
            // in the sandbox with nothing under review.
            assert_eq!(
                (after.flag("ProductionAccessEnabled"), after.review("Status")),
                (false, None),
                "case {name:?} ({read} read): expected a sandbox account with no review, got {:?}",
                after.body
            );
            assert_eq!(
                after.body_without("Details"),
                before.body_without("Details"),
                "case {name:?} ({read} read): expected every other reported attribute to be \
                 untouched, got {:?}",
                after.body
            );
        }
    }
}

#[tokio::test]
async fn a_later_submission_replaces_the_details_whole() {
    let ses = Ses::new();
    // Each submission carries the whole of the details, so what the account reports
    // after one is that submission alone — a member the earlier one carried and the
    // later one leaves out is gone, not merged in.
    let rows = details_cases();
    for pair in rows.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        let case = format!("{} then {}", first.name, second.name);
        for submission in [first, second] {
            let written = ses.put_details(submission.body.clone()).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from the submission of {:?}, got {} {:?}",
                submission.name, written.status, written.body
            );
        }
        let after = ses.account().await;
        assert_eq!(
            after.submitted_details(),
            second.expected,
            "case {case:?}: expected the later submission's details {:?}, got {:?}",
            second.expected,
            after.details()
        );
    }
}

#[tokio::test]
async fn a_production_access_request_is_reviewed_before_the_account_leaves_the_sandbox() {
    let ses = Ses::new();
    let fresh = ses.account().await;
    assert!(
        !fresh.flag("ProductionAccessEnabled"),
        "case fresh: expected a new account to be in the sandbox, got {:?}",
        fresh.body
    );

    let asked = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[0],
            "WebsiteURL": "https://appeal.example.com",
            "ProductionAccessEnabled": true,
        }))
        .await;
    assert_eq!(
        asked.status, 200,
        "case asked: expected 200 from the request, got {} {:?}",
        asked.status, asked.body
    );

    // While the request is under review the account is still in the sandbox, and
    // reads do not decide it: every report shows the same pending review, on the
    // same case.
    let under_review = ses.account().await;
    let case_id = under_review.review("CaseId");
    assert_eq!(
        (
            under_review.flag("ProductionAccessEnabled"),
            under_review.review("Status"),
        ),
        (false, Some(REVIEW_STATUS_PENDING.to_string())),
        "case under-review: expected a sandbox account with the request pending, got {:?}",
        under_review.body
    );
    assert!(
        case_id.as_deref().is_some_and(|id| !id.is_empty()),
        "case under-review: expected the review to name the support case it opened, got {:?}",
        under_review.details()
    );
    let read_again = ses.account().await;
    assert_eq!(
        (
            read_again.flag("ProductionAccessEnabled"),
            read_again.review("Status"),
            read_again.review("CaseId"),
        ),
        (false, Some(REVIEW_STATUS_PENDING.to_string()), case_id.clone()),
        "case read-again: expected the pending review to be unchanged by a read, got {:?}",
        read_again.body
    );

    // "If there is already an ongoing account details update under review": a
    // submission made meanwhile is refused, and nothing it carried is persisted.
    let stored = ses.stored_details();
    let conflicted = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[1],
            "WebsiteURL": "https://second.example.com",
        }))
        .await;
    assert_eq!(
        (conflicted.status, conflicted.error_code().as_str()),
        (409, "ConflictException"),
        "case conflict: expected a 409 ConflictException, got {} {:?}",
        conflicted.status,
        conflicted.body
    );
    assert_eq!(
        conflicted.message(),
        "There is already an ongoing account details update under review.",
        "case conflict: expected the ongoing-review message, got {:?}",
        conflicted.message()
    );
    assert_eq!(
        ses.stored_details(),
        stored,
        "case conflict: expected the refused submission to leave the details alone"
    );

    // Once the decision the clock owed the request falls due, the account reports
    // the access it was granted and the review that granted it, on the same case.
    ses.bring_the_decision_due();
    let decided = ses.account().await;
    assert_eq!(
        (
            decided.flag("ProductionAccessEnabled"),
            decided.review("Status"),
            decided.review("CaseId"),
        ),
        (true, Some(REVIEW_STATUS_GRANTED.to_string()), case_id.clone()),
        "case decided: expected production access granted on the same case, got {:?}",
        decided.body
    );

    // A submission that says nothing about production access leaves both the access
    // and the review it last reported alone.
    let silent = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[1],
            "WebsiteURL": "https://silent.example.com",
        }))
        .await;
    assert_eq!(
        silent.status, 200,
        "case silent: expected 200 from the submission, got {} {:?}",
        silent.status, silent.body
    );
    let after_silent = ses.account().await;
    assert_eq!(
        (
            after_silent.flag("ProductionAccessEnabled"),
            after_silent.review("Status"),
            after_silent.review("CaseId"),
        ),
        (true, Some(REVIEW_STATUS_GRANTED.to_string()), case_id.clone()),
        "case silent: expected the account's access and review to be untouched, got {:?}",
        after_silent.body
    );

    // Asking for no production access puts the account back in the sandbox at once,
    // with nothing left under review.
    let withdrawn = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[0],
            "WebsiteURL": "https://withdrawn.example.com",
            "ProductionAccessEnabled": false,
        }))
        .await;
    assert_eq!(
        withdrawn.status, 200,
        "case withdrawn: expected 200 from the submission, got {} {:?}",
        withdrawn.status, withdrawn.body
    );
    let sandboxed = ses.account().await;
    assert_eq!(
        (
            sandboxed.flag("ProductionAccessEnabled"),
            sandboxed.review("Status"),
        ),
        (false, None),
        "case withdrawn: expected the account back in the sandbox with no review, got {:?}",
        sandboxed.body
    );

    // And a fresh request opens a case of its own: the id identifies one request.
    let asked_again = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[0],
            "WebsiteURL": "https://appeal-again.example.com",
            "ProductionAccessEnabled": true,
        }))
        .await;
    assert_eq!(
        asked_again.status, 200,
        "case asked-again: expected 200 from the request, got {} {:?}",
        asked_again.status, asked_again.body
    );
    let reviewed_again = ses.account().await;
    assert_eq!(
        reviewed_again.review("Status"),
        Some(REVIEW_STATUS_PENDING.to_string()),
        "case asked-again: expected the new request to be pending, got {:?}",
        reviewed_again.body
    );
    assert_ne!(
        reviewed_again.review("CaseId"),
        case_id,
        "case asked-again: expected a case id of its own, got {:?}",
        reviewed_again.details()
    );
}

#[tokio::test]
async fn a_request_that_asks_for_access_the_account_already_has_opens_no_review() {
    // An account already out of the sandbox: asking for production access again is
    // asking for what it holds, so there is nothing to review and nothing changes.
    let ses = Ses::new();
    ses.persist(&AccountState {
        production_access_enabled: true,
        ..AccountState::default()
    });

    let asked = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[1],
            "WebsiteURL": "https://already.example.com",
            "ProductionAccessEnabled": true,
        }))
        .await;
    assert_eq!(
        asked.status, 200,
        "case already-granted: expected 200 from the request, got {} {:?}",
        asked.status, asked.body
    );
    let after = ses.account().await;
    assert_eq!(
        (after.flag("ProductionAccessEnabled"), after.review("Status")),
        (true, None),
        "case already-granted: expected the access to stand with no review opened, got {:?}",
        after.body
    );

    // A second submission is accepted, not refused as an ongoing review.
    let again = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[0],
            "WebsiteURL": "https://already.example.com/again",
            "ProductionAccessEnabled": true,
        }))
        .await;
    assert_eq!(
        again.status, 200,
        "case already-granted-twice: expected 200 from the second request, got {} {:?}",
        again.status, again.body
    );
}

#[tokio::test]
async fn a_value_outside_a_members_modelled_set_is_refused_and_nothing_is_stored() {
    // One row per enum-valued member, each with a value its model does not declare:
    // the lower-case spelling of a declared value, a value of the sibling member's
    // catalog, and a value no catalog in the service carries. The member the
    // refusal has to name is the one the request's own JSON names.
    let rows: &[(&str, &str, &str, Value)] = &[
        (
            "mail-type-lower-case",
            "mailType",
            "marketing",
            json!({
                "MailType": MAIL_TYPE_VALUES[0].to_lowercase(),
                "WebsiteURL": "https://refused.example.com",
            }),
        ),
        (
            "mail-type-unknown",
            "mailType",
            "NEWSLETTER",
            json!({
                "MailType": "NEWSLETTER",
                "WebsiteURL": "https://refused.example.com",
            }),
        ),
        (
            "contact-language-unknown",
            "contactLanguage",
            "FR",
            json!({
                "MailType": MAIL_TYPE_VALUES[1],
                "WebsiteURL": "https://refused.example.com",
                "ContactLanguage": "FR",
            }),
        ),
        (
            "contact-language-is-a-mail-type",
            "contactLanguage",
            MAIL_TYPE_VALUES[0],
            json!({
                "MailType": MAIL_TYPE_VALUES[1],
                "WebsiteURL": "https://refused.example.com",
                "ContactLanguage": MAIL_TYPE_VALUES[0],
            }),
        ),
    ];

    // An account whose attributes are away from their defaults, so a write that
    // reached any of them would show.
    let account = AccountState {
        dedicated_ip_auto_warmup_enabled: false,
        vdm_enabled: FEATURE_ENABLED.to_string(),
        ..AccountState::default()
    };

    for (case, member, value, body) in rows {
        let ses = Ses::new();
        ses.persist(&account);
        let before = ses.account().await;

        let refused = ses.put_details(body.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.body
        );
        let message = refused.message();
        assert!(
            message.contains(member) && message.contains(value),
            "case {case:?}: expected the refusal to name the member {member:?} and the value \
             {value:?}, got {message:?}"
        );
        assert_eq!(
            ses.stored_details(),
            None,
            "case {case:?}: expected the refused request to persist no details"
        );
        assert_eq!(
            ses.account().await.body,
            before.body,
            "case {case:?}: expected the account report to be unchanged by the refusal"
        );
    }
}

#[tokio::test]
async fn the_details_are_reported_exactly_as_the_model_shapes_them() {
    // The report's `Details` carries only the members the model declares on
    // `AccountDetails`, so a client reading it finds nothing the shape does not
    // name.
    const MEMBERS: &[&str] = &[
        "MailType",
        "WebsiteURL",
        "ContactLanguage",
        "UseCaseDescription",
        "AdditionalContactEmailAddresses",
        "ReviewDetails",
    ];

    let ses = Ses::new();
    let submitted = ses
        .put_details(json!({
            "MailType": MAIL_TYPE_VALUES[0],
            "WebsiteURL": "https://shape.example.com",
            "ContactLanguage": CONTACT_LANGUAGE_VALUES[1],
            "UseCaseDescription": "Announcements.",
            "AdditionalContactEmailAddresses": ["ops@shape.example.com"],
            "ProductionAccessEnabled": true,
        }))
        .await;
    assert_eq!(
        submitted.status, 200,
        "case shape: expected 200 from the submission, got {} {:?}",
        submitted.status, submitted.body
    );

    let reported = ses.account().await;
    let details = reported.details();
    let object: &Map<String, Value> = details
        .as_object()
        .unwrap_or_else(|| panic!("case shape: expected a Details object, got {details:?}"));
    for member in object.keys() {
        assert!(
            MEMBERS.contains(&member.as_str()),
            "case shape: expected only the modelled members {MEMBERS:?}, got {member:?}"
        );
    }
    assert_eq!(
        object.len(),
        MEMBERS.len(),
        "case shape: expected every modelled member to be reported for a submission that \
         carried them all, got {details:?}"
    );
}
