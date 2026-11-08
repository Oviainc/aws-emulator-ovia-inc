// SPDX-License-Identifier: CC0-1.0

//! `GetInstanceAccessDetails` — the temporary access Lightsail grants to one
//! instance, driven over the wire through the same front door a client uses.
//!
//! The rule under test: the call resolves `instanceName` against the ONE stored
//! instance record and GRANTS the access it reports — "Returns temporary SSH keys
//! you can use to connect to a specific virtual private server, or instance" —
//! under the protocol the request names, `ssh` when it names none. An SSH access
//! is granted a real temporary OpenSSH key pair with the instant it expires, and
//! the SAME pair for as long as that grant is live; an RDP access to a Windows
//! instance is told the instance's Administrator password, or what is needed to
//! decrypt it, under the conditions `InstanceAccessDetails$password` and
//! `$passwordData` state, and an instance of any other platform is told neither.
//! Either way the host key the instance itself presents is recorded on it — its
//! SSH host key on a Linux/Unix instance, its RDP certificate on a Windows one —
//! and reported from then on, so two accesses are authenticated by the same key
//! until `DeleteKnownHostKeys` empties the record. The addresses and the user
//! name are the instance's own, the ones `GetInstance` reports, and a name the
//! store does not hold is refused with the service's `DoesNotExist` envelope.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! the blueprint platform (which decides the login user and the form of the
//! recorded host key), the IP address type (which decides which addresses exist),
//! the key pair the instance was launched with (which decides what an RDP access
//! may be told), the bundle and the Region.
//!
//! Every key assertion goes through real key material: the granted private key is
//! parsed by the OpenSSH key crate and its public half is compared with the
//! certificate issued over it, and a reported fingerprint is recomputed from the
//! key blob or the certificate's own DER. Nothing here asserts a string shape
//! where a digest can be checked.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use base64::Engine;
use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;
use sha2::{Digest, Sha256};

use crate::instance_lifecycle::{error_code, wait_for_state, Emulator};

/// The `key_id` the granted access certificate carries, which the service states
/// the certificate was issued for.
const CERTIFICATE_KEY_ID: &str = "lightsail-instance-access";

/// The algorithm of the key material this emulator generates, OpenSSH's own
/// default.
const SSH_HOST_KEY_ALGORITHM: &str = "ssh-ed25519";

/// One instance to grant access to, and what the rule says the grant then
/// reports.
#[derive(Debug)]
struct Row {
    case: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// `None` leaves `ipAddressType` out of the request, which the service
    /// defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
    /// `None` leaves `keyPairName` out, so the instance is launched with the
    /// Lightsail default key pair.
    key_pair_name: Option<&'static str>,
    /// The user name the blueprint signs in as, from the Lightsail "Connecting to
    /// your instance" defaults this service publishes
    /// (`crate::state::BLUEPRINT_USERNAMES`), and `None` for a blueprint family
    /// it publishes none for — where the member must be absent rather than
    /// guessed.
    username: Option<&'static str>,
    /// Whether the blueprint is a Windows one, which decides both the form of the
    /// recorded host key and whether an RDP access is told a password.
    is_windows: bool,
}

/// The table every case here runs. Blueprint ids and bundle ids are the catalogs
/// `GetBlueprints` and `GetBundles` publish (`crate::state::BLUEPRINTS` and
/// `crate::state::BUNDLE_SPECS`); the availability zones are ones
/// `crate::state::REGION_SPECS` publishes.
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux, no ipAddressType and no key pair",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
        key_pair_name: None,
        username: Some("ec2-user"),
        is_windows: false,
    },
    Row {
        case: "ubuntu, ipv4 only, smallest bundle",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
        key_pair_name: None,
        username: Some("ubuntu"),
        is_windows: false,
    },
    Row {
        case: "windows, dualstack, default key pair",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("dualstack"),
        key_pair_name: None,
        username: Some("Administrator"),
        is_windows: true,
    },
    Row {
        case: "windows, ipv4 only, launched with a custom key pair",
        availability_zone: "eu-central-1b",
        region: "eu-central-1",
        blueprint_id: "windows_server_2019",
        bundle_id: "medium_3_0",
        ip_address_type: Some("ipv4"),
        key_pair_name: Some("access-probe-pair"),
        username: Some("Administrator"),
        is_windows: true,
    },
    Row {
        case: "bitnami application blueprint, ipv6 only, so there is no public IPv4 address",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("ipv6"),
        key_pair_name: None,
        username: Some("bitnami"),
        is_windows: false,
    },
    Row {
        case: "a hosting-panel blueprint this service publishes no login user for",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "plesk_ubuntu_18_0_35",
        bundle_id: "small_3_0",
        ip_address_type: Some("dualstack"),
        key_pair_name: Some("panel_pair_7"),
        username: None,
        is_windows: false,
    },
];

/// Names nothing created, differing along the length, the case, the digits and
/// the separators. Every one satisfies the modelled `ResourceName` pattern
/// `^\w[\w\-]*\w$`, so the centralized walk passes it through to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The `CreateInstances` body for `row`, naming one instance.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    if let Some(key_pair_name) = row.key_pair_name {
        body["keyPairName"] = json!(key_pair_name);
    }
    body
}

/// Create the instance `row` describes under `name` and wait until it serves, so
/// the access granted for it is the access to a running instance.
async fn running_instance(emulator: &Emulator, row: &Row, name: &str) {
    emulator.ok("CreateInstances", create_body(row, name)).await;
    wait_for_state(emulator, name, "running").await;
}

/// `GetInstanceAccessDetails` for `name`, over `protocol` when one is named,
/// answering the `accessDetails` it reports.
async fn access(emulator: &Emulator, name: &str, protocol: Option<&str>) -> Value {
    let mut body = json!({"instanceName": name});
    if let Some(protocol) = protocol {
        body["protocol"] = json!(protocol);
    }
    emulator
        .ok("GetInstanceAccessDetails", body)
        .await["accessDetails"]
        .clone()
}

/// The member `member` of `details` as a string, or a panic naming the case.
fn text<'a>(case: &str, details: &'a Value, member: &str) -> &'a str {
    details[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a {member}, got {details:?}"))
}

/// The epoch seconds of the timestamp member `member`, which awsJson renders as a
/// number.
fn seconds(case: &str, details: &Value, member: &str) -> f64 {
    details[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected the timestamp {member}, got {details:?}"))
}

/// The one host key `details` reports, or a panic naming the case.
fn single_host_key<'a>(case: &str, details: &'a Value) -> &'a Value {
    let keys = details["hostKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a hostKeys list, got {details:?}"));
    assert_eq!(
        keys.len(),
        1,
        "case {case}: an instance presents one host key, got {:?}",
        details["hostKeys"]
    );
    &keys[0]
}

/// `bytes` as the colon-separated lowercase hex an X.509 fingerprint is published
/// in.
fn colon_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The DER behind a PEM-encoded certificate.
fn certificate_der(case: &str, pem: &str) -> Vec<u8> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .unwrap_or_else(|error| {
            panic!("case {case}: the reported certificate must be PEM, got {error} for {pem:?}")
        })
}

/// Assert the addresses, the user name and the instance name `details` reports
/// are the instance's own — the values `GetInstance` reports for it.
async fn assert_instance_members(emulator: &Emulator, row: &Row, name: &str, details: &Value) {
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]
        .clone();
    assert_eq!(
        instance["location"]["regionName"].as_str(),
        Some(row.region),
        "case {}: the access is granted for the instance in {}, got {:?}",
        row.case,
        row.region,
        instance["location"]
    );
    assert_eq!(
        details["instanceName"].as_str(),
        Some(name),
        "case {}: expected instanceName {name}, got {details:?}",
        row.case
    );
    assert_eq!(
        details["ipAddress"], instance["publicIpAddress"],
        "case {}: the access address must be the instance's public address {:?}, got {:?}",
        row.case, instance["publicIpAddress"], details["ipAddress"]
    );
    assert_eq!(
        details["ipv6Addresses"], instance["ipv6Addresses"],
        "case {}: expected the instance's IPv6 addresses {:?}, got {:?}",
        row.case, instance["ipv6Addresses"], details["ipv6Addresses"]
    );
    assert_eq!(
        details["username"].as_str(),
        row.username,
        "case {}: expected the blueprint's login user {:?}, got {:?}",
        row.case,
        row.username,
        details["username"]
    );
    assert_eq!(
        details["username"], instance["username"],
        "case {}: the access user must be the one the instance reports {:?}, got {:?}",
        row.case, instance["username"], details["username"]
    );
}

/// Assert the host key `details` reports is the form `row`'s platform presents,
/// and that every published property of it is a property of real key material.
fn assert_host_key(row: &Row, details: &Value) {
    let key = single_host_key(row.case, details);
    let witnessed_at = seconds(row.case, key, "witnessedAt");
    assert!(
        witnessed_at > 0.0,
        "case {}: a recorded host key carries the instant it was witnessed, got {key:?}",
        row.case
    );
    if row.is_windows {
        // "For RDP certificates, the algorithm is always `x509-cert`", and the
        // certificate itself is what `publicKey` carries.
        assert_eq!(
            key["algorithm"].as_str(),
            Some("x509-cert"),
            "case {}: a Windows instance presents an RDP certificate, got {key:?}",
            row.case
        );
        let pem = text(row.case, key, "publicKey");
        assert!(
            pem.starts_with("-----BEGIN CERTIFICATE-----"),
            "case {}: expected a PEM certificate, got {pem:?}",
            row.case
        );
        let der = certificate_der(row.case, pem);
        assert_eq!(
            key["fingerprintSHA256"].as_str(),
            Some(colon_hex(&Sha256::digest(&der)).as_str()),
            "case {}: the SHA-256 fingerprint must be the digest of the certificate, got {key:?}",
            row.case
        );
        assert_eq!(
            key["fingerprintSHA1"].as_str(),
            Some(colon_hex(&sha1::Sha1::digest(&der)).as_str()),
            "case {}: the SHA-1 fingerprint must be the digest of the certificate, got {key:?}",
            row.case
        );
        // "The returned RDP certificate is valid after this point in time", and
        // the window reported is the certificate's own.
        let not_valid_before = seconds(row.case, key, "notValidBefore");
        let not_valid_after = seconds(row.case, key, "notValidAfter");
        let window = lightsail_state::RDP_CERTIFICATE_VALID_MILLIS as f64 / 1_000.0;
        assert!(
            (not_valid_after - not_valid_before - window).abs() < 1.0,
            "case {}: expected a {window}s validity window, got {not_valid_before} to \
             {not_valid_after}",
            row.case
        );
        assert!(
            not_valid_before <= witnessed_at,
            "case {}: the certificate must be valid by the time it was witnessed, got \
             {not_valid_before} against {witnessed_at}",
            row.case
        );
    } else {
        assert_eq!(
            key["algorithm"].as_str(),
            Some(SSH_HOST_KEY_ALGORITHM),
            "case {}: a Linux/Unix instance presents an SSH host key, got {key:?}",
            row.case
        );
        // The blob `publicKey` carries is a real OpenSSH public key, and both
        // fingerprints are digests of it: `SHA256:<base64>` is what
        // `ssh-keygen -l` prints, and the SHA-1 form is the same encoding over a
        // SHA-1 digest.
        let body = text(row.case, key, "publicKey");
        let openssh = format!("{SSH_HOST_KEY_ALGORITHM} {body}");
        let public = ssh_key::PublicKey::from_openssh(&openssh).unwrap_or_else(|error| {
            panic!(
                "case {}: the reported host key must be an OpenSSH public key, got {error} for \
                 {body:?}",
                row.case
            )
        });
        assert_eq!(
            key["fingerprintSHA256"].as_str(),
            Some(
                public
                    .fingerprint(ssh_key::HashAlg::Sha256)
                    .to_string()
                    .as_str()
            ),
            "case {}: the SHA-256 fingerprint must be the key's own, got {key:?}",
            row.case
        );
        let blob = base64::engine::general_purpose::STANDARD
            .decode(body)
            .expect("the reported host key body is base64");
        assert_eq!(
            key["fingerprintSHA1"].as_str(),
            Some(
                format!(
                    "SHA1:{}",
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .encode(sha1::Sha1::digest(&blob))
                )
                .as_str()
            ),
            "case {}: the SHA-1 fingerprint must be the digest of the key blob, got {key:?}",
            row.case
        );
        // "This value is listed only for RDP certificates."
        for member in ["notValidBefore", "notValidAfter"] {
            assert!(
                key[member].is_null(),
                "case {}: {member} is listed only for RDP certificates, got {key:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SSH access
// ---------------------------------------------------------------------------

/// RULE: a request that names no protocol is served over `ssh` and granted a real
/// temporary OpenSSH key pair — a private key whose public half the reported
/// certificate is issued over, expiring at the reported instant — beside the
/// instance's own addresses, login user and recorded host key. No password is
/// reported: `password` and `passwordData` are documented for RDP access to a
/// Windows instance.
#[tokio::test]
async fn an_ssh_access_is_granted_a_real_temporary_key_pair() {
    for row in ROWS {
        let emulator = Emulator::new();
        let name = "access-probe";
        running_instance(&emulator, row, name).await;
        let before = lightsail_state::now_ms();

        let details = access(&emulator, name, None).await;

        assert_eq!(
            details["protocol"].as_str(),
            Some(lightsail_state::INSTANCE_ACCESS_PROTOCOL_SSH),
            "case {}: a request naming no protocol defaults to ssh, got {details:?}",
            row.case
        );
        assert_instance_members(&emulator, row, name, &details).await;
        assert_host_key(row, &details);

        // The granted pair, read back with the crate that writes it: the private
        // key parses, and the certificate reported as `certKey` is issued over
        // THAT key's public half and expires when the response says it does.
        let private_key = text(row.case, &details, "privateKey");
        let private = ssh_key::PrivateKey::from_openssh(private_key).unwrap_or_else(|error| {
            panic!(
                "case {}: privateKey must be an OpenSSH private key, got {error}",
                row.case
            )
        });
        assert_eq!(
            private.algorithm().as_str(),
            SSH_HOST_KEY_ALGORITHM,
            "case {}: expected an {SSH_HOST_KEY_ALGORITHM} key, got {:?}",
            row.case,
            private.algorithm()
        );
        let cert_key = text(row.case, &details, "certKey");
        let expires_at = seconds(row.case, &details, "expiresAt");
        match row.username {
            // A certificate states the user it is good for, so one is issued
            // where the blueprint publishes a login user: over THIS key's public
            // half, naming that user, expiring when the response says the keys do.
            Some(username) => {
                let certificate =
                    ssh_key::Certificate::from_openssh(cert_key).unwrap_or_else(|error| {
                        panic!(
                            "case {}: certKey must be an OpenSSH certificate, got {error}",
                            row.case
                        )
                    });
                assert_eq!(
                    certificate.public_key(),
                    private.public_key().key_data(),
                    "case {}: the certificate must be issued over the granted key's public half",
                    row.case
                );
                assert_eq!(
                    certificate.key_id(),
                    CERTIFICATE_KEY_ID,
                    "case {}: expected the certificate to state what it was issued for, got {:?}",
                    row.case,
                    certificate.key_id()
                );
                let principals: Vec<String> = certificate.valid_principals().to_vec();
                assert_eq!(
                    principals,
                    vec![username.to_string()],
                    "case {}: expected the login user as the certificate's principal, got \
                     {principals:?}",
                    row.case
                );
                assert!(
                    (certificate.valid_before() as f64 - expires_at).abs() < 1.0,
                    "case {}: the certificate must expire when expiresAt says, got {} against \
                     {expires_at}",
                    row.case,
                    certificate.valid_before()
                );
            }
            // A blueprint this service publishes no login user for is handed the
            // pair's public half instead, which is the rest of what the member
            // documents — and it is this pair's public half, not another key.
            None => {
                let public = ssh_key::PublicKey::from_openssh(cert_key).unwrap_or_else(|error| {
                    panic!(
                        "case {}: certKey must be an OpenSSH public key, got {error}",
                        row.case
                    )
                });
                assert_eq!(
                    public.key_data(),
                    private.public_key().key_data(),
                    "case {}: expected the granted key's own public half, got {cert_key:?}",
                    row.case
                );
            }
        }

        let window = lightsail_state::TEMPORARY_SSH_KEY_VALID_MILLIS as f64 / 1_000.0;
        let granted_at = before as f64 / 1_000.0;
        assert!(
            expires_at - granted_at >= window - 5.0 && expires_at - granted_at <= window + 5.0,
            "case {}: expected keys expiring {window}s after the grant, got {expires_at} \
             against {granted_at}",
            row.case
        );

        for member in ["password", "passwordData"] {
            assert!(
                details[member].is_null(),
                "case {}: {member} is reported for RDP access to a Windows instance only, got \
                 {details:?}",
                row.case
            );
        }
    }
}

/// RULE: the keys are the resource this call creates, so a second call inside the
/// grant's window reports the SAME pair and the same expiry rather than retiring
/// the pair the first answer handed out — and the host key recorded for the
/// instance is witnessed once, so its `witnessedAt` does not move either.
#[tokio::test]
async fn a_second_access_inside_the_window_reports_the_same_grant() {
    for row in ROWS {
        let emulator = Emulator::new();
        let name = "asked-twice";
        running_instance(&emulator, row, name).await;

        let first = access(&emulator, name, None).await;
        let again = access(&emulator, name, None).await;

        for member in ["privateKey", "certKey", "expiresAt", "hostKeys"] {
            assert_eq!(
                first[member], again[member],
                "case {}: expected the same {member} on a second access, got {:?} then {:?}",
                row.case, first[member], again[member]
            );
        }
        // And the store holds exactly one grant and one recorded key, not one per
        // call.
        let recorded = lightsail_state::with_state(emulator.engine(), |store| {
            let instance = store.instance(name).expect("the suite created this instance");
            (
                instance.known_host_keys.len(),
                instance.access.ssh.is_some(),
            )
        })
        .expect("the suite's engine is the shared in-memory one");
        assert_eq!(
            recorded,
            (1, true),
            "case {}: expected one recorded host key and one held grant, got {recorded:?}",
            row.case
        );
    }
}

/// RULE: the keys are TEMPORARY — once the grant's window has run out the next
/// access is granted a fresh pair, which expires later than the one it replaced.
/// The host key the instance presents is a different lifetime and is not
/// re-witnessed by it.
#[tokio::test]
async fn an_expired_grant_is_replaced_by_a_fresh_pair() {
    let row = &ROWS[0];
    let emulator = Emulator::new();
    let name = "expired-grant";
    running_instance(&emulator, row, name).await;

    let first = access(&emulator, name, None).await;

    // Age the held grant past its window. The window is the hour the service
    // grants access for, which no test can wait out, so the grant is moved into
    // the past through the same public state API a handler writes it with.
    lightsail_state::with_state(emulator.engine(), |store| {
        let instance = store
            .instance_mut(name)
            .expect("the suite created this instance");
        let held = instance
            .access
            .ssh
            .as_mut()
            .expect("the first access held a grant");
        held.created_at_ms -= lightsail_state::TEMPORARY_SSH_KEY_VALID_MILLIS + 1_000;
        held.expires_at_ms -= lightsail_state::TEMPORARY_SSH_KEY_VALID_MILLIS + 1_000;
    })
    .expect("the suite's engine is the shared in-memory one");

    let again = access(&emulator, name, None).await;

    assert_ne!(
        first["privateKey"], again["privateKey"],
        "case an expired grant: expected a fresh private key, got the expired one again"
    );
    assert_ne!(
        first["certKey"], again["certKey"],
        "case an expired grant: expected a fresh certificate, got the expired one again"
    );
    assert!(
        seconds("an expired grant", &again, "expiresAt")
            > seconds("an expired grant", &first, "expiresAt"),
        "case an expired grant: the fresh pair must expire later, got {:?} then {:?}",
        first["expiresAt"],
        again["expiresAt"]
    );
    assert_eq!(
        first["hostKeys"], again["hostKeys"],
        "case an expired grant: the instance's host key is not re-witnessed by a new grant, got \
         {:?} then {:?}",
        first["hostKeys"], again["hostKeys"]
    );
    // The fresh pair is the one the store now holds.
    let held = lightsail_state::with_state(emulator.engine(), |store| {
        store
            .instance(name)
            .and_then(|instance| instance.access.ssh.clone())
            .map(|keys| keys.private_key)
    })
    .expect("the suite's engine is the shared in-memory one");
    assert_eq!(
        held.as_deref(),
        again["privateKey"].as_str(),
        "case an expired grant: the store must hold the pair the answer handed out"
    );
}

// ---------------------------------------------------------------------------
// RDP access
// ---------------------------------------------------------------------------

/// RULE: an RDP access to a Windows instance launched with the default key pair
/// is told the instance's Administrator password — the same one on every later
/// call, because the password is the instance's — and is told no `passwordData`,
/// which is "only needed if `password` is empty". No SSH key pair is granted:
/// `privateKey`, `certKey` and `expiresAt` are documented for SSH access.
#[tokio::test]
async fn an_rdp_access_to_a_windows_instance_is_told_the_administrator_password() {
    let row = &ROWS[2];
    let emulator = Emulator::new();
    let name = "rdp-default-pair";
    running_instance(&emulator, row, name).await;

    let details = access(&emulator, name, Some("rdp")).await;

    assert_eq!(
        details["protocol"].as_str(),
        Some(lightsail_state::INSTANCE_ACCESS_PROTOCOL_RDP),
        "case {}: expected the protocol asked for, got {details:?}",
        row.case
    );
    assert_instance_members(&emulator, row, name, &details).await;
    assert_host_key(row, &details);

    let password = text(row.case, &details, "password");
    assert_eq!(
        password.chars().count(),
        lightsail_state::ADMINISTRATOR_PASSWORD_LEN,
        "case {}: expected a {}-character password, got {password:?}",
        row.case,
        lightsail_state::ADMINISTRATOR_PASSWORD_LEN
    );
    assert!(
        password
            .chars()
            .all(|character| character.is_ascii_graphic()),
        "case {}: a Windows password is printable ASCII with no space, got {password:?}",
        row.case
    );
    assert!(
        details["passwordData"].is_null(),
        "case {}: passwordData is needed only where the password is empty, got {details:?}",
        row.case
    );
    for member in ["privateKey", "certKey", "expiresAt"] {
        assert!(
            details[member].is_null(),
            "case {}: {member} is reported for SSH access only, got {details:?}",
            row.case
        );
    }

    // The password is the one the service HOLDS for the instance, so a second
    // call reports it again rather than rotating it.
    let again = access(&emulator, name, Some("rdp")).await;
    assert_eq!(
        again["password"].as_str(),
        Some(password),
        "case {}: expected the held password again, got {:?}",
        row.case,
        again["password"]
    );
    let held = lightsail_state::with_state(emulator.engine(), |store| {
        store
            .instance(name)
            .and_then(|instance| instance.access.administrator_password.clone())
    })
    .expect("the suite's engine is the shared in-memory one");
    assert_eq!(
        held.as_deref(),
        Some(password),
        "case {}: the store must hold the password the answer reported",
        row.case
    );
}

/// RULE: "If you create an instance using any key pair other than the default
/// (`LightsailDefaultKeyPair`), `password` will always be an empty string" — so an
/// RDP access to such an instance is told the empty password and the key pair its
/// password is encrypted to, which is the pair the instance was launched with.
#[tokio::test]
async fn an_rdp_access_with_a_custom_key_pair_is_told_the_pair_to_decrypt_with() {
    let row = &ROWS[3];
    let emulator = Emulator::new();
    let name = "rdp-custom-pair";
    running_instance(&emulator, row, name).await;

    let details = access(&emulator, name, Some("rdp")).await;

    assert_eq!(
        details["password"].as_str(),
        Some(""),
        "case {}: a custom key pair means an empty password, got {details:?}",
        row.case
    );
    assert_eq!(
        details["passwordData"]["keyPairName"].as_str(),
        row.key_pair_name,
        "case {}: expected the pair the instance was launched with, got {:?}",
        row.case,
        details["passwordData"]
    );
    assert_ne!(
        row.key_pair_name,
        Some(lightsail_state::DEFAULT_SSH_KEY_NAME),
        "case {}: this row must name a pair other than the default one",
        row.case
    );
    // Nothing holds the public half of a custom key pair in this emulator, so the
    // ciphertext that would be encrypted to it is absent rather than invented.
    assert!(
        details["passwordData"]["ciphertext"].is_null(),
        "case {}: expected no ciphertext for a pair whose public half is not held, got {:?}",
        row.case,
        details["passwordData"]
    );
    assert!(
        lightsail_state::with_state(emulator.engine(), |store| {
            store
                .instance(name)
                .is_some_and(|instance| instance.access.administrator_password.is_none())
        })
        .expect("the suite's engine is the shared in-memory one"),
        "case {}: no password is held where none can be handed out",
        row.case
    );
}

/// RULE: "Password will be an empty string if the password for your new instance
/// is not ready yet", and so is the ciphertext — an instance that is still coming
/// up is told both empty, and presents no host key to record either. Once it
/// serves, the same call is told its password and its recorded certificate.
#[tokio::test]
async fn an_rdp_access_before_the_instance_is_ready_is_told_nothing_to_sign_in_with() {
    let row = &ROWS[2];
    let emulator = Emulator::new();
    let name = "not-ready-yet";
    // Not waited for: the clock owes this instance `running`, so the access below
    // reaches it while it is still coming up.
    emulator.ok("CreateInstances", create_body(row, name)).await;

    let pending = access(&emulator, name, Some("rdp")).await;
    assert_eq!(
        pending["password"].as_str(),
        Some(""),
        "case an instance that is not ready: expected the empty password, got {pending:?}"
    );
    assert_eq!(
        pending["passwordData"]["ciphertext"].as_str(),
        Some(""),
        "case an instance that is not ready: expected the empty ciphertext, got {:?}",
        pending["passwordData"]
    );
    assert_eq!(
        pending["passwordData"]["keyPairName"].as_str(),
        Some(lightsail_state::DEFAULT_SSH_KEY_NAME),
        "case an instance that is not ready: expected the default key pair, got {:?}",
        pending["passwordData"]
    );
    assert_eq!(
        pending["hostKeys"].as_array().map(Vec::len),
        Some(0),
        "case an instance that is not ready: it presents no host key to record, got {:?}",
        pending["hostKeys"]
    );

    wait_for_state(&emulator, name, "running").await;
    let ready = access(&emulator, name, Some("rdp")).await;
    assert!(
        !text("an instance that has come up", &ready, "password").is_empty(),
        "case an instance that has come up: expected its password, got {ready:?}"
    );
    assert!(
        ready["passwordData"].is_null(),
        "case an instance that has come up: expected no passwordData beside the password, got \
         {ready:?}"
    );
    assert_host_key(row, &ready);
}

/// RULE: `password` and `passwordData` are documented "for a Windows Server-based
/// instance", so an RDP access to an instance of any other platform is told
/// neither — and is told no SSH key pair either, because it did not ask for SSH
/// access. What it is told is the instance's own addresses and the host key the
/// instance presents.
#[tokio::test]
async fn an_rdp_access_to_a_non_windows_instance_is_told_no_password() {
    for row in ROWS.iter().filter(|row| !row.is_windows) {
        let emulator = Emulator::new();
        let name = "rdp-on-linux";
        running_instance(&emulator, row, name).await;

        let details = access(&emulator, name, Some("rdp")).await;

        for member in ["password", "passwordData", "privateKey", "certKey", "expiresAt"] {
            assert!(
                details[member].is_null(),
                "case {}: {member} must be absent for RDP access to a non-Windows instance, got \
                 {details:?}",
                row.case
            );
        }
        assert_instance_members(&emulator, row, name, &details).await;
        assert_host_key(row, &details);
    }
}

// ---------------------------------------------------------------------------
// The recorded host key
// ---------------------------------------------------------------------------

/// RULE: the host key is recorded for the instance, so emptying that record with
/// `DeleteKnownHostKeys` makes the next access witness the key the instance
/// presents then — a different key, recorded at a later instant. That is how a
/// caller recovers from a host key mismatch.
#[tokio::test]
async fn deleting_the_known_host_keys_makes_the_next_access_witness_the_key_afresh() {
    for row in [&ROWS[0], &ROWS[2]] {
        let emulator = Emulator::new();
        let name = "rewitnessed";
        running_instance(&emulator, row, name).await;

        let first = access(&emulator, name, None).await;
        let was = single_host_key(row.case, &first).clone();

        emulator
            .ok("DeleteKnownHostKeys", json!({"instanceName": name}))
            .await;
        assert_eq!(
            lightsail_state::with_state(emulator.engine(), |store| store
                .instance(name)
                .map(|instance| instance.known_host_keys.len()))
            .expect("the suite's engine is the shared in-memory one"),
            Some(0),
            "case {}: the delete must leave no recorded host key",
            row.case
        );

        let again = access(&emulator, name, None).await;
        let now = single_host_key(row.case, &again).clone();
        assert_ne!(
            was["publicKey"], now["publicKey"],
            "case {}: the next access must witness the key presented then, got the deleted one \
             again",
            row.case
        );
        assert!(
            seconds(row.case, &now, "witnessedAt") >= seconds(row.case, &was, "witnessedAt"),
            "case {}: the fresh record is witnessed no earlier than the deleted one, got {:?} \
             then {:?}",
            row.case,
            was["witnessedAt"],
            now["witnessedAt"]
        );
        assert_host_key(row, &again);
    }
}

/// RULE: the key is the one the instance itself presents, so an instance that is
/// not running presents none: a STOPPED instance is still granted temporary keys
/// — the keys are the service's, not the instance's — and reports the key it
/// witnessed while it was running, unchanged.
#[tokio::test]
async fn a_stopped_instance_is_granted_keys_and_reports_the_key_it_already_witnessed() {
    let row = &ROWS[1];
    let emulator = Emulator::new();
    let name = "stopped-access";
    running_instance(&emulator, row, name).await;

    let running = access(&emulator, name, None).await;
    emulator
        .ok("StopInstance", json!({"instanceName": name, "force": true}))
        .await;
    wait_for_state(&emulator, name, "stopped").await;

    let stopped = access(&emulator, name, None).await;
    assert_eq!(
        stopped["hostKeys"], running["hostKeys"],
        "case a stopped instance: expected the key witnessed while it ran, got {:?}",
        stopped["hostKeys"]
    );
    assert_eq!(
        stopped["privateKey"], running["privateKey"],
        "case a stopped instance: the live grant is reported again, got {:?}",
        stopped["privateKey"]
    );

    // And an instance that has never run while anybody asked for access reports
    // no host key at all, rather than one nothing presented.
    let fresh = "stopped-before-access";
    emulator
        .ok("CreateInstances", create_body(row, fresh))
        .await;
    wait_for_state(&emulator, fresh, "running").await;
    emulator
        .ok("StopInstance", json!({"instanceName": fresh, "force": true}))
        .await;
    wait_for_state(&emulator, fresh, "stopped").await;
    let details = access(&emulator, fresh, None).await;
    assert_eq!(
        details["hostKeys"].as_array().map(Vec::len),
        Some(0),
        "case an instance stopped before any access: nothing presented a key to record, got {:?}",
        details["hostKeys"]
    );
    assert!(
        details["privateKey"].as_str().is_some(),
        "case an instance stopped before any access: it is still granted keys, got {details:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: the call resolves `instanceName` against the store, so a name that was
/// never created — or has since been deleted, which takes every grant made for it
/// with it — is refused with the service's `DoesNotExist` envelope.
#[tokio::test]
async fn access_to_an_unknown_instance_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        for protocol in [None, Some("ssh"), Some("rdp")] {
            let mut body = json!({"instanceName": name});
            if let Some(protocol) = protocol {
                body["protocol"] = json!(protocol);
            }
            let (status, answered) = emulator.call("GetInstanceAccessDetails", body).await;
            assert_eq!(
                status, 400,
                "case {name:?} over {protocol:?}: expected HTTP 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {name:?} over {protocol:?}: expected the wire error NotFoundException, got \
                 {answered:?}"
            );
            assert_eq!(
                answered["code"].as_str(),
                Some("DoesNotExist"),
                "case {name:?} over {protocol:?}: expected the body code DoesNotExist, got \
                 {answered:?}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(format!("The Instance does not exist: {name}").as_str()),
                "case {name:?} over {protocol:?}: expected the message to name the resource type \
                 and the requested name, got {answered:?}"
            );
        }
    }

    // A deleted instance is refused exactly like one that never existed.
    let name = "short-lived";
    running_instance(&emulator, &ROWS[0], name).await;
    access(&emulator, name, None).await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;
    let (status, answered) = emulator
        .call("GetInstanceAccessDetails", json!({"instanceName": name}))
        .await;
    assert_eq!(
        (status, answered["code"].as_str()),
        (400, Some("DoesNotExist")),
        "case after the instance is deleted: expected the DoesNotExist envelope, got {status} \
         {answered:?}"
    );
}

/// RULE: a protocol outside the modelled `InstanceAccessProtocol` enum is refused
/// by the centralized constraint walk before the handler runs, so nothing is
/// granted for it.
#[tokio::test]
async fn a_protocol_the_model_does_not_name_is_refused() {
    let emulator = Emulator::new();
    let name = "bad-protocol";
    running_instance(&emulator, &ROWS[0], name).await;

    for protocol in ["SSH", "telnet", "vnc", ""] {
        let (status, answered) = emulator
            .call(
                "GetInstanceAccessDetails",
                json!({"instanceName": name, "protocol": protocol}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case protocol {protocol:?}: expected HTTP 400, got {status} {answered:?}"
        );
    }
    // And no grant was made by any of them.
    assert!(
        lightsail_state::with_state(emulator.engine(), |store| {
            store
                .instance(name)
                .is_some_and(|instance| instance.access.ssh.is_none()
                    && instance.known_host_keys.is_empty())
        })
        .expect("the suite's engine is the shared in-memory one"),
        "case a refused protocol: nothing may be granted or recorded"
    );
}
