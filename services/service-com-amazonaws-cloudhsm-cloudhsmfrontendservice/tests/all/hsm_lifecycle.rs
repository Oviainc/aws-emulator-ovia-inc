// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudHSM Classic HSM lifecycle over the wire.
//!
//! Every case drives the real awsJson1_1 front door — `POST /` with
//! `X-Amz-Target: CloudHsmFrontendService.<Operation>` — through the same
//! [`MultiServiceRouter`] the production router mounts, so what is exercised is the
//! dispatch → deserialize → handle → serialize walk, not a handler called directly.
//! Nothing reaches into the service's state struct, so an answer that is not computed
//! from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * `CreateHsm` persists every member it was given and allocates the identifiers the
//!   model publishes for an HSM, so a later `DescribeHsm` reports them back;
//! * a create that repeats a `ClientToken` provisions nothing and reports the first
//!   ARN;
//! * a created HSM is `PENDING` and becomes `RUNNING` once provisioning is due;
//! * `ListHsms` reports exactly the stored ARNs, pages them, and shrinks on delete;
//! * `ModifyHsm` moves ONLY the members the request supplied;
//! * `DeleteHsm` removes the record, so a second delete, a describe and the list all
//!   see it gone;
//! * an identifier that no stored HSM answers to is `CloudHsmServiceException` (400),
//!   and a parameter the model's constraints cannot reject is `InvalidRequestException`
//!   (400).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cloudhsm_cloudhsmfrontendservice::state;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulator ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An `IamRoleArn` that satisfies the model's pattern
/// (`^arn:aws(-iso)?:iam::[0-9]{12}:role/[a-zA-Z0-9_\+=,\.\-@]{1,64}$`).
fn iam_role_arn(role: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role}")
}

/// One emulator: a shared engine, one mounted CloudHSM service, no signature checking
/// (these tests assert service behaviour, not SigV4).
struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    fn new() -> Self {
        Self::in_region("us-east-1")
    }

    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudhsm_cloudhsmfrontendservice::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// Send one operation and return `(status, body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("CloudHsmFrontendService.{operation}"),
            )
            // The credential scope is what routes the request to `cloudhsm` and what
            // names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{}/cloudhsm/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000",
                    self.region
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Send an operation that must succeed, returning its body.
    async fn ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} {payload}"
        );
        payload
    }

    /// Create an HSM from `request` and return its ARN.
    async fn create_hsm(&self, case: &str, request: Value) -> String {
        let payload = self.ok(case, "CreateHsm", request).await;
        string(&payload, "HsmArn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateHsm must report an HsmArn: {payload}"))
    }

    /// Describe the HSM `hsm_arn` names.
    async fn describe_hsm(&self, case: &str, hsm_arn: &str) -> Value {
        self.ok(case, "DescribeHsm", json!({ "HsmArn": hsm_arn }))
            .await
    }

    /// Every ARN `ListHsms` reports, following `NextToken` to the end.
    async fn list_all_hsms(&self, case: &str) -> Vec<String> {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let request = match &token {
                Some(next) => json!({ "NextToken": next }),
                None => json!({}),
            };
            let payload = self.ok(case, "ListHsms", request).await;
            let page = payload
                .get("HsmList")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: ListHsms must report an HsmList: {payload}")
                });
            seen.extend(
                page.iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string()),
            );
            token = string(&payload, "NextToken");
            if token.is_none() {
                return seen;
            }
            assert!(
                seen.len() <= 10_000,
                "case {case:?}: ListHsms paged past every plausible store size"
            );
        }
    }
}

/// A present, non-null string member of a response body.
fn string(payload: &Value, member: &str) -> Option<String> {
    payload
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A present, non-null string member of a response body, or a failed assertion.
fn require(case: &str, payload: &Value, member: &str) -> String {
    string(payload, member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be reported, got {payload}"))
}

/// The wire error code of a failure body (`__type`, possibly namespace-qualified).
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert an operation failed with `status` and the wire error code `code`.
async fn assert_error(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: Value,
    status: u16,
    code: &str,
) {
    let (actual_status, payload) = emulator.call(operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: {operation} must answer HTTP {status}, got {actual_status} {payload}"
    );
    assert_eq!(
        error_code(&payload),
        code,
        "case {case:?}: {operation} must answer {code}, got {payload}"
    );
}

/// Whether `value` is exactly `len` lowercase hex digits — the form every
/// `hsm-`/`eni-`/`vpc-` identifier in this model ends in.
fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Whether `value` is one to `max` ASCII digits — the `^\d{1,N}$` form the model's
/// `HsmSerialNumber` and `Timestamp` members carry.
fn is_digits(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// CreateHsm / DescribeHsm
// ---------------------------------------------------------------------------

/// One row of the create table: a label, the `CreateHsm` request, and the members a
/// later `DescribeHsm` must report back for it.
struct CreateCase {
    case: &'static str,
    request: Value,
}

/// Create requests spanning the class: the four required members are always there, and
/// the rows differ along what the rule generalises over — which optional members are
/// supplied (none, one, all), the subnet named, the SSH key's form and length, and the
/// IAM role.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            // The bootstrap issue's own row: required members only.
            case: "required members only",
            request: json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDemulator",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
            }),
        },
        CreateCase {
            case: "every optional member supplied",
            request: json!({
                "SubnetId": "subnet-ffffffff",
                "SshKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEmulatorKey",
                "IamRoleArn": iam_role_arn("hsm-role_2"),
                "SubscriptionType": "PRODUCTION",
                "EniIp": "10.1.2.3",
                "ExternalId": "external-id/42",
                "SyslogIp": "192.168.0.254",
                "ClientToken": "tokenEveryOptional1",
            }),
        },
        CreateCase {
            case: "syslog only",
            request: json!({
                "SubnetId": "subnet-00000000",
                "SshKey": "AAAA",
                "IamRoleArn": iam_role_arn("r"),
                "SubscriptionType": "PRODUCTION",
                "SyslogIp": "0.0.0.0",
            }),
        },
        CreateCase {
            case: "eni ip at the top of the range",
            request: json!({
                "SubnetId": "subnet-deadbeef",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("role.with.dots-and_underscores"),
                "SubscriptionType": "PRODUCTION",
                "EniIp": "255.255.255.255",
            }),
        },
        CreateCase {
            case: "external id only",
            request: json!({
                "SubnetId": "subnet-0a1b2c3d",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC0123456789",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
                "ExternalId": "team:payments",
            }),
        },
    ]
}

#[tokio::test]
async fn describe_hsm_reports_the_record_create_hsm_persisted() {
    let emulator = Emulator::new();

    let mut arns = BTreeSet::new();
    let mut eni_ids = BTreeSet::new();
    let mut serials = BTreeSet::new();

    for CreateCase { case, request } in create_cases() {
        let arn = emulator.create_hsm(case, request.clone()).await;

        // The ARN is the `HsmArn` form the model publishes:
        // `^arn:aws(-iso)?:cloudhsm:[a-zA-Z0-9\-]*:[0-9]{12}:hsm-[0-9a-f]{8}$`.
        let prefix = format!("arn:aws:cloudhsm:{}:{ACCOUNT}:hsm-", emulator.region);
        let hsm_id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: HsmArn must start with {prefix:?}, got {arn:?}")
        });
        assert!(
            is_lower_hex(hsm_id, 8),
            "case {case:?}: HsmArn must end in hsm-<8 lowercase hex>, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: every create must mint a fresh HsmArn, got the known {arn:?}"
        );

        let described = emulator.describe_hsm(case, &arn).await;

        // Every member the request supplied comes back out of the stored record.
        assert_eq!(
            string(&described, "HsmArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: DescribeHsm must report the ARN it was asked about, got {described}"
        );
        for (input_member, output_member) in [
            ("SubnetId", "SubnetId"),
            ("IamRoleArn", "IamRoleArn"),
            ("SubscriptionType", "SubscriptionType"),
            ("SshKey", "SshPublicKey"),
            ("SyslogIp", "SyslogIp"),
            ("EniIp", "EniIp"),
        ] {
            if let Some(expected) = string(&request, input_member) {
                // `SyslogIp` and `ExternalId` are stored but not reported by
                // `DescribeHsm`'s model; only the members the output declares are
                // asserted here, and `ModifyHsm` covers the rest.
                if output_member == "SyslogIp" {
                    continue;
                }
                assert_eq!(
                    string(&described, output_member).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: {output_member} must report the {input_member} the create \
                     persisted, got {described}"
                );
            }
        }

        // The identifiers the service allocates: each in the form the model publishes,
        // and each distinct per HSM.
        let eni_id = require(case, &described, "EniId");
        assert!(
            eni_id
                .strip_prefix("eni-")
                .is_some_and(|id| is_lower_hex(id, 8)),
            "case {case:?}: EniId must be eni-<8 lowercase hex>, got {eni_id:?}"
        );
        assert!(
            eni_ids.insert(eni_id.clone()),
            "case {case:?}: every HSM must get its own ENI, got the known {eni_id:?}"
        );

        let vpc_id = require(case, &described, "VpcId");
        assert!(
            vpc_id
                .strip_prefix("vpc-")
                .is_some_and(|id| is_lower_hex(id, 8)),
            "case {case:?}: VpcId must be vpc-<8 lowercase hex>, got {vpc_id:?}"
        );

        let zone = require(case, &described, "AvailabilityZone");
        assert!(
            zone.starts_with(&emulator.region)
                && zone.len() == emulator.region.len() + 1
                && zone.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "case {case:?}: AvailabilityZone must be <region><letter>, got {zone:?}"
        );

        let serial = require(case, &described, "SerialNumber");
        assert!(
            is_digits(&serial, 16),
            "case {case:?}: SerialNumber must be 1..16 digits, got {serial:?}"
        );
        assert!(
            serials.insert(serial.clone()),
            "case {case:?}: every HSM must get its own serial number, got the known {serial:?}"
        );

        // An ENI always has an address, whether or not the create named one.
        let eni_ip = require(case, &described, "EniIp");
        assert!(
            eni_ip.parse::<std::net::Ipv4Addr>().is_ok(),
            "case {case:?}: EniIp must be an IPv4 address, got {eni_ip:?}"
        );

        // The subscription starts at the create and is open-ended.
        let start = require(case, &described, "SubscriptionStartDate");
        assert!(
            is_digits(&start, 16),
            "case {case:?}: SubscriptionStartDate must be epoch-second digits, got {start:?}"
        );
        assert_eq!(
            string(&described, "SubscriptionEndDate"),
            None,
            "case {case:?}: a live subscription has no end date, got {described}"
        );
        assert_eq!(
            require(case, &described, "SshKeyLastUpdated"),
            start,
            "case {case:?}: the SSH key was last updated at the create, got {described}"
        );

        // A `CreateHsm` HSM is uninitialized: no partitions and no server certificate.
        assert_eq!(
            described.get("Partitions").and_then(Value::as_array),
            Some(&vec![]),
            "case {case:?}: a freshly created HSM holds no partitions, got {described}"
        );
        assert_eq!(
            string(&described, "ServerCertUri"),
            None,
            "case {case:?}: an uninitialized HSM has no server certificate, got {described}"
        );

        // The appliance description is reported, not left empty.
        for member in ["VendorName", "HsmType", "SoftwareVersion"] {
            assert!(
                !require(case, &described, member).is_empty(),
                "case {case:?}: {member} must be reported, got {described}"
            );
        }

        // Reading twice answers identically: a describe reports state, it does not
        // recompute or mint anything.
        let again = emulator.describe_hsm(case, &arn).await;
        assert_eq!(
            again, described,
            "case {case:?}: two describes of one HSM must answer identically"
        );
    }

    // Every created HSM is in the list, and nothing else is.
    let listed: BTreeSet<String> = emulator
        .list_all_hsms("every create is listed")
        .await
        .into_iter()
        .collect();
    assert_eq!(
        listed, arns,
        "ListHsms must report exactly the stored HsmArns"
    );
}

#[tokio::test]
async fn hsms_in_one_subnet_share_a_vpc_and_zone_and_different_subnets_do_not() {
    let emulator = Emulator::new();

    // Two HSMs in the same subnet, one in another: the VPC and the zone are properties
    // of the SUBNET, so they are shared by the first two and resolved independently for
    // the third.
    let mut placements = Vec::new();
    for (case, subnet) in [
        ("first in subnet A", "subnet-aaaaaaaa"),
        ("second in subnet A", "subnet-aaaaaaaa"),
        ("only one in subnet B", "subnet-bbbbbbbb"),
    ] {
        let arn = emulator
            .create_hsm(
                case,
                json!({
                    "SubnetId": subnet,
                    "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                    "IamRoleArn": iam_role_arn("CloudHsmRole"),
                    "SubscriptionType": "PRODUCTION",
                }),
            )
            .await;
        let described = emulator.describe_hsm(case, &arn).await;
        placements.push((
            case,
            require(case, &described, "VpcId"),
            require(case, &described, "AvailabilityZone"),
        ));
    }

    assert_eq!(
        (placements[0].1.clone(), placements[0].2.clone()),
        (placements[1].1.clone(), placements[1].2.clone()),
        "case {:?}: two HSMs in one subnet must report one VPC and one zone, got {placements:?}",
        placements[1].0
    );
    assert_ne!(
        placements[0].1, placements[2].1,
        "case {:?}: a different subnet must resolve to a different VPC, got {placements:?}",
        placements[2].0
    );
}

#[tokio::test]
async fn arns_carry_the_region_the_request_was_signed_for() {
    // The rule generalises over the signing region, so the table is three regions
    // rather than the one the issue recorded.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let arn = emulator
            .create_hsm(
                region,
                json!({
                    "SubnetId": "subnet-0123abcd",
                    "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                    "IamRoleArn": iam_role_arn("CloudHsmRole"),
                    "SubscriptionType": "PRODUCTION",
                }),
            )
            .await;
        assert!(
            arn.starts_with(&format!("arn:aws:cloudhsm:{region}:{ACCOUNT}:hsm-")),
            "case {region:?}: the HsmArn must carry the signing region, got {arn:?}"
        );
        let described = emulator.describe_hsm(region, &arn).await;
        let zone = require(region, &described, "AvailabilityZone");
        assert!(
            zone.starts_with(region),
            "case {region:?}: the AvailabilityZone must be in the signing region, got {zone:?}"
        );
    }
}

#[tokio::test]
async fn describe_hsm_resolves_an_hsm_by_its_serial_number() {
    let emulator = Emulator::new();

    // Three HSMs, so resolving by serial number has to pick the right one rather than
    // the only one.
    let mut arns = Vec::new();
    for case in ["first", "second", "third"] {
        arns.push(
            emulator
                .create_hsm(
                    case,
                    json!({
                        "SubnetId": "subnet-0123abcd",
                        "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                        "IamRoleArn": iam_role_arn("CloudHsmRole"),
                        "SubscriptionType": "PRODUCTION",
                    }),
                )
                .await,
        );
    }

    for (case, arn) in [("first", &arns[0]), ("second", &arns[1]), ("third", &arns[2])] {
        let by_arn = emulator.describe_hsm(case, arn).await;
        let serial = require(case, &by_arn, "SerialNumber");

        let by_serial = emulator
            .ok(case, "DescribeHsm", json!({ "HsmSerialNumber": serial }))
            .await;
        assert_eq!(
            by_serial, by_arn,
            "case {case:?}: HsmSerialNumber must resolve the same HSM as its ARN"
        );

        // An ARN and a serial number together must name the same HSM.
        let both = emulator
            .ok(
                case,
                "DescribeHsm",
                json!({ "HsmArn": arn, "HsmSerialNumber": serial }),
            )
            .await;
        assert_eq!(
            both, by_arn,
            "case {case:?}: a matching ARN and serial number must resolve that HSM"
        );
    }

    // A serial number belonging to another HSM contradicts the ARN, so the pair names
    // nothing.
    let other_serial = require(
        "mismatched pair",
        &emulator.describe_hsm("mismatched pair", &arns[1]).await,
        "SerialNumber",
    );
    assert_error(
        &emulator,
        "mismatched pair",
        "DescribeHsm",
        json!({ "HsmArn": &arns[0], "HsmSerialNumber": other_serial }),
        400,
        "CloudHsmServiceException",
    )
    .await;

    // Neither identifier given: the model cannot express "at least one of", so the
    // service refuses the request as invalid.
    assert_error(
        &emulator,
        "no identifier",
        "DescribeHsm",
        json!({}),
        400,
        "InvalidRequestException",
    )
    .await;
}

// ---------------------------------------------------------------------------
// ClientToken idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repeated_client_token_provisions_nothing_and_reports_the_first_arn() {
    let emulator = Emulator::new();

    // The rows differ in the token's form and length (the model's `ClientToken` is
    // `^[a-zA-Z0-9]{1,64}$`), and in how much of the rest of the request repeats.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "single character token",
            "a",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
            }),
        ),
        (
            "64 character token at the model's bound",
            "A1234567890123456789012345678901234567890123456789012345678901b",
            json!({
                "SubnetId": "subnet-ffffffff",
                "SshKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5",
                "IamRoleArn": iam_role_arn("other-role"),
                "SubscriptionType": "PRODUCTION",
                "EniIp": "10.9.8.7",
            }),
        ),
        (
            "mixed case alphanumeric token",
            "Idempotent42Token",
            json!({
                "SubnetId": "subnet-00000000",
                "SshKey": "AAAA",
                "IamRoleArn": iam_role_arn("r"),
                "SubscriptionType": "PRODUCTION",
                "SyslogIp": "10.0.0.9",
            }),
        ),
    ];

    let mut first_arns = Vec::new();
    for (case, token, base) in &cases {
        let mut request = base.clone();
        request["ClientToken"] = json!(token);

        let first = emulator.create_hsm(case, request.clone()).await;
        let second = emulator.create_hsm(case, request.clone()).await;
        assert_eq!(
            second, first,
            "case {case:?}: a repeated ClientToken must report the first HsmArn"
        );

        // A third call with the same token but a different request body is still the
        // same create: the token alone decides.
        let mut varied = request.clone();
        varied["SubnetId"] = json!("subnet-12341234");
        let third = emulator.create_hsm(case, varied).await;
        assert_eq!(
            third, first,
            "case {case:?}: the ClientToken alone decides idempotence"
        );

        first_arns.push(first);
    }

    let listed = emulator.list_all_hsms("one HSM per token").await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "{} tokens must have provisioned {} HSMs, got {listed:?}",
        cases.len(),
        cases.len()
    );
    for arn in &first_arns {
        assert!(
            listed.contains(arn),
            "the HSM minted for each token must be listed: {arn:?} missing from {listed:?}"
        );
    }

    // A token whose HSM has been deleted no longer stands for a create, so a retry
    // provisions a new HSM.
    let (case, token, base) = &cases[0];
    emulator
        .ok(case, "DeleteHsm", json!({ "HsmArn": &first_arns[0] }))
        .await;
    let mut request = base.clone();
    request["ClientToken"] = json!(token);
    let recreated = emulator.create_hsm(case, request).await;
    assert_ne!(
        recreated, first_arns[0],
        "case {case:?}: a token whose HSM was deleted must provision a new HSM"
    );
}

// ---------------------------------------------------------------------------
// The provisioning transition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_hsm_is_pending_and_becomes_running_once_provisioning_is_due() {
    let emulator = Emulator::new();

    // Two HSMs created back to back: the transition is owed per HSM, from its own
    // create, so both must be seen pending and both must reach RUNNING.
    let mut arns = Vec::new();
    for case in ["first", "second"] {
        arns.push(
            emulator
                .create_hsm(
                    case,
                    json!({
                        "SubnetId": "subnet-0123abcd",
                        "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                        "IamRoleArn": iam_role_arn("CloudHsmRole"),
                        "SubscriptionType": "PRODUCTION",
                    }),
                )
                .await,
        );
    }

    for (case, arn) in [("first", &arns[0]), ("second", &arns[1])] {
        let described = emulator.describe_hsm(case, arn).await;
        assert_eq!(
            require(case, &described, "Status"),
            "PENDING",
            "case {case:?}: an HSM is PENDING until provisioning is due, got {described}"
        );
    }

    // The transition is owed by the clock, not by a poll: wait past the published span
    // without describing anything in between.
    tokio::time::sleep(state::PROVISIONING_DELAY + std::time::Duration::from_millis(250)).await;

    for (case, arn) in [("first", &arns[0]), ("second", &arns[1])] {
        let described = emulator.describe_hsm(case, arn).await;
        assert_eq!(
            require(case, &described, "Status"),
            "RUNNING",
            "case {case:?}: a provisioned HSM is RUNNING, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// ModifyHsm
// ---------------------------------------------------------------------------

/// Every member `DescribeHsm` reports that `ModifyHsm` can move, and the members it
/// must never move, as `(member, value)` pairs read off the describe body.
fn describe_snapshot(payload: &Value) -> Vec<(String, Option<String>)> {
    [
        "HsmArn",
        "Status",
        "StatusDetails",
        "AvailabilityZone",
        "EniId",
        "EniIp",
        "SubscriptionType",
        "SubscriptionStartDate",
        "SubscriptionEndDate",
        "VpcId",
        "SubnetId",
        "IamRoleArn",
        "SerialNumber",
        "VendorName",
        "HsmType",
        "SoftwareVersion",
        "SshPublicKey",
        "SshKeyLastUpdated",
        "ServerCertUri",
        "ServerCertLastUpdated",
    ]
    .into_iter()
    .map(|member| (member.to_string(), string(payload, member)))
    .collect()
}

/// One row of the modify table: the patch to send, and the `DescribeHsm` members that
/// must report the patched values afterwards.
///
/// `SyslogIp` and `ExternalId` are stored by `ModifyHsm` but `DescribeHsmResponse`
/// declares no member for either (`smithy_query(query="operation",
/// operation="DescribeHsm")`), so a row that patches one of them has nothing to assert
/// beyond "every member the request did NOT supply is unchanged".
struct ModifyCase {
    case: &'static str,
    patch: Value,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn modify_hsm_moves_only_the_members_the_request_supplied() {
    // Each row is one subset of `ModifyHsm`'s optional members — the axis the rule
    // generalises over — paired with the `DescribeHsm` members that must follow it.
    let cases = vec![
        ModifyCase {
            // The issue's own row: one member, nothing else touched.
            case: "syslog ip alone",
            patch: json!({ "SyslogIp": "10.0.0.9" }),
            expected: vec![],
        },
        ModifyCase {
            case: "eni ip alone",
            patch: json!({ "EniIp": "10.4.5.6" }),
            expected: vec![("EniIp", "10.4.5.6")],
        },
        ModifyCase {
            case: "iam role alone",
            patch: json!({ "IamRoleArn": iam_role_arn("ModifiedRole") }),
            expected: vec![],
        },
        ModifyCase {
            case: "external id alone",
            patch: json!({ "ExternalId": "external:2" }),
            expected: vec![],
        },
        ModifyCase {
            case: "subnet alone",
            patch: json!({ "SubnetId": "subnet-99999999" }),
            expected: vec![("SubnetId", "subnet-99999999")],
        },
        ModifyCase {
            case: "every modifiable member at once",
            patch: json!({
                "SubnetId": "subnet-77777777",
                "EniIp": "172.16.0.1",
                "ExternalId": "all-at-once",
                "SyslogIp": "172.16.0.2",
            }),
            expected: vec![("SubnetId", "subnet-77777777"), ("EniIp", "172.16.0.1")],
        },
        ModifyCase {
            case: "no member at all",
            patch: json!({}),
            expected: vec![],
        },
    ];

    for ModifyCase {
        case,
        patch,
        expected,
    } in cases
    {
        // A fresh emulator per row, so each row starts from the same stored record.
        let emulator = Emulator::new();
        let arn = emulator
            .create_hsm(
                case,
                json!({
                    "SubnetId": "subnet-0123abcd",
                    "SshKey": "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDemulator",
                    "IamRoleArn": iam_role_arn("CloudHsmRole"),
                    "SubscriptionType": "PRODUCTION",
                    "EniIp": "10.1.1.1",
                    "ExternalId": "original",
                    "SyslogIp": "10.1.1.2",
                }),
            )
            .await;
        let before = emulator.describe_hsm(case, &arn).await;

        let mut request = patch.clone();
        request["HsmArn"] = json!(&arn);
        let modified = emulator.ok(case, "ModifyHsm", request).await;
        assert_eq!(
            string(&modified, "HsmArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: ModifyHsm must report the ARN it modified, got {modified}"
        );

        let iam_role = patch.get("IamRoleArn").and_then(Value::as_str);
        let after = emulator.describe_hsm(case, &arn).await;
        let supplied: BTreeSet<String> = expected
            .iter()
            .map(|(member, _)| (*member).to_string())
            .chain(iam_role.map(|_| "IamRoleArn".to_string()))
            // Moving the HSM to another subnet moves it to that subnet's VPC and zone.
            .chain(
                patch
                    .get("SubnetId")
                    .map(|_| ["VpcId", "AvailabilityZone"].map(str::to_string))
                    .into_iter()
                    .flatten(),
            )
            // The status is driven by the clock, not by this call; it is asserted in
            // its own test and excluded here so the row cannot race the transition.
            .chain(["Status".to_string()])
            .collect();

        for (member, expected_value) in expected {
            assert_eq!(
                string(&after, member).as_deref(),
                Some(expected_value),
                "case {case:?}: {member} must report the modified value, got {after}"
            );
        }
        if let Some(role) = iam_role {
            assert_eq!(
                string(&after, "IamRoleArn").as_deref(),
                Some(role),
                "case {case:?}: IamRoleArn must report the modified value, got {after}"
            );
        }

        for (member, was) in describe_snapshot(&before) {
            if supplied.contains(&member) {
                continue;
            }
            assert_eq!(
                string(&after, &member),
                was,
                "case {case:?}: {member} was not supplied, so it must be unchanged, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn modify_hsm_refuses_an_arn_no_stored_hsm_answers_to() {
    let emulator = Emulator::new();
    // A well-formed ARN that was never created, and one whose HSM has been deleted.
    let created = emulator
        .create_hsm(
            "deleted",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
            }),
        )
        .await;
    emulator
        .ok("deleted", "DeleteHsm", json!({ "HsmArn": &created }))
        .await;

    for (case, arn) in [
        (
            "never created",
            format!("arn:aws:cloudhsm:us-east-1:{ACCOUNT}:hsm-deadbeef"),
        ),
        ("deleted", created),
    ] {
        assert_error(
            &emulator,
            case,
            "ModifyHsm",
            json!({ "HsmArn": arn, "SyslogIp": "10.0.0.9" }),
            400,
            "CloudHsmServiceException",
        )
        .await;
        assert_error(
            &emulator,
            case,
            "DescribeHsm",
            json!({ "HsmArn": arn }),
            400,
            "CloudHsmServiceException",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// ListHsms / DeleteHsm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_hsms_reports_exactly_the_stored_arns_and_pages_them_once_each() {
    let emulator = Emulator::new();

    assert_eq!(
        emulator.list_all_hsms("empty store").await,
        Vec::<String>::new(),
        "an empty store must report an empty HsmList"
    );

    // One HSM past the page size, so the answer is actually paged: a store that fits in
    // one page emits no NextToken, a larger one emits one and still reports every ARN
    // exactly once.
    let total = state::LIST_PAGE_SIZE + 1;
    let mut created = BTreeSet::new();
    for index in 0..total {
        let case = "paged store";
        created.insert(
            emulator
                .create_hsm(
                    case,
                    json!({
                        "SubnetId": format!("subnet-{index:08x}"),
                        "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                        "IamRoleArn": iam_role_arn("CloudHsmRole"),
                        "SubscriptionType": "PRODUCTION",
                    }),
                )
                .await,
        );
    }

    let first_page = emulator.ok("paged store", "ListHsms", json!({})).await;
    assert_eq!(
        first_page
            .get("HsmList")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(state::LIST_PAGE_SIZE),
        "case {:?}: a page carries at most LIST_PAGE_SIZE ARNs, got {first_page}",
        "paged store"
    );
    assert!(
        string(&first_page, "NextToken").is_some(),
        "case {:?}: a store larger than one page must emit a NextToken, got {first_page}",
        "paged store"
    );

    let walked = emulator.list_all_hsms("paged store").await;
    assert_eq!(
        walked.len(),
        total,
        "following NextToken must report every ARN exactly once, got {} of {total}",
        walked.len()
    );
    assert_eq!(
        walked.iter().cloned().collect::<BTreeSet<String>>(),
        created,
        "following NextToken must report exactly the stored ARNs"
    );

    // A token this service did not mint is refused, not silently treated as the start.
    assert_error(
        &emulator,
        "forged token",
        "ListHsms",
        json!({ "NextToken": "zzz" }),
        400,
        "InvalidRequestException",
    )
    .await;

    // Deleting shrinks the list by exactly the deleted ARN.
    let victim = created.iter().next().expect("the store is not empty").clone();
    let deleted = emulator
        .ok("delete shrinks the list", "DeleteHsm", json!({ "HsmArn": &victim }))
        .await;
    assert_eq!(
        string(&deleted, "Status").as_deref(),
        Some("DELETED"),
        "DeleteHsm must report the status of the HSM it removed, got {deleted}"
    );

    created.remove(&victim);
    assert_eq!(
        emulator
            .list_all_hsms("delete shrinks the list")
            .await
            .into_iter()
            .collect::<BTreeSet<String>>(),
        created,
        "the deleted ARN must be gone from ListHsms and nothing else with it"
    );

    // A second delete of the same ARN has nothing to remove.
    assert_error(
        &emulator,
        "second delete",
        "DeleteHsm",
        json!({ "HsmArn": victim }),
        400,
        "CloudHsmServiceException",
    )
    .await;
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_ip_address_outside_the_octet_range_is_refused() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_hsm(
            "baseline",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
            }),
        )
        .await;

    // The model's `IpAddress` pattern (`^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$`) bounds
    // each octet's digit count only, so every row below reaches the handler. The rows
    // span each octet position and the published bound (255 is in range, 256 is not).
    for (case, value) in [
        ("first octet past the bound", "256.0.0.1"),
        ("second octet past the bound", "10.256.0.1"),
        ("third octet past the bound", "10.0.300.1"),
        ("fourth octet past the bound", "10.0.0.999"),
    ] {
        assert_error(
            &emulator,
            case,
            "CreateHsm",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
                "EniIp": value,
            }),
            400,
            "InvalidRequestException",
        )
        .await;
        assert_error(
            &emulator,
            case,
            "ModifyHsm",
            json!({ "HsmArn": &arn, "SyslogIp": value }),
            400,
            "InvalidRequestException",
        )
        .await;
    }

    // The bound itself is accepted, and a refused modify changed nothing.
    let before = emulator.describe_hsm("at the bound", &arn).await;
    emulator
        .ok(
            "at the bound",
            "ModifyHsm",
            json!({ "HsmArn": &arn, "EniIp": "255.255.255.255" }),
        )
        .await;
    let after = emulator.describe_hsm("at the bound", &arn).await;
    assert_eq!(
        string(&after, "EniIp").as_deref(),
        Some("255.255.255.255"),
        "case {:?}: 255.255.255.255 is inside the octet range, got {after}",
        "at the bound"
    );
    assert_eq!(
        string(&after, "SubnetId"),
        string(&before, "SubnetId"),
        "case {:?}: a modify of EniIp must not move the subnet, got {after}",
        "at the bound"
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_before_the_handler() {
    let emulator = Emulator::new();

    // These rows break constraints the model DOES express, so the boundary validator
    // refuses them with HTTP 400 `ValidationException` before any handler runs — and
    // nothing is stored.
    for (case, request) in [
        (
            "malformed subnet id",
            json!({
                "SubnetId": "not-a-subnet",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "PRODUCTION",
            }),
        ),
        (
            "malformed iam role arn",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": "not-an-arn",
                "SubscriptionType": "PRODUCTION",
            }),
        ),
        (
            "subscription type outside the enum",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "IamRoleArn": iam_role_arn("CloudHsmRole"),
                "SubscriptionType": "TRIAL",
            }),
        ),
        (
            "missing a required member",
            json!({
                "SubnetId": "subnet-0123abcd",
                "SshKey": "ssh-rsa AAAAB3NzaC1yc2E=",
                "SubscriptionType": "PRODUCTION",
            }),
        ),
    ] {
        let (status, payload) = emulator.call("CreateHsm", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateHsm must refuse the request, got {status} {payload}"
        );
    }

    assert_eq!(
        emulator.list_all_hsms("nothing stored").await,
        Vec::<String>::new(),
        "a refused create must store nothing"
    );
}
