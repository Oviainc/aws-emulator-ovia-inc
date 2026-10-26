// SPDX-License-Identifier: CC0-1.0

//! The suppression-list read, end to end over HTTP.
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

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::SUPPRESSION_LIST_REASON_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    suppression_key, ComAmazonawsSesv2SimpleemailserviceV2State, SuppressedDestinationRecord,
    MAX_TENANT_NAME_CHARS,
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

    /// Persist one suppression-list entry.
    fn seed(&self, record: &SuppressedDestinationRecord) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.suppressed_destinations.insert(
            suppression_key(record.tenant_name.as_deref(), &record.email_address),
            record.clone(),
        );
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
            message: "Tenant absent_tenant does not exist.".to_string(),
        },
        TenantCase {
            name: "a name at the documented length bound the account never created",
            tenant: longest.clone(),
            status: 404,
            code: "NotFoundException",
            message: format!("Tenant {longest} does not exist."),
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
