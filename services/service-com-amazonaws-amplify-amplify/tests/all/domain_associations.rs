// SPDX-License-Identifier: CC0-1.0

//! `CreateDomainAssociation` / `GetDomainAssociation`: a custom domain is a persisted
//! resource of the app it is associated with.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `CreateDomainAssociation` records a NEW association of its app: the record it
//!    answers is computed from the request (the ARN extending the app's, the
//!    subdomain DNS records, the certificate the request asked for) and
//!    `GetDomainAssociation` answers that same persisted record.
//! 2. A `domainName` outside its modelled `@pattern` is refused as a request
//!    validation failure — HTTP 400 `ValidationException` naming the member and the
//!    constraint — whatever shape the malformed name takes, while every name the
//!    pattern admits is accepted.
//! 3. Associating a domain is asynchronous: the association is created in the first
//!    status of the modelled progression, the clock owes it the later ones, a read
//!    materialises every step already due and never walks backwards, and the record
//!    is the same on every read once the progression ends.
//! 4. An association is addressed through its app, so an unknown app, an unknown
//!    domain, or a domain associated with another app is the modelled 404, and a
//!    second create under the same name is the modelled 400.
//! 5. An app's set of associations is BOUNDED: one past the cap answers the modelled
//!    `LimitExceededException`.
//! 6. `ListDomainAssociations` pages over exactly the associations its app holds, in a
//!    stable order, and a listed association agrees member for member with a read of
//!    the same one.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    DNS_UNVERIFIED_REASON, DOMAIN_NAME_PATTERN, DOMAIN_STATUS_SEQUENCE, DOMAIN_STATUS_STEP_MILLIS,
    MAX_DOMAIN_ASSOCIATIONS_PER_APP,
};

use crate::app_branch_lifecycle::{bool_member, list_member, string_member, Emulator, Reply};

/// The status a create answers with: the first of the modelled progression.
const CREATING: &str = "CREATING";

/// The status at which the certificate has been requested, and so the first one that
/// reports a `certificateVerificationDNSRecord`.
const REQUESTING_CERTIFICATE: &str = "REQUESTING_CERTIFICATE";

/// The last status the clock owes a new association: nothing here resolves DNS, so no
/// record of the caller's is ever observed.
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The certificate types `CertificateType` models
/// (`smithy_query(query="shape",shape="CertificateType")`).
const AMPLIFY_MANAGED: &str = "AMPLIFY_MANAGED";
const CUSTOM: &str = "CUSTOM";

/// A certificate ARN satisfying `CertificateArn`'s modelled pattern
/// `^arn:aws:acm:[a-z0-9-]+:\d{12}:certificate\/.+$`
/// (`smithy_query(query="shape",shape="CertificateSettings")`).
const CUSTOM_CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// How long to wait before expecting `steps` clock-owed status changes: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_status_steps(steps: i64) -> Duration {
    Duration::from_millis(
        u64::try_from(DOMAIN_STATUS_STEP_MILLIS.saturating_mul(steps) + 400).unwrap_or(1_400),
    )
}

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

/// `ListDomainAssociations` over the wire.
async fn list_domains(emulator: &Emulator, app_id: &str, query: &str) -> Reply {
    emulator
        .call("GET", &format!("/apps/{app_id}/domains{query}"), None)
        .await
}

/// The `domainAssociations` page of a listing reply.
fn page_records(reply: &Reply) -> Vec<Value> {
    reply
        .member("domainAssociations")
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("domainAssociations is not a list in {:?}", reply.body))
}

/// A member that may legitimately be absent, as the value a client sees: JSON `null`
/// for a member the body does not carry.
fn optional_member(record: &Value, member: &str) -> Value {
    record.get(member).cloned().unwrap_or(Value::Null)
}

/// The record without the members the CLOCK owns — the status, the reason it carries
/// and the certificate verification record, each of which changes as the association
/// proceeds. What is left is what a create and every later read must agree on.
fn clock_independent(record: &Value) -> Value {
    let mut copy = record.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("domainStatus");
        members.remove("statusReason");
        members.remove("certificateVerificationDNSRecord");
        members.remove("certificate");
    }
    copy
}

/// Where `status` sits in the modelled progression, for the monotonicity assertion.
fn progression_index(case: &str, status: &str) -> usize {
    DOMAIN_STATUS_SEQUENCE
        .iter()
        .position(|candidate| *candidate == status)
        .unwrap_or_else(|| {
            panic!("case {case:?}: status {status:?} is not one of {DOMAIN_STATUS_SEQUENCE:?}")
        })
}

/// Assert `reply` is the modelled 404 whose message names `entity` and `identifier`.
fn assert_not_found(case: &str, reply: &Reply, entity: &str, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected the modelled NotFoundException, got {reply:?}"
    );
    assert_eq!(
        reply.message().as_deref(),
        Some(format!("{entity} {identifier} not found.").as_str()),
        "case {case:?}: expected the 404 to name {entity} {identifier}, got {reply:?}"
    );
}

/// One `CreateDomainAssociation` request, and what the rule says the record it
/// produces must carry.
struct CreateCase {
    /// What the case varies, named in every assertion it makes.
    case: &'static str,
    /// The branch names created on the app before the association.
    branches: &'static [&'static str],
    /// The request body.
    request: Value,
    /// The `domainName` the request carries, which addresses the record afterwards.
    domain_name: &'static str,
    /// `enableAutoSubDomain` the record must report.
    enable_auto_sub_domain: bool,
    /// `autoSubDomainCreationPatterns` the record must report, as a client sees it.
    auto_sub_domain_creation_patterns: Value,
    /// `autoSubDomainIAMRole` the record must report, as a client sees it.
    auto_sub_domain_iam_role: Value,
    /// The certificate `type` the record must report.
    certificate_type: &'static str,
    /// The certificate's `customCertificateArn`, as a client sees it.
    custom_certificate_arn: Value,
    /// `(prefix, branchName, dnsRecord-suffix-after-the-app-default-domain-less-part)`
    /// for each subdomain, in the order the request stated them: the expected DNS
    /// record is `<name> CNAME <branch>.<appId>.amplifyapp.com`, built below from the
    /// app id the create was answered with.
    sub_domains: &'static [(&'static str, &'static str, &'static str)],
}

/// The create cases: a table spanning the domain-name forms the pattern admits, the
/// subdomain counts and prefixes (apex and labelled), both certificate types, and
/// which optional members the request carries.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "two labels, one labelled subdomain, no optional members",
            branches: &["main"],
            request: json!({
                "domainName": "example.com",
                "subDomainSettings": [{ "prefix": "www", "branchName": "main" }],
            }),
            domain_name: "example.com",
            enable_auto_sub_domain: false,
            auto_sub_domain_creation_patterns: Value::Null,
            auto_sub_domain_iam_role: Value::Null,
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: Value::Null,
            sub_domains: &[("www", "main", "www.example.com")],
        },
        CreateCase {
            case: "apex subdomain (empty prefix) beside a labelled one",
            branches: &["main", "release"],
            request: json!({
                "domainName": "roundtrip-site.io",
                "subDomainSettings": [
                    { "prefix": "", "branchName": "main" },
                    { "prefix": "beta", "branchName": "release" },
                ],
                "enableAutoSubDomain": true,
            }),
            domain_name: "roundtrip-site.io",
            enable_auto_sub_domain: true,
            auto_sub_domain_creation_patterns: Value::Null,
            auto_sub_domain_iam_role: Value::Null,
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: Value::Null,
            sub_domains: &[
                ("", "main", "roundtrip-site.io"),
                ("beta", "release", "beta.roundtrip-site.io"),
            ],
        },
        CreateCase {
            case: "four labels, no subdomains, automatic subdomain patterns and role",
            branches: &[],
            request: json!({
                "domainName": "shop.eu.example.co",
                "subDomainSettings": [],
                "enableAutoSubDomain": true,
                "autoSubDomainCreationPatterns": ["feature/*", "release-*"],
                "autoSubDomainIAMRole": "arn:aws:iam::000000000000:role/amplify-subdomains",
            }),
            domain_name: "shop.eu.example.co",
            enable_auto_sub_domain: true,
            auto_sub_domain_creation_patterns: json!(["feature/*", "release-*"]),
            auto_sub_domain_iam_role: json!("arn:aws:iam::000000000000:role/amplify-subdomains"),
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: Value::Null,
            sub_domains: &[],
        },
        CreateCase {
            case: "trailing root dot, Amplify-managed certificate stated explicitly",
            branches: &["main"],
            request: json!({
                "domainName": "rooted.example.net.",
                "subDomainSettings": [{ "prefix": "docs", "branchName": "main" }],
                "certificateSettings": { "type": AMPLIFY_MANAGED },
            }),
            domain_name: "rooted.example.net.",
            enable_auto_sub_domain: false,
            auto_sub_domain_creation_patterns: Value::Null,
            auto_sub_domain_iam_role: Value::Null,
            certificate_type: AMPLIFY_MANAGED,
            custom_certificate_arn: Value::Null,
            sub_domains: &[("docs", "main", "docs.rooted.example.net.")],
        },
        CreateCase {
            case: "hyphenated and digit-leading labels, caller's own certificate",
            branches: &["main"],
            request: json!({
                "domainName": "9to5-app.my-team.example",
                "subDomainSettings": [{ "prefix": "app", "branchName": "main" }],
                "certificateSettings": {
                    "type": CUSTOM,
                    "customCertificateArn": CUSTOM_CERTIFICATE_ARN,
                },
            }),
            domain_name: "9to5-app.my-team.example",
            enable_auto_sub_domain: false,
            auto_sub_domain_creation_patterns: Value::Null,
            auto_sub_domain_iam_role: Value::Null,
            certificate_type: CUSTOM,
            custom_certificate_arn: json!(CUSTOM_CERTIFICATE_ARN),
            sub_domains: &[("app", "main", "app.9to5-app.my-team.example")],
        },
    ]
}

/// Rule 1: a created association is computed from the request and from the app it is
/// created on, and `GetDomainAssociation` answers that same persisted record.
#[tokio::test]
async fn a_created_domain_association_reads_back_through_get() {
    let emulator = Emulator::new();

    for expected in create_cases() {
        let case = expected.case;
        let app_id = emulator.app_id_of_new_app(case).await;
        for branch in expected.branches {
            let created = emulator
                .create_branch(&app_id, &json!({ "branchName": branch }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateBranch {branch}, got {created:?}"
            );
        }
        let app = emulator.get_app(&app_id).await.member("app");
        let app_arn = string_member(&app, "appArn");
        let default_domain = string_member(&app, "defaultDomain");

        let reply = create_domain(&emulator, &app_id, &expected.request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomainAssociation, got {reply:?}"
        );
        let record = reply.member("domainAssociation");

        assert_eq!(
            string_member(&record, "domainAssociationArn"),
            format!("{app_arn}/domains/{}", expected.domain_name),
            "case {case:?}: the ARN must extend the app's, got {record:?}"
        );
        assert_eq!(
            string_member(&record, "domainName"),
            expected.domain_name,
            "case {case:?}: the record must carry the requested domain, got {record:?}"
        );
        assert_eq!(
            bool_member(&record, "enableAutoSubDomain"),
            expected.enable_auto_sub_domain,
            "case {case:?}: enableAutoSubDomain must follow the request, got {record:?}"
        );
        assert_eq!(
            optional_member(&record, "autoSubDomainCreationPatterns"),
            expected.auto_sub_domain_creation_patterns,
            "case {case:?}: the subdomain patterns must follow the request, got {record:?}"
        );
        assert_eq!(
            optional_member(&record, "autoSubDomainIAMRole"),
            expected.auto_sub_domain_iam_role,
            "case {case:?}: the subdomain role must follow the request, got {record:?}"
        );
        assert_eq!(
            string_member(&record, "domainStatus"),
            CREATING,
            "case {case:?}: a created association starts in the first status, got {record:?}"
        );
        assert_eq!(
            optional_member(&record, "certificateVerificationDNSRecord"),
            Value::Null,
            "case {case:?}: no certificate is requested yet, so there is no record to \
             verify, got {record:?}"
        );
        let certificate = record
            .get("certificate")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no certificate in {record:?}"));
        assert_eq!(
            string_member(&certificate, "type"),
            expected.certificate_type,
            "case {case:?}: the certificate type must follow the request, got {certificate:?}"
        );
        assert_eq!(
            optional_member(&certificate, "customCertificateArn"),
            expected.custom_certificate_arn,
            "case {case:?}: the caller's certificate ARN must be echoed, got {certificate:?}"
        );

        let sub_domains = list_member(&record, "subDomains");
        assert_eq!(
            sub_domains.len(),
            expected.sub_domains.len(),
            "case {case:?}: expected {} subdomain(s), got {sub_domains:?}",
            expected.sub_domains.len()
        );
        for (actual, (prefix, branch, name)) in sub_domains.iter().zip(expected.sub_domains) {
            let setting = actual
                .get("subDomainSetting")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no subDomainSetting in {actual:?}"));
            assert_eq!(
                (
                    string_member(&setting, "prefix"),
                    string_member(&setting, "branchName")
                ),
                ((*prefix).to_string(), (*branch).to_string()),
                "case {case:?}: the subdomain setting must be echoed, got {actual:?}"
            );
            assert_eq!(
                string_member(actual, "dnsRecord"),
                format!("{name} CNAME {branch}.{default_domain}"),
                "case {case:?}: the subdomain must point at its branch, got {actual:?}"
            );
            assert!(
                !bool_member(actual, "verified"),
                "case {case:?}: nothing is verified before any DNS record is observed, \
                 got {actual:?}"
            );
        }

        let read = get_domain(&emulator, &app_id, expected.domain_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDomainAssociation, got {read:?}"
        );
        assert_eq!(
            clock_independent(&read.member("domainAssociation")),
            clock_independent(&record),
            "case {case:?}: a read must answer the record the create persisted"
        );
    }
}

/// Rule 2: the domain names the modelled `@pattern` admits, each tested on a form the
/// issue did not cite. Sources: `DomainName`'s `@pattern`
/// `^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])(\.)?$`
/// and its `@length(0, 64)` (`smithy_query(query="shape",shape="DomainAssociation")`).
/// Every row is at most 64 characters, so the length half of the constraint — which
/// the boundary validator enforces on its own — never decides the case.
const ACCEPTED_DOMAIN_NAMES: &[&str] = &[
    // The smallest name two labels can make: `{1,62}` plus a closing alphanumeric.
    "a.bc",
    "example.com",
    // Every label at its published maximum would exceed @length(0, 64); 58 + 4 does not.
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.com",
    // Hyphens inside a label, digits leading one, many labels, and the root dot.
    "my-team-9.sub.example.co.uk",
    "9lives.example.io",
    "trailing.example.org.",
    // An IDN in its punycode form is letters, digits and hyphens like any other.
    "xn--bcher-kva.example",
];

/// Rule 2: the forms the pattern refuses, one per clause of the pattern.
const REFUSED_DOMAIN_NAMES: &[(&str, &str)] = &[
    ("", "no label at all, which `@length(0, 64)` admits"),
    ("example", "a single label, with no dotted prefix"),
    ("a.b", "a final label of one character, below `{1,62}` plus one"),
    ("-bad.example.com", "a label opening with a hyphen"),
    ("bad-.example.com", "a label closing on a hyphen"),
    ("example..com", "an empty label between two dots"),
    ("example.com..", "a second root dot"),
    (".example.com", "an empty leading label"),
    ("exa mple.com", "a space, which no label admits"),
    ("exa_mple.com", "an underscore, which no label admits"),
    ("exa*mple.com", "a wildcard, which no label admits"),
];

/// Rule 2: a `domainName` outside the modelled pattern is refused as a request
/// validation failure, and one inside it is accepted — whatever form it takes.
#[tokio::test]
async fn a_domain_name_outside_its_modelled_pattern_is_refused() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("domain-name-pattern").await;

    for (name, why) in REFUSED_DOMAIN_NAMES {
        let reply = create_domain(
            &emulator,
            &app_id,
            &json!({ "domainName": name, "subDomainSettings": [] }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {why:?} ({name:?}): expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {why:?} ({name:?}): expected a request validation refusal, got {reply:?}"
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(
                format!(
                    "1 validation error detected: Value '{name}' at 'domainName' failed to \
                     satisfy constraint: Member must satisfy regular expression pattern: \
                     {DOMAIN_NAME_PATTERN}"
                )
                .as_str()
            ),
            "case {why:?} ({name:?}): the refusal must name the member and the constraint, \
             got {reply:?}"
        );

        // A read of the same malformed name takes the same refusal — where the name
        // can be addressed at all: the empty name makes no `{domainName}` path
        // segment, so it addresses the collection (`GET /apps/{appId}/domains`)
        // rather than a member of it, and a name holding a space is not a URI a
        // client can send in the first place.
        if !name.is_empty() && !name.contains(' ') {
            let read = get_domain(&emulator, &app_id, name).await;
            assert_eq!(
                read.status, 400,
                "case {why:?} ({name:?}): a read of a malformed name is the same refusal, \
                 got {read:?}"
            );
            assert_eq!(
                read.error_code.as_deref(),
                Some("ValidationException"),
                "case {why:?} ({name:?}): a read of a malformed name is the same refusal, \
                 got {read:?}"
            );
        }
    }

    for name in ACCEPTED_DOMAIN_NAMES {
        assert!(
            name.len() <= 64,
            "case {name:?}: a row of this table must stay inside @length(0, 64)"
        );
        let reply = create_domain(
            &emulator,
            &app_id,
            &json!({ "domainName": name, "subDomainSettings": [] }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the pattern admits this name, so it must be accepted, got {reply:?}"
        );
        assert_eq!(
            string_member(&reply.member("domainAssociation"), "domainName"),
            *name,
            "case {name:?}: the accepted name must be the one stored, got {reply:?}"
        );
    }
}

/// Rule 3: the clock owes a new association its later statuses; a read materialises
/// every step already due, never walks backwards, and reports the same record once
/// the progression ends.
#[tokio::test]
async fn the_clock_progresses_a_new_association_through_the_modelled_statuses() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("status-progression").await;
    let domain = "progression.example.com";

    let created = create_domain(
        &emulator,
        &app_id,
        &json!({
            "domainName": domain,
            "subDomainSettings": [{ "prefix": "www", "branchName": "main" }],
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"create\": expected HTTP 200, got {created:?}"
    );
    assert_eq!(
        string_member(&created.member("domainAssociation"), "domainStatus"),
        CREATING,
        "case \"create\": an association starts in the first status, got {created:?}"
    );

    // Each read observes a status at or after the one before it, and the chain is
    // walked step by step rather than jumped: the waits below are one step apart.
    let mut seen = Vec::new();
    let mut highest = 0;
    for step in 0..DOMAIN_STATUS_SEQUENCE.len() {
        let read = get_domain(&emulator, &app_id, domain).await;
        assert_eq!(
            read.status, 200,
            "case \"poll {step}\": expected HTTP 200, got {read:?}"
        );
        let record = read.member("domainAssociation");
        let status = string_member(&record, "domainStatus");
        let index = progression_index(&format!("poll {step}"), &status);
        assert!(
            index >= highest,
            "case \"poll {step}\": a status never walks backwards: saw {seen:?} then {status:?}"
        );
        highest = index;
        seen.push(status);
        sleep(past_the_status_steps(1));
    }

    // Past the whole chain, the association rests in its last status: the certificate
    // verification record is published, and the reason states what is being waited on.
    let settled = get_domain(&emulator, &app_id, domain).await;
    let record = settled.member("domainAssociation");
    assert_eq!(
        string_member(&record, "domainStatus"),
        PENDING_VERIFICATION,
        "case \"settled\": the last status the clock owes, got {record:?}"
    );
    assert_eq!(
        string_member(&record, "statusReason"),
        DNS_UNVERIFIED_REASON,
        "case \"settled\": the resting status states why it rests, got {record:?}"
    );
    let verification = string_member(&record, "certificateVerificationDNSRecord");
    assert!(
        verification.starts_with('_')
            && verification.contains(&format!(".{domain}. CNAME "))
            && verification.ends_with(".acm-validations.aws."),
        "case \"settled\": the verification record must be publishable in the domain's \
         zone, got {verification:?}"
    );
    let certificate = record
        .get("certificate")
        .cloned()
        .unwrap_or_else(|| panic!("case \"settled\": no certificate in {record:?}"));
    assert_eq!(
        optional_member(&certificate, "certificateVerificationDNSRecord"),
        json!(verification.clone()),
        "case \"settled\": the certificate carries the same record, got {certificate:?}"
    );
    assert!(
        seen.iter().any(|status| status == REQUESTING_CERTIFICATE)
            || seen.iter().any(|status| status == PENDING_VERIFICATION),
        "case \"settled\": the reads must have observed the association move on, saw {seen:?}"
    );

    // A read after the progression ends changes nothing a later caller sees.
    let again = get_domain(&emulator, &app_id, domain).await;
    assert_eq!(
        again.member("domainAssociation"),
        record,
        "case \"re-read\": a settled association reads the same on every call"
    );

    // The verification record is minted per association, so a second association of
    // the same app carries a different one.
    let other = create_domain(
        &emulator,
        &app_id,
        &json!({ "domainName": "second.example.com", "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "case \"second association\": expected HTTP 200, got {other:?}"
    );
    sleep(past_the_status_steps(
        i64::try_from(DOMAIN_STATUS_SEQUENCE.len()).unwrap_or(3),
    ));
    let other_verification = string_member(
        &get_domain(&emulator, &app_id, "second.example.com")
            .await
            .member("domainAssociation"),
        "certificateVerificationDNSRecord",
    );
    assert_ne!(
        other_verification, verification,
        "case \"second association\": each association verifies through its own record"
    );
}

/// Rule 4: an association is reachable only through its own app, and a name already
/// associated with that app is refused instead of replacing the first association.
#[tokio::test]
async fn an_association_is_addressed_through_its_own_app() {
    let emulator = Emulator::new();
    let owner = emulator.app_id_of_new_app("owner").await;
    let other = emulator.app_id_of_new_app("other").await;
    let domain = "owned.example.com";

    let created = create_domain(
        &emulator,
        &owner,
        &json!({ "domainName": domain, "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"create\": expected HTTP 200, got {created:?}"
    );

    // An unknown app: the 404 names the app, not the domain.
    let unknown_app = "d0000000000000";
    assert_not_found(
        "create on an unknown app",
        &create_domain(
            &emulator,
            unknown_app,
            &json!({ "domainName": domain, "subDomainSettings": [] }),
        )
        .await,
        "App",
        unknown_app,
    );
    assert_not_found(
        "read on an unknown app",
        &get_domain(&emulator, unknown_app, domain).await,
        "App",
        unknown_app,
    );

    // A known app that holds no such association, and the same name on another app.
    assert_not_found(
        "read of a domain the app has not associated",
        &get_domain(&emulator, &owner, "absent.example.com").await,
        "Domain",
        "absent.example.com",
    );
    assert_not_found(
        "read of another app's domain",
        &get_domain(&emulator, &other, domain).await,
        "Domain",
        domain,
    );

    // The same name IS creatable on another app: the association belongs to the app.
    let elsewhere = create_domain(
        &emulator,
        &other,
        &json!({ "domainName": domain, "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"same name, other app\": expected HTTP 200, got {elsewhere:?}"
    );

    // A second create on the SAME app is refused, and the first association stands.
    let duplicate = create_domain(
        &emulator,
        &owner,
        &json!({ "domainName": domain, "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        duplicate.status, 400,
        "case \"duplicate\": expected HTTP 400, got {duplicate:?}"
    );
    assert_eq!(
        duplicate.error_code.as_deref(),
        Some("BadRequestException"),
        "case \"duplicate\": expected the modelled BadRequestException, got {duplicate:?}"
    );
    assert_eq!(
        duplicate.message().as_deref(),
        Some(
            format!("Failed to create domain. The domain {domain} already exists for the app {owner}")
                .as_str()
        ),
        "case \"duplicate\": the refusal is built from the request, got {duplicate:?}"
    );
    assert_eq!(
        get_domain(&emulator, &owner, domain).await.status,
        200,
        "case \"duplicate\": the first association must still be readable"
    );
}

/// Rule 1 (the cross-member half) and rule 5: a `CUSTOM` certificate without its ARN
/// is refused, and an app holds at most its published number of associations.
#[tokio::test]
async fn a_create_is_refused_when_it_cannot_be_served() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("refusals").await;

    // "This field is required only when the certificate type is CUSTOM": the request
    // that names no certificate is refused by this operation, and the one whose ARN
    // is outside `CertificateArn`'s own modelled pattern is refused upstream of it.
    for (settings, why, expected_code) in [
        (
            json!({ "type": CUSTOM }),
            "no ARN member at all",
            "BadRequestException",
        ),
        (
            json!({ "type": CUSTOM, "customCertificateArn": "" }),
            "an empty ARN, outside the member's own modelled pattern",
            "ValidationException",
        ),
    ] {
        let reply = create_domain(
            &emulator,
            &app_id,
            &json!({
                "domainName": "custom-cert.example.com",
                "subDomainSettings": [],
                "certificateSettings": settings,
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {why:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(expected_code),
            "case {why:?}: expected {expected_code}, got {reply:?}"
        );
    }
    let accepted = create_domain(
        &emulator,
        &app_id,
        &json!({
            "domainName": "custom-cert.example.com",
            "subDomainSettings": [],
            "certificateSettings": {
                "type": CUSTOM,
                "customCertificateArn": CUSTOM_CERTIFICATE_ARN,
            },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"CUSTOM with its ARN\": expected HTTP 200, got {accepted:?}"
    );

    // The cap counts the associations the app already holds, including the one above.
    let capped = emulator.app_id_of_new_app("capped").await;
    for index in 0..MAX_DOMAIN_ASSOCIATIONS_PER_APP {
        let reply = create_domain(
            &emulator,
            &capped,
            &json!({ "domainName": format!("site-{index}.example.com"), "subDomainSettings": [] }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case \"association {index}\": expected HTTP 200 inside the cap, got {reply:?}"
        );
    }
    let past_the_cap = create_domain(
        &emulator,
        &capped,
        &json!({ "domainName": "one-too-many.example.com", "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        past_the_cap.status, 429,
        "case \"past the cap\": expected HTTP 429, got {past_the_cap:?}"
    );
    assert_eq!(
        past_the_cap.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"past the cap\": expected the modelled LimitExceededException, got \
         {past_the_cap:?}"
    );
    assert_not_found(
        "past the cap",
        &get_domain(&emulator, &capped, "one-too-many.example.com").await,
        "Domain",
        "one-too-many.example.com",
    );
}

/// Rule 6: a listing pages over exactly the associations its app holds, in the stable
/// order a page boundary needs, and each element is the record a read answers.
#[tokio::test]
async fn a_listing_pages_over_exactly_the_apps_associations() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("listing").await;
    let other = emulator.app_id_of_new_app("listing-other").await;

    // An app that has associated nothing lists nothing — not a 404.
    let empty = list_domains(&emulator, &app_id, "").await;
    assert_eq!(
        empty.status, 200,
        "case \"no associations\": expected HTTP 200, got {empty:?}"
    );
    assert!(
        page_records(&empty).is_empty(),
        "case \"no associations\": expected an empty page, got {empty:?}"
    );

    // Created out of order, so the listing's order is the store's and not the
    // request's.
    let domains = ["m.example.com", "a.example.com", "z.example.com"];
    for domain in domains {
        let created = create_domain(
            &emulator,
            &app_id,
            &json!({ "domainName": domain, "subDomainSettings": [] }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {domain:?}: expected HTTP 200 from CreateDomainAssociation, got {created:?}"
        );
    }
    let elsewhere = create_domain(
        &emulator,
        &other,
        &json!({ "domainName": "b.example.com", "subDomainSettings": [] }),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"another app's association\": expected HTTP 200, got {elsewhere:?}"
    );

    let mut sorted = domains;
    sorted.sort_unstable();

    // A table over the page sizes the model admits: an absent bound serves the whole
    // set, and a bound of 1 or 2 pages through it with the token being the name of
    // the first record the page did not return.
    for page_size in [None, Some(1), Some(2), Some(3)] {
        let case = format!("page size {page_size:?}");
        let query = page_size.map_or_else(String::new, |size| format!("?maxResults={size}"));
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let path = match token.as_deref() {
                Some(next) => format!("{query}{separator}nextToken={next}"),
                None => query.clone(),
            };
            let page = list_domains(&emulator, &app_id, &path).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {page:?}"
            );
            let records = page_records(&page);
            assert!(
                records.len() <= page_size.unwrap_or(domains.len()),
                "case {case:?}: a page must not exceed the requested bound, got {records:?}"
            );
            for record in &records {
                seen.push(string_member(record, "domainName"));
            }
            token = page
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages together are exactly this app's associations, in order"
        );
    }

    // An element of a page is the record a read of the same association answers.
    let listed = list_domains(&emulator, &app_id, "").await;
    for record in page_records(&listed) {
        let domain = string_member(&record, "domainName");
        let read = get_domain(&emulator, &app_id, &domain).await;
        assert_eq!(
            read.member("domainAssociation"),
            record,
            "case {domain:?}: a listed association must agree with a read of it"
        );
    }

    // A token this service never issued is refused rather than serving an arbitrary
    // suffix of the collection, and an unknown app is the same 404 every app-scoped
    // operation gives.
    let bad_token = list_domains(&emulator, &app_id, "?nextToken=not%20a%20domain").await;
    assert_eq!(
        bad_token.status, 400,
        "case \"invalid nextToken\": expected HTTP 400, got {bad_token:?}"
    );
    assert_eq!(
        bad_token.error_code.as_deref(),
        Some("BadRequestException"),
        "case \"invalid nextToken\": expected the modelled BadRequestException, got {bad_token:?}"
    );
    assert_not_found(
        "listing an unknown app",
        &list_domains(&emulator, "d0000000000000", "").await,
        "App",
        "d0000000000000",
    );
}
