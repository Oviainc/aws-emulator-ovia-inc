// SPDX-License-Identifier: CC0-1.0

//! What `CreateConnector` persists, and what a described connector reports.
//!
//! Three rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. a created connector is STORED: the call mints a `ConnectorId` of the modelled
//!    shape, and a later `DescribeConnector` reports every member the caller supplied
//!    exactly as supplied plus every member the service resolves — the ARN, the id, the
//!    egress type, the status and the nested documented defaults
//!    ([`created_connectors_are_described_exactly_as_configured`]);
//! 2. a connector whose members cannot hold together is refused with the modelled
//!    `InvalidRequestException` and NOTHING is persisted for it — the next create mints
//!    the very next id ([`connectors_whose_members_cannot_hold_together_are_refused`]);
//! 3. a `ConnectorId` the store does not hold answers this service's generic not-found
//!    error ([`describing_a_connector_that_was_never_created_is_not_found`]).
//!
//! The rows differ along the protocol configuration (AS2 or SFTP), the egress type
//! (service-managed or VPC Lattice), how many of the nested members the request
//! supplies, which enum value each carries, the list lengths, and where a numeric
//! member sits relative to its published bounds. The harness (router, request signing,
//! `assert_members`) is the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{
    assert_members, is_modelled_connector_id, member_of, Emulator, ACCOUNT, REGION, ROLE,
};

/// A `VpcLatticeResourceConfigurationArn` satisfying the member's modelled pattern
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}$`.
const RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-0123456789abcdef0";

/// A `ProfileId` (`len 19..19`, pattern `^p-([0-9a-f]{17})$`) for the local AS2 profile.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";

/// A second `ProfileId`, for the partner side of the same AS2 connector.
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// The 17 hex digits of a minted id, as a number — so a test can assert that a refused
/// create minted nothing.
fn id_sequence(connector_id: &str) -> u64 {
    u64::from_str_radix(&connector_id[2..], 16).expect("a minted id ends in 17 hex digits")
}

/// Create a connector, assert the call succeeded, and return its `ConnectorId`.
///
/// Kept HERE rather than on the shared [`Emulator`] (beside `create_server`) because it
/// carries the TABLE ROW's label into every assertion message, which only this suite
/// has.
async fn create_connector(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateConnector returned no ConnectorId: {}", answer.body))
        .to_string()
}

/// Describe `connector_id`, assert the read succeeded, and return the `Connector`.
async fn describe_connector(emulator: &Emulator, case: &str, connector_id: &str) -> Value {
    let answer = emulator
        .call("DescribeConnector", json!({"ConnectorId": connector_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("Connector").clone()
}

// ---------------------------------------------------------------------------
// Rule 1: a created connector is stored, and described as configured.
// ---------------------------------------------------------------------------

/// RULE: `CreateConnector` persists the connector it is given and answers a minted
/// `ConnectorId`; `DescribeConnector` then reports every supplied member exactly as
/// supplied and every member the service resolves — `Arn`, `ConnectorId`, `EgressType`
/// (`VPC_LATTICE` with an `EgressConfig`, else the modelled default `SERVICE_MANAGED`),
/// `Status` `ACTIVE`, the tag set as a list, and each nested member at the default its
/// own model doc states.
///
/// The rows differ along the protocol configuration, the egress type, how many nested
/// members the request supplies, which enum value each carries, the list lengths, and
/// whether `MaxConcurrentConnections` / `PortNumber` are absent, at a published bound,
/// or in between.
#[tokio::test]
async fn created_connectors_are_described_exactly_as_configured() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "AS2, nothing optional supplied",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                },
            }),
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "Tags": [],
                "SftpConfig": null,
                "EgressConfig": null,
                "LoggingRole": null,
                "SecurityPolicyName": null,
                "IpAddressType": null,
                "ErrorMessage": null,
                "ServiceManagedEgressIpAddresses": null,
            }),
        ),
        (
            "AS2, every nested member supplied at a non-default enum value",
            json!({
                "AccessRole": ROLE,
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "http://partner.example.com:8080/as2",
                "IpAddressType": "DUALSTACK",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for 2026-10",
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "BasicAuthSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:as2-basic",
                    "PreserveContentType": "ENABLED",
                },
                "Tags": [
                    {"Key": "team", "Value": "edi"},
                    {"Key": "env", "Value": "test"},
                ],
            }),
            json!({
                "AccessRole": ROLE,
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "http://partner.example.com:8080/as2",
                "IpAddressType": "DUALSTACK",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for 2026-10",
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "BasicAuthSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:as2-basic",
                    "PreserveContentType": "ENABLED",
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "Tags": [
                    {"Key": "team", "Value": "edi"},
                    {"Key": "env", "Value": "test"},
                ],
                "SftpConfig": null,
                "EgressConfig": null,
            }),
        ),
        (
            "SFTP service-managed, MaxConcurrentConnections left to its documented default of 1",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            }),
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "MaxConcurrentConnections": 1,
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "As2Config": null,
                "EgressConfig": null,
                "Tags": [],
            }),
        ),
        (
            "SFTP service-managed at the published MaxConcurrentConnections maximum, with \
             two trusted host keys and a security policy",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
                "IpAddressType": "IPV4",
            }),
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "IpAddressType": "IPV4",
            }),
        ),
        (
            "SFTP over VPC Lattice, no port: PortNumber takes its documented default of 22 \
             and Url stays null",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            json!({
                "AccessRole": ROLE,
                "Url": null,
                "EgressType": "VPC_LATTICE",
                "Status": "ACTIVE",
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 22,
                    },
                },
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "MaxConcurrentConnections": 1,
                },
            }),
        ),
        (
            "SFTP over VPC Lattice at the low end of PortNumber's modelled range (1)",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 3},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 1,
                    },
                },
            }),
            json!({
                "EgressType": "VPC_LATTICE",
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 1,
                    },
                },
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 3},
                "Url": null,
            }),
        ),
        (
            "SFTP over VPC Lattice at the high end of PortNumber's modelled range (65535), \
             with a blank Url that names no endpoint",
            json!({
                "AccessRole": ROLE,
                "Url": "",
                "SftpConfig": {"UserSecretId": "sftp", "TrustedHostKeys": []},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
            }),
            json!({
                "EgressType": "VPC_LATTICE",
                // A blank Url names no endpoint, so the connector reports none rather
                // than the empty string the model says must be null.
                "Url": null,
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
                "SftpConfig": {
                    "UserSecretId": "sftp",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                },
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();

    for (case, request, expected) in cases {
        let connector_id = create_connector(&emulator, case, request).await;
        assert!(
            is_modelled_connector_id(&connector_id),
            "case {case:?}: expected a ConnectorId matching ^c-([0-9a-f]{{17}})$, \
             got {connector_id:?}"
        );

        let described = describe_connector(&emulator, case, &connector_id).await;
        assert_members(case, &described, &expected);

        let arn = member_of(&described, "Arn").as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:connector/{connector_id}"),
            "case {case:?}: expected the connector's own ARN, got {arn:?}"
        );
        let reported_id = member_of(&described, "ConnectorId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported_id, connector_id,
            "case {case:?}: expected the described connector to report the minted id \
             {connector_id:?}, got {reported_id:?}"
        );

        // A read must not change what a later read sees.
        let again = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            again, described,
            "case {case:?}: two reads of one connector must answer the same thing, \
             got {again:?} after {described:?}"
        );

        minted.push(connector_id);
    }

    // Every create minted its OWN connector, and the earlier ones are all still
    // readable: a create never overwrites a stored connector.
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "expected one distinct ConnectorId per create, got {minted:?}"
    );
    for connector_id in &minted {
        let described = describe_connector(&emulator, "all connectors still readable", connector_id).await;
        let reported_id = member_of(&described, "ConnectorId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            &reported_id, connector_id,
            "expected {connector_id:?} to still be stored, got {described:?}"
        );
    }
}

/// RULE (same rule, varied along the signed region): the ARN a connector reports is
/// built from the region the caller signed for, so it is not fixed to one region.
#[tokio::test]
async fn a_connectors_arn_carries_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let answer = emulator
            .call_in(
                region,
                "CreateConnector",
                json!({
                    "AccessRole": ROLE,
                    "Url": "sftp://partner.example.com",
                    "SftpConfig": {"UserSecretId": "sftp"},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected CreateConnector 200, got {} with body {}",
            answer.status, answer.body
        );
        let connector_id = answer.get("ConnectorId").as_str().unwrap_or_default().to_string();

        let described = describe_connector(&emulator, region, &connector_id).await;
        let arn = member_of(&described, "Arn").as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:transfer:{region}:{ACCOUNT}:connector/{connector_id}"),
            "case {region:?}: expected the ARN to carry the signed region, got {arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a configuration that cannot hold together is refused, and stores nothing.
// ---------------------------------------------------------------------------

/// RULE: a `CreateConnector` whose members violate a requirement the model states
/// across them is refused with HTTP 400 `InvalidRequestException` and persists nothing
/// — proved by the id counter, which only a stored connector moves.
///
/// The rows cover each documented requirement, from both sides where it has two, and
/// each numeric row sits one step past a published bound.
#[tokio::test]
async fn connectors_whose_members_cannot_hold_together_are_refused() {
    let emulator = Emulator::new();

    let before = create_connector(
        &emulator,
        "a connector created before the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "neither As2Config nor SftpConfig: no protocol configured",
            json!({"AccessRole": ROLE, "Url": "sftp://partner.example.com"}),
            "As2Config",
        ),
        (
            "both As2Config and SftpConfig: two protocols configured",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
            "SftpConfig",
        ),
        (
            "AS2 without a Url",
            json!({
                "AccessRole": ROLE,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
            }),
            "Url",
        ),
        (
            "service-managed SFTP without a Url",
            json!({"AccessRole": ROLE, "SftpConfig": {"UserSecretId": "sftp"}}),
            "Url",
        ),
        (
            "service-managed SFTP whose Url is the empty string: Url's len 0..255 admits \
             it, but it names no endpoint",
            json!({"AccessRole": ROLE, "Url": "", "SftpConfig": {"UserSecretId": "sftp"}}),
            "Url",
        ),
        (
            "VPC Lattice with a Url, which the model says must be null",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp"},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            "Url",
        ),
        (
            "an AS2 connector with an EgressConfig, which routes to an SFTP server",
            json!({
                "AccessRole": ROLE,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            "EgressConfig",
        ),
        (
            "MaxConcurrentConnections one past the published maximum of 5",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 6},
            }),
            "MaxConcurrentConnections",
        ),
    ];

    for (case, request, member_at_fault) in cases {
        let answer = emulator.call("CreateConnector", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.body
        );
        assert!(
            answer.get("ConnectorId").is_null(),
            "case {case:?}: a refused create must answer no ConnectorId, got {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty(),
            "case {case:?}: InvalidRequestException models Message as required, got {}",
            answer.body
        );
        // The refusal NAMES the member at fault, so a caller can fix the request from
        // the message alone.
        assert!(
            message.contains(member_at_fault),
            "case {case:?}: expected the message to name {member_at_fault:?}, \
             got {message:?}"
        );
    }

    // Nothing was persisted for any refused create: the next create mints the id
    // immediately after the one minted before them.
    let after = create_connector(
        &emulator,
        "a connector created after the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    assert_eq!(
        id_sequence(&after),
        id_sequence(&before) + 1,
        "expected the refused creates to persist nothing, but the id moved from \
         {before:?} to {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a connector the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `DescribeConnector` for a syntactically valid `ConnectorId` the store does not
/// hold answers this service's generic not-found error — HTTP 400
/// `ResourceNotFoundException`, `ResourceType` `Connector`, `Resource` the id the
/// request named, and a fixed `Message` carrying no identifier.
///
/// The rows are ids from the low end of the id space, a mid value, the high end, and
/// one that neighbours an id that WAS minted.
#[tokio::test]
async fn describing_a_connector_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    let minted = create_connector(
        &emulator,
        "one connector the store does hold",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    let neighbour = format!("c-{:017x}", id_sequence(&minted) + 1);

    let absent = [
        "c-00000000000000000".to_string(),
        "c-0000000000000dead".to_string(),
        "c-fffffffffffffffff".to_string(),
        neighbour,
    ];

    let mut messages: Vec<(String, String)> = Vec::new();

    for connector_id in &absent {
        assert!(
            is_modelled_connector_id(connector_id),
            "the probe id {connector_id:?} must satisfy ConnectorId's modelled pattern"
        );
        let answer = emulator
            .call("DescribeConnector", json!({"ConnectorId": connector_id}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {connector_id:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {connector_id:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_members(
            connector_id,
            &answer.body,
            &json!({"ResourceType": "Connector", "Resource": connector_id}),
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(connector_id.as_str()),
            "case {connector_id:?}: expected no identifier in Message, got {message:?}"
        );
        messages.push((connector_id.clone(), message));
    }

    let (first_case, first) = messages.first().expect("every row produced a message").clone();
    for (case, message) in &messages {
        assert_eq!(
            message, &first,
            "case {case:?}: expected the same fixed Connector message as case \
             {first_case:?} ({first:?}), got {message:?}"
        );
    }
}
