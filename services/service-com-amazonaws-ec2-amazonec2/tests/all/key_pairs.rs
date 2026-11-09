// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateKeyPair` and the `DescribeKeyPairs` read it round-trips
//! through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=CreateKeyPair`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation GENERATES A REAL KEY PAIR OF THE TYPE AND IN
//! THE FORMAT THE REQUEST ASKED FOR, hands the caller the private half once and stores the
//! public half under a service-minted id: the private key parses as a key of that type, its
//! public half is the public key the store reports, the fingerprint is the one that type is
//! fingerprinted with, and the record reads back through `DescribeKeyPairs`. A name the
//! account already holds is refused, and a dry run stores nothing.
//!
//! Every table row is a distinct input along what that generalises over — the key type, the
//! private-key format, whether either is defaulted, the name's length and character set, and
//! the tag count — so the one case a recording happens to contain is one row among many
//! rather than the specification. The two published bounds on a name (255 characters, ASCII)
//! are each tested AT the bound and one past it.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ::base64::Engine as _;
use ::pkcs1::der::Decode as _;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateKeyPair` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateKeyPair", params).await
}

/// `CreateKeyPair` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateKeyPair, got {status} with body {body}"
    );
    body
}

/// One `DescribeKeyPairs` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeKeyPairs", params).await
}

/// `DescribeKeyPairs` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeKeyPairs, got {status} with body {body}"
    );
    body
}

/// A scalar member of the one key pair a `DescribeKeyPairs` body reports, or `None` when the
/// response carries no such member.
///
/// The key pair is the element that has a `keyPairId` child, so a member is read off the
/// record rather than found anywhere in the document.
fn described_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let record = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "keyPairId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no key pair (no <keyPairId>) in {body}"));
    child_text(record, name)
}

/// The same member, required: a panic names the case when the response does not carry it.
fn described(case: &str, body: &str, name: &str) -> String {
    described_member(case, body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the key pair in {body}"))
}

/// The tags one described key pair carries, as key/value pairs in document order.
fn described_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let record = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "keyPairId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no key pair (no <keyPairId>) in {body}"));
    super::wire::set_items(record, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The form fields naming a key pair's tags, as one `key-pair` tag specification.
fn tag_fields(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut fields = Vec::new();
    if tags.is_empty() {
        return fields;
    }
    fields.push(param("TagSpecification.1.ResourceType", "key-pair"));
    for (index, (key, value)) in tags.iter().enumerate() {
        fields.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    fields
}

/// One row of the create table: the case, the key name, the `KeyType` the request names (or
/// none), the `KeyFormat` it names (or none), and the tags it assigns.
type CreateCase = (
    &'static str,
    String,
    Option<&'static str>,
    Option<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// The key type a row that names none gets: "Default: rsa"
/// (`CreateKeyPairRequest$KeyType`), read off the crate's own constant rather than a second
/// copy of that default.
fn defaulted_key_type() -> &'static str {
    ec2::state::KEY_TYPE_RSA
}

/// Every row of the create table: both key types, both formats, each defaulted and named,
/// with names of different lengths and character sets and different tag counts.
fn create_cases() -> Vec<CreateCase> {
    vec![
        (
            "rsa and pem both defaulted",
            "deploy-key".to_string(),
            None,
            None,
            Vec::new(),
        ),
        (
            "rsa pem named, one tag",
            "ops_key.1".to_string(),
            Some("rsa"),
            Some("pem"),
            vec![("Owner", "TeamA")],
        ),
        (
            "rsa ppk",
            "windows-admin".to_string(),
            Some("rsa"),
            Some("ppk"),
            Vec::new(),
        ),
        (
            "ed25519 pem, two tags",
            "ed-key".to_string(),
            Some("ed25519"),
            Some("pem"),
            vec![("Owner", "TeamB"), ("Env", "staging")],
        ),
        (
            "ed25519 ppk",
            "ed-putty".to_string(),
            Some("ed25519"),
            Some("ppk"),
            Vec::new(),
        ),
        (
            "name of one character",
            "k".to_string(),
            Some("ed25519"),
            None,
            Vec::new(),
        ),
        (
            "name at the published maximum of 255 characters",
            "n".repeat(ec2::state::MAX_KEY_PAIR_NAME_CHARS),
            None,
            None,
            Vec::new(),
        ),
        (
            "name of punctuation and digits",
            "key-2026.06_v2/west".to_string(),
            Some("rsa"),
            None,
            Vec::new(),
        ),
    ]
}

/// The form fields one create-table row sends.
fn create_fields(row: &CreateCase) -> Vec<Param> {
    let (_, key_name, key_type, key_format, tags) = row;
    let mut fields = vec![param("KeyName", key_name)];
    if let Some(key_type) = key_type {
        fields.push(param("KeyType", key_type));
    }
    if let Some(key_format) = key_format {
        fields.push(param("KeyFormat", key_format));
    }
    fields.extend(tag_fields(tags));
    fields
}

/// The public key of the SSH key `openssh` is the one-line public key of, or a panic naming
/// the case: proof the reported public key material is a key a client can read.
fn parsed_public_key(case: &str, openssh: &str) -> ::ssh_key::PublicKey {
    ::ssh_key::PublicKey::from_openssh(openssh)
        .unwrap_or_else(|error| panic!("case {case:?}: {openssh:?} must be an SSH public key ({error})"))
}

/// The modulus of the RSA private key the PEM `material` carries, with leading zero bytes
/// dropped — what an SSH `mpint` of that modulus carries.
fn rsa_pem_modulus(case: &str, material: &str) -> Vec<u8> {
    let (label, der) = ::pkcs1::der::Document::from_pem(material).unwrap_or_else(|error| {
        panic!("case {case:?}: the key material must be a PEM document ({error}): {material}")
    });
    assert_eq!(
        label, "RSA PRIVATE KEY",
        "case {case:?}: expected a PKCS#1 PEM block, got the label {label:?}"
    );
    let key = ::pkcs1::RsaPrivateKey::from_der(der.as_bytes()).unwrap_or_else(|error| {
        panic!("case {case:?}: the key material must be a PKCS#1 RSA private key ({error})")
    });
    let modulus = key.modulus.as_bytes().to_vec();
    assert_eq!(
        modulus.len() * 8,
        2048,
        "case {case:?}: expected a 2048-bit modulus, got {} bits",
        modulus.len() * 8
    );
    modulus
}

/// The lines of a PuTTY private key file, with the `<header>: <value>` lines read out of it
/// — the file's own description of what it holds.
fn putty_headers(case: &str, material: &str) -> Vec<(String, String)> {
    material
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(header, value)| (header.to_string(), value.to_string()))
        .inspect(|(header, _)| {
            assert!(
                !header.is_empty(),
                "case {case:?}: a PuTTY file header must be named: {material}"
            )
        })
        .collect()
}

/// The value of the PuTTY header `name`, or a panic naming the case.
fn putty_header(case: &str, material: &str, name: &str) -> String {
    putty_headers(case, material)
        .into_iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("case {case:?}: no {name:?} line in {material}"))
}

/// The base64 body that follows the `<name>-Lines: <count>` header of a PuTTY file, decoded
/// — and the count asserted against the lines actually there.
fn putty_blob(case: &str, material: &str, name: &str) -> Vec<u8> {
    let header = format!("{name}-Lines: ");
    let (_, rest) = material
        .split_once(&header)
        .unwrap_or_else(|| panic!("case {case:?}: no {header:?} line in {material}"));
    let mut lines = rest.lines();
    let count: usize = lines
        .next()
        .unwrap_or_default()
        .parse()
        .unwrap_or_else(|error| panic!("case {case:?}: {header:?} must carry a count ({error})"));
    let body: String = lines.by_ref().take(count).collect();
    ::base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .unwrap_or_else(|error| panic!("case {case:?}: {name} must be base64 ({error}): {material}"))
}

/// Every key pair the whole store reports, by id.
async fn described_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = describe_ok(case, router, &[]).await;
    elements(&body, "keyPairId")
}

/// `CreateKeyPair` generates a real key pair of the asked-for type in the asked-for format,
/// answers it with a service-minted id, and stores a public half that belongs to the private
/// half it handed back — for every row of the table.
#[tokio::test]
async fn create_key_pair_generates_the_asked_for_key_and_stores_its_public_half() {
    let router = emulator();

    for row in create_cases() {
        let (case, key_name, key_type, key_format, tags) = row.clone();
        let expected_type = key_type.unwrap_or_else(defaulted_key_type);
        let expected_format = key_format.unwrap_or(ec2::state::KEY_FORMAT_PEM);
        let body = create_ok(case, &router, &create_fields(&row)).await;

        let reported_name = element(&body, "keyName")
            .unwrap_or_else(|| panic!("case {case:?}: no <keyName> in {body}"));
        assert_eq!(
            reported_name, key_name,
            "case {case:?}: expected the created key pair to be named {key_name:?}, \
             got {reported_name:?}"
        );
        let key_pair_id = element(&body, "keyPairId")
            .unwrap_or_else(|| panic!("case {case:?}: no <keyPairId> in {body}"));
        assert!(
            is_long_id("key", &key_pair_id),
            "case {case:?}: expected a minted key pair id, got {key_pair_id:?}"
        );
        let fingerprint = element(&body, "keyFingerprint")
            .unwrap_or_else(|| panic!("case {case:?}: no <keyFingerprint> in {body}"));
        let material = super::ec2_query::unescaped(
            &element(&body, "keyMaterial")
                .unwrap_or_else(|| panic!("case {case:?}: no <keyMaterial> in {body}")),
        );

        // The public key the store holds for this key pair, which is also what proves the
        // private half the caller was handed is that key pair's.
        let described_body = describe_ok(
            case,
            &router,
            &[
                param("KeyPairId.1", &key_pair_id),
                param("IncludePublicKey", "true"),
            ],
        )
        .await;
        let public_key = described(case, &described_body, "publicKey");
        let parsed_public = parsed_public_key(case, &public_key);

        match expected_type {
            "ed25519" => {
                assert_eq!(
                    parsed_public.algorithm().as_str(),
                    "ssh-ed25519",
                    "case {case:?}: expected an Ed25519 public key, got {public_key:?}"
                );
                // "For ED25519 key pairs, the key fingerprint is the base64-encoded SHA-256
                // digest, which is the default for OpenSSH" — so the reported fingerprint is
                // the one OpenSSH itself computes over this very public key.
                let expected = parsed_public
                    .fingerprint(::ssh_key::HashAlg::Sha256)
                    .to_string();
                assert_eq!(
                    fingerprint, expected,
                    "case {case:?}: expected the fingerprint of the stored public key \
                     ({expected}), got {fingerprint}"
                );
            }
            _ => {
                assert_eq!(
                    parsed_public.algorithm().as_str(),
                    "ssh-rsa",
                    "case {case:?}: expected an RSA public key, got {public_key:?}"
                );
                // "For RSA key pairs, the key fingerprint is the SHA-1 digest of the DER
                // encoded private key": 20 bytes, as the colon-separated hex pairs EC2
                // publishes a key fingerprint in.
                let octets: Vec<&str> = fingerprint.split(':').collect();
                assert_eq!(
                    octets.len(),
                    20,
                    "case {case:?}: expected a 20-octet SHA-1 fingerprint, got {fingerprint}"
                );
                assert!(
                    octets
                        .iter()
                        .all(|octet| octet.len() == 2
                            && octet.chars().all(|digit| digit.is_ascii_hexdigit()
                                && !digit.is_ascii_uppercase())),
                    "case {case:?}: expected lowercase hex octet pairs, got {fingerprint}"
                );
            }
        }

        match (expected_type, expected_format) {
            // "An unencrypted PEM encoded PKCS#1 private key" (`CreateKeyPair`), whose
            // modulus is the modulus of the public key the store reports.
            ("rsa", "pem") => {
                let modulus = rsa_pem_modulus(case, &material);
                let public_modulus = match parsed_public.key_data() {
                    ::ssh_key::public::KeyData::Rsa(rsa) => rsa
                        .n
                        .as_positive_bytes()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: the public modulus must be positive")
                        })
                        .to_vec(),
                    other => panic!("case {case:?}: expected RSA public key data, got {other:?}"),
                };
                assert_eq!(
                    public_modulus, modulus,
                    "case {case:?}: the stored public key must be the public half of the \
                     private key the caller was handed"
                );
            }
            // An Ed25519 private key has no PKCS#1 form, so the PEM private key is the
            // OpenSSH one `ssh-keygen` writes, and it carries its own public half.
            ("ed25519", "pem") => {
                let private = ::ssh_key::PrivateKey::from_openssh(&material).unwrap_or_else(
                    |error| {
                        panic!(
                            "case {case:?}: the key material must be an OpenSSH private key \
                             ({error}): {material}"
                        )
                    },
                );
                assert_eq!(
                    private.public_key().to_openssh().unwrap_or_default().trim_end(),
                    public_key,
                    "case {case:?}: the stored public key must be the public half of the \
                     private key the caller was handed"
                );
            }
            // "An unencrypted PPK formatted private key for use with PuTTY"
            // (`CreateKeyPair`): the file names the key's algorithm, carries no encryption,
            // names the key pair, and holds the same public key blob in its public lines.
            (_, "ppk") => {
                assert_eq!(
                    putty_header(case, &material, "Encryption"),
                    "none",
                    "case {case:?}: the PuTTY file must be unencrypted: {material}"
                );
                assert_eq!(
                    putty_header(case, &material, "Comment"),
                    key_name,
                    "case {case:?}: the PuTTY file must name the key pair: {material}"
                );
                let algorithm = parsed_public.algorithm().as_str().to_string();
                assert_eq!(
                    putty_header(case, &material, "PuTTY-User-Key-File-2"),
                    algorithm,
                    "case {case:?}: the PuTTY file must name the key's algorithm: {material}"
                );
                let public_blob = putty_blob(case, &material, "Public");
                let rebuilt = format!(
                    "{algorithm} {}",
                    ::base64::engine::general_purpose::STANDARD.encode(&public_blob)
                );
                assert_eq!(
                    parsed_public_key(case, &rebuilt).key_data(),
                    parsed_public.key_data(),
                    "case {case:?}: the PuTTY file's public blob must be this key pair's \
                     public key: {material}"
                );
                assert!(
                    !putty_blob(case, &material, "Private").is_empty(),
                    "case {case:?}: the PuTTY file must carry the private half: {material}"
                );
                let mac = putty_header(case, &material, "Private-MAC");
                assert!(
                    mac.len() == 40 && mac.chars().all(|digit| digit.is_ascii_hexdigit()),
                    "case {case:?}: expected a hex SHA-1 keyed digest, got {mac:?}"
                );
            }
            (key_type, key_format) => {
                panic!("case {case:?}: untested combination {key_type}/{key_format}")
            }
        }

        // The round trip: every member the create answered reads back off the stored record,
        // beside the type and the creation time only a read reports.
        assert_eq!(
            described(case, &described_body, "keyName"),
            key_name,
            "case {case:?}: DescribeKeyPairs must report the stored name"
        );
        assert_eq!(
            described(case, &described_body, "keyFingerprint"),
            fingerprint,
            "case {case:?}: DescribeKeyPairs must report the fingerprint the create answered"
        );
        assert_eq!(
            described(case, &described_body, "keyType"),
            expected_type,
            "case {case:?}: DescribeKeyPairs must report the key type that was created"
        );
        let create_time = described(case, &described_body, "createTime");
        assert!(
            super::ec2_query::as_timestamp(&create_time).is_some()
                || create_time.contains('T'),
            "case {case:?}: expected an ISO 8601 creation time, got {create_time:?}"
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let mut reported_tags = described_tags(case, &described_body);
        reported_tags.sort();
        let mut expected_sorted = expected_tags.clone();
        expected_sorted.sort();
        assert_eq!(
            reported_tags, expected_sorted,
            "case {case:?}: DescribeKeyPairs must report the tags the create assigned"
        );

        // "If true, the public key material is included in the response. Default: false"
        // (`$IncludePublicKey`): a read that did not ask for it is not given it.
        let without_public_key =
            describe_ok(case, &router, &[param("KeyName.1", &key_name)]).await;
        assert_eq!(
            described_member(case, &without_public_key, "publicKey"),
            None,
            "case {case:?}: a read that did not ask for the public key must not report one: \
             {without_public_key}"
        );
        // The private half is held nowhere, so no read can answer one.
        assert!(
            !without_public_key.contains("PRIVATE KEY")
                && !without_public_key.contains("keyMaterial"),
            "case {case:?}: a read must never report private key material: \
             {without_public_key}"
        );
    }
}

/// Two key pairs of one account carry different ids, different fingerprints and different
/// key material: the key is generated per request rather than drawn from one fixed key.
#[tokio::test]
async fn every_created_key_pair_is_a_different_key() {
    let router = emulator();
    let mut fingerprints: Vec<String> = Vec::new();
    let mut materials: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();

    for (index, key_type) in ["rsa", "ed25519", "rsa", "ed25519"].iter().enumerate() {
        let case = format!("key {index} of type {key_type}");
        let body = create_ok(
            &case,
            &router,
            &[
                param("KeyName", &format!("rotating-{index}")),
                param("KeyType", key_type),
            ],
        )
        .await;
        ids.push(element(&body, "keyPairId").unwrap_or_default());
        fingerprints.push(element(&body, "keyFingerprint").unwrap_or_default());
        materials.push(element(&body, "keyMaterial").unwrap_or_default());
    }

    for values in [&ids, &fingerprints, &materials] {
        let mut unique = values.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            values.len(),
            "expected four distinct values, got {values:?}"
        );
    }
}

/// One row of a refusal table: the case, the action, the request's form fields, the EC2
/// error code the service answers, the HTTP status, and the message it words the refusal
/// with when that message is fixed by the request.
type RefusalCase = (
    &'static str,
    &'static str,
    Vec<Param>,
    &'static str,
    u16,
    Option<String>,
);

/// A name the account already holds, a value outside a modelled catalog, a name past either
/// published bound, a selector that resolves to nothing and an undocumented filter are each
/// refused with EC2's own code for that class — and a dry run reports that the request would
/// have succeeded.
#[tokio::test]
async fn key_pair_requests_outside_the_rules_are_refused() {
    let router = emulator();
    let held = "already-held";
    create_ok("the key pair the duplicate case collides with", &router, &[param("KeyName", held)])
        .await;
    let body = create_ok(
        "the key pair the selector cases resolve",
        &router,
        &[param("KeyName", "resolvable"), param("KeyType", "ed25519")],
    )
    .await;
    let held_id = element(&body, "keyPairId").unwrap_or_default();

    let over_long = "n".repeat(ec2::state::MAX_KEY_PAIR_NAME_CHARS + 1);
    let cases: Vec<RefusalCase> = vec![
        (
            "a name the account already holds",
            "CreateKeyPair",
            vec![param("KeyName", held)],
            "InvalidKeyPair.Duplicate",
            400,
            // EC2's own sentence for this error class, written out rather than read off the
            // crate's constant, so a drift in that constant fails this row.
            Some("The keypair already exists".to_string()),
        ),
        (
            "a name one character past the published maximum",
            "CreateKeyPair",
            vec![param("KeyName", &over_long)],
            "InvalidParameterValue",
            400,
            None,
        ),
        (
            "a name outside ASCII",
            "CreateKeyPair",
            vec![param("KeyName", "clé-de-déploiement")],
            "InvalidParameterValue",
            400,
            None,
        ),
        (
            "an empty name",
            "CreateKeyPair",
            vec![param("KeyName", "")],
            "MissingParameter",
            400,
            None,
        ),
        (
            // The modelled `KeyType` enum is enforced at the BOUNDARY, before any handler
            // runs, so an out-of-catalog value is refused as a modelled-constraint
            // violation rather than by this operation.
            "a key type outside the modelled catalog",
            "CreateKeyPair",
            vec![param("KeyName", "dsa-please"), param("KeyType", "dsa")],
            "ValidationError",
            400,
            None,
        ),
        (
            // `KeyFormat` is a modelled enum too, so its violation is refused the same way.
            "a key format outside the modelled catalog",
            "CreateKeyPair",
            vec![param("KeyName", "der-please"), param("KeyFormat", "der")],
            "ValidationError",
            400,
            None,
        ),
        (
            "a tag specification naming another resource type",
            "CreateKeyPair",
            vec![
                param("KeyName", "mistagged"),
                param("TagSpecification.1.ResourceType", "instance"),
                param("TagSpecification.1.Tag.1.Key", "Owner"),
                param("TagSpecification.1.Tag.1.Value", "TeamA"),
            ],
            "InvalidParameterValue",
            400,
            None,
        ),
        (
            "a dry run of a request that would have succeeded",
            "CreateKeyPair",
            vec![param("KeyName", "never-stored"), param("DryRun", "true")],
            "DryRunOperation",
            412,
            Some("Request would have succeeded, but DryRun flag is set.".to_string()),
        ),
        (
            "a dry run that would have been refused as a duplicate is refused for that reason",
            "CreateKeyPair",
            vec![param("KeyName", held), param("DryRun", "true")],
            "InvalidKeyPair.Duplicate",
            400,
            Some("The keypair already exists".to_string()),
        ),
        (
            "a read naming a key pair name the account holds none of",
            "DescribeKeyPairs",
            vec![param("KeyName.1", "no-such-key")],
            "InvalidKeyPair.NotFound",
            400,
            Some("The key pair 'no-such-key' does not exist".to_string()),
        ),
        (
            "a read naming a key pair id the account holds none of",
            "DescribeKeyPairs",
            vec![param("KeyPairId.1", "key-00000000000000999")],
            "InvalidKeyPair.NotFound",
            400,
            Some("The key pair ID 'key-00000000000000999' does not exist".to_string()),
        ),
        (
            "a read naming one resolvable and one unresolvable name",
            "DescribeKeyPairs",
            vec![param("KeyName.1", "resolvable"), param("KeyName.2", "ghost")],
            "InvalidKeyPair.NotFound",
            400,
            Some("The key pair 'ghost' does not exist".to_string()),
        ),
        (
            "a read filtering on a name the operation does not publish",
            "DescribeKeyPairs",
            filters(&[("key-type", &["rsa"])]),
            "InvalidParameterValue",
            400,
            Some("The filter 'key-type' is invalid".to_string()),
        ),
        (
            "a dry run of a read that would have succeeded",
            "DescribeKeyPairs",
            vec![param("DryRun", "true")],
            "DryRunOperation",
            412,
            Some("Request would have succeeded, but DryRun flag is set.".to_string()),
        ),
    ];

    for (case, action, params, expected_code, expected_status, expected_message) in cases {
        let (status, body) = super::ec2_query::call(&router, REGION, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} \
             in {body}"
        );
        if let Some(expected_message) = expected_message {
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {message:?}"
            );
        }
    }

    // Nothing the refusals above named was stored: the store still holds exactly the two key
    // pairs this test created, in id order.
    let ids = described_ids("the store after the refusals", &router).await;
    assert_eq!(
        ids.len(),
        2,
        "expected the two created key pairs and nothing a refusal named, got {ids:?}"
    );
    assert!(
        ids.contains(&held_id),
        "expected the store to still hold {held_id:?}, got {ids:?}"
    );
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "expected the key pairs to be reported in id order, got {ids:?}"
    );
}

/// What `DescribeKeyPairs` selects: each published filter narrows the store to the key pairs
/// that hold its value, several filters are joined with AND, and a request naming no
/// selector reports every key pair the account holds.
#[tokio::test]
async fn describe_key_pairs_selects_on_every_published_filter() {
    let router = emulator();

    // Three key pairs that differ in the one member each filter selects on.
    let mut created: Vec<(&str, String, String)> = Vec::new();
    for (key_name, key_type, tags) in [
        ("alpha", "rsa", vec![("Owner", "TeamA")]),
        ("beta", "ed25519", vec![("Owner", "TeamB")]),
        ("gamma", "ed25519", vec![("Env", "prod")]),
    ] {
        let mut fields = vec![param("KeyName", key_name), param("KeyType", key_type)];
        fields.extend(tag_fields(&tags));
        let body = create_ok(key_name, &router, &fields).await;
        created.push((
            key_name,
            element(&body, "keyPairId").unwrap_or_default(),
            element(&body, "keyFingerprint").unwrap_or_default(),
        ));
    }
    let (_, alpha_id, alpha_fingerprint) = created[0].clone();

    // The filter table: each row is a selector and the key names it must report.
    let cases: Vec<(&str, Vec<Param>, Vec<&str>)> = vec![
        ("no selector reports every key pair", Vec::new(), vec!["alpha", "beta", "gamma"]),
        (
            "key-name selects one",
            filters(&[("key-name", &["beta"])]),
            vec!["beta"],
        ),
        (
            "key-name selects several",
            filters(&[("key-name", &["alpha", "gamma"])]),
            vec!["alpha", "gamma"],
        ),
        (
            "key-name matches a wildcard",
            filters(&[("key-name", &["*a*"])]),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "key-pair-id selects the key pair it names",
            filters(&[("key-pair-id", &[alpha_id.as_str()])]),
            vec!["alpha"],
        ),
        (
            "fingerprint selects the key with that fingerprint",
            filters(&[("fingerprint", &[alpha_fingerprint.as_str()])]),
            vec!["alpha"],
        ),
        (
            "tag-key selects every key pair tagged with that key",
            filters(&[("tag-key", &["Owner"])]),
            vec!["alpha", "beta"],
        ),
        (
            "tag:<key> selects on the key and the value",
            filters(&[("tag:Owner", &["TeamB"])]),
            vec!["beta"],
        ),
        (
            "two filters are joined with AND",
            filters(&[("tag-key", &["Owner"]), ("key-name", &["beta"])]),
            vec!["beta"],
        ),
        (
            "a filter nothing holds selects nothing",
            filters(&[("key-name", &["delta"])]),
            Vec::new(),
        ),
        (
            "KeyNames and KeyPairIds are a union",
            vec![
                param("KeyName.1", "gamma"),
                param("KeyPairId.1", alpha_id.as_str()),
            ],
            vec!["alpha", "gamma"],
        ),
        (
            "a selector and a filter that disagree select nothing",
            {
                let mut fields = indexed("KeyName", &["alpha"]);
                fields.extend(filters(&[("tag:Owner", &["TeamB"])]));
                fields
            },
            Vec::new(),
        ),
    ];

    for (case, params, expected) in cases {
        let body = describe_ok(case, &router, &params).await;
        let reported = elements(&body, "keyName");
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the key pairs {expected:?}, got {reported:?} in {body}"
        );
    }

    // Reading twice answers the same thing: this operation reports and writes nothing.
    let first = describe_ok("the first read of the whole store", &router, &[]).await;
    let second = describe_ok("the second read of the whole store", &router, &[]).await;
    assert_eq!(
        elements(&first, "keyPairId"),
        elements(&second, "keyPairId"),
        "a read must not change what a later caller sees"
    );
    assert_eq!(
        elements(&first, "keyFingerprint"),
        elements(&second, "keyFingerprint"),
        "a read must not change what a later caller sees"
    );
}
