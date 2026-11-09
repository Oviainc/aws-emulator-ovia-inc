// SPDX-License-Identifier: CC0-1.0

//! The certificate revocation list a private CA publishes to object storage.
//!
//! The RULE under test: a CA whose `CrlConfiguration` is enabled publishes its OWN CRL —
//! the model carries no operation a client could publish one with — as a DER-encoded RFC
//! 5280 v2 `CertificateList` object in the bucket the configuration names, keyed by the
//! CA's id under the configuration's path, issued by the CA's X.500 distinguished name,
//! whose `nextUpdate` is `ExpirationInDays` days after it went out, carrying an increasing
//! CRL number and one entry per revoked certificate with that certificate's serial, its
//! revocation date and its `CRLReason` code. It is deposited whenever what it says can have
//! changed: when the configuration is stored, when the CA takes the certificate it signs
//! the CRL under, when the CA issues its first certificate and on every revocation.
//!
//! The rows differ along what the rule generalises over: which call stored the
//! configuration, the CRL lifetime (both published bounds of `ExpirationInDays`), the path
//! the object is keyed under, the ACL it is stored with, how many certificates are revoked
//! and the reason each was revoked for (every member `RevocationReason` declares).
//!
//! Every call goes over the wire through the two-service harness in
//! [`super::crl_bucket_resolution`]: the private CA and the object storage it publishes to,
//! mounted on ONE engine, so what the assertions read is what the CA itself deposited.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::REVOCATION_REASON_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::crl_reason_code;
use sha2::Digest;
use x509_cert::crl::CertificateList;
use x509_cert::der::asn1::{Any, OctetString};
use x509_cert::der::{Decode, DecodePem};
use x509_cert::ext::pkix::AuthorityKeyIdentifier;
use x509_cert::time::Time;
use x509_cert::{Certificate, Version};

use crate::crl_bucket_resolution::Emulator;
use crate::private_ca_lifecycle::{
    blob, ca_certificate_for, csr_pem, root_ca_request, string, subject_common_name,
    wait_for_the_clock,
};

/// The `cRLNumber` CRL extension (RFC 5280 section 5.2.3).
const CRL_NUMBER_OID: &str = "2.5.29.20";

/// The `reasonCode` CRL entry extension (RFC 5280 section 5.3.1).
const CRL_REASON_OID: &str = "2.5.29.21";

/// The `authorityKeyIdentifier` CRL extension (RFC 5280 section 5.2.1).
const AUTHORITY_KEY_IDENTIFIER_OID: &str = "2.5.29.35";

/// The `subjectKeyIdentifier` certificate extension (RFC 5280 section 4.2.1.2), which is
/// where the identifier the CRL's authority key identifier names is published.
const SUBJECT_KEY_IDENTIFIER_OID: &str = "2.5.29.14";

/// A day, the unit `ExpirationInDays` counts the CRL's validity period in.
const ONE_DAY: u64 = 24 * 60 * 60;

/// The revocation configuration a row submits: CRLs enabled for `bucket`, expiring after
/// `days`, optionally under a custom path and with a chosen object ACL.
fn crl_configuration(
    bucket: &str,
    days: i32,
    custom_path: Option<&str>,
    object_acl: Option<&str>,
) -> Value {
    let mut crl = json!({
        "Enabled": true,
        "S3BucketName": bucket,
        "ExpirationInDays": days,
    });
    if let Some(path) = custom_path {
        crl["CustomPath"] = json!(path);
    }
    if let Some(acl) = object_acl {
        crl["S3ObjectAcl"] = json!(acl);
    }
    json!({"CrlConfiguration": crl})
}

/// Create a CA — with `revocation` as its revocation configuration, when a row supplies
/// one — and activate it, answering its ARN and the certificate it was activated with.
///
/// `pub(crate)` so a sibling suite that drives the same two-service emulator activates a CA
/// through one helper rather than growing a second copy of it.
pub(crate) async fn active_ca(
    emulator: &Emulator,
    case: &str,
    common_name: &str,
    revocation: Option<&Value>,
) -> (String, String) {
    let mut body = root_ca_request(common_name);
    if let Some(revocation) = revocation {
        body["RevocationConfiguration"] = revocation.clone();
    }
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    // A test may wait for a due instant; a handler may not.
    wait_for_the_clock();
    // The certificate a CA takes is the one its own certification request asks for.
    let csr = string(
        &emulator
            .ok(
                case,
                "GetCertificateAuthorityCsr",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await,
        "Csr",
        case,
    );
    let certificate = ca_certificate_for(&csr);
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&certificate)}),
        )
        .await;
    (arn, certificate)
}

/// The CA's id: the last segment of its ARN, which is what names its CRL object.
fn certificate_authority_id(arn: &str, case: &str) -> String {
    arn.rsplit_once('/')
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: no id in {arn:?}"))
}

/// `GET /<bucket>/<key>` against the emulator's own object storage.
async fn object(emulator: &Emulator, bucket: &str, key: &str) -> (u16, Vec<u8>) {
    emulator
        .s3("GET", &format!("{bucket}/{key}"), &[], Vec::new())
        .await
}

/// The CRL object at `key`, decoded, or a panic naming the case.
async fn published_crl(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    key: &str,
) -> CertificateList {
    let (status, body) = object(emulator, bucket, key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the CA to have published {key:?} in {bucket:?}, got {status} \
         ({} bytes)",
        body.len()
    );
    CertificateList::from_der(&body).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a DER-encoded CRL at {key:?}, got {error} for {body:?}")
    })
}

/// An X.509 time as the Unix second it names.
fn seconds(time: &Time) -> u64 {
    time.to_unix_duration().as_secs()
}

/// The Unix second `time` falls in.
fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .expect("a time after the epoch")
        .as_secs()
}

/// The DER value of the CRL extension `oid` names, when the CRL carries one.
fn crl_extension(crl: &CertificateList, oid: &str) -> Option<Vec<u8>> {
    crl.tbs_cert_list
        .crl_extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id.to_string() == oid)
        .map(|extension| extension.extn_value.as_bytes().to_vec())
}

/// The CRL's number, read off its `cRLNumber` extension.
fn crl_number(crl: &CertificateList, case: &str) -> u64 {
    let value = crl_extension(crl, CRL_NUMBER_OID)
        .unwrap_or_else(|| panic!("case {case:?}: expected a cRLNumber extension on the CRL"));
    u64::from_der(&value).unwrap_or_else(|error| {
        panic!("case {case:?}: expected cRLNumber to carry an INTEGER, got {error}")
    })
}

/// The DER value of the certificate extension `oid` names, when the certificate carries
/// one.
fn certificate_extension(certificate: &Certificate, oid: &str) -> Option<Vec<u8>> {
    certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id.to_string() == oid)
        .map(|extension| extension.extn_value.as_bytes().to_vec())
}

/// The identifier of the key `certificate` holds: the one it publishes as its subject key
/// identifier (RFC 5280 section 4.2.1.2), else the leftmost 160 bits of the SHA-256 digest
/// over its `subjectPublicKey` (method 1 of RFC 7093 section 2), computed here from the
/// certificate rather than read off anything the service wrote.
fn key_identifier_of(certificate: &Certificate, case: &str) -> Vec<u8> {
    match certificate_extension(certificate, SUBJECT_KEY_IDENTIFIER_OID) {
        Some(encoded) => OctetString::from_der(&encoded)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected a KeyIdentifier OCTET STRING, got {error}")
            })
            .as_bytes()
            .to_vec(),
        None => sha2::Sha256::digest(
            certificate
                .tbs_certificate
                .subject_public_key_info
                .subject_public_key
                .raw_bytes(),
        )[..20]
            .to_vec(),
    }
}

/// A serial number as the VALUE it denotes: a DER integer carries no leading zero, and one
/// is added only to keep the value positive, so two spellings compare equal once stripped.
fn serial_value(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().skip_while(|byte| *byte == 0).collect()
}

/// The serial a certificate ARN carries, as that same value.
fn serial_of_arn(certificate_arn: &str, case: &str) -> Vec<u8> {
    let serial = certificate_arn
        .rsplit_once("/certificate/")
        .map(|(_, serial)| serial)
        .unwrap_or_else(|| panic!("case {case:?}: no serial in {certificate_arn:?}"));
    serial_value(&hex::decode(serial).expect("a certificate ARN carries a hex serial"))
}

/// Every entry of a published CRL: the revoked serial, the second it was revoked at and
/// the `CRLReason` code its entry extension carries.
fn revoked_entries(crl: &CertificateList, case: &str) -> BTreeMap<Vec<u8>, (u64, u8)> {
    crl.tbs_cert_list
        .revoked_certificates
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let extensions = entry.crl_entry_extensions.as_ref().unwrap_or_else(|| {
                panic!("case {case:?}: expected CRL entry extensions on every entry")
            });
            let reason = extensions
                .iter()
                .find(|extension| extension.extn_id.to_string() == CRL_REASON_OID)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a reasonCode extension on every entry")
                });
            let encoded = Any::from_der(reason.extn_value.as_bytes()).unwrap_or_else(|error| {
                panic!("case {case:?}: expected reasonCode to carry an ENUMERATED, got {error}")
            });
            let code = *encoded.value().first().unwrap_or_else(|| {
                panic!("case {case:?}: expected reasonCode to carry one byte of value")
            });
            (
                serial_value(entry.serial_number.as_bytes()),
                (seconds(&entry.revocation_date), code),
            )
        })
        .collect()
}

/// A CA with CRLs enabled publishes its CRL, whichever call stored the configuration,
/// under the key the configuration's path and the CA's id name, with the fields RFC 5280
/// and `CrlConfiguration` require.
#[tokio::test]
async fn an_enabled_crl_configuration_publishes_the_cas_own_crl() {
    let emulator = Emulator::new();

    // (case, bucket, whether the bucket's ACLs are enabled, ExpirationInDays, CustomPath,
    //  S3ObjectAcl, the key prefix the object is expected under, whether the configuration
    //  is stored by the UPDATE rather than by the create)
    //
    // `ExpirationInDays` is modelled `range 1..5000`, so the first two rows sit on its
    // published bounds; `CustomPath` is modelled `^(/|[-a-zA-Z0-9;?:@&=+$,%_.!~*()']+(/…)*)$`,
    // so `/` is the one value that names no folder at all.
    #[allow(clippy::type_complexity)]
    let rows: [(
        &str,
        &str,
        bool,
        i32,
        Option<&str>,
        Option<&str>,
        &str,
        bool,
    ); 6] = [
        (
            "the published floor of the CRL lifetime, configured by the update",
            "crl-floor",
            false,
            1,
            None,
            Some("BUCKET_OWNER_FULL_CONTROL"),
            "crl",
            true,
        ),
        (
            "the published ceiling of the CRL lifetime, configured by the create",
            "crl-ceiling",
            false,
            5000,
            None,
            Some("BUCKET_OWNER_FULL_CONTROL"),
            "crl",
            false,
        ),
        (
            "a custom path of one folder",
            "crl-one-folder",
            false,
            7,
            Some("crls"),
            Some("BUCKET_OWNER_FULL_CONTROL"),
            "crls",
            true,
        ),
        (
            "a custom path of nested folders, configured by the create",
            "crl-nested-folders",
            false,
            30,
            Some("pca/revocation/crls"),
            Some("BUCKET_OWNER_FULL_CONTROL"),
            "pca/revocation/crls",
            false,
        ),
        (
            "the one custom path that names no folder",
            "crl-root-path",
            false,
            90,
            Some("/"),
            Some("BUCKET_OWNER_FULL_CONTROL"),
            "crl",
            true,
        ),
        (
            "a publicly readable CRL in a bucket whose ACLs are enabled",
            "crl-public-read",
            true,
            7,
            None,
            Some("PUBLIC_READ"),
            "crl",
            true,
        ),
    ];

    for (case, bucket, acls_enabled, days, custom_path, object_acl, prefix, by_update) in rows {
        // The bucket is created through S3's own front door, so what the CA writes to is
        // object storage rather than anything this test seeded. A publicly readable object
        // needs a bucket whose ACLs affect permissions, which is the Object Ownership
        // setting S3 documents for exactly that ("ObjectWriter") with Block Public Access
        // turned off.
        let ownership = acls_enabled.then_some("ObjectWriter");
        let created = emulator.create_bucket_owned(bucket, ownership).await;
        assert_eq!(
            created, 200,
            "case {case:?}: expected CreateBucket for {bucket:?} to answer 200, got {created}"
        );
        if acls_enabled {
            let (status, body) = emulator
                .s3(
                    "PUT",
                    &format!("{bucket}?publicAccessBlock"),
                    &[("content-type", "application/xml")],
                    b"<PublicAccessBlockConfiguration><BlockPublicAcls>false</BlockPublicAcls>\
                      <IgnorePublicAcls>false</IgnorePublicAcls>\
                      <BlockPublicPolicy>false</BlockPublicPolicy>\
                      <RestrictPublicBuckets>false</RestrictPublicBuckets>\
                      </PublicAccessBlockConfiguration>"
                        .to_vec(),
                )
                .await;
            assert_eq!(
                status,
                200,
                "case {case:?}: expected PutPublicAccessBlock to answer 200, got {status} {:?}",
                String::from_utf8_lossy(&body)
            );
        }

        let configuration = crl_configuration(bucket, days, custom_path, object_acl);
        let published_at = SystemTime::now();
        let (arn, ca_certificate) = active_ca(
            &emulator,
            case,
            bucket,
            (!by_update).then_some(&configuration),
        )
        .await;
        if by_update {
            emulator
                .ok(
                    case,
                    "UpdateCertificateAuthority",
                    &json!({
                        "CertificateAuthorityArn": arn,
                        "RevocationConfiguration": configuration,
                    }),
                )
                .await;
        }

        let id = certificate_authority_id(&arn, case);
        let key = format!("{prefix}/{id}.crl");
        let crl = published_crl(&emulator, case, bucket, &key).await;

        // "Version: The current version number defined in RFC 5280 is V2."
        assert_eq!(
            crl.tbs_cert_list.version,
            Version::V2,
            "case {case:?}: expected a v2 CRL, got {:?}",
            crl.tbs_cert_list.version
        );
        // "Issuer: The X.500 distinguished name of your private CA that issued the CRL."
        let issuer = subject_common_name(&crl.tbs_cert_list.issuer);
        assert_eq!(
            issuer, bucket,
            "case {case:?}: expected the CA's own distinguished name as issuer, got {issuer:?}"
        );
        // "Signature Algorithm: The name of the algorithm used to sign the CRL." — the one
        // the CA was created with, which `root_ca_request` names SHA256WITHRSA.
        let algorithm = crl.signature_algorithm.oid.to_string();
        assert_eq!(
            algorithm, "1.2.840.113549.1.1.11",
            "case {case:?}: expected the CA's own signature algorithm, got {algorithm:?}"
        );
        assert_eq!(
            crl.tbs_cert_list.signature.oid, crl.signature_algorithm.oid,
            "case {case:?}: the signed list names the algorithm the signature was made with"
        );

        // "Last Update: The issue date and time of this CRL" and "Next Update: The day and
        // time by which the next CRL will be issued", the latter `ExpirationInDays` days on.
        let this_update = seconds(&crl.tbs_cert_list.this_update);
        let next_update = crl
            .tbs_cert_list
            .next_update
            .as_ref()
            .map(seconds)
            .unwrap_or_else(|| panic!("case {case:?}: expected a nextUpdate on the CRL"));
        assert!(
            this_update >= unix_seconds(published_at)
                && this_update <= unix_seconds(SystemTime::now()),
            "case {case:?}: expected thisUpdate inside the call that published it, got \
             {this_update}"
        );
        assert_eq!(
            next_update - this_update,
            u64::try_from(days).expect("a positive lifetime") * ONE_DAY,
            "case {case:?}: expected nextUpdate {days} day(s) after thisUpdate, got \
             {this_update} .. {next_update}"
        );

        // RFC 5280 section 5.1.2.6: a CRL with nothing revoked carries no entry list.
        assert!(
            crl.tbs_cert_list.revoked_certificates.is_none(),
            "case {case:?}: expected no entries before anything is revoked, got {:?}",
            crl.tbs_cert_list.revoked_certificates
        );
        // "X509v3 CRL Number: Decimal sequence number for the CRL" — the first one the CA
        // publishes.
        let number = crl_number(&crl, case);
        assert_eq!(
            number, 1,
            "case {case:?}: expected the CA's first CRL to be number 1, got {number}"
        );
        // "X509v3 Authority Key Identifier: Identifies the public key associated with the
        // private key used to sign the certificate" — the identifier the CA's own
        // certificate publishes for its key.
        let encoded = crl_extension(&crl, AUTHORITY_KEY_IDENTIFIER_OID).unwrap_or_else(|| {
            panic!("case {case:?}: expected an authorityKeyIdentifier extension on the CRL")
        });
        let authority_key = AuthorityKeyIdentifier::from_der(&encoded)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected an AuthorityKeyIdentifier, got {error}")
            })
            .key_identifier
            .map(|identifier| identifier.as_bytes().to_vec());
        let ca_certificate = Certificate::from_pem(&ca_certificate)
            .unwrap_or_else(|error| panic!("case {case:?}: the CA certificate parses: {error}"));
        assert_eq!(
            authority_key,
            Some(key_identifier_of(&ca_certificate, case)),
            "case {case:?}: expected the CRL's authority key identifier to be the identifier \
             of the key the CA's own certificate holds"
        );

        // The object is where the configuration says and nowhere else: a configured custom
        // path moves it off the default folder.
        if prefix != "crl" {
            let (status, _) = object(&emulator, bucket, &format!("crl/{id}.crl")).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected no CRL under the default folder, got {status}"
            );
        }

        // The ACL the configuration names is the one the object carries: a publicly
        // readable CRL grants the all-users group READ, which S3 reports on the object.
        let (status, acl_body) = emulator
            .s3("GET", &format!("{bucket}/{key}?acl"), &[], Vec::new())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the object's ACL to be readable, got {status}"
        );
        let acl = String::from_utf8_lossy(&acl_body).to_string();
        let public = acl.contains("http://acs.amazonaws.com/groups/global/AllUsers");
        assert_eq!(
            public,
            object_acl == Some("PUBLIC_READ"),
            "case {case:?}: expected a publicly readable CRL only for PUBLIC_READ, got {acl:?}"
        );
    }
}

/// Every `RevocationReason` the model declares: each revocation refreshes the published
/// CRL, which gains the revoked serial with its revocation date and its reason code, keeps
/// the serials revoked before it, and carries the next CRL number.
#[tokio::test]
async fn revoking_a_certificate_refreshes_the_published_crl() {
    let emulator = Emulator::new();
    let bucket = "crl-revocations";
    let case = "the CA the revocations are made against";
    assert_eq!(
        emulator.create_bucket(bucket).await,
        200,
        "the bucket the CRL is published to is created through S3"
    );

    let configuration = crl_configuration(bucket, 7, None, Some("BUCKET_OWNER_FULL_CONTROL"));
    let (arn, _certificate) = active_ca(&emulator, case, bucket, Some(&configuration)).await;
    let id = certificate_authority_id(&arn, case);
    let key = format!("crl/{id}.crl");
    let csr = csr_pem("revoked-leaf");

    // The CRL the CA published on activation: nothing is revoked yet.
    let initial = published_crl(&emulator, case, bucket, &key).await;
    assert!(
        revoked_entries(&initial, case).is_empty(),
        "case {case:?}: expected no entries before anything is revoked"
    );
    let mut number = crl_number(&initial, case);
    let mut expected: BTreeMap<Vec<u8>, u8> = BTreeMap::new();

    for reason in REVOCATION_REASON_VALUES {
        let case = format!("a certificate revoked for {reason}");
        let issued = emulator
            .ok(
                &case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Type": "DAYS", "Value": 30},
                    // Each row wants a certificate of its own over the one certification
                    // request, which is what a token per call asks for: "If you change the
                    // idempotency token for each call, Amazon Web Services Private CA
                    // recognizes that you are requesting multiple certificates"
                    // (`IssueCertificateRequest$IdempotencyToken`).
                    "IdempotencyToken": format!("crl-{reason}"),
                }),
            )
            .await;
        let certificate_arn = string(&issued, "CertificateArn", &case);
        let serial = serial_of_arn(&certificate_arn, &case);
        let serial_hex = certificate_arn
            .rsplit_once("/certificate/")
            .map(|(_, serial)| serial.to_string())
            .expect("an issued certificate's ARN carries its serial");

        // Issuing changes nothing a CRL says — a CRL lists what was REVOKED — so the
        // entries are still the ones revoked so far.
        let after_issue = published_crl(&emulator, &case, bucket, &key).await;
        let listed: BTreeMap<Vec<u8>, u8> = revoked_entries(&after_issue, &case)
            .into_iter()
            .map(|(serial, (_, code))| (serial, code))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected issuing to leave the CRL's entries alone"
        );

        let revoked_at = SystemTime::now();
        emulator
            .ok(
                &case,
                "RevokeCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "CertificateSerial": serial_hex,
                    "RevocationReason": reason,
                }),
            )
            .await;

        let refreshed = published_crl(&emulator, &case, bucket, &key).await;
        let code = crl_reason_code(reason)
            .unwrap_or_else(|| panic!("case {case:?}: every declared reason has a CRLReason"));
        expected.insert(serial.clone(), code);
        let entries = revoked_entries(&refreshed, &case);
        let listed: BTreeMap<Vec<u8>, u8> = entries
            .iter()
            .map(|(serial, (_, code))| (serial.clone(), *code))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the CRL to list every serial revoked so far, each with \
             the reason code its revocation named"
        );
        let (listed_at, _) = entries
            .get(&serial)
            .unwrap_or_else(|| panic!("case {case:?}: expected the revoked serial to be listed"));
        assert!(
            *listed_at >= unix_seconds(revoked_at)
                && *listed_at <= unix_seconds(SystemTime::now()),
            "case {case:?}: expected the entry's revocation date to be when the revocation \
             was accepted, got {listed_at}"
        );

        // "X509v3 CRL Number: Decimal sequence number for the CRL": each CRL that goes out
        // carries the next number.
        let refreshed_number = crl_number(&refreshed, &case);
        assert!(
            refreshed_number > number,
            "case {case:?}: expected a CRL number above {number}, got {refreshed_number}"
        );
        number = refreshed_number;
    }
}

/// A CA whose CRLs are not enabled publishes nothing: the bucket stays empty however many
/// certificates it issues, and reading the CA does not deposit an object either.
#[tokio::test]
async fn a_ca_without_enabled_crls_publishes_nothing() {
    let emulator = Emulator::new();
    let bucket = "crl-disabled";
    assert_eq!(
        emulator.create_bucket(bucket).await,
        200,
        "the bucket the rows would publish to is created through S3"
    );

    // (case, the revocation configuration the CA is created with, whether a CRL appears)
    let rows: [(&str, Option<Value>, bool); 4] = [
        ("no revocation configuration at all", None, false),
        (
            "a configuration with CRLs disabled",
            Some(json!({"CrlConfiguration": {"Enabled": false}})),
            false,
        ),
        (
            "a configuration that enables OCSP only",
            Some(json!({"OcspConfiguration": {"Enabled": true}})),
            false,
        ),
        (
            "a configuration with CRLs enabled, the control",
            Some(crl_configuration(
                bucket,
                7,
                None,
                Some("BUCKET_OWNER_FULL_CONTROL"),
            )),
            true,
        ),
    ];

    let csr = csr_pem("unpublished-leaf");
    for (index, (case, revocation, publishes)) in rows.iter().enumerate() {
        let (arn, _certificate) =
            active_ca(&emulator, case, &format!("no-crl-{index}"), revocation.as_ref()).await;
        emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Validity": {"Type": "DAYS", "Value": 30},
                }),
            )
            .await;
        // A read reports; it deposits nothing a later caller could not have seen anyway.
        emulator
            .ok(
                case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;

        let id = certificate_authority_id(&arn, case);
        let (status, _) = object(&emulator, bucket, &format!("crl/{id}.crl")).await;
        let expected = if *publishes { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected the CRL object to answer {expected}, got {status}"
        );
    }
}
