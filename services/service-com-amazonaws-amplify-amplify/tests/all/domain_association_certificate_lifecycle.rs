// SPDX-License-Identifier: CC0-1.0

//! The custom-domain workflow a polling client runs: a domain association is
//! provisioned asynchronously, so the record it answers is INCOMPLETE while the
//! provisioning runs and the client polls it to a TERMINAL status.
//!
//! Two rules, each asserted over a TABLE spanning what it generalises over rather
//! than over one recorded request:
//!
//! 1. An association's record carries only what the service has issued: a member it
//!    has no value for is ABSENT from the body — never present and empty — and a
//!    subdomain's `dnsRecord` is the unresolved placeholder until a record exists to
//!    publish, so `statusReason`, the certificate verification record and (on a
//!    REPORTED record) the apex subdomain's `prefix` are absent. The call that WROTE
//!    the association echoes the subdomain settings its caller configured, so the
//!    prefix it stated is the prefix that call answers.
//! 2. The clock drives every association to a TERMINAL `domainStatus` inside a
//!    bounded window — so a poll loop is always told the outcome instead of hanging —
//!    and the terminal failure carries the certificate service's own fixed reason.
//! 3. `DeleteDomainAssociation` answers the record it removed, and the association is
//!    gone from the app afterwards: a read of it is the modelled 404, it is out of
//!    the app's listing, and the same name is associable again. (Implementing that
//!    operation is what lets the lifecycle above be driven end to end; no open issue
//!    carries its stub.)
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    CERTIFICATE_VERIFICATION_FAILED_REASON, DOMAIN_STATUS_SEQUENCE, DOMAIN_STATUS_STEP_MILLIS,
};

use crate::app_branch_lifecycle::{bool_member, list_member, string_member, Emulator, Reply};

/// The statuses a client's poll loop stops at: the two outcomes of provisioning a
/// custom domain, out of `DomainStatus`'s modelled values
/// (`smithy_query(query="shape",shape="DomainStatus")`). Every other value of that
/// enum reports work still in progress.
const TERMINAL_STATUSES: &[&str] = &["AVAILABLE", "FAILED"];

/// The terminal status an association of this emulator reaches: nothing here resolves
/// DNS, so the records that prove the caller controls the domain are never observed
/// and the certificate cannot be issued.
const FAILED: &str = "FAILED";

/// The certificate types `CertificateType` models
/// (`smithy_query(query="shape",shape="CertificateType")`).
const AMPLIFY_MANAGED: &str = "AMPLIFY_MANAGED";
const CUSTOM: &str = "CUSTOM";

/// A certificate ARN satisfying `CertificateArn`'s modelled pattern
/// `^arn:aws:acm:[a-z0-9-]+:\d{12}:certificate\/.+$`
/// (`smithy_query(query="shape",shape="CertificateSettings")`).
const CUSTOM_CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// `CreateDomainAssociation` over the wire.
async fn create_domain(emulator: &Emulator, app_id: &str, request: &Value) -> Reply {
    emulator
        .call("POST", &format!("/apps/{app_id}/domains"), Some(request))
        .await
}

/// `GetDomainAssociation` over the wire.
async fn get_domain(emulator: &Emulator, app_id: &str, domain_name: &str) -> Reply {
    emulator
        .call("GET", &format!("/apps/{app_id}/domains/{domain_name}"), None)
        .await
}

/// `DeleteDomainAssociation` over the wire.
async fn delete_domain(emulator: &Emulator, app_id: &str, domain_name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/apps/{app_id}/domains/{domain_name}"),
            None,
        )
        .await
}

/// One `CreateDomainAssociation` request, and what rule 1 says the record it answers
/// must carry.
struct Case {
    /// What the case varies, named in every assertion it makes.
    case: &'static str,
    /// The `domainName` the request carries, which addresses the record afterwards.
    domain_name: &'static str,
    /// The certificate `type` the request asks for, and the record must report.
    certificate_type: &'static str,
    /// The `customCertificateArn` the request carries, if any.
    custom_certificate_arn: Option<&'static str>,
    /// `(prefix, dnsRecord)` per requested subdomain, in the order the request states
    /// them. Each expected record is written out rather than computed, so the table
    /// pins the form and not the implementation.
    sub_domains: &'static [(&'static str, &'static str)],
}

/// The table: subdomain counts from none to several, prefixes from the apex (the
/// empty prefix) through single-label, hyphenated and digit-bearing ones, and both
/// modelled certificate types.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "apex subdomain only, default certificate",
            domain_name: "apex-only.example.com",
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: None,
            sub_domains: &[("", "* CNAME <pending>")],
        },
        Case {
            case: "one labelled subdomain",
            domain_name: "labelled.example.com",
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: None,
            sub_domains: &[("www", "www CNAME <pending>")],
        },
        Case {
            case: "apex and three labelled subdomains, mixed prefix forms",
            domain_name: "mixed.example.io",
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: None,
            sub_domains: &[
                ("", "* CNAME <pending>"),
                ("docs-v2", "docs-v2 CNAME <pending>"),
                ("9lives", "9lives CNAME <pending>"),
                ("a", "a CNAME <pending>"),
            ],
        },
        Case {
            case: "no subdomains at all",
            domain_name: "bare.example.co.uk",
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: None,
            sub_domains: &[],
        },
        Case {
            case: "the caller's own certificate",
            domain_name: "own-cert.example.net",
            certificate_type: CUSTOM,
            custom_certificate_arn: Some(CUSTOM_CERTIFICATE_ARN),
            sub_domains: &[("", "* CNAME <pending>"), ("beta", "beta CNAME <pending>")],
        },
    ]
}

impl Case {
    /// The request body this case sends.
    fn request(&self) -> Value {
        let settings: Vec<Value> = self
            .sub_domains
            .iter()
            .map(|(prefix, _)| json!({ "prefix": prefix, "branchName": "main" }))
            .collect();
        let mut request = json!({
            "domainName": self.domain_name,
            "subDomainSettings": settings,
        });
        if let Some(arn) = self.custom_certificate_arn {
            request["certificateSettings"] = json!({ "type": self.certificate_type, "customCertificateArn": arn });
        } else {
            request["certificateSettings"] = json!({ "type": self.certificate_type });
        }
        request
    }
}

/// How long to wait before expecting `steps` clock-owed status changes: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_status_steps(steps: i64) -> Duration {
    Duration::from_millis(
        u64::try_from(DOMAIN_STATUS_STEP_MILLIS.saturating_mul(steps) + 400).unwrap_or(1_400),
    )
}

/// Every path through `record` whose value is the EMPTY STRING, at any depth, as
/// `parent.member` paths.
///
/// A member the service has no value for is absent from the body, so a record that
/// reaches the wire carries no empty string anywhere: this is the invariant rule 1
/// states, checked over the whole record rather than member by member.
fn empty_string_members(path: &str, record: &Value, found: &mut Vec<String>) {
    match record {
        Value::String(text) if text.is_empty() => found.push(path.to_string()),
        Value::Object(members) => {
            for (name, value) in members {
                empty_string_members(&format!("{path}.{name}"), value, found);
            }
        }
        Value::Array(elements) => {
            for (index, value) in elements.iter().enumerate() {
                empty_string_members(&format!("{path}[{index}]"), value, found);
            }
        }
        _ => {}
    }
}

/// The member as a client sees it: JSON `null` for a member the body does not carry.
fn optional_member(record: &Value, member: &str) -> Value {
    record.get(member).cloned().unwrap_or(Value::Null)
}

/// Which view of an association an answer carries.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// The record the call that WROTE the association answered: its subdomain
    /// settings are the caller's configuration, echoed as configured.
    Written,
    /// The record a REPORTING operation answered: the service's own view, which
    /// carries a member only where it has a value for it.
    Reported,
}

/// Assert rule 1 over one answered record, whatever answered it.
fn assert_only_valued_members(case: &str, from: &str, answer: Answer, record: &Value, expected: &Case) {
    if matches!(answer, Answer::Reported) {
        // The invariant over the WHOLE record, rather than member by member: a
        // reported record states no member it has no value for, so no string in it
        // anywhere is empty. A written record is exempt only for the configuration
        // it echoes, which is asserted per subdomain below.
        let mut empty = Vec::new();
        empty_string_members("$", record, &mut empty);
        assert!(
            empty.is_empty(),
            "case {case:?} ({from}): a member with no value is absent, but {empty:?} are \
             present and empty in {record:?}"
        );
    }
    assert_eq!(
        optional_member(record, "updateStatus"),
        Value::Null,
        "case {case:?} ({from}): no update is in progress, so updateStatus is absent, \
         got {record:?}"
    );
    assert_eq!(
        optional_member(record, "certificateVerificationDNSRecord"),
        Value::Null,
        "case {case:?} ({from}): no certificate was issued, so there is no verification \
         record, got {record:?}"
    );

    let certificate = record
        .get("certificate")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?} ({from}): no certificate in {record:?}"));
    assert_eq!(
        string_member(&certificate, "type"),
        expected.certificate_type,
        "case {case:?} ({from}): the certificate type follows the request, got {certificate:?}"
    );
    assert_eq!(
        optional_member(&certificate, "customCertificateArn"),
        expected
            .custom_certificate_arn
            .map_or(Value::Null, |arn| json!(arn)),
        "case {case:?} ({from}): the caller's certificate ARN is echoed, got {certificate:?}"
    );
    assert_eq!(
        optional_member(&certificate, "certificateVerificationDNSRecord"),
        Value::Null,
        "case {case:?} ({from}): the certificate carries no verification record either, \
         got {certificate:?}"
    );

    let sub_domains = list_member(record, "subDomains");
    assert_eq!(
        sub_domains.len(),
        expected.sub_domains.len(),
        "case {case:?} ({from}): expected {} subdomain(s), got {sub_domains:?}",
        expected.sub_domains.len()
    );
    for (actual, (prefix, dns_record)) in sub_domains.iter().zip(expected.sub_domains) {
        let setting = actual
            .get("subDomainSetting")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?} ({from}): no subDomainSetting in {actual:?}"));
        assert_eq!(
            optional_member(&setting, "prefix"),
            match answer {
                // The configuration the request stated, echoed as stated.
                Answer::Written => json!(prefix),
                // The service's own view: the empty prefix is the apex, the
                // associated domain itself, which has no prefix to report.
                Answer::Reported if prefix.is_empty() => Value::Null,
                Answer::Reported => json!(prefix),
            },
            "case {case:?} ({from}): expected the {answer:?} view of the prefix {prefix:?}, \
             got {setting:?}"
        );
        assert_eq!(
            string_member(actual, "dnsRecord"),
            *dns_record,
            "case {case:?} ({from}): until a record exists the subdomain reports the \
             placeholder, got {actual:?}"
        );
        assert!(
            !bool_member(actual, "verified"),
            "case {case:?} ({from}): nothing is verified while no DNS record is observed, \
             got {actual:?}"
        );
    }
}

/// Rule 1: a created association reports only what the service has issued.
#[tokio::test]
async fn a_created_association_omits_the_members_it_has_no_value_for() {
    let emulator = Emulator::new();

    for expected in cases() {
        let case = expected.case;
        let app_id = emulator.app_id_of_new_app(case).await;
        let created = create_domain(&emulator, &app_id, &expected.request()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomainAssociation, got {created:?}"
        );
        let record = created.member("domainAssociation");
        assert_eq!(
            optional_member(&record, "statusReason"),
            Value::Null,
            "case {case:?}: an association that is simply proceeding carries no reason, \
             got {record:?}"
        );
        assert_only_valued_members(case, "create", Answer::Written, &record, &expected);

        // The same record, read straight back: the create answered what it persisted,
        // and a read that arrives before the next due instant reports it unchanged.
        let read = get_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDomainAssociation, got {read:?}"
        );
        assert_only_valued_members(
            case,
            "read",
            Answer::Reported,
            &read.member("domainAssociation"),
            &expected,
        );
    }
}

/// Rule 2: a poll loop reaches a terminal status inside a bounded window, and the
/// terminal failure carries the certificate service's own reason.
#[tokio::test]
async fn a_poll_loop_reaches_a_terminal_status_and_is_told_why() {
    let emulator = Emulator::new();

    for expected in cases() {
        let case = expected.case;
        let app_id = emulator.app_id_of_new_app(case).await;
        let created = create_domain(&emulator, &app_id, &expected.request()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomainAssociation, got {created:?}"
        );
        assert!(
            !TERMINAL_STATUSES.contains(
                &string_member(&created.member("domainAssociation"), "domainStatus").as_str()
            ),
            "case {case:?}: provisioning is asynchronous, so a create answers a \
             non-terminal status, got {created:?}"
        );

        // The loop a client runs: poll until the status is one it stops at. The budget
        // is the whole modelled progression plus a margin, so a status the clock never
        // reaches fails the case instead of hanging.
        let budget = DOMAIN_STATUS_SEQUENCE.len() + 4;
        let mut record = Value::Null;
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..budget {
            let read = get_domain(&emulator, &app_id, expected.domain_name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetDomainAssociation, got {read:?}"
            );
            record = read.member("domainAssociation");
            let status = string_member(&record, "domainStatus");
            seen.push(status.clone());
            if TERMINAL_STATUSES.contains(&status.as_str()) {
                break;
            }
            sleep(past_the_status_steps(1));
        }
        let status = string_member(&record, "domainStatus");
        assert!(
            TERMINAL_STATUSES.contains(&status.as_str()),
            "case {case:?}: the poll must be told an outcome inside {budget} reads, saw \
             {seen:?}"
        );
        assert_eq!(
            status, FAILED,
            "case {case:?}: no DNS record of the caller's is ever observed here, so the \
             certificate cannot be issued, saw {seen:?}"
        );
        assert_eq!(
            string_member(&record, "statusReason"),
            CERTIFICATE_VERIFICATION_FAILED_REASON,
            "case {case:?}: the terminal failure carries the certificate service's reason, \
             got {record:?}"
        );

        // The terminal record still reports only what the service has issued, and it is
        // the same record on every later read: nothing a read does moves it on.
        assert_only_valued_members(case, "terminal", Answer::Reported, &record, &expected);
        sleep(past_the_status_steps(2));
        let again = get_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            again.member("domainAssociation"),
            record,
            "case {case:?}: a terminal association reads the same on every call"
        );
    }
}

/// Rule 3: a delete answers the record it removed, and the association is gone from
/// the app afterwards.
#[tokio::test]
async fn a_deleted_association_leaves_its_app_and_frees_its_name() {
    let emulator = Emulator::new();

    for expected in cases() {
        let case = expected.case;
        let app_id = emulator.app_id_of_new_app(case).await;
        let created = create_domain(&emulator, &app_id, &expected.request()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomainAssociation, got {created:?}"
        );

        let deleted = delete_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteDomainAssociation, got {deleted:?}"
        );
        let removed = deleted.member("domainAssociation");
        assert_eq!(
            string_member(&removed, "domainName"),
            expected.domain_name,
            "case {case:?}: a delete answers the record it removed, got {removed:?}"
        );
        assert_only_valued_members(case, "delete", Answer::Reported, &removed, &expected);

        // Gone from the app: neither the read nor the listing can reach it.
        let read = get_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted association is not found, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {read:?}"
        );
        let listed = emulator
            .call("GET", &format!("/apps/{app_id}/domains"), None)
            .await;
        assert_eq!(
            listed.member("domainAssociations"),
            json!([]),
            "case {case:?}: the app lists no association after the delete, got {listed:?}"
        );
        let second_delete = delete_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            second_delete.status, 404,
            "case {case:?}: deleting what is already gone is not found, got {second_delete:?}"
        );

        // And the name is associable again, which is what removing it from the app's
        // map means.
        let again = create_domain(&emulator, &app_id, &expected.request()).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: the freed name is associable again, got {again:?}"
        );
    }

    // An association is addressed through its own app, so neither an unknown app nor
    // another app's association can be deleted here.
    let owner = emulator.app_id_of_new_app("delete-owner").await;
    let other = emulator.app_id_of_new_app("delete-other").await;
    let domain = "deletable.example.com";
    assert_eq!(
        create_domain(
            &emulator,
            &owner,
            &json!({ "domainName": domain, "subDomainSettings": [] })
        )
        .await
        .status,
        200,
        "case \"another app's association\": expected HTTP 200 from CreateDomainAssociation"
    );
    assert_eq!(
        delete_domain(&emulator, &other, domain).await.status,
        404,
        "case \"another app's association\": one app cannot delete another's"
    );
    assert_eq!(
        delete_domain(&emulator, "d0000000000000", domain).await.status,
        404,
        "case \"unknown app\": an association of an app that does not exist is not found"
    );
    assert_eq!(
        get_domain(&emulator, &owner, domain).await.status,
        200,
        "case \"another app's association\": the owner's association must still be readable"
    );
    // A malformed name is the same request-validation refusal every operation taking
    // a `domainName` gives, rather than a 404.
    let malformed = delete_domain(&emulator, &owner, "-bad.example.com").await;
    assert_eq!(
        malformed.status, 400,
        "case \"malformed name\": expected HTTP 400, got {malformed:?}"
    );
    assert_eq!(
        malformed.error_code.as_deref(),
        Some("ValidationException"),
        "case \"malformed name\": expected a request validation refusal, got {malformed:?}"
    );
}
