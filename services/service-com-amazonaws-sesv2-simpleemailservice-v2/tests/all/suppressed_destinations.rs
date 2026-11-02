// SPDX-License-Identifier: CC0-1.0

//! The suppression-list read and listing, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: `GetSuppressedDestination` reports the suppression-list
//! entry the request names, taken from the list the request selects — a tenant's when
//! it carries `TenantName`, the account-level list when it omits it — with every
//! member computed from the stored record, and refuses with a terminal 404
//! `NotFoundException` when that list holds no entry for the address. The tables below
//! span what the rule generalises over: each `SuppressionListReason` value, every
//! combination of the optional attributes, both kinds of list, the spelling of the
//! address, each way a lookup can miss, and the `TenantName` values a tenant list
//! cannot be read for.
//!
//! The rule the listing cases pin: `ListSuppressedDestinations` reports a summary per
//! entry of the list the request selects — the same two lists the read selects between
//! — narrowed to the `Reasons` the request names and to the window its `StartDate` and
//! `EndDate` bound, in one deterministic order, cut into pages of at most `PageSize`
//! entries whose `NextToken` resumes the walk strictly after the last entry returned,
//! and refuses a `NextToken` this listing never minted with a terminal 400
//! `InvalidNextTokenException`, and a window whose `EndDate` precedes its `StartDate`
//! with a terminal 400 `BadRequestException` naming both bounds. Those tables span the
//! selections, each reason subset,
//! each side of the window and its bounds, every page size from one to past the end,
//! and the ways a token can fail to be a position in this listing.
//!
//! The rule the write cases pin: `PutSuppressedDestination` puts the address the
//! request names on the list it selects — a tenant's when it carries `TenantName`, the
//! account-level list otherwise — carrying the `Reason` the request supplies and the
//! instant of the write, replacing whatever entry that list already held for the
//! address, and refuses the write with a terminal 400 `BadRequestException` while the
//! account has not been granted production access, leaving every list untouched. Those
//! tables span both reasons, both kinds of list, the spellings of an address, a
//! repeated write of one address, and each way a write is refused. A written entry is
//! also reported by the listing of the list the write targeted — narrowed to the
//! reason the write supplied — and by no other list's, over rows differing in the list
//! targeted, the number of addresses written to it and the reasons they carry.
//!
//! One further rule, under all of those: an entry's storage key names exactly one
//! (list, address) pair and carries its own list's prefix and no other's — whatever
//! either part of the pair holds — which is what keeps one list's entries, and one
//! listing's page tokens, out of another's.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::SUPPRESSION_LIST_REASON_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, identity_key, suppression_key, suppression_list_prefix,
    ComAmazonawsSesv2SimpleemailserviceV2State, SuppressedDestinationRecord,
    CONTACTS_PAGE_TOKEN_TAG, MAX_TENANT_NAME_CHARS,
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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the body reached by its path through the nested structures, or
    /// `Null` when any step is absent.
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

    /// A number at `path`, or `None` when the member is absent.
    fn number_at(&self, path: &[&str]) -> Option<f64> {
        self.at(path).as_f64()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// persist the suppression-list entries it is about and read back what the handler
/// left behind.
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

    /// `GetSuppressedDestination`: the address as a path label, and the tenant whose
    /// list to read as the `TenantName` query parameter when the case names one.
    async fn get_suppressed(&self, address: &str, tenant: Option<&str>) -> Reply {
        let query = match tenant {
            Some(tenant) => format!("?TenantName={tenant}"),
            None => String::new(),
        };
        self.call(
            "GET",
            &format!("/v2/email/suppression/addresses/{address}{query}"),
            None,
        )
        .await
    }

    /// `ListSuppressedDestinations`: every filter a case names, as query parameters.
    async fn list_suppressed(&self, query: &[(&str, String)]) -> Reply {
        let mut path = String::from("/v2/email/suppression/addresses");
        for (index, (name, value)) in query.iter().enumerate() {
            path.push(if index == 0 { '?' } else { '&' });
            path.push_str(name);
            path.push('=');
            path.push_str(&crate::contact_lists::query_escaped(value));
        }
        self.call("GET", &path, None).await
    }

    /// `CreateTenant`, so a case that reads a tenant's suppression list targets a
    /// tenant this account really holds.
    async fn create_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    /// Create every tenant a case's rows name, asserting each create was accepted.
    async fn create_tenants(&self, names: &[&str]) {
        for name in names {
            let created = self.create_tenant(name).await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: expected the tenant to be created, got {} {:?}",
                created.status, created.body
            );
        }
    }

    /// Persist one suppression-list entry, through the one gated path a suppression
    /// write takes, so the bound that path enforces is exercised here too.
    fn seed(&self, record: &SuppressedDestinationRecord) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let stored = store.suppress_destination(
            suppression_key(record.tenant_name.as_deref(), &record.email_address),
            record.clone(),
        );
        assert!(
            stored,
            "case {:?}: expected the suppression-list entry to be stored",
            record.email_address
        );
    }

    /// `PutSuppressedDestination`: the address and the reason in the request body,
    /// plus the tenant whose list to write to when the case names one.
    async fn put_suppressed(&self, address: &str, reason: &str, tenant: Option<&str>) -> Reply {
        let mut body = json!({"EmailAddress": address, "Reason": reason});
        if let Some(tenant) = tenant {
            body["TenantName"] = json!(tenant);
        }
        self.call("PUT", "/v2/email/suppression/addresses", Some(body))
            .await
    }

    /// Grant this account production access, which is what takes it out of the
    /// sandbox.
    ///
    /// It is set on the one account record every account operation reads and writes
    /// (the record `GetAccount` reports `ProductionAccessEnabled` from), so a case
    /// that needs a suppression write to be permitted grants it here.
    fn grant_production_access(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().account.production_access_enabled = true;
    }

    /// Every stored entry, as `(key, reason, address)`, so a case can show that a
    /// read left the table exactly as it found it.
    fn stored(&self) -> Vec<(String, String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .suppressed_destinations
            .iter()
            .map(|(key, record)| {
                (
                    key.clone(),
                    record.reason.clone(),
                    record.email_address.clone(),
                )
            })
            .collect()
    }
}

/// An instant `seconds_ago` before now, truncated to whole seconds so the modelled
/// epoch-seconds timestamp a response carries compares exactly.
fn moment(seconds_ago: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now is after the epoch");
    SystemTime::UNIX_EPOCH + Duration::from_secs(now.as_secs() - seconds_ago)
}

/// The epoch seconds of `at`, which is what a restJson1 timestamp member carries.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// One row of the read table: the entry to persist and the lookup to make.
struct ReadCase {
    name: &'static str,
    record: SuppressedDestinationRecord,
    /// The address the request spells, which need not be the stored spelling.
    lookup: String,
}

/// Assert the reported entry mirrors the stored record, member by member.
fn assert_mirrors(case: &str, record: &SuppressedDestinationRecord, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the suppression-list read, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "EmailAddress"]),
        Some(record.email_address.clone()),
        "case {case:?}: expected EmailAddress {:?}, got {:?}",
        record.email_address,
        reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "Reason"]),
        Some(record.reason.clone()),
        "case {case:?}: expected Reason {:?}, got {:?}",
        record.reason,
        reply.body
    );
    assert_eq!(
        reply.number_at(&["SuppressedDestination", "LastUpdateTime"]),
        Some(epoch_seconds(record.last_update_time)),
        "case {case:?}: expected LastUpdateTime {:?}, got {:?}",
        epoch_seconds(record.last_update_time),
        reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "TenantName"]),
        record.tenant_name.clone(),
        "case {case:?}: expected TenantName {:?}, got {:?}",
        record.tenant_name,
        reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "Attributes", "MessageId"]),
        record.message_id.clone(),
        "case {case:?}: expected Attributes.MessageId {:?}, got {:?}",
        record.message_id,
        reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "Attributes", "FeedbackId"]),
        record.feedback_id.clone(),
        "case {case:?}: expected Attributes.FeedbackId {:?}, got {:?}",
        record.feedback_id,
        reply.body
    );
    // "An optional value that can contain additional information about the reasons
    // that the address was added": an entry that carries neither identifier reports
    // no `Attributes` object at all.
    let attributes = reply.at(&["SuppressedDestination", "Attributes"]);
    let expected_attributes = record.message_id.is_some() || record.feedback_id.is_some();
    assert_eq!(
        !attributes.is_null(),
        expected_attributes,
        "case {case:?}: expected the Attributes member {}, got {attributes:?}",
        if expected_attributes {
            "present"
        } else {
            "absent"
        }
    );
}

/// `at` as the instant a `@httpQuery` timestamp carries on the wire.
fn date_time(at: SystemTime) -> String {
    aws_smithy_types::DateTime::from(at)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant the modelled timestamp format can carry")
}

/// The addresses a listing reported, in the order it reported them.
fn listed(reply: &Reply) -> Vec<String> {
    reply
        .at(&["SuppressedDestinationSummaries"])
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("EmailAddress")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The two `SuppressionListReason` values the model declares, taken from the catalog
/// rather than spelled out here:
/// `smithy_query(query="shape",shape="SuppressionListReason")`.
fn modelled_reasons() -> (&'static str, &'static str) {
    match SUPPRESSION_LIST_REASON_VALUES {
        [first, second] => (*first, *second),
        other => panic!("the SuppressionListReason catalog changed shape: {other:?}"),
    }
}

/// A stored entry, with the members a row does not vary left unset.
fn entry(address: &str, reason: &str, at: SystemTime) -> SuppressedDestinationRecord {
    SuppressedDestinationRecord {
        email_address: address.to_string(),
        reason: reason.to_string(),
        last_update_time: at,
        message_id: None,
        feedback_id: None,
        tenant_name: None,
    }
}

#[tokio::test]
async fn every_stored_suppression_entry_reads_back_member_for_member() {
    // The `SuppressionListReason` values the model declares, so each row carries a
    // reason from the catalog rather than one spelled out here:
    // `smithy_query(query="shape",shape="SuppressionListReason")`.
    let [bounce, complaint] = match SUPPRESSION_LIST_REASON_VALUES {
        [first, second] => [*first, *second],
        other => panic!("the SuppressionListReason catalog changed shape: {other:?}"),
    };
    assert_eq!(
        (bounce, complaint),
        ("BOUNCE", "COMPLAINT"),
        "case catalog: expected the two modelled suppression reasons, got {SUPPRESSION_LIST_REASON_VALUES:?}"
    );

    let cases = [
        ReadCase {
            name: "account list, bounce, no attributes",
            record: entry("plain-bounce@example.com", bounce, moment(3_600)),
            lookup: "plain-bounce@example.com".to_string(),
        },
        ReadCase {
            name: "account list, complaint, both attributes",
            record: SuppressedDestinationRecord {
                message_id: Some("0100018f0d1c0a2b-11112222-000000".to_string()),
                feedback_id: Some("0100018f0d1c0a2b-33334444-000000".to_string()),
                ..entry("full-attributes@example.com", complaint, moment(10))
            },
            lookup: "full-attributes@example.com".to_string(),
        },
        ReadCase {
            name: "account list, bounce, message id only",
            record: SuppressedDestinationRecord {
                message_id: Some("0100018f0d1c0a2b-55556666-000000".to_string()),
                ..entry("message-only@example.com", bounce, moment(86_400))
            },
            lookup: "message-only@example.com".to_string(),
        },
        ReadCase {
            name: "account list, complaint, feedback id only",
            record: SuppressedDestinationRecord {
                feedback_id: Some("0100018f0d1c0a2b-77778888-000000".to_string()),
                ..entry("feedback-only@example.com", complaint, moment(1))
            },
            lookup: "feedback-only@example.com".to_string(),
        },
        ReadCase {
            name: "tenant list, complaint",
            record: SuppressedDestinationRecord {
                tenant_name: Some("tenant_one".to_string()),
                ..entry("on-a-tenant-list@example.com", complaint, moment(120))
            },
            lookup: "on-a-tenant-list@example.com".to_string(),
        },
        ReadCase {
            name: "tenant list, bounce, both attributes",
            record: SuppressedDestinationRecord {
                tenant_name: Some("tenant-two".to_string()),
                message_id: Some("0100018f0d1c0a2b-9999aaaa-000000".to_string()),
                feedback_id: Some("0100018f0d1c0a2b-bbbbcccc-000000".to_string()),
                ..entry("tenant-with-attributes@example.com", bounce, moment(7_200))
            },
            lookup: "tenant-with-attributes@example.com".to_string(),
        },
        ReadCase {
            name: "the domain is matched without regard to case",
            record: entry("Mixed.Case@Example.COM", bounce, moment(30)),
            lookup: "Mixed.Case@EXAMPLE.com".to_string(),
        },
        ReadCase {
            name: "the address label arrives percent-encoded",
            record: entry("encoded@example.com", complaint, moment(45)),
            lookup: "encoded%40example.com".to_string(),
        },
        ReadCase {
            name: "a long local part",
            record: entry(
                "a-very-long-local-part-that-a-mailbox-may-well-have@sub.domain.example.org",
                bounce,
                moment(5),
            ),
            lookup: "a-very-long-local-part-that-a-mailbox-may-well-have@sub.domain.example.org"
                .to_string(),
        },
    ];

    let ses = Ses::new();
    // The tenant rows read a tenant's own suppression list, so the tenants they
    // name are created through `CreateTenant` first: a tenant list belongs to a
    // tenant the account really holds.
    ses.create_tenants(&["tenant_one", "tenant-two"]).await;
    for case in &cases {
        ses.seed(&case.record);
    }
    // Every row is read from the one table holding every row, so a lookup that
    // answered from anything but the entry it names would show up here.
    for case in &cases {
        let reply = ses
            .get_suppressed(&case.lookup, case.record.tenant_name.as_deref())
            .await;
        assert_mirrors(case.name, &case.record, &reply);
    }
}

/// One row of the refusal table: what is stored, and the lookup that must miss it.
struct MissCase {
    name: &'static str,
    stored: Vec<SuppressedDestinationRecord>,
    lookup: &'static str,
    tenant: Option<&'static str>,
}

#[tokio::test]
async fn an_address_absent_from_the_named_list_is_refused_as_not_found() {
    let account_entry = entry("suppressed@example.com", "BOUNCE", moment(60));
    let tenant_entry = SuppressedDestinationRecord {
        tenant_name: Some("tenant_one".to_string()),
        ..entry("suppressed@example.com", "COMPLAINT", moment(60))
    };

    let cases = [
        MissCase {
            name: "nothing is suppressed at all",
            stored: Vec::new(),
            lookup: "nobody@example.com",
            tenant: None,
        },
        MissCase {
            name: "another address is suppressed on the account list",
            stored: vec![account_entry.clone()],
            lookup: "someone-else@example.com",
            tenant: None,
        },
        MissCase {
            name: "the entry is on a tenant list, the read targets the account list",
            stored: vec![tenant_entry.clone()],
            lookup: "suppressed@example.com",
            tenant: None,
        },
        MissCase {
            name: "the entry is on the account list, the read targets a tenant list",
            stored: vec![account_entry.clone()],
            lookup: "suppressed@example.com",
            tenant: Some("tenant_one"),
        },
        MissCase {
            name: "the entry is on another tenant's list",
            stored: vec![tenant_entry.clone()],
            lookup: "suppressed@example.com",
            tenant: Some("tenant-two"),
        },
        MissCase {
            name: "the local part differs in case, which belongs to the mailbox",
            stored: vec![entry("Casey@example.com", "BOUNCE", moment(60))],
            lookup: "casey@example.com",
            tenant: None,
        },
        MissCase {
            name: "the address has no local part at all",
            stored: vec![account_entry.clone()],
            lookup: "example.com",
            tenant: None,
        },
    ];

    for case in &cases {
        let ses = Ses::new();
        ses.create_tenants(&["tenant_one", "tenant-two"]).await;
        for record in &case.stored {
            ses.seed(record);
        }
        let reply = ses.get_suppressed(case.lookup, case.tenant).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException, got {} {} {:?}",
            case.name,
            reply.status,
            reply.error_code(),
            reply.body
        );
        // The refusal names the address the request asked about, taken from the
        // request rather than from anything stored.
        assert_eq!(
            reply.message(),
            format!(
                "Email address {} does not exist on your suppression list.",
                case.lookup
            ),
            "case {:?}: expected the refusal to name {:?}, got {:?}",
            case.name,
            case.lookup,
            reply.message()
        );
    }
}

/// One row of the tenant table: the `TenantName` the request carries, and the
/// refusal it must draw.
struct TenantCase {
    name: &'static str,
    tenant: String,
    status: u16,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_tenant_the_account_does_not_hold_is_refused_before_the_list_is_read() {
    // The documented `TenantName` form: "The name can contain up to 64 alphanumeric
    // characters, including letters, numbers, hyphens (-) and underscores (_) only"
    // (`smithy_query(query="shape",shape="TenantName")`), so `MAX_TENANT_NAME_CHARS`
    // characters is the last accepted length and one more the first refused one.
    let longest = "t".repeat(MAX_TENANT_NAME_CHARS);
    let too_long = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    let cases = [
        TenantCase {
            name: "a well-formed tenant the account never created",
            tenant: "absent_tenant".to_string(),
            status: 404,
            code: "NotFoundException",
            message: "The requested tenant <absent_tenant> does not exist.".to_string(),
        },
        TenantCase {
            name: "a name at the documented length bound the account never created",
            tenant: longest.clone(),
            status: 404,
            code: "NotFoundException",
            message: format!("The requested tenant <{longest}> does not exist."),
        },
        TenantCase {
            name: "a name one character past the documented length bound",
            tenant: too_long.clone(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: {too_long}. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
        TenantCase {
            name: "a name carrying a character the form excludes",
            tenant: "tenant.one".to_string(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: tenant.one. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
    ];

    let ses = Ses::new();
    ses.create_tenants(&["tenant_one"]).await;
    // The address IS suppressed on the tenant list that exists, so each row below is
    // refused for the tenant it names and not for the address.
    ses.seed(&SuppressedDestinationRecord {
        tenant_name: Some("tenant_one".to_string()),
        ..entry("shared@example.com", "BOUNCE", moment(15))
    });
    for case in &cases {
        let reply = ses
            .get_suppressed("shared@example.com", Some(case.tenant.as_str()))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (case.status, case.code),
            "case {:?}: expected {} {}, got {} {} {:?}",
            case.name,
            case.status,
            case.code,
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {:?}: expected the refusal to read {:?}, got {:?}",
            case.name,
            case.message,
            reply.message()
        );
    }
    // The control: the same address on the tenant list the account does hold.
    let served = ses
        .get_suppressed("shared@example.com", Some("tenant_one"))
        .await;
    assert_eq!(
        served.status, 200,
        "case {:?}: expected 200 for the tenant the account holds, got {} {:?}",
        "an existing tenant", served.status, served.body
    );
}

#[tokio::test]
async fn reading_a_suppression_entry_changes_nothing_a_later_caller_sees() {
    let ses = Ses::new();
    ses.create_tenants(&["tenant_one"]).await;
    let stored = [
        entry("read-twice@example.com", "BOUNCE", moment(90)),
        SuppressedDestinationRecord {
            tenant_name: Some("tenant_one".to_string()),
            ..entry("read-twice@example.com", "COMPLAINT", moment(90))
        },
    ];
    for record in &stored {
        ses.seed(record);
    }
    let before = ses.stored();

    for record in &stored {
        let first = ses
            .get_suppressed(&record.email_address, record.tenant_name.as_deref())
            .await;
        assert_mirrors("repeat read", record, &first);
        // A missed lookup in between must not create the entry it failed to find.
        let missed = ses
            .get_suppressed("never-suppressed@example.com", record.tenant_name.as_deref())
            .await;
        assert_eq!(
            missed.status, 404,
            "case {:?}: expected 404 for an address nothing suppressed, got {} {:?}",
            "repeat read", missed.status, missed.body
        );
        let second = ses
            .get_suppressed(&record.email_address, record.tenant_name.as_deref())
            .await;
        assert_eq!(
            second.body, first.body,
            "case {:?}: expected the second read to answer exactly what the first did ({:?}), got {:?}",
            record.email_address, first.body, second.body
        );
    }

    assert_eq!(
        ses.stored(),
        before,
        "case {:?}: expected the suppression table untouched by the reads, got {:?}",
        "repeat read",
        ses.stored()
    );
}

/// The tenants the listing fixture holds, and the entries on each list.
///
/// The same table backs every listing case, so a case that answered from another
/// list, or from more than the list it selected, shows up as a different set of
/// addresses.
const TENANTS: [&str; 3] = ["tenant_one", "tenant-two", "tenant_empty"];

/// Every entry the listing fixture persists: the account-level list, two tenant
/// lists, and a tenant holding nothing. The addresses are spelled so that the
/// storage order the listing pages over is also their alphabetical order, which is
/// what each case's expected sequence is written in.
///
/// Every instant is derived from the ONE `base` the case captured, never from a
/// fresh clock reading: a row's stored instant and the window bound a query asks
/// about have to be the same instant, and [`moment`] truncates to whole seconds, so
/// reading the clock twice could put them a second apart.
fn fixture(base: SystemTime) -> Vec<SuppressedDestinationRecord> {
    vec![
        entry("alpha@example.com", "BOUNCE", before(base, 400)),
        entry("bravo@example.com", "COMPLAINT", before(base, 300)),
        entry("charlie@example.com", "BOUNCE", before(base, 200)),
        entry("delta@example.com", "COMPLAINT", before(base, 100)),
        SuppressedDestinationRecord {
            tenant_name: Some("tenant_one".to_string()),
            ..entry("alpha@example.com", "COMPLAINT", before(base, 150))
        },
        SuppressedDestinationRecord {
            tenant_name: Some("tenant_one".to_string()),
            ..entry("echo@example.com", "BOUNCE", before(base, 250))
        },
        SuppressedDestinationRecord {
            tenant_name: Some("tenant-two".to_string()),
            ..entry("foxtrot@example.com", "BOUNCE", before(base, 50))
        },
    ]
}

/// `seconds_ago` before the fixture's time base.
fn before(base: SystemTime, seconds_ago: u64) -> SystemTime {
    base - Duration::from_secs(seconds_ago)
}

/// The fixture, seeded behind a fresh router with every tenant it names created,
/// with the time base its instants were derived from.
async fn seeded() -> (Ses, SystemTime) {
    let base = moment(0);
    let ses = Ses::new();
    ses.create_tenants(&TENANTS).await;
    for record in &fixture(base) {
        ses.seed(record);
    }
    (ses, base)
}

/// The query parameters that select a list: a tenant's when the case names one.
fn selection(tenant: Option<&str>) -> Vec<(&'static str, String)> {
    match tenant {
        Some(tenant) => vec![("TenantName", tenant.to_string())],
        None => Vec::new(),
    }
}

/// One row of the selection table: the list to list, and the addresses on it.
struct SelectionCase {
    name: &'static str,
    tenant: Option<&'static str>,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn the_listing_reports_the_entries_of_the_list_the_request_selects() {
    let cases = [
        SelectionCase {
            name: "the account-level list",
            tenant: None,
            expected: vec![
                "alpha@example.com",
                "bravo@example.com",
                "charlie@example.com",
                "delta@example.com",
            ],
        },
        SelectionCase {
            name: "a tenant list holding two entries, one of them also on the account list",
            tenant: Some("tenant_one"),
            expected: vec!["alpha@example.com", "echo@example.com"],
        },
        SelectionCase {
            name: "a tenant list holding one entry",
            tenant: Some("tenant-two"),
            expected: vec!["foxtrot@example.com"],
        },
        SelectionCase {
            name: "a tenant list holding nothing",
            tenant: Some("tenant_empty"),
            expected: Vec::new(),
        },
    ];

    let (ses, base) = seeded().await;
    for case in &cases {
        let reply = ses.list_suppressed(&selection(case.tenant)).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the listing, got {} {:?}",
            case.name, reply.status, reply.body
        );
        assert_eq!(
            listed(&reply),
            case.expected,
            "case {:?}: expected the listing to report {:?}, got {:?}",
            case.name,
            case.expected,
            listed(&reply)
        );
        // The member is always there, empty list and all: a list holding nothing is
        // reported as an empty listing, not as a response that omits it.
        assert!(
            reply.at(&["SuppressedDestinationSummaries"]).is_array(),
            "case {:?}: expected the SuppressedDestinationSummaries member present, got {:?}",
            case.name,
            reply.body
        );
        // Nothing remains behind a listing that reported its whole list.
        assert_eq!(
            reply.text_at(&["NextToken"]),
            None,
            "case {:?}: expected no NextToken on a complete listing, got {:?}",
            case.name,
            reply.body
        );
        // Every summary is the stored entry's projection: the three members
        // `SuppressedDestinationSummary` models, each taken from the record.
        let stored: Vec<SuppressedDestinationRecord> = fixture(base)
            .into_iter()
            .filter(|record| record.tenant_name.as_deref() == case.tenant)
            .collect();
        let summaries = reply.at(&["SuppressedDestinationSummaries"]);
        for (index, record) in stored.iter().enumerate() {
            let summary = summaries
                .get(index)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                summary.get("Reason").and_then(Value::as_str),
                Some(record.reason.as_str()),
                "case {:?}: expected Reason {:?} for {:?}, got {summary:?}",
                case.name,
                record.reason,
                record.email_address
            );
            assert_eq!(
                summary.get("LastUpdateTime").and_then(Value::as_f64),
                Some(epoch_seconds(record.last_update_time)),
                "case {:?}: expected LastUpdateTime {:?} for {:?}, got {summary:?}",
                case.name,
                epoch_seconds(record.last_update_time),
                record.email_address
            );
            let mut members: Vec<String> = summary
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default();
            members.sort();
            assert_eq!(
                members,
                vec![
                    "EmailAddress".to_string(),
                    "LastUpdateTime".to_string(),
                    "Reason".to_string()
                ],
                "case {:?}: expected a summary to carry exactly the modelled members, got {members:?}",
                case.name
            );
        }
    }
}

/// One row of the filter table: the query the request carries, and the addresses of
/// the account-level list it must leave.
struct FilterCase {
    name: &'static str,
    query: Vec<(&'static str, String)>,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn the_listing_narrows_to_the_reasons_and_the_window_the_request_names() {
    // The reasons come from the modelled catalog rather than from literals here.
    let (bounce, complaint) = modelled_reasons();
    // Seeded first, so every window bound below is derived from the SAME instant the
    // stored entries were.
    let (ses, base) = seeded().await;
    let cases = [
        FilterCase {
            name: "no filter at all",
            query: Vec::new(),
            expected: vec![
                "alpha@example.com",
                "bravo@example.com",
                "charlie@example.com",
                "delta@example.com",
            ],
        },
        FilterCase {
            name: "one reason",
            query: vec![("Reason", bounce.to_string())],
            expected: vec!["alpha@example.com", "charlie@example.com"],
        },
        FilterCase {
            name: "the other reason",
            query: vec![("Reason", complaint.to_string())],
            expected: vec!["bravo@example.com", "delta@example.com"],
        },
        FilterCase {
            name: "both reasons, which excludes nothing",
            query: vec![
                ("Reason", bounce.to_string()),
                ("Reason", complaint.to_string()),
            ],
            expected: vec![
                "alpha@example.com",
                "bravo@example.com",
                "charlie@example.com",
                "delta@example.com",
            ],
        },
        FilterCase {
            name: "a start date only",
            query: vec![("StartDate", date_time(before(base, 250)))],
            expected: vec!["charlie@example.com", "delta@example.com"],
        },
        FilterCase {
            name: "an end date only",
            query: vec![("EndDate", date_time(before(base, 250)))],
            expected: vec!["alpha@example.com", "bravo@example.com"],
        },
        FilterCase {
            name: "both ends of the window",
            query: vec![
                ("StartDate", date_time(before(base, 350))),
                ("EndDate", date_time(before(base, 150))),
            ],
            expected: vec!["bravo@example.com", "charlie@example.com"],
        },
        FilterCase {
            name: "a start date exactly at an entry's last update",
            query: vec![("StartDate", date_time(before(base, 300)))],
            expected: vec![
                "bravo@example.com",
                "charlie@example.com",
                "delta@example.com",
            ],
        },
        FilterCase {
            name: "an end date exactly at an entry's last update",
            query: vec![("EndDate", date_time(before(base, 300)))],
            expected: vec!["alpha@example.com", "bravo@example.com"],
        },
        FilterCase {
            name: "a window beginning after every entry",
            query: vec![("StartDate", date_time(before(base, 10)))],
            expected: Vec::new(),
        },
        FilterCase {
            name: "a window ending before every entry",
            query: vec![("EndDate", date_time(before(base, 500)))],
            expected: Vec::new(),
        },
        FilterCase {
            name: "a window as wide as one second",
            query: vec![
                ("StartDate", date_time(before(base, 301))),
                ("EndDate", date_time(before(base, 300))),
            ],
            expected: vec!["bravo@example.com"],
        },
        FilterCase {
            name: "a reason and a window together",
            query: vec![
                ("Reason", complaint.to_string()),
                ("StartDate", date_time(before(base, 350))),
                ("EndDate", date_time(before(base, 150))),
            ],
            expected: vec!["bravo@example.com"],
        },
    ];

    for case in &cases {
        let reply = ses.list_suppressed(&case.query).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the listing, got {} {:?}",
            case.name, reply.status, reply.body
        );
        assert_eq!(
            listed(&reply),
            case.expected,
            "case {:?}: expected the listing to report {:?}, got {:?}",
            case.name,
            case.expected,
            listed(&reply)
        );
    }

    // The same filters select on a tenant's list, not only on the account's.
    let tenant_bounce = ses
        .list_suppressed(&[
            ("TenantName", "tenant_one".to_string()),
            ("Reason", bounce.to_string()),
        ])
        .await;
    assert_eq!(
        listed(&tenant_bounce),
        vec!["echo@example.com".to_string()],
        "case {:?}: expected the tenant list narrowed to one reason, got {:?}",
        "a reason on a tenant list",
        tenant_bounce.body
    );
}

#[tokio::test]
async fn a_page_walk_reports_every_entry_once_and_terminates() {
    let (bounce, _) = modelled_reasons();
    let whole = vec![
        "alpha@example.com".to_string(),
        "bravo@example.com".to_string(),
        "charlie@example.com".to_string(),
        "delta@example.com".to_string(),
    ];
    let (ses, _base) = seeded().await;

    // One page size per row: every size from one entry to one past the whole list,
    // plus the non-positive sizes, which bound nothing.
    for page_size in [1_i64, 2, 3, 4, 5, 0, -1] {
        let case = format!("PageSize={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = vec![("PageSize", page_size.to_string())];
            if let Some(token) = &token {
                query.push(("NextToken", token.clone()));
            }
            let reply = ses.list_suppressed(&query).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 from page {pages}, got {} {:?}",
                reply.status, reply.body
            );
            let page = listed(&reply);
            if page_size > 0 {
                assert!(
                    page.len() <= page_size as usize,
                    "case {case:?}: expected at most {page_size} entries on a page, got {page:?}"
                );
            } else {
                assert_eq!(
                    page, whole,
                    "case {case:?}: expected a non-positive page size to bound nothing, got {page:?}"
                );
            }
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: expected the walk to terminate, got {pages} pages and {walked:?}"
            );
            match reply.text_at(&["NextToken"]) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the walk to report every entry once, in order, got {walked:?}"
        );
    }

    // A token resumes strictly after the entry it names, whatever filters the walk
    // carries alongside it.
    let first = ses
        .list_suppressed(&[
            ("PageSize", "1".to_string()),
            ("Reason", bounce.to_string()),
        ])
        .await;
    assert_eq!(
        listed(&first),
        vec!["alpha@example.com".to_string()],
        "case {:?}: expected the first filtered page to report one entry, got {:?}",
        "a filtered walk",
        first.body
    );
    let token = first
        .text_at(&["NextToken"])
        .expect("a filtered page with an entry behind it carries a token");
    let second = ses
        .list_suppressed(&[
            ("PageSize", "1".to_string()),
            ("Reason", bounce.to_string()),
            ("NextToken", token),
        ])
        .await;
    assert_eq!(
        listed(&second),
        vec!["charlie@example.com".to_string()],
        "case {:?}: expected the token to resume after the entry it names, got {:?}",
        "a filtered walk",
        second.body
    );
    assert_eq!(
        second.text_at(&["NextToken"]),
        None,
        "case {:?}: expected the last page to carry no token, got {:?}",
        "a filtered walk",
        second.body
    );
}

#[tokio::test]
async fn a_next_token_this_listing_never_minted_is_refused() {
    let (ses, _base) = seeded().await;
    // A token this listing did mint, for the control below.
    let minted = ses
        .list_suppressed(&[("PageSize", "1".to_string())])
        .await
        .text_at(&["NextToken"])
        .expect("a first page with entries behind it carries a token");
    // A token minted on a tenant's list, which is a position in that list and in no
    // other.
    let minted_for_tenant = ses
        .list_suppressed(&[
            ("TenantName", "tenant_one".to_string()),
            ("PageSize", "1".to_string()),
        ])
        .await
        .text_at(&["NextToken"])
        .expect("a tenant page with an entry behind it carries a token");

    let cases = [
        ("a token the caller made up", "not-a-token".to_string()),
        ("an empty token", String::new()),
        (
            "a value that is not base64 at all",
            "***not base64***".to_string(),
        ),
        (
            "base64 of a value this listing never minted",
            aws_smithy_types::base64::encode("alpha@example.com"),
        ),
        (
            "a token belonging to another listing of this service",
            encode_tagged_page_token(CONTACTS_PAGE_TOKEN_TAG, "alpha@example.com"),
        ),
        (
            "a token naming a position in a tenant's list, used on the account list",
            minted_for_tenant.clone(),
        ),
    ];

    for (case, token) in &cases {
        let reply = ses
            .list_suppressed(&[("NextToken", token.clone())])
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            "Token is invalid.",
            "case {case:?}: expected the refusal to read \"Token is invalid.\", got {:?}",
            reply.message()
        );
    }

    // The control: the token this listing minted is a position in it.
    let resumed = ses.list_suppressed(&[("NextToken", minted)]).await;
    assert_eq!(
        resumed.status, 200,
        "case {:?}: expected 200 for a token this listing minted, got {} {:?}",
        "the minted token", resumed.status, resumed.body
    );
    assert_eq!(
        listed(&resumed),
        vec![
            "bravo@example.com".to_string(),
            "charlie@example.com".to_string(),
            "delta@example.com".to_string()
        ],
        "case {:?}: expected the rest of the list after the first entry, got {:?}",
        "the minted token",
        resumed.body
    );
    // And the tenant token is a position in the list it was minted on.
    let resumed_tenant = ses
        .list_suppressed(&[
            ("TenantName", "tenant_one".to_string()),
            ("NextToken", minted_for_tenant),
        ])
        .await;
    assert_eq!(
        listed(&resumed_tenant),
        vec!["echo@example.com".to_string()],
        "case {:?}: expected the rest of the tenant's list, got {} {:?}",
        "the tenant token on its own list",
        resumed_tenant.status,
        resumed_tenant.body
    );
}

/// One bound of a listing window, as the refusal of an inverted window names it: the
/// wire value the request carried, cut where the minutes end.
fn to_the_minute(at: SystemTime) -> String {
    date_time(at)
        .chars()
        .take("YYYY-MM-DDThh:mm".len())
        .collect()
}

#[tokio::test]
async fn a_listing_window_whose_end_precedes_its_start_is_refused() {
    let (ses, base) = seeded().await;

    // The rows differ along the size of the inversion and the list selected: a second
    // apart, a whole day apart, past every stored entry, and on a tenant's list.
    let inverted: [(&str, SystemTime, SystemTime, Option<&str>); 4] = [
        (
            "the end one second before the start",
            before(base, 100),
            before(base, 101),
            None,
        ),
        (
            "the end a day before the start",
            base,
            before(base, 86_400),
            None,
        ),
        (
            "both bounds before every stored entry",
            before(base, 100_000),
            before(base, 200_000),
            None,
        ),
        (
            "an inverted window on a tenant's list",
            before(base, 10),
            before(base, 500),
            Some("tenant_one"),
        ),
    ];

    for (case, start, end, tenant) in inverted {
        let mut query = selection(tenant);
        query.push(("StartDate", date_time(start)));
        query.push(("EndDate", date_time(end)));
        let reply = ses.list_suppressed(&query).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException for an inverted window, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        // The refusal names both bounds as the request gave them.
        let expected = format!(
            "Start date {} must be before end date {}",
            to_the_minute(start),
            to_the_minute(end)
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {:?}",
            reply.message()
        );
    }

    // The bounds themselves are accepted: the window is closed, so a window whose two
    // bounds are the same instant still holds the entry stamped there, and one a
    // second wide holds it too.
    let bravo = before(base, 300);
    for (case, start, end) in [
        ("the two bounds are the same instant", bravo, bravo),
        ("a window one second wide", before(base, 301), bravo),
    ] {
        let reply = ses
            .list_suppressed(&[
                ("StartDate", date_time(start)),
                ("EndDate", date_time(end)),
            ])
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 for a window that is not inverted, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            listed(&reply),
            vec!["bravo@example.com".to_string()],
            "case {case:?}: expected the entry stamped inside the window, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_tenant_the_account_does_not_hold_is_refused_before_the_list_is_listed() {
    // The same two refusals the read makes, for the same reasons: the documented
    // `TenantName` form bounds the name, and a well-formed name the account never
    // created names no list.
    let too_long = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    let longest = "t".repeat(MAX_TENANT_NAME_CHARS);
    let cases = [
        TenantCase {
            name: "a well-formed tenant the account never created",
            tenant: "absent_tenant".to_string(),
            status: 404,
            code: "NotFoundException",
            message: "The requested tenant <absent_tenant> does not exist.".to_string(),
        },
        TenantCase {
            name: "a name at the documented length bound the account never created",
            tenant: longest.clone(),
            status: 404,
            code: "NotFoundException",
            message: format!("The requested tenant <{longest}> does not exist."),
        },
        TenantCase {
            name: "a name one character past the documented length bound",
            tenant: too_long.clone(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: {too_long}. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
        TenantCase {
            name: "a name carrying a character the form excludes",
            tenant: "tenant.one".to_string(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: tenant.one. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
    ];

    let (ses, _base) = seeded().await;
    for case in &cases {
        let reply = ses.list_suppressed(&selection(Some(&case.tenant))).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (case.status, case.code),
            "case {:?}: expected {} {}, got {} {} {:?}",
            case.name,
            case.status,
            case.code,
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {:?}: expected the refusal to read {:?}, got {:?}",
            case.name,
            case.message,
            reply.message()
        );
    }
}

#[tokio::test]
async fn listing_the_suppression_list_changes_nothing_a_later_caller_sees() {
    let (bounce, _) = modelled_reasons();
    let (ses, base) = seeded().await;
    let stored_before = ses.stored();

    let queries: Vec<Vec<(&'static str, String)>> = vec![
        Vec::new(),
        vec![("TenantName", "tenant_one".to_string())],
        vec![("PageSize", "1".to_string())],
        vec![("Reason", bounce.to_string())],
        vec![("StartDate", date_time(before(base, 350)))],
    ];
    for query in &queries {
        let case = format!("{query:?}");
        let first = ses.list_suppressed(query).await;
        // A refused listing in between must leave no trace either.
        let refused = ses
            .list_suppressed(&[("NextToken", "not-a-token".to_string())])
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 for a token this listing never minted, got {} {:?}",
            refused.status, refused.body
        );
        let second = ses.list_suppressed(query).await;
        assert_eq!(
            second.body, first.body,
            "case {case:?}: expected the second listing to answer exactly what the first did ({:?}), got {:?}",
            first.body, second.body
        );
    }

    assert_eq!(
        ses.stored(),
        stored_before,
        "case {:?}: expected the suppression table untouched by the listings, got {:?}",
        "repeat listing",
        ses.stored()
    );
}

/// One row of the write table: the request the case makes, and the address the read
/// after it asks about.
struct WriteCase {
    name: &'static str,
    address: &'static str,
    reason: &'static str,
    /// The tenant whose list the write targets, or the account-level list.
    tenant: Option<&'static str>,
    /// The address the read spells, which need not be the written spelling: a lookup
    /// compares the domain without regard to case.
    lookup: &'static str,
}

/// The window one write happened in, as the epoch seconds either side of it.
///
/// A second of slack each way, so the assertion is about the stamp being the instant
/// of the write rather than about the clock's resolution.
fn window() -> f64 {
    epoch_seconds(SystemTime::now())
}

/// Assert the entry a write left is what the read after it reports, member for
/// member, with `LastUpdateTime` inside the window the write happened in.
fn assert_written(case: &WriteCase, opened: f64, closed: f64, reply: &Reply) {
    let name = case.name;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected 200 reading the written entry back, got {} {:?}",
        reply.status, reply.body
    );
    // The address is reported exactly as the write spelled it: the local part belongs
    // to the receiving mailbox.
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "EmailAddress"]),
        Some(case.address.to_string()),
        "case {name:?}: expected EmailAddress {:?}, got {:?}",
        case.address,
        reply.body
    );
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "Reason"]),
        Some(case.reason.to_string()),
        "case {name:?}: expected Reason {:?}, got {:?}",
        case.reason,
        reply.body
    );
    // `TenantName` "is present only when the suppressed destination is on a tenant's
    // suppression list", so an account-level entry reports none.
    assert_eq!(
        reply.text_at(&["SuppressedDestination", "TenantName"]),
        case.tenant.map(str::to_string),
        "case {name:?}: expected TenantName {:?}, got {:?}",
        case.tenant,
        reply.body
    );
    // No message and no feedback event caused this suppression — the caller asked for
    // it — so the entry carries no `Attributes` at all.
    let attributes = reply.at(&["SuppressedDestination", "Attributes"]);
    assert!(
        attributes.is_null(),
        "case {name:?}: expected no Attributes on an entry the caller wrote, got {attributes:?}"
    );
    let stamped = reply.number_at(&["SuppressedDestination", "LastUpdateTime"]);
    assert!(
        stamped.is_some_and(|at| at >= opened && at <= closed),
        "case {name:?}: expected LastUpdateTime within {opened}..{closed}, got {stamped:?}"
    );
}

#[tokio::test]
async fn every_accepted_suppression_write_reads_back_member_for_member() {
    let (bounce, complaint) = modelled_reasons();
    // The rows differ along what the rule generalises over: the reason, the list the
    // write selects, and the spelling of the address.
    let cases = [
        WriteCase {
            name: "account list, bounce",
            address: "written-bounce@example.com",
            reason: bounce,
            tenant: None,
            lookup: "written-bounce@example.com",
        },
        WriteCase {
            name: "account list, complaint",
            address: "written-complaint@example.com",
            reason: complaint,
            tenant: None,
            lookup: "written-complaint@example.com",
        },
        WriteCase {
            name: "tenant list, bounce",
            address: "tenant-bounce@example.com",
            reason: bounce,
            tenant: Some("tenant_one"),
            lookup: "tenant-bounce@example.com",
        },
        WriteCase {
            name: "another tenant's list, complaint",
            address: "tenant-complaint@example.com",
            reason: complaint,
            tenant: Some("tenant-two"),
            lookup: "tenant-complaint@example.com",
        },
        WriteCase {
            name: "the domain is matched without regard to case",
            address: "Mixed.Case@Example.COM",
            reason: bounce,
            tenant: None,
            lookup: "Mixed.Case@EXAMPLE.com",
        },
        WriteCase {
            name: "a plus-tagged local part",
            address: "tagged+suppression@example.com",
            reason: complaint,
            tenant: None,
            lookup: "tagged%2Bsuppression%40example.com",
        },
        WriteCase {
            name: "a long local part in a subdomain",
            address: "a-very-long-local-part-that-a-mailbox-may-well-have@sub.domain.example.org",
            reason: bounce,
            tenant: Some("tenant_one"),
            lookup: "a-very-long-local-part-that-a-mailbox-may-well-have@sub.domain.example.org",
        },
    ];

    let ses = Ses::new();
    // A write to a tenant's list targets a tenant the account really holds.
    ses.create_tenants(&["tenant_one", "tenant-two"]).await;
    // The account-wide restriction is not what these rows are about: this account
    // holds production access, so each write below is permitted.
    ses.grant_production_access();

    for case in &cases {
        let opened = window() - 1.0;
        let accepted = ses
            .put_suppressed(case.address, case.reason, case.tenant)
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200 from the suppression write, got {} {:?}",
            case.name, accepted.status, accepted.body
        );
        let closed = window() + 1.0;
        let reply = ses.get_suppressed(case.lookup, case.tenant).await;
        assert_written(case, opened, closed, &reply);
    }

    // Every row wrote one entry, and no row wrote any other: the table holds exactly
    // one entry per row.
    let stored = ses.stored();
    assert_eq!(
        stored.len(),
        cases.len(),
        "case {:?}: expected one stored entry per written row, got {stored:?}",
        "write table"
    );
}

#[tokio::test]
async fn a_repeated_suppression_write_replaces_the_one_entry_it_names() {
    let (bounce, complaint) = modelled_reasons();
    let ses = Ses::new();
    ses.create_tenants(&["tenant_one"]).await;
    ses.grant_production_access();

    // One address, written repeatedly to each of the two lists it can be on. The
    // sequences differ so the list each write landed on is visible in the reason the
    // read after it reports.
    let address = "Repeat.Writes@Example.com";
    let plan: [(&str, Option<&str>, [&str; 3]); 2] = [
        ("account list", None, [bounce, complaint, complaint]),
        ("tenant list", Some("tenant_one"), [complaint, bounce, bounce]),
    ];

    for (case, tenant, reasons) in plan {
        let mut previous_stamp = 0.0_f64;
        for reason in reasons {
            let accepted = ses.put_suppressed(address, reason, tenant).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: expected 200 from the repeated write, got {} {:?}",
                accepted.status, accepted.body
            );
            let reply = ses.get_suppressed(address, tenant).await;
            assert_eq!(
                reply.text_at(&["SuppressedDestination", "Reason"]),
                Some(reason.to_string()),
                "case {case:?}: expected the latest write to leave Reason {reason:?}, got {:?}",
                reply.body
            );
            // The entry was written again, so its `LastUpdateTime` cannot have moved
            // backwards.
            let stamp = reply
                .number_at(&["SuppressedDestination", "LastUpdateTime"])
                .unwrap_or_default();
            assert!(
                stamp >= previous_stamp,
                "case {case:?}: expected LastUpdateTime at or after the previous write's \
                 {previous_stamp}, got {stamp}"
            );
            previous_stamp = stamp;
        }
    }

    // Three writes to each of the two lists left one entry per list, each holding the
    // reason its own last write supplied: the lists do not see each other's entry.
    let stored = ses.stored();
    assert_eq!(
        stored.len(),
        2,
        "case {:?}: expected one entry per list written to, got {stored:?}",
        "repeat write"
    );
    for (case, tenant, expected) in [
        ("account list", None, complaint),
        ("tenant list", Some("tenant_one"), bounce),
    ] {
        let reply = ses.get_suppressed(address, tenant).await;
        assert_eq!(
            reply.text_at(&["SuppressedDestination", "Reason"]),
            Some(expected.to_string()),
            "case {case:?}: expected the list to still hold Reason {expected:?}, got {:?}",
            reply.body
        );
    }
}

/// One row of the sandbox table: the write a sandboxed account attempts.
struct SandboxCase {
    name: &'static str,
    address: &'static str,
    reason: &'static str,
    tenant: Option<&'static str>,
}

#[tokio::test]
async fn an_account_without_production_access_adds_no_address_to_any_suppression_list() {
    let (bounce, complaint) = modelled_reasons();
    let cases = [
        SandboxCase {
            name: "account list, bounce",
            address: "sandboxed-bounce@example.com",
            reason: bounce,
            tenant: None,
        },
        SandboxCase {
            name: "account list, complaint",
            address: "sandboxed-complaint@example.com",
            reason: complaint,
            tenant: None,
        },
        SandboxCase {
            name: "tenant list, bounce",
            address: "sandboxed-tenant@example.com",
            reason: bounce,
            tenant: Some("tenant_one"),
        },
        SandboxCase {
            name: "another tenant's list, complaint",
            address: "sandboxed-other-tenant@example.com",
            reason: complaint,
            tenant: Some("tenant-two"),
        },
        SandboxCase {
            name: "a mixed-case domain",
            address: "Sandboxed.Mixed@Example.COM",
            reason: bounce,
            tenant: None,
        },
        SandboxCase {
            name: "a plus-tagged local part",
            address: "sandboxed+tagged@example.com",
            reason: complaint,
            tenant: None,
        },
    ];

    // The restriction is account-wide, so the refusal says what the account's state is
    // and carries no part of the request: every row expects the same sentence.
    let expected = "Your account is still in the sandbox.";

    for case in &cases {
        // A fresh emulator per row: an account nobody has granted production access to
        // is in the sandbox, which is the state under test.
        let ses = Ses::new();
        ses.create_tenants(&["tenant_one", "tenant-two"]).await;

        let refused = ses
            .put_suppressed(case.address, case.reason, case.tenant)
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException, got {} {} {:?}",
            case.name,
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {:?}: expected the refusal to read {expected:?}, got {:?}",
            case.name,
            refused.message()
        );

        // The effect is absent: nothing was stored, so the read after the refusal
        // still misses.
        let stored = ses.stored();
        assert!(
            stored.is_empty(),
            "case {:?}: expected the refused write to store nothing, got {stored:?}",
            case.name
        );
        let missed = ses.get_suppressed(case.address, case.tenant).await;
        assert_eq!(
            (missed.status, missed.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException after the refused write, got {} {} {:?}",
            case.name,
            missed.status,
            missed.error_code(),
            missed.body
        );

        // The control: the very same request from an account that holds production
        // access is accepted, so what the refusal above reports is the account's
        // restriction and not anything about the request.
        ses.grant_production_access();
        let accepted = ses
            .put_suppressed(case.address, case.reason, case.tenant)
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200 once the account holds production access, got {} {:?}",
            case.name, accepted.status, accepted.body
        );
        let served = ses.get_suppressed(case.address, case.tenant).await;
        assert_eq!(
            served.status, 200,
            "case {:?}: expected the permitted write to be readable back, got {} {:?}",
            case.name, served.status, served.body
        );
    }
}

#[tokio::test]
async fn a_suppression_write_the_account_cannot_make_stores_nothing() {
    let (bounce, _complaint) = modelled_reasons();
    // The same two `TenantName` refusals the read and the listing make, for the same
    // reasons: the documented form bounds the name, and a well-formed name the account
    // never created names no list.
    let longest = "t".repeat(MAX_TENANT_NAME_CHARS);
    let too_long = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    let tenant_cases = [
        TenantCase {
            name: "a well-formed tenant the account never created",
            tenant: "absent_tenant".to_string(),
            status: 404,
            code: "NotFoundException",
            message: "The requested tenant <absent_tenant> does not exist.".to_string(),
        },
        TenantCase {
            name: "a name at the documented length bound the account never created",
            tenant: longest.clone(),
            status: 404,
            code: "NotFoundException",
            message: format!("The requested tenant <{longest}> does not exist."),
        },
        TenantCase {
            name: "a name one character past the documented length bound",
            tenant: too_long.clone(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: {too_long}. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
        TenantCase {
            name: "a name carrying a character the form excludes",
            tenant: "tenant.one".to_string(),
            status: 400,
            code: "BadRequestException",
            message: format!(
                "Invalid tenant name: tenant.one. The name can contain up to \
                 {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, numbers, \
                 hyphens (-) and underscores (_) only."
            ),
        },
    ];

    let ses = Ses::new();
    ses.create_tenants(&["tenant_one"]).await;
    // Production access is granted, so no row below is refused for the account's
    // sandbox restriction.
    ses.grant_production_access();

    for case in &tenant_cases {
        let refused = ses
            .put_suppressed("named-tenant@example.com", bounce, Some(&case.tenant))
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (case.status, case.code),
            "case {:?}: expected {} {}, got {} {} {:?}",
            case.name,
            case.status,
            case.code,
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            case.message,
            "case {:?}: expected the refusal to read {:?}, got {:?}",
            case.name,
            case.message,
            refused.message()
        );
        let stored = ses.stored();
        assert!(
            stored.is_empty(),
            "case {:?}: expected the refused write to store nothing, got {stored:?}",
            case.name
        );
    }

    // A `Reason` outside the modelled `SuppressionListReason` catalog names no reason
    // an address can be suppressed for, so the write is refused and stores nothing.
    // The catalog is the model's — `smithy_query(query="shape",
    // shape="SuppressionListReason")` — so the constraint is refused before the
    // handler runs, with the input error this operation models; what is asserted here
    // is the refusal and that no entry appeared.
    for reason in ["SUPPRESSED", "bounce", "", "BOUNCE,COMPLAINT"] {
        let refused = ses
            .put_suppressed("unmodelled-reason@example.com", reason, None)
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {reason:?}: expected 400 BadRequestException for a reason outside the modelled \
             catalog, got {} {} {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        for value in SUPPRESSION_LIST_REASON_VALUES {
            assert!(
                refused.message().contains(value),
                "case {reason:?}: expected the refusal to name the catalog value {value:?}, got \
                 {:?}",
                refused.message()
            );
        }
        let stored = ses.stored();
        assert!(
            stored.is_empty(),
            "case {reason:?}: expected the refused write to store nothing, got {stored:?}"
        );
    }

    // The control: the tenant the account does hold, and a reason from the catalog.
    let accepted = ses
        .put_suppressed("named-tenant@example.com", bounce, Some("tenant_one"))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected 200 for the tenant the account holds, got {} {:?}",
        "an existing tenant", accepted.status, accepted.body
    );
}

/// One row of the write-then-list table: the list every write of the row targets, and
/// the `(address, reason)` pairs written to it, each reason named by its position in
/// the modelled `SuppressionListReason` catalog.
///
/// Every row targets a different list, so what one row wrote is also the evidence that
/// no other row's listing reports it.
struct WriteListCase {
    name: &'static str,
    tenant: Option<&'static str>,
    writes: &'static [(&'static str, usize)],
}

/// The `(address, reason, stamp)` triples a listing reported, in the order it reported
/// them.
fn listed_entries(reply: &Reply) -> Vec<(String, String, f64)> {
    reply
        .at(&["SuppressedDestinationSummaries"])
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let text = |member: &str| {
                        item.get(member)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (
                        text("EmailAddress"),
                        text("Reason"),
                        item.get("LastUpdateTime")
                            .and_then(Value::as_f64)
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_suppression_write_is_reported_by_the_listing_of_the_list_it_targeted() {
    let (bounce, complaint) = modelled_reasons();
    let catalog = [bounce, complaint];
    // The rows differ along what the rule generalises over: which list the writes
    // target, how many addresses are written to it, which reason each write carries,
    // and the spelling of an address. The last row writes nothing, so a list of this
    // account that no write targeted reports no entry.
    let cases = [
        WriteListCase {
            name: "the account-level list, three addresses across both reasons",
            tenant: None,
            writes: &[
                ("account-bounce@example.com", 0),
                ("account-complaint@example.com", 1),
                ("Account.Mixed@Example.COM", 0),
            ],
        },
        WriteListCase {
            name: "a tenant's list, one address per reason",
            tenant: Some("tenant_one"),
            writes: &[
                ("tenant-one-bounce@example.com", 0),
                ("tenant-one-complaint@example.com", 1),
            ],
        },
        WriteListCase {
            name: "another tenant's list, a single address",
            tenant: Some("tenant-two"),
            writes: &[("tenant-two-complaint@example.com", 1)],
        },
        WriteListCase {
            name: "a tenant's list no write targeted",
            tenant: Some("tenant_empty"),
            writes: &[],
        },
    ];

    let ses = Ses::new();
    ses.create_tenants(&["tenant_one", "tenant-two", "tenant_empty"])
        .await;
    // The account-wide restriction is not what these rows are about, so this account
    // holds production access and each write below is permitted.
    ses.grant_production_access();

    let opened = window() - 1.0;
    for case in &cases {
        for (address, reason) in case.writes {
            let accepted = ses
                .put_suppressed(address, catalog[*reason], case.tenant)
                .await;
            assert_eq!(
                accepted.status, 200,
                "case {:?}: expected 200 writing {address:?}, got {} {:?}",
                case.name, accepted.status, accepted.body
            );
        }
    }
    let closed = window() + 1.0;

    for case in &cases {
        // Within one list the entries are ordered by address, so that is the order the
        // row's own writes are expected back in.
        let mut expected: Vec<(String, String)> = case
            .writes
            .iter()
            .map(|(address, reason)| ((*address).to_string(), catalog[*reason].to_string()))
            .collect();
        expected.sort_by_key(|(address, _)| identity_key(address));

        let reply = ses.list_suppressed(&selection(case.tenant)).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the listing of the written list, got {} {:?}",
            case.name, reply.status, reply.body
        );
        let reported = listed_entries(&reply);
        let pairs: Vec<(String, String)> = reported
            .iter()
            .map(|(address, reason, _)| (address.clone(), reason.clone()))
            .collect();
        assert_eq!(
            pairs, expected,
            "case {:?}: expected the listing to report exactly this list's writes {expected:?}, \
             got {pairs:?}",
            case.name
        );
        // Each reported entry is stamped with the instant of the write that made it.
        for (address, _, stamp) in &reported {
            assert!(
                *stamp >= opened && *stamp <= closed,
                "case {:?}: expected {address:?} to carry a LastUpdateTime within \
                 {opened}..{closed}, got {stamp}",
                case.name
            );
        }

        // Narrowed to one reason, the same listing reports the writes that carried it
        // and no other — on the same list, which is what shows the reason reported is
        // the one the write supplied rather than a default.
        for reason in catalog {
            let mut query = selection(case.tenant);
            query.push(("Reason", reason.to_string()));
            let narrowed = ses.list_suppressed(&query).await;
            assert_eq!(
                narrowed.status, 200,
                "case {:?}: expected 200 listing {reason:?} entries, got {} {:?}",
                case.name, narrowed.status, narrowed.body
            );
            let expected_for_reason: Vec<String> = expected
                .iter()
                .filter(|(_, carried)| carried == reason)
                .map(|(address, _)| address.clone())
                .collect();
            let got = listed(&narrowed);
            assert_eq!(
                got, expected_for_reason,
                "case {:?}: expected the {reason:?} listing to report \
                 {expected_for_reason:?}, got {got:?}",
                case.name
            );
        }

        // And every written address is readable back as the entry of this list alone:
        // the list the write did not target still misses it.
        for (address, reason) in case.writes {
            let served = ses.get_suppressed(address, case.tenant).await;
            assert_eq!(
                served.text_at(&["SuppressedDestination", "Reason"]),
                Some(catalog[*reason].to_string()),
                "case {:?}: expected {address:?} to read back with the reason its write \
                 supplied, got {} {:?}",
                case.name,
                served.status,
                served.body
            );
            let elsewhere = ses
                .get_suppressed(address, Some("tenant_empty"))
                .await
                .status;
            let expected_elsewhere = if case.tenant == Some("tenant_empty") {
                200
            } else {
                404
            };
            assert_eq!(
                elsewhere, expected_elsewhere,
                "case {:?}: expected the list the write did not target to answer \
                 {expected_elsewhere} for {address:?}, got {elsewhere}",
                case.name
            );
        }
    }
}

/// The suppression lists a storage key can belong to: the account-level list (`None`)
/// and tenant lists whose names differ along what the key must stay one-to-one over —
/// length, a shared leading character, the documented form, and names carrying the
/// separator the key joins its parts with, which a name outside the documented
/// `TenantName` form could otherwise use to reach another list's key space.
const KEYED_LISTS: &[(&str, Option<&str>)] = &[
    ("the account-level list", None),
    ("a one-character name", Some("a")),
    ("a name sharing its first character", Some("ab")),
    ("the documented form", Some("tenant-one_1")),
    ("a name carrying the key separator", Some("a\u{1f}x")),
    ("a name ending in the key separator", Some("a\u{1f}")),
    ("a name that is only the separator", Some("\u{1f}")),
];

/// Addresses that stay distinct after the key normalises them (the domain is matched
/// without regard to case, the local part exactly), including one carrying the
/// separator.
const KEYED_ADDRESSES: &[&str] = &[
    "reader@example.com",
    "other@example.com",
    "\u{1f}reader@example.com",
    "reader@example.com\u{1f}",
];

#[test]
fn a_suppression_key_names_one_list_and_one_address() {
    // One key per (list, address) pair: no two pairs may share a key, whatever either
    // part holds, or one list's entry would answer another list's read.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (case, tenant) in KEYED_LISTS {
        for address in KEYED_ADDRESSES {
            let pair = format!("{case} / {address:?}");
            let key = suppression_key(*tenant, address);
            let previous = seen.insert(key.clone(), pair.clone());
            assert!(
                previous.is_none(),
                "case {pair:?}: a key must name one (list, address) pair, but this one \
                 collides with {previous:?}"
            );
        }
    }

    // And a key carries a list's prefix only if it belongs to that very list, which is
    // what lets a listing tell a position in its own list from one in another's.
    for (case, tenant) in KEYED_LISTS {
        let prefix = suppression_list_prefix(*tenant);
        for (other_case, other) in KEYED_LISTS {
            for address in KEYED_ADDRESSES {
                let key = suppression_key(*other, address);
                let same_list = other == tenant;
                assert_eq!(
                    key.starts_with(&prefix),
                    same_list,
                    "case {case:?} vs {other_case:?} / {address:?}: expected the key \
                     {key:?} to carry the prefix {prefix:?} only for its own list, got {}",
                    key.starts_with(&prefix)
                );
            }
        }
    }
}
