// SPDX-License-Identifier: CC0-1.0

//! `ListRecommendations`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and what a finding reports is read back against the identity record it
//! was derived from through `GetEmailIdentity`.
//!
//! The rule these cases pin: no request creates a recommendation — one exists for each
//! mail-authentication mechanism a resource of the account has not set up, and every
//! member of a reported finding is COMPUTED from that resource's own record: the resource
//! it is about is the identity whose setup it reports, its status is `OPEN` while the
//! mechanism is not in place and `FIXED` once the search that settles it has fallen due,
//! and the instants it carries are when the identity was created and when that search
//! settled. A listing reports the findings of the account in the Region the call executes
//! in, ordered by resource and then by type, narrowed by every key the request's filter
//! names — a key whose value names nothing the account holds narrows the listing to
//! nothing rather than being refused — and windowed by its `PageSize`/`NextToken`.
//! A filter value outside the value set its key filters on, one that is not a resource
//! ARN of this service, a page size outside the documented range and a token this listing
//! never handed out are each refused as invalid input.
//!
//! The tables below span what the rule generalises over: both identity kinds and their
//! spellings, a domain whose DNS zone can never exist, the number of identities (none,
//! one, several), each mechanism before and after its search settles, every
//! `ListRecommendationsFilterKey` and every value of each enum it filters on (including
//! the ones no finding of this account carries), a resource ARN of another case, of
//! another resource kind and of an identity never created, both documented page-size
//! bounds and one past each, and every refusal leg.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    RECOMMENDATION_IMPACT_VALUES, RECOMMENDATION_STATUS_VALUES, RECOMMENDATION_TYPE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, CONTACT_LISTS_PAGE_TOKEN_TAG,
    DEFAULT_REGION, MAIL_FROM_MX_SETTLE, RECOMMENDATION_IMPACT_HIGH, RECOMMENDATION_STATUS_FIXED,
    RECOMMENDATION_STATUS_OPEN, RECOMMENDATION_TYPE_DKIM, RECOMMENDATION_TYPE_SPF,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The one account this emulator serves, and so the account every ARN it mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// One reported finding as the cases compare it: `(ResourceArn, Type, Status)` — the
/// three members that say which resource a finding is about, which mechanism it reports
/// and where that mechanism stands.
type Finding = (String, String, String);

/// One row of the filter table: the case, the request body, and the findings the narrowed
/// listing must report.
type FilterCase = (String, Value, Vec<Finding>);

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
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

    /// The reported `Recommendations`, as the list of objects it is.
    fn recommendations(&self) -> Vec<Value> {
        self.body
            .get("Recommendations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Each reported finding as `(ResourceArn, Type, Status)`, in the order reported.
    fn findings(&self) -> Vec<Finding> {
        self.recommendations()
            .iter()
            .map(|recommendation| {
                (
                    member(recommendation, "ResourceArn"),
                    member(recommendation, "Type"),
                    member(recommendation, "Status"),
                )
            })
            .collect()
    }

    /// The `NextToken` the listing handed out, absent when it reported the last page.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// One string member of a reported finding, or `""` when absent.
fn member(recommendation: &Value, name: &str) -> String {
    recommendation
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One timestamp member of a reported finding, in seconds since the epoch: restJson1
/// writes a `Timestamp` as epoch seconds.
fn instant(recommendation: &Value, name: &str) -> f64 {
    recommendation
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

/// The ARN of the email identity `identity` in the Region every case signs for, written
/// out rather than minted through the crate's own helper so the expectation is the wire
/// form and not the code under test.
fn identity_arn_of(identity: &str) -> String {
    format!("arn:aws:ses:{DEFAULT_REGION}:{ACCOUNT_ID}:identity/{identity}")
}

/// The emulated service behind its router, plus the shared engine so a case can make a
/// transition the clock owes fall due instead of sleeping for it.
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
        let host = format!("email.{DEFAULT_REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host.clone())
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{DEFAULT_REGION}/ses/aws4_request, SignedHeaders=host, Signature=00"),
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

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn create_contact_list(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/contact-lists",
            Some(json!({ "ContactListName": name })),
        )
        .await
    }

    async fn put_mail_from(&self, identity: &str, domain: &str, behavior: Option<&str>) -> Reply {
        let mut body = json!({ "MailFromDomain": domain });
        if let Some(behavior) = behavior {
            body["BehaviorOnMxFailure"] = json!(behavior);
        }
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/mail-from"),
            Some(body),
        )
        .await
    }

    /// `ListRecommendations` with whatever body the case supplies, so a case can leave
    /// every optional member out entirely.
    async fn list(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/vdm/recommendations", Some(body))
            .await
    }

    /// Move the transitions the clock owes `identity` out of reach, so a case reading the
    /// state a mechanism starts in cannot race the spans in which they fall due.
    fn hold_transitions_pending(&self, identity: &str) {
        self.with_identity(identity, |record| {
            let held = SystemTime::now() + MAIL_FROM_MX_SETTLE + Duration::from_secs(3600);
            record.verification_due_at = held;
            if let Some(mail_from) = record.mail_from.as_mut() {
                mail_from.mx_lookup_due_at = held;
            }
        });
    }

    /// Make the search for the identity's own DNS records due, as the clock does once the
    /// span has passed, without sleeping for it.
    fn make_verification_due(&self, identity: &str) {
        self.with_identity(identity, |record| {
            record.verification_due_at = SystemTime::now() - Duration::from_secs(1);
        });
    }

    /// The same for the search for the custom MAIL FROM domain's MX record, with the
    /// identity backdated so the instant the search settled is an instant after the
    /// identity existed and before this call: the clock owes both, and nothing sleeps for
    /// either.
    fn make_mx_lookup_due(&self, identity: &str) {
        self.with_identity(identity, |record| {
            let created_at = SystemTime::now() - Duration::from_secs(3600);
            record.created_at = created_at;
            let mail_from = record
                .mail_from
                .as_mut()
                .expect("the identity must hold a MAIL FROM configuration");
            mail_from.mx_lookup_due_at = created_at + Duration::from_secs(60);
        });
    }

    /// Reach the stored record of `identity` and change the due instants on it.
    fn with_identity(&self, identity: &str, change: impl FnOnce(&mut IdentityRecord)) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        change(record);
    }
}

/// The stored record a case reaches through [`Ses::with_identity`].
type IdentityRecord = service_com_amazonaws_sesv2_simpleemailservice_v2::state::EmailIdentityRecord;

/// One row of the derivation table: an identity to create, and the findings a listing
/// must report for it while every search the clock owes is still outstanding.
struct DerivationCase {
    case: &'static str,
    identity: &'static str,
    /// `(Type, Status)` pairs, in the order the listing must report them.
    expected: Vec<(&'static str, &'static str)>,
}

/// The rows of the derivation table: both identity kinds, their spellings, a subdomain
/// identity and a domain under a top-level name the DNS root does not delegate.
fn derivation_cases() -> Vec<DerivationCase> {
    let both_open = vec![
        (RECOMMENDATION_TYPE_DKIM, RECOMMENDATION_STATUS_OPEN),
        (RECOMMENDATION_TYPE_SPF, RECOMMENDATION_STATUS_OPEN),
    ];
    vec![
        DerivationCase {
            case: "a domain identity",
            identity: "example.com",
            expected: both_open.clone(),
        },
        DerivationCase {
            case: "a domain identity spelled in mixed case",
            identity: "Mixed.Example.COM",
            expected: both_open.clone(),
        },
        DerivationCase {
            case: "a subdomain identity several labels deep",
            identity: "out.mail.example.net",
            expected: both_open.clone(),
        },
        DerivationCase {
            case: "a domain no DNS zone can cover",
            identity: "nowhere.invalid",
            expected: both_open,
        },
        DerivationCase {
            case: "an email-address identity, whose domain carries the authentication",
            identity: "sender@example.org",
            expected: Vec::new(),
        },
        DerivationCase {
            case: "an email-address identity spelled in mixed case",
            identity: "Sender@Example.ORG",
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_finding_is_derived_from_the_identity_record_it_is_about() {
    for case in derivation_cases() {
        let ses = Ses::new();
        let created = ses.create_identity(case.identity).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected the create to be accepted, got {} {:?}",
            case.case,
            created.status,
            created.message()
        );
        ses.hold_transitions_pending(case.identity);

        let listed = ses.list(json!({})).await;
        assert_eq!(
            listed.status,
            200,
            "case {:?}: expected the listing to be answered, got {} {:?}",
            case.case,
            listed.status,
            listed.message()
        );
        let expected: Vec<Finding> = case
            .expected
            .iter()
            .map(|(kind, status)| {
                (
                    identity_arn_of(case.identity),
                    (*kind).to_string(),
                    (*status).to_string(),
                )
            })
            .collect();
        assert_eq!(
            listed.findings(),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.case,
            listed.findings()
        );

        for recommendation in listed.recommendations() {
            assert_eq!(
                member(&recommendation, "Impact"),
                RECOMMENDATION_IMPACT_HIGH,
                "case {:?}: expected every finding to carry impact {RECOMMENDATION_IMPACT_HIGH}, got {:?}",
                case.case,
                member(&recommendation, "Impact")
            );
            assert!(
                member(&recommendation, "Description").len() > 1,
                "case {:?}: expected the finding to describe what it asks for, got {:?}",
                case.case,
                member(&recommendation, "Description")
            );
            // The description describes the mechanism the finding is about, so the
            // findings of one resource cannot all carry one sentence.
            let descriptions: Vec<String> = listed
                .recommendations()
                .iter()
                .map(|reported| member(reported, "Description"))
                .collect();
            let mut distinct = descriptions.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(
                distinct.len(),
                descriptions.len(),
                "case {:?}: expected one description per mechanism, got {descriptions:?}",
                case.case
            );
            // Nothing has changed the finding since it was raised, so the two instants
            // are the one instant the identity was created at.
            assert_eq!(
                instant(&recommendation, "LastUpdatedTimestamp"),
                instant(&recommendation, "CreatedTimestamp"),
                "case {:?}: expected an outstanding finding's last update to be when it was raised, got {} and {}",
                case.case,
                instant(&recommendation, "LastUpdatedTimestamp"),
                instant(&recommendation, "CreatedTimestamp")
            );
        }

        // Reporting only: a second read of the same records reports exactly what the
        // first did, so reading the listing changes nothing a later caller sees.
        let again = ses.list(json!({})).await;
        assert_eq!(
            again.findings(),
            listed.findings(),
            "case {:?}: expected a second listing to report {:?}, got {:?}",
            case.case,
            listed.findings(),
            again.findings()
        );
    }
}

#[tokio::test]
async fn the_dkim_finding_follows_the_search_for_the_identitys_records() {
    // `(case, identity, whether the records this identity was told to publish can ever be
    // found)`: a domain under a top-level name the DNS root does not delegate answers "no
    // such name" however its owner configures DNS, so its search never succeeds.
    let cases: [(&str, &str, bool); 3] = [
        ("a domain whose zone can exist", "example.com", true),
        ("a subdomain whose zone can exist", "mail.example.net", true),
        ("a domain no DNS zone can cover", "nowhere.invalid", false),
    ];
    for (case, identity, can_be_found) in cases {
        let ses = Ses::new();
        ses.create_identity(identity).await;
        ses.hold_transitions_pending(identity);

        let before = ses.list(json!({"Filter": {"TYPE": RECOMMENDATION_TYPE_DKIM}})).await;
        let outstanding = before.recommendations();
        assert_eq!(
            outstanding.len(),
            1,
            "case {case:?}: expected one DKIM finding, got {:?}",
            before.findings()
        );
        assert_eq!(
            member(&outstanding[0], "Status"),
            RECOMMENDATION_STATUS_OPEN,
            "case {case:?}: expected the finding to be open while the search is outstanding, got {:?}",
            member(&outstanding[0], "Status")
        );

        ses.make_verification_due(identity);
        let after = ses.list(json!({"Filter": {"TYPE": RECOMMENDATION_TYPE_DKIM}})).await;
        let settled = after.recommendations();
        let expected_status = if can_be_found {
            RECOMMENDATION_STATUS_FIXED
        } else {
            RECOMMENDATION_STATUS_OPEN
        };
        assert_eq!(
            member(&settled[0], "Status"),
            expected_status,
            "case {case:?}: expected status {expected_status} once the search fell due, got {:?}",
            member(&settled[0], "Status")
        );
        assert!(
            instant(&settled[0], "LastUpdatedTimestamp")
                >= instant(&settled[0], "CreatedTimestamp"),
            "case {case:?}: expected the finding's last update not to precede when it was raised, got {} and {}",
            instant(&settled[0], "LastUpdatedTimestamp"),
            instant(&settled[0], "CreatedTimestamp")
        );

        // The finding reports the identity record's own DKIM state, so the status a read
        // of that identity reports and the status of the finding agree.
        let read_back = ses.get_identity(identity).await;
        let dkim_status = read_back
            .body
            .pointer("/DkimAttributes/Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let records_found = dkim_status == "SUCCESS";
        assert_eq!(
            records_found,
            expected_status == RECOMMENDATION_STATUS_FIXED,
            "case {case:?}: expected the finding's status {expected_status} to agree with the identity's DKIM status, got {dkim_status:?}"
        );
    }
}

#[tokio::test]
async fn the_spf_finding_follows_the_custom_mail_from_configuration() {
    // The behaviour on MX failure is part of the configuration but not of the mechanism
    // the finding reports, so a row drives each form of the request.
    let cases: [(&str, Option<&str>); 3] = [
        ("no behaviour named", None),
        ("mail sent with SES's own domain on failure", Some("USE_DEFAULT_VALUE")),
        ("mail rejected on failure", Some("REJECT_MESSAGE")),
    ];
    for (case, behavior) in cases {
        let identity = "example.com";
        let spf_filter = json!({"Filter": {"TYPE": RECOMMENDATION_TYPE_SPF}});
        let ses = Ses::new();
        ses.create_identity(identity).await;
        ses.hold_transitions_pending(identity);

        let unconfigured = ses.list(spf_filter.clone()).await;
        assert_eq!(
            member(&unconfigured.recommendations()[0], "Status"),
            RECOMMENDATION_STATUS_OPEN,
            "case {case:?}: expected the finding to be open while no custom MAIL FROM domain is configured, got {:?}",
            unconfigured.findings()
        );

        let configured = ses
            .put_mail_from(identity, &format!("mail.{identity}"), behavior)
            .await;
        assert_eq!(
            configured.status,
            200,
            "case {case:?}: expected the configuration to be accepted, got {} {:?}",
            configured.status,
            configured.message()
        );
        ses.hold_transitions_pending(identity);
        let pending = ses.list(spf_filter.clone()).await;
        assert_eq!(
            member(&pending.recommendations()[0], "Status"),
            RECOMMENDATION_STATUS_OPEN,
            "case {case:?}: expected the finding to stay open while the MX search is outstanding, got {:?}",
            pending.findings()
        );

        ses.make_mx_lookup_due(identity);
        let settled = ses.list(spf_filter).await;
        assert_eq!(
            member(&settled.recommendations()[0], "Status"),
            RECOMMENDATION_STATUS_FIXED,
            "case {case:?}: expected the finding to be fixed once the MX search fell due, got {:?}",
            settled.findings()
        );
        assert!(
            instant(&settled.recommendations()[0], "LastUpdatedTimestamp")
                > instant(&settled.recommendations()[0], "CreatedTimestamp"),
            "case {case:?}: expected the finding's last update to be the instant the search settled, got {} and {}",
            instant(&settled.recommendations()[0], "LastUpdatedTimestamp"),
            instant(&settled.recommendations()[0], "CreatedTimestamp")
        );
    }
}

/// The two identities every filter and paging case lists over, with every search settled
/// for the first and every one outstanding for the second — so the listing carries both
/// statuses, two resources and both types.
///
/// Returns the service, ready to list.
async fn two_identities() -> Ses {
    let ses = Ses::new();
    ses.create_identity(SETTLED).await;
    ses.put_mail_from(SETTLED, &format!("mail.{SETTLED}"), None)
        .await;
    ses.make_verification_due(SETTLED);
    ses.make_mx_lookup_due(SETTLED);
    // A listing settles what the clock owes, so the first identity's findings are fixed
    // from here on.
    ses.list(json!({})).await;
    ses.create_identity(OUTSTANDING).await;
    ses.hold_transitions_pending(OUTSTANDING);
    ses
}

/// The identity whose findings are all `FIXED`. Ordered before [`OUTSTANDING`] by ARN, so
/// the listing's own order is the order the cases expect.
const SETTLED: &str = "alpha.example.com";

/// The identity whose findings are all `OPEN`.
const OUTSTANDING: &str = "beta.example.com";

/// Every finding the two identities yield, in the order a listing must report them.
fn whole_listing() -> Vec<Finding> {
    vec![
        (
            identity_arn_of(SETTLED),
            RECOMMENDATION_TYPE_DKIM.to_string(),
            RECOMMENDATION_STATUS_FIXED.to_string(),
        ),
        (
            identity_arn_of(SETTLED),
            RECOMMENDATION_TYPE_SPF.to_string(),
            RECOMMENDATION_STATUS_FIXED.to_string(),
        ),
        (
            identity_arn_of(OUTSTANDING),
            RECOMMENDATION_TYPE_DKIM.to_string(),
            RECOMMENDATION_STATUS_OPEN.to_string(),
        ),
        (
            identity_arn_of(OUTSTANDING),
            RECOMMENDATION_TYPE_SPF.to_string(),
            RECOMMENDATION_STATUS_OPEN.to_string(),
        ),
    ]
}

/// The findings of `whole_listing` that satisfy `keep`.
fn listing_where(keep: impl Fn(&Finding) -> bool) -> Vec<Finding> {
    whole_listing().into_iter().filter(|row| keep(row)).collect()
}

#[tokio::test]
async fn every_filter_key_narrows_the_listing() {
    let ses = two_identities().await;
    let contact_list = "a-list";
    ses.create_contact_list(contact_list).await;

    // One row per key and per value of the enum each key filters on, taken from the
    // model's own value sets (`smithy_query(query="shape",shape="RecommendationType")`
    // and its `RecommendationStatus`/`RecommendationImpact` siblings, which the crate
    // states once as `RECOMMENDATION_*_VALUES`) — including every value no finding of
    // this account carries, which narrows the listing to nothing.
    let mut cases: Vec<FilterCase> = vec![
        (
            "no filter at all".to_string(),
            json!({}),
            whole_listing(),
        ),
        (
            "the resource of the settled identity".to_string(),
            json!({"Filter": {"RESOURCE_ARN": identity_arn_of(SETTLED)}}),
            listing_where(|(arn, _, _)| arn == &identity_arn_of(SETTLED)),
        ),
        (
            "the same resource with its domain spelled in another case".to_string(),
            json!({"Filter": {"RESOURCE_ARN": identity_arn_of(&SETTLED.to_uppercase())}}),
            listing_where(|(arn, _, _)| arn == &identity_arn_of(SETTLED)),
        ),
        (
            "an identity the account never created".to_string(),
            json!({"Filter": {"RESOURCE_ARN": identity_arn_of("never-created.example.com")}}),
            Vec::new(),
        ),
        (
            "the same identity in another Region, which this call does not reach".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:ses:us-west-2:{ACCOUNT_ID}:identity/{SETTLED}")}}),
            Vec::new(),
        ),
        (
            "the same identity in another account, which this call does not reach".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:ses:{DEFAULT_REGION}:111122223333:identity/{SETTLED}")}}),
            Vec::new(),
        ),
        (
            "a resource of another kind, which carries no finding".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:ses:{DEFAULT_REGION}:{ACCOUNT_ID}:contact-list/{contact_list}")}}),
            Vec::new(),
        ),
        (
            "a resource and a status together".to_string(),
            json!({"Filter": {"RESOURCE_ARN": identity_arn_of(OUTSTANDING), "STATUS": RECOMMENDATION_STATUS_OPEN}}),
            listing_where(|(arn, _, _)| arn == &identity_arn_of(OUTSTANDING)),
        ),
        (
            "a status and a type together".to_string(),
            json!({"Filter": {"STATUS": RECOMMENDATION_STATUS_OPEN, "TYPE": RECOMMENDATION_TYPE_DKIM}}),
            listing_where(|(_, kind, status)| {
                kind == RECOMMENDATION_TYPE_DKIM && status == RECOMMENDATION_STATUS_OPEN
            }),
        ),
        (
            "a status and an impact together".to_string(),
            json!({"Filter": {"STATUS": RECOMMENDATION_STATUS_FIXED, "IMPACT": RECOMMENDATION_IMPACT_HIGH}}),
            listing_where(|(_, _, status)| status == RECOMMENDATION_STATUS_FIXED),
        ),
    ];
    for kind in RECOMMENDATION_TYPE_VALUES {
        cases.push((
            format!("the type {kind}"),
            json!({"Filter": {"TYPE": kind}}),
            listing_where(|(_, reported, _)| reported == kind),
        ));
    }
    for status in RECOMMENDATION_STATUS_VALUES {
        cases.push((
            format!("the status {status}"),
            json!({"Filter": {"STATUS": status}}),
            listing_where(|(_, _, reported)| reported == status),
        ));
    }
    for impact in RECOMMENDATION_IMPACT_VALUES {
        cases.push((
            format!("the impact {impact}"),
            json!({"Filter": {"IMPACT": impact}}),
            if *impact == RECOMMENDATION_IMPACT_HIGH {
                whole_listing()
            } else {
                Vec::new()
            },
        ));
    }

    for (case, body, expected) in cases {
        let listed = ses.list(body).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected the listing to be answered, got {} {:?}",
            listed.status,
            listed.message()
        );
        assert_eq!(
            listed.findings(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.findings()
        );
    }
}

#[tokio::test]
async fn a_page_is_a_window_over_the_whole_listing() {
    let ses = two_identities().await;
    let whole = whole_listing();
    // Every page size the listing can be walked with, from one finding per page to more
    // than the listing holds, plus the documented ceiling.
    for size in [1_usize, 2, 3, 4, 5, 100] {
        let case = format!("page size {size}");
        let mut walked: Vec<Finding> = Vec::new();
        let mut token: Option<String> = None;
        // One more request than pages, so a token that never clears is caught rather
        // than looping forever.
        for _ in 0..=whole.len() + 1 {
            let mut body = json!({"PageSize": size});
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let page = ses.list(body).await;
            assert_eq!(
                page.status,
                200,
                "case {case:?}: expected the page to be answered, got {} {:?}",
                page.status,
                page.message()
            );
            let findings = page.findings();
            assert!(
                findings.len() <= size,
                "case {case:?}: expected at most {size} finding(s) in a page, got {:?}",
                findings
            );
            walked.extend(findings);
            token = page.next_token();
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to terminate, got the further token {token:?}"
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the walk to report {whole:?} exactly once each, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_account_holding_no_identity_has_no_findings() {
    let ses = Ses::new();
    let listed = ses.list(json!({})).await;
    assert_eq!(
        listed.status,
        200,
        "case empty account: expected the listing to be answered, got {} {:?}",
        listed.status,
        listed.message()
    );
    assert_eq!(
        listed.findings(),
        Vec::new(),
        "case empty account: expected no findings, got {:?}",
        listed.findings()
    );
    assert_eq!(
        listed.next_token(),
        None,
        "case empty account: expected no further page, got {:?}",
        listed.next_token()
    );
}

#[tokio::test]
async fn a_request_this_operation_cannot_read_is_refused() {
    let ses = two_identities().await;
    let whole = whole_listing();
    // `(case, request body, the message the refusal must carry)`.
    let cases: Vec<(String, Value, String)> = vec![
        (
            "a type outside the modelled value set".to_string(),
            json!({"Filter": {"TYPE": "AUTHENTICATION"}}),
            "Invalid recommendation TYPE specified".to_string(),
        ),
        (
            "a modelled type spelled in the wrong case".to_string(),
            json!({"Filter": {"TYPE": "dkim"}}),
            "Invalid recommendation TYPE specified".to_string(),
        ),
        (
            "an impact outside the modelled value set".to_string(),
            json!({"Filter": {"IMPACT": "MEDIUM"}}),
            "Invalid recommendation IMPACT specified".to_string(),
        ),
        (
            "a status outside the modelled value set".to_string(),
            json!({"Filter": {"STATUS": "CLOSED"}}),
            "Invalid recommendation STATUS specified".to_string(),
        ),
        (
            "a resource that is not an ARN at all".to_string(),
            json!({"Filter": {"RESOURCE_ARN": "alpha.example.com"}}),
            "Filter resource is invalid".to_string(),
        ),
        (
            "a resource ARN of another vendor".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:s3:::{SETTLED}")}}),
            "Filter resource is invalid".to_string(),
        ),
        (
            "a resource ARN naming a type but no resource".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:ses:{DEFAULT_REGION}:{ACCOUNT_ID}:identity")}}),
            "Filter resource is invalid".to_string(),
        ),
        (
            "a resource ARN whose resource is empty".to_string(),
            json!({"Filter": {"RESOURCE_ARN": format!("arn:aws:ses:{DEFAULT_REGION}:{ACCOUNT_ID}:identity/")}}),
            "Filter resource is invalid".to_string(),
        ),
        (
            "a page size below the documented range".to_string(),
            json!({"PageSize": 0}),
            "PageSize 0 is outside the supported range of 1 to 100.".to_string(),
        ),
        (
            "a page size that is not a count of items at all".to_string(),
            json!({"PageSize": -1}),
            "PageSize -1 is outside the supported range of 1 to 100.".to_string(),
        ),
        (
            "a page size above the documented range".to_string(),
            json!({"PageSize": 101}),
            "PageSize 101 is outside the supported range of 1 to 100.".to_string(),
        ),
        (
            "a token this listing never handed out".to_string(),
            json!({"NextToken": "bm90LWEtdG9rZW4="}),
            "Invalid next token.".to_string(),
        ),
        (
            "a token another listing of this service handed out".to_string(),
            json!({"NextToken": ::aws_smithy_types::base64::encode(format!("{CONTACT_LISTS_PAGE_TOKEN_TAG}{SETTLED}"))}),
            "Invalid next token.".to_string(),
        ),
    ];
    for (case, body, expected) in cases {
        let refused = ses.list(body).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            refused.message()
        );
    }

    // Each documented page-size bound itself is accepted, and the refusals above left
    // the listing exactly as it was.
    for size in [1_usize, 100] {
        let case = format!("the bound {size}");
        let accepted = ses.list(json!({"PageSize": size})).await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: expected the bound to be accepted, got {} {:?}",
            accepted.status,
            accepted.message()
        );
    }
    let listed = ses.list(json!({})).await;
    assert_eq!(
        listed.findings(),
        whole,
        "case after the refusals: expected {whole:?}, got {:?}",
        listed.findings()
    );
}
