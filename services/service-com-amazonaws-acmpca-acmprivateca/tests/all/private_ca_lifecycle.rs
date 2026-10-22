// SPDX-License-Identifier: CC0-1.0

//! The private-CA lifecycle, driven over the wire the way a client drives it:
//! `POST /` with `X-Amz-Target: ACMPrivateCA.<Op>` and an awsJson1_1 body, through the
//! same multi-service router the emulator boots.
//!
//! Every suite here builds its own engine, so one test's state is invisible to another.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::SETTLE_AFTER;
use service_com_amazonaws_acmpca_acmprivateca::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service and the
/// region every ARN the request mints must carry. The router verifies no signature here
/// (`set_request_authentication(false)`); the scope is read for routing and the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
                             Credential=AKIDTESTTESTTEST/20260101/eu-west-1/acm-pca/aws4_request, \
                             SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// Long enough that a transition the clock owed at `SETTLE_AFTER` has come due.
fn wait_for_the_clock() {
    // A test may wait for a due instant; a handler may not. `tokio`'s timer is not in
    // this workspace's feature set, so the current-thread test runtime parks instead.
    std::thread::sleep(SETTLE_AFTER + Duration::from_millis(250));
}

/// One emulator with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` and return the HTTP status with the parsed JSON body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://acm-pca.eu-west-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", "acm-pca.eu-west-1.amazonaws.com")
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation`, requiring success.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// Call `operation`, requiring the modelled error `code` with HTTP 400.
    async fn refused(&self, case: &str, operation: &str, body: &Value, code: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse with 400 {code}, got {status} {parsed:?}"
        );
        let wire_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            wire_code.contains(code),
            "case {case:?}: expected {operation} to answer {code}, got {wire_code:?} ({parsed:?})"
        );
        parsed
    }
}

/// The smallest configuration `CreateCertificateAuthority` accepts, with `common_name`
/// as the subject.
fn root_ca_request(common_name: &str) -> Value {
    json!({
        "CertificateAuthorityConfiguration": {
            "KeyAlgorithm": "RSA_2048",
            "SigningAlgorithm": "SHA256WITHRSA",
            "Subject": {"CommonName": common_name}
        },
        "CertificateAuthorityType": "ROOT"
    })
}

/// A PEM block of `label` around `content`, base64 in 64-character lines (RFC 7468).
fn pem(label: &str, content: &str) -> String {
    let encoded = aws_smithy_types::base64::encode(content.as_bytes());
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The decoded body of the first PEM block in `text`.
fn pem_content(text: &str) -> String {
    let body: String = text
        .lines()
        .skip_while(|line| !line.starts_with("-----BEGIN "))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END "))
        .collect();
    String::from_utf8(aws_smithy_types::base64::decode(&body).expect("the PEM body is base64"))
        .expect("the decoded body is text")
}

/// A `key: value` line of a decoded CSR or certificate body.
fn field(text: &str, key: &str) -> String {
    let decoded = pem_content(text);
    decoded
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key:?} field in {decoded:?}"))
        .to_string()
}

/// A blob member as awsJson1_1 carries it.
fn blob(text: &str) -> Value {
    Value::String(aws_smithy_types::base64::encode(text.as_bytes()))
}

/// `Value` member as a `&str`, or a panic naming the case.
fn string(value: &Value, member: &str, case: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member:?} in {value:?}"))
        .to_string()
}

/// Create a CA and drive it all the way to ACTIVE, returning its ARN and the CA
/// certificate imported into it.
async fn active_ca(emulator: &Emulator, common_name: &str) -> (String, String) {
    let created = emulator
        .ok(common_name, "CreateCertificateAuthority", &root_ca_request(common_name))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", common_name);
    wait_for_the_clock();
    let certificate = pem("CERTIFICATE", &format!("ca certificate for {common_name}"));
    emulator
        .ok(
            common_name,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&certificate)}),
        )
        .await;
    (arn, certificate)
}

/// Steps 1 to 6 of the lifecycle, in the order a client script runs them: create,
/// read the CSR, import the signed CA certificate, issue an end-entity certificate and
/// read it back. Every answer comes from what an earlier call persisted.
#[tokio::test]
async fn the_private_ca_lifecycle_runs_end_to_end() {
    let emulator = Emulator::new();
    let case = "lifecycle";

    // 1. CreateCertificateAuthority mints an ARN in the signed region and account.
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request("example-root"))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    let prefix = format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a CA ARN under {prefix:?}, got {arn:?}"
    );
    let id = arn[prefix.len()..].to_string();
    assert_eq!(
        id.len(),
        36,
        "case {case:?}: expected a 36-character uuid CA id, got {id:?}"
    );

    // A CA is returned in CREATING and its CSR is not a product of creation yet.
    let describing = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    assert_eq!(
        string(&describing["CertificateAuthority"], "Status", case),
        "CREATING",
        "case {case:?}: a CA reads CREATING until creation settles, got {describing:?}"
    );
    emulator
        .refused(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
            "RequestInProgressException",
        )
        .await;
    emulator
        .refused(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&pem("CERTIFICATE", "too early")),
            }),
            "RequestInProgressException",
        )
        .await;

    wait_for_the_clock();

    // 4. The describe between create and import reports PENDING_CERTIFICATE, with the
    // configuration the create call supplied read back out of state.
    let pending = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    let authority = &pending["CertificateAuthority"];
    assert_eq!(
        string(authority, "Status", case),
        "PENDING_CERTIFICATE",
        "case {case:?}: a settled CA waits for its certificate, got {authority:?}"
    );
    assert_eq!(string(authority, "Arn", case), arn);
    assert_eq!(string(authority, "OwnerAccount", case), ACCOUNT);
    assert_eq!(string(authority, "Type", case), "ROOT");
    assert_eq!(
        string(authority, "UsageMode", case),
        "GENERAL_PURPOSE",
        "case {case:?}: UsageMode defaults to GENERAL_PURPOSE, got {authority:?}"
    );
    assert_eq!(
        string(authority, "KeyStorageSecurityStandard", case),
        "FIPS_140_2_LEVEL_3_OR_HIGHER",
        "case {case:?}: KeyStorageSecurityStandard defaults to FIPS_140_2_LEVEL_3_OR_HIGHER, \
         got {authority:?}"
    );
    assert_eq!(
        authority["CertificateAuthorityConfiguration"]["Subject"]["CommonName"],
        json!("example-root"),
        "case {case:?}: the describe echoes the stored subject, got {authority:?}"
    );
    assert_eq!(
        authority["CertificateAuthorityConfiguration"]["KeyAlgorithm"],
        json!("RSA_2048"),
        "case {case:?}: the describe echoes the stored key algorithm, got {authority:?}"
    );
    assert!(
        !string(authority, "Serial", case).is_empty(),
        "case {case:?}: a CA carries a serial, got {authority:?}"
    );
    assert!(
        authority.get("NotBefore").is_none() && authority.get("NotAfter").is_none(),
        "case {case:?}: a CA has no validity window before its certificate is imported, \
         got {authority:?}"
    );

    // 2. The CSR is stable: it was derived once, at create time.
    let csr = string(
        &emulator
            .ok(case, "GetCertificateAuthorityCsr", &json!({"CertificateAuthorityArn": arn}))
            .await,
        "Csr",
        case,
    );
    let csr_again = string(
        &emulator
            .ok(case, "GetCertificateAuthorityCsr", &json!({"CertificateAuthorityArn": arn}))
            .await,
        "Csr",
        case,
    );
    assert_eq!(
        csr, csr_again,
        "case {case:?}: repeated GetCertificateAuthorityCsr must return the same CSR"
    );
    assert!(
        csr.starts_with("-----BEGIN CERTIFICATE REQUEST-----"),
        "case {case:?}: expected a PEM certification request, got {csr:?}"
    );
    assert_eq!(
        field(&csr, "subject"),
        "CN=example-root",
        "case {case:?}: the CSR carries the CA's own subject, got {csr:?}"
    );
    assert_eq!(field(&csr, "certificate-authority"), arn);

    // 3. Importing the signed CA certificate activates the CA.
    let ca_certificate = pem("CERTIFICATE", "the root CA certificate");
    let ca_chain = pem("CERTIFICATE", "an intermediate");
    let imported = emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&ca_certificate),
                "CertificateChain": blob(&ca_chain),
            }),
        )
        .await;
    assert_eq!(
        imported,
        Value::Object(serde_json::Map::new()),
        "case {case:?}: ImportCertificateAuthorityCertificate answers an empty body"
    );

    let active = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    let authority = &active["CertificateAuthority"];
    assert_eq!(
        string(authority, "Status", case),
        "ACTIVE",
        "case {case:?}: importing a certificate activates the CA, got {authority:?}"
    );
    let not_before = authority["NotBefore"].as_f64().unwrap_or_default();
    let not_after = authority["NotAfter"].as_f64().unwrap_or_default();
    assert!(
        not_after > not_before && not_before > 0.0,
        "case {case:?}: an ACTIVE CA carries its certificate's validity window, \
         got {authority:?}"
    );

    // GetCertificateAuthorityCertificate is the read of exactly those bytes.
    let read_back = emulator
        .ok(
            case,
            "GetCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    assert_eq!(
        string(&read_back, "Certificate", case),
        ca_certificate,
        "case {case:?}: the CA certificate reads back byte for byte"
    );
    assert_eq!(
        string(&read_back, "CertificateChain", case),
        ca_chain,
        "case {case:?}: the imported chain reads back byte for byte"
    );

    // 5. IssueCertificate allocates a serial under the CA and is idempotent per token.
    let csr_to_sign = pem("CERTIFICATE REQUEST", "an end entity's request");
    let issue = json!({
        "CertificateAuthorityArn": arn,
        "Csr": blob(&csr_to_sign),
        "SigningAlgorithm": "SHA256WITHRSA",
        "Validity": {"Value": 365, "Type": "DAYS"},
        "IdempotencyToken": "tok-1",
    });
    let certificate_arn = string(
        &emulator.ok(case, "IssueCertificate", &issue).await,
        "CertificateArn",
        case,
    );
    let serial_prefix = format!("{arn}/certificate/");
    assert!(
        certificate_arn.starts_with(&serial_prefix),
        "case {case:?}: expected a certificate ARN under {serial_prefix:?}, \
         got {certificate_arn:?}"
    );
    let replayed = string(
        &emulator.ok(case, "IssueCertificate", &issue).await,
        "CertificateArn",
        case,
    );
    assert_eq!(
        replayed, certificate_arn,
        "case {case:?}: the same IdempotencyToken must not mint a second certificate"
    );

    // Issuance is asynchronous: the certificate is not retrievable in the same instant.
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
            "RequestInProgressException",
        )
        .await;
    wait_for_the_clock();

    // 6. GetCertificate returns the PEM issued for that ARN, chained to the CA.
    let got = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
        )
        .await;
    let certificate = string(&got, "Certificate", case);
    assert!(
        certificate.starts_with("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: expected a PEM certificate, got {certificate:?}"
    );
    assert_eq!(
        field(&certificate, "serial"),
        certificate_arn[serial_prefix.len()..],
        "case {case:?}: the certificate carries the serial its ARN names"
    );
    assert_eq!(
        field(&certificate, "issuer"),
        "CN=example-root",
        "case {case:?}: the certificate is issued by the CA's own subject"
    );
    assert_eq!(
        string(&got, "CertificateChain", case),
        format!("{ca_certificate}{ca_chain}"),
        "case {case:?}: an issued certificate chains through the CA certificate and its chain"
    );

    // A certificate ARN this CA never issued does not resolve.
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "CertificateArn": format!("{arn}/certificate/0123456789abcdef"),
            }),
            "ResourceNotFoundException",
        )
        .await;

    // Nor does one issued by a different CA, even presented with its own CA's ARN.
    let (other_arn, _) = active_ca(&emulator, "other-root").await;
    let other_certificate_arn = string(
        &emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": other_arn,
                    "Csr": blob(&csr_to_sign),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": 30, "Type": "DAYS"},
                }),
            )
            .await,
        "CertificateArn",
        case,
    );
    emulator
        .refused(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "CertificateArn": other_certificate_arn,
            }),
            "ResourceNotFoundException",
        )
        .await;
}

/// `Validity`/`ValidityNotBefore` resolution, over every `ValidityPeriodType` member.
///
/// The rule is the one `Validity$Type` documents: `DAYS`/`MONTHS`/`YEARS` are relative
/// to the start of the validity period, `ABSOLUTE` is seconds since the Unix epoch, and
/// `END_DATE` is UTCTime `YYMMDDHHMMSS` (a year field of 50 or more means 19YY) or
/// GeneralizedTime `YYYYMMDDHHMMSS`. The `ABSOLUTE` 2524608000 and `END_DATE`
/// 491231235959 rows are the sample values that documentation publishes.
#[tokio::test]
async fn validity_resolves_the_way_each_validity_period_type_is_documented() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "validity-root").await;
    let csr = blob(&pem("CERTIFICATE REQUEST", "an end entity's request"));

    // 1767225600 = 2026-01-01T00:00:00Z, 1769817600 = 2026-01-31T00:00:00Z,
    // 1835395200 = 2028-02-29T00:00:00Z.
    let rows: &[(&str, i64, &str, Option<i64>, &str)] = &[
        ("one day", 1, "DAYS", Some(1_767_225_600), "2026-01-02T00:00:00Z"),
        ("a year of days", 365, "DAYS", Some(1_767_225_600), "2027-01-01T00:00:00Z"),
        ("one month", 1, "MONTHS", Some(1_767_225_600), "2026-02-01T00:00:00Z"),
        ("thirteen months", 13, "MONTHS", Some(1_767_225_600), "2027-02-01T00:00:00Z"),
        ("one year", 1, "YEARS", Some(1_767_225_600), "2027-01-01T00:00:00Z"),
        ("four years", 4, "YEARS", Some(1_767_225_600), "2030-01-01T00:00:00Z"),
        // A month shorter than the starting day of month clamps to its last day.
        ("month end clamp", 1, "MONTHS", Some(1_769_817_600), "2026-02-28T00:00:00Z"),
        ("leap day clamp", 1, "YEARS", Some(1_835_395_200), "2029-02-28T00:00:00Z"),
        ("absolute sample", 2_524_608_000, "ABSOLUTE", None, "2050-01-01T00:00:00Z"),
        ("utctime sample", 491_231_235_959, "END_DATE", None, "2049-12-31T23:59:59Z"),
        (
            "generalized time",
            20_351_231_235_959,
            "END_DATE",
            None,
            "2035-12-31T23:59:59Z",
        ),
    ];

    // Issue every row first and wait for the clock ONCE, rather than once per row.
    let mut issued: Vec<(&str, i64, &str, &str, String)> = Vec::with_capacity(rows.len());
    for (case, value, period, not_before, expected) in rows {
        let mut body = json!({
            "CertificateAuthorityArn": arn,
            "Csr": csr,
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": value, "Type": period},
        });
        if let Some(seconds) = not_before {
            body["ValidityNotBefore"] = json!({"Value": seconds, "Type": "ABSOLUTE"});
        }
        let certificate_arn = string(
            &emulator.ok(case, "IssueCertificate", &body).await,
            "CertificateArn",
            case,
        );
        issued.push((case, *value, period, expected, certificate_arn));
    }
    wait_for_the_clock();

    for (case, value, period, expected, certificate_arn) in issued {
        let certificate = string(
            &emulator
                .ok(
                    case,
                    "GetCertificate",
                    &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
                )
                .await,
            "Certificate",
            case,
        );
        assert_eq!(
            field(&certificate, "not-after"),
            *expected,
            "case {case:?}: Validity {value} {period} must expire at {expected}"
        );
    }

    // A validity that resolves to the start of the window or earlier is not a validity,
    // and neither is one that resolves outside the range of dates a calendar holds.
    let refusals: &[(&str, i64, &str)] = &[
        // UTCTime year 50 means 1950, which is behind any issuance instant.
        ("utctime nineteen fifty", 501_231_235_959, "END_DATE"),
        ("thirteen digits", 1_234_567_890_123, "END_DATE"),
        ("not a calendar date", 20_351_331_235_959, "END_DATE"),
        ("february thirtieth", 20_360_230_000_000, "END_DATE"),
        ("hour twenty-five", 20_351_231_250_000, "END_DATE"),
        ("absolute in the past", 1, "ABSOLUTE"),
        // Magnitudes no calendar represents: refused, never wrapped or panicked on.
        ("days past the calendar", i64::MAX, "DAYS"),
        ("months past the calendar", i64::MAX, "MONTHS"),
        ("years past the calendar", i64::MAX, "YEARS"),
        ("absolute past the clock", i64::MAX, "ABSOLUTE"),
        ("end date past the clock", i64::MAX, "END_DATE"),
    ];
    for (case, value, period) in refusals {
        emulator
            .refused(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": csr,
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": value, "Type": period},
                }),
                "InvalidArgsException",
            )
            .await;
    }
}

/// The revocation rules `CreateCertificateAuthority`'s `RevocationConfiguration` member
/// documents: "A configuration disabling CRLs or OCSP must contain only the
/// Enabled=False parameter, and will fail if other parameters such as CustomCname or
/// ExpirationInDays are included."
#[tokio::test]
async fn revocation_configurations_are_accepted_only_where_documented() {
    let emulator = Emulator::new();

    let rows: &[(&str, Value, bool)] = &[
        ("crl disabled alone", json!({"CrlConfiguration": {"Enabled": false}}), true),
        (
            "crl disabled with a cname",
            json!({"CrlConfiguration": {"Enabled": false, "CustomCname": "crl.example.com"}}),
            false,
        ),
        (
            "crl disabled with an expiration",
            json!({"CrlConfiguration": {"Enabled": false, "ExpirationInDays": 7}}),
            false,
        ),
        (
            "crl disabled with a bucket",
            json!({"CrlConfiguration": {"Enabled": false, "S3BucketName": "crl-bucket"}}),
            false,
        ),
        (
            "crl enabled without a bucket",
            json!({"CrlConfiguration": {"Enabled": true, "ExpirationInDays": 7}}),
            false,
        ),
        (
            "crl enabled without an expiration",
            json!({"CrlConfiguration": {"Enabled": true, "S3BucketName": "crl-bucket"}}),
            false,
        ),
        (
            "crl enabled with both",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 7,
                    "S3BucketName": "crl-bucket",
                }
            }),
            true,
        ),
        ("ocsp disabled alone", json!({"OcspConfiguration": {"Enabled": false}}), true),
        (
            "ocsp disabled with a cname",
            json!({"OcspConfiguration": {"Enabled": false, "OcspCustomCname": "ocsp.example.com"}}),
            false,
        ),
        (
            "ocsp enabled with a cname",
            json!({"OcspConfiguration": {"Enabled": true, "OcspCustomCname": "ocsp.example.com"}}),
            true,
        ),
    ];

    // Create (or refuse) every row first, then wait for the clock ONCE before reading
    // the accepted ones back.
    let mut created: Vec<(&str, &Value, String)> = Vec::new();
    for (case, revocation, accepted) in rows {
        let mut body = root_ca_request("revocation-root");
        body["RevocationConfiguration"] = revocation.clone();
        if *accepted {
            let response = emulator.ok(case, "CreateCertificateAuthority", &body).await;
            let arn = string(&response, "CertificateAuthorityArn", case);
            created.push((case, revocation, arn));
        } else {
            emulator
                .refused(case, "CreateCertificateAuthority", &body, "InvalidArgsException")
                .await;
        }
    }
    wait_for_the_clock();

    for (case, revocation, arn) in created {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            described["CertificateAuthority"]["RevocationConfiguration"],
            *revocation,
            "case {case:?}: the describe echoes the stored revocation configuration"
        );
    }
}

/// A certificate body or certification request the service cannot read as PEM is
/// malformed; one it can read is accepted whatever its line endings and block count.
#[tokio::test]
async fn pem_bodies_are_accepted_only_where_they_are_well_formed() {
    let emulator = Emulator::new();

    let certificate = pem("CERTIFICATE", "a certificate");
    let malformed: &[(&str, String)] = &[
        ("not pem at all", "a certificate".to_string()),
        ("the wrong label", pem("CERTIFICATE REQUEST", "a request")),
        (
            "a body that is not base64",
            "-----BEGIN CERTIFICATE-----\nnot base64 at all!!\n-----END CERTIFICATE-----\n"
                .to_string(),
        ),
        (
            "an empty block",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "a block with no end",
            "-----BEGIN CERTIFICATE-----\nYSBjZXJ0aWZpY2F0ZQ==\n".to_string(),
        ),
    ];
    for (case, body) in malformed {
        let (arn, _) = {
            let created = emulator
                .ok(case, "CreateCertificateAuthority", &root_ca_request("pem-root"))
                .await;
            (string(&created, "CertificateAuthorityArn", case), ())
        };
        wait_for_the_clock();
        emulator
            .refused(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn, "Certificate": blob(body)}),
                "MalformedCertificateException",
            )
            .await;
        // The CA is untouched by a refused import, so it still takes a good one.
        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&certificate)}),
            )
            .await;
    }

    // Accepted bodies: CRLF line endings, no trailing newline, and a chain of several
    // blocks.
    let accepted: &[(&str, String, Option<String>)] = &[
        ("crlf", certificate.replace('\n', "\r\n"), None),
        (
            "no trailing newline",
            certificate.trim_end().to_string(),
            None,
        ),
        (
            "a two-certificate chain",
            certificate.clone(),
            Some(format!(
                "{}{}",
                pem("CERTIFICATE", "an intermediate"),
                pem("CERTIFICATE", "a root")
            )),
        ),
        ("an empty chain", certificate.clone(), Some(String::new())),
    ];
    for (case, body, chain) in accepted {
        let created = emulator
            .ok(case, "CreateCertificateAuthority", &root_ca_request("pem-root"))
            .await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        wait_for_the_clock();
        let mut import = json!({"CertificateAuthorityArn": arn, "Certificate": blob(body)});
        if let Some(chain) = chain {
            import["CertificateChain"] = blob(chain);
        }
        emulator
            .ok(case, "ImportCertificateAuthorityCertificate", &import)
            .await;
        let read_back = emulator
            .ok(
                case,
                "GetCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        assert_eq!(
            string(&read_back, "Certificate", case),
            certificate,
            "case {case:?}: a well-formed body reads back in canonical PEM form"
        );
        let expected_chain = chain
            .as_deref()
            .filter(|chain| !chain.is_empty())
            .map(ToString::to_string);
        assert_eq!(
            read_back.get("CertificateChain").and_then(Value::as_str).map(ToString::to_string),
            expected_chain,
            "case {case:?}: the chain reads back exactly as imported"
        );
    }

    // A certification request is held to the same rule, under its own error.
    let (arn, _) = active_ca(&emulator, "csr-root").await;
    for (case, body) in [
        ("not pem at all", "a request".to_string()),
        ("the wrong label", certificate.clone()),
    ] {
        emulator
            .refused(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&body),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": 7, "Type": "DAYS"},
                }),
                "MalformedCSRException",
            )
            .await;
    }
}

/// An ARN that is not this service's refers to no resource; a well-formed one that
/// names nothing stored is not found. The rule holds for every operation that takes a
/// CA ARN.
#[tokio::test]
async fn ca_arns_are_refused_by_form_and_then_by_existence() {
    let emulator = Emulator::new();
    let certificate = blob(&pem("CERTIFICATE", "a certificate"));

    let malformed = [
        ("no ca id", format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority")),
        (
            "an extra resource segment",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/abc/extra"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abc"),
        ),
    ];
    let unknown = format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         00000000-0000-4000-8000-000000000000"
    );

    for (operation, extra) in [
        ("DescribeCertificateAuthority", json!({})),
        ("GetCertificateAuthorityCsr", json!({})),
        ("GetCertificateAuthorityCertificate", json!({})),
        (
            "ImportCertificateAuthorityCertificate",
            json!({"Certificate": certificate}),
        ),
        (
            "IssueCertificate",
            json!({
                "Csr": blob(&pem("CERTIFICATE REQUEST", "a request")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Value": 7, "Type": "DAYS"},
            }),
        ),
    ] {
        for (case, arn) in &malformed {
            let case = format!("{operation}: {case}");
            let mut body = extra.clone();
            body["CertificateAuthorityArn"] = json!(arn);
            emulator
                .refused(&case, operation, &body, "InvalidArnException")
                .await;
        }
        let case = format!("{operation}: a CA that was never created");
        let mut body = extra.clone();
        body["CertificateAuthorityArn"] = json!(unknown);
        emulator
            .refused(&case, operation, &body, "ResourceNotFoundException")
            .await;
    }

    // GetCertificate reads two ARNs, and holds the certificate ARN to the same rule.
    let (arn, _) = active_ca(&emulator, "arn-root").await;
    for (case, certificate_arn) in [
        ("not a certificate ARN", arn.clone()),
        (
            "a certificate of no CA",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate/abc"),
        ),
    ] {
        emulator
            .refused(
                case,
                "GetCertificate",
                &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
                "InvalidArnException",
            )
            .await;
    }
}

/// `CreateCertificateAuthority` refuses a configuration or a tag set the service cannot
/// accept, and only those.
#[tokio::test]
async fn create_refuses_an_unusable_configuration_or_tag_set() {
    let emulator = Emulator::new();

    // A subject with no relative distinguished name names nobody.
    let mut body = root_ca_request("ignored");
    body["CertificateAuthorityConfiguration"]["Subject"] = json!({});
    emulator
        .refused(
            "an empty subject",
            "CreateCertificateAuthority",
            &body,
            "InvalidArgsException",
        )
        .await;

    // Any single X.500 attribute is enough, not only CommonName.
    for (case, subject) in [
        ("common name", json!({"CommonName": "example"})),
        ("organization", json!({"Organization": "Example Inc"})),
        ("country", json!({"Country": "GB"})),
        (
            "a custom attribute",
            json!({"CustomAttributes": [{"ObjectIdentifier": "2.5.4.3", "Value": "example"}]}),
        ),
    ] {
        let mut body = root_ca_request("ignored");
        body["CertificateAuthorityConfiguration"]["Subject"] = subject;
        emulator.ok(case, "CreateCertificateAuthority", &body).await;
    }

    let tag_rows: &[(&str, Value, bool)] = &[
        ("one tag", json!([{"Key": "owner", "Value": "pki"}]), true),
        ("a valueless tag", json!([{"Key": "owner"}]), true),
        (
            "two distinct keys",
            json!([{"Key": "owner", "Value": "pki"}, {"Key": "tier", "Value": "root"}]),
            true,
        ),
        (
            "a repeated key",
            json!([{"Key": "owner", "Value": "pki"}, {"Key": "owner", "Value": "sre"}]),
            false,
        ),
        ("a reserved key", json!([{"Key": "aws:created-by", "Value": "me"}]), false),
    ];
    for (case, tags, accepted) in tag_rows {
        let mut body = root_ca_request("tagged-root");
        body["Tags"] = tags.clone();
        if *accepted {
            emulator.ok(case, "CreateCertificateAuthority", &body).await;
        } else {
            emulator
                .refused(case, "CreateCertificateAuthority", &body, "InvalidTagException")
                .await;
        }
    }
}

/// `CreateCertificateAuthority` is `@idempotent` on its token, and only on its token.
#[tokio::test]
async fn create_mints_one_ca_per_idempotency_token() {
    let emulator = Emulator::new();
    let case = "idempotency";

    let mut with_token = root_ca_request("token-root");
    with_token["IdempotencyToken"] = json!("tok-1");
    let first = string(
        &emulator.ok(case, "CreateCertificateAuthority", &with_token).await,
        "CertificateAuthorityArn",
        case,
    );
    let repeated = string(
        &emulator.ok(case, "CreateCertificateAuthority", &with_token).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_eq!(
        first, repeated,
        "case {case:?}: the same IdempotencyToken must answer with the first CA"
    );

    let mut other_token = root_ca_request("token-root");
    other_token["IdempotencyToken"] = json!("tok-2");
    let second = string(
        &emulator.ok(case, "CreateCertificateAuthority", &other_token).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_ne!(
        first, second,
        "case {case:?}: a different IdempotencyToken must mint a different CA"
    );

    let untokened = root_ca_request("token-root");
    let third = string(
        &emulator.ok(case, "CreateCertificateAuthority", &untokened).await,
        "CertificateAuthorityArn",
        case,
    );
    let fourth = string(
        &emulator.ok(case, "CreateCertificateAuthority", &untokened).await,
        "CertificateAuthorityArn",
        case,
    );
    assert_ne!(
        third, fourth,
        "case {case:?}: without a token every call mints its own CA"
    );
}

/// Issuing repeatedly from one CA: every certificate gets its own serial and ARN, and
/// each one reads back as the certificate issued for exactly that ARN. An invariant over
/// a run of issuances, not a single case.
#[tokio::test]
async fn every_issuance_from_one_ca_is_its_own_certificate() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "serial-root").await;
    const ISSUANCES: usize = 16;

    let mut arns: Vec<String> = Vec::with_capacity(ISSUANCES);
    for index in 0..ISSUANCES {
        let case = format!("issuance {index}");
        // The request differs per issuance, so two certificates never derive from
        // identical input either.
        let csr = pem("CERTIFICATE REQUEST", &format!("end entity {index}"));
        let issued = emulator
            .ok(
                &case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Value": index as i64 + 1, "Type": "DAYS"},
                }),
            )
            .await;
        arns.push(string(&issued, "CertificateArn", &case));
    }
    wait_for_the_clock();

    let mut seen = std::collections::HashSet::new();
    for (index, certificate_arn) in arns.iter().enumerate() {
        let case = format!("issuance {index}");
        assert!(
            seen.insert(certificate_arn.clone()),
            "case {case:?}: {certificate_arn:?} was minted twice"
        );
        let got = emulator
            .ok(
                &case,
                "GetCertificate",
                &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
            )
            .await;
        let certificate = string(&got, "Certificate", &case);
        let serial = certificate_arn
            .rsplit_once("/certificate/")
            .map(|(_, serial)| serial.to_string())
            .unwrap_or_default();
        assert_eq!(
            field(&certificate, "serial"),
            serial,
            "case {case:?}: {certificate_arn:?} must answer with the certificate of its serial"
        );
        assert_eq!(
            field(&certificate, "csr-sha256"),
            sha256_hex(&pem("CERTIFICATE REQUEST", &format!("end entity {index}"))),
            "case {case:?}: the certificate records the request it answers"
        );
    }
}

/// A CA that holds no certificate cannot sign with one, and cannot hand one out.
#[tokio::test]
async fn a_ca_without_its_certificate_can_neither_issue_nor_serve_one() {
    let emulator = Emulator::new();
    let case = "pending CA";
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request("pending-root"))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    wait_for_the_clock();

    emulator
        .refused(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob(&pem("CERTIFICATE REQUEST", "a request")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Value": 7, "Type": "DAYS"},
            }),
            "InvalidStateException",
        )
        .await;
    emulator
        .refused(
            case,
            "GetCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn}),
            "InvalidStateException",
        )
        .await;

    // And once it is ACTIVE it cannot take a second certificate.
    let certificate = blob(&pem("CERTIFICATE", "a certificate"));
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": certificate}),
        )
        .await;
    emulator
        .refused(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": certificate}),
            "InvalidStateException",
        )
        .await;
}

/// A CA whose UsageMode is SHORT_LIVED_CERTIFICATE issues only inside the seven days
/// its documentation limits it to.
#[tokio::test]
async fn a_short_lived_ca_refuses_a_validity_beyond_seven_days() {
    let emulator = Emulator::new();
    let case = "short lived";
    let mut body = root_ca_request("short-lived-root");
    body["UsageMode"] = json!("SHORT_LIVED_CERTIFICATE");
    let arn = string(
        &emulator.ok(case, "CreateCertificateAuthority", &body).await,
        "CertificateAuthorityArn",
        case,
    );
    wait_for_the_clock();
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&pem("CERTIFICATE", "a certificate")),
            }),
        )
        .await;

    for (case, days, accepted) in [
        ("one day", 1, true),
        ("seven days", 7, true),
        ("eight days", 8, false),
        ("a year", 365, false),
    ] {
        let request = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&pem("CERTIFICATE REQUEST", "a request")),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": days, "Type": "DAYS"},
        });
        if accepted {
            emulator.ok(case, "IssueCertificate", &request).await;
        } else {
            emulator
                .refused(case, "IssueCertificate", &request, "InvalidArgsException")
                .await;
        }
    }
}

/// The emulator's state survives between requests, which is what makes the lifecycle
/// above a lifecycle: the CA read in one call is the CA written by an earlier one.
#[tokio::test]
async fn a_created_ca_is_visible_to_every_later_request() {
    let emulator = Emulator::new();
    let case = "shared state";
    let mut arns = Vec::new();
    for index in 0..3 {
        let created = emulator
            .ok(
                case,
                "CreateCertificateAuthority",
                &root_ca_request(&format!("root-{index}")),
            )
            .await;
        arns.push(string(&created, "CertificateAuthorityArn", case));
    }
    wait_for_the_clock();
    for (index, arn) in arns.iter().enumerate() {
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            described["CertificateAuthority"]["CertificateAuthorityConfiguration"]["Subject"]
                ["CommonName"],
            json!(format!("root-{index}")),
            "case {case:?}: {arn:?} must still be the CA that was created"
        );
        let created_at = described["CertificateAuthority"]["CreatedAt"]
            .as_f64()
            .unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        assert!(
            created_at > 0.0 && created_at <= now + 1.0,
            "case {case:?}: CreatedAt must be the instant the CA was created, got {created_at}"
        );
    }
}

/// Hex-encoded SHA-256 of `text`, the digest a derived certificate records for the
/// request it answers.
fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

/// "The specified signing algorithm family (RSA or ECDSA) must match the algorithm
/// family of the CA's secret key." The rule holds over every `KeyAlgorithm` /
/// `SigningAlgorithm` pair the model declares, not only the RSA one the issue cited.
#[tokio::test]
async fn a_ca_signs_only_with_its_own_key_family() {
    let emulator = Emulator::new();

    // Rows drawn from KEY_ALGORITHM_VALUES / SIGNING_ALGORITHM_VALUES in `ops::types`.
    let rows: &[(&str, &str, &str, bool)] = &[
        ("rsa with rsa", "RSA_2048", "SHA256WITHRSA", true),
        ("rsa with rsa-pss", "RSA_4096", "SHA512WITHRSA_PSS", true),
        ("rsa with ecdsa", "RSA_2048", "SHA256WITHECDSA", false),
        ("ec with ecdsa", "EC_prime256v1", "SHA384WITHECDSA", true),
        ("ec with rsa", "EC_secp521r1", "SHA256WITHRSA", false),
        ("lattice with lattice", "ML_DSA_44", "ML_DSA_44", true),
        ("lattice with rsa", "ML_DSA_87", "SHA384WITHRSA", false),
        ("sm2 with sm3", "SM2", "SM3WITHSM2", true),
        ("sm2 with ecdsa", "SM2", "SHA256WITHECDSA", false),
    ];

    // Create every CA, then wait for the clock ONCE, then activate them all.
    let mut authorities: Vec<String> = Vec::with_capacity(rows.len());
    for (case, key_algorithm, _, _) in rows {
        let mut body = root_ca_request("family-root");
        body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!(key_algorithm);
        // The CA's own CSR is signed with a signature of its key's family.
        body["CertificateAuthorityConfiguration"]["SigningAlgorithm"] = json!(match *key_algorithm {
            "RSA_2048" | "RSA_4096" => "SHA256WITHRSA",
            "EC_prime256v1" | "EC_secp521r1" => "SHA256WITHECDSA",
            "SM2" => "SM3WITHSM2",
            lattice => lattice,
        });
        authorities.push(string(
            &emulator.ok(case, "CreateCertificateAuthority", &body).await,
            "CertificateAuthorityArn",
            case,
        ));
    }
    wait_for_the_clock();

    for ((case, key_algorithm, signing_algorithm, accepted), arn) in rows.iter().zip(&authorities) {
        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Certificate": blob(&pem("CERTIFICATE", &format!("ca for {key_algorithm}"))),
                }),
            )
            .await;
        let request = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&pem("CERTIFICATE REQUEST", "a request")),
            "SigningAlgorithm": signing_algorithm,
            "Validity": {"Value": 7, "Type": "DAYS"},
        });
        if *accepted {
            emulator.ok(case, "IssueCertificate", &request).await;
        } else {
            emulator
                .refused(case, "IssueCertificate", &request, "InvalidArgsException")
                .await;
        }
    }
}

/// Every store this service grows per request is bounded, and the bound is answered with
/// the quota error the operation models rather than by growing without limit.
#[tokio::test]
async fn the_per_request_stores_are_bounded() {
    use service_com_amazonaws_acmpca_acmprivateca::state::{
        MAX_CERTIFICATES_PER_CA, MAX_CERTIFICATE_AUTHORITIES, MAX_IDEMPOTENCY_TOKENS,
    };

    let emulator = Emulator::new();

    // Certificates per CA: the last one inside the bound is accepted, the next refused.
    let (arn, _) = active_ca(&emulator, "bounded-root").await;
    let issue = |index: usize| {
        json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&pem("CERTIFICATE REQUEST", &format!("end entity {index}"))),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": 7, "Type": "DAYS"},
        })
    };
    // One issuance for every slot the bound allows; the next one is past it.
    for index in 0..MAX_CERTIFICATES_PER_CA {
        emulator
            .ok(&format!("certificate {index}"), "IssueCertificate", &issue(index))
            .await;
    }
    emulator
        .refused(
            "one certificate past the quota",
            "IssueCertificate",
            &issue(MAX_CERTIFICATES_PER_CA),
            "LimitExceededException",
        )
        .await;

    // Idempotency tokens: beyond the bound the oldest record is evicted, so the token it
    // held no longer replays while the newest one still does.
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "token-bound-root").await;
    let mut minted: Vec<String> = Vec::new();
    for index in 0..=MAX_IDEMPOTENCY_TOKENS {
        let case = format!("token {index}");
        let mut body = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&pem("CERTIFICATE REQUEST", &format!("end entity {index}"))),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": 7, "Type": "DAYS"},
        });
        body["IdempotencyToken"] = json!(format!("tok-{index}"));
        minted.push(string(
            &emulator.ok(&case, "IssueCertificate", &body).await,
            "CertificateArn",
            &case,
        ));
    }
    let replay = |index: usize| {
        let mut body = json!({
            "CertificateAuthorityArn": arn,
            "Csr": blob(&pem("CERTIFICATE REQUEST", &format!("end entity {index}"))),
            "SigningAlgorithm": "SHA256WITHRSA",
            "Validity": {"Value": 7, "Type": "DAYS"},
        });
        body["IdempotencyToken"] = json!(format!("tok-{index}"));
        body
    };
    let newest = MAX_IDEMPOTENCY_TOKENS;
    assert_eq!(
        string(
            &emulator.ok("newest token", "IssueCertificate", &replay(newest)).await,
            "CertificateArn",
            "newest token",
        ),
        minted[newest],
        "case {:?}: a live idempotency record must still replay",
        "newest token"
    );
    assert_ne!(
        string(
            &emulator.ok("evicted token", "IssueCertificate", &replay(0)).await,
            "CertificateArn",
            "evicted token",
        ),
        minted[0],
        "case {:?}: the oldest record is evicted at the bound, so its token mints anew",
        "evicted token"
    );

    // Certificate authorities: likewise bounded.
    let emulator = Emulator::new();
    for index in 0..MAX_CERTIFICATE_AUTHORITIES {
        emulator
            .ok(
                &format!("ca {index}"),
                "CreateCertificateAuthority",
                &root_ca_request(&format!("root-{index}")),
            )
            .await;
    }
    emulator
        .refused(
            "one CA past the quota",
            "CreateCertificateAuthority",
            &root_ca_request("one-too-many"),
            "LimitExceededException",
        )
        .await;
}
