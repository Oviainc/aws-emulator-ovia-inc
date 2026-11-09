// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ImportKeyPair` and the `DescribeKeyPairs` read it round-trips
//! through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=ImportKeyPair`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that AN IMPORTED KEY PAIR IS THE PUBLIC KEY THE REQUEST CARRIED:
//! the pair's `KeyType` and `KeyFingerprint` are derived from that key material and from
//! nothing else — "if you used `ImportKeyPair` to provide Amazon Web Services the public
//! key: for RSA key pairs, the key fingerprint is the MD5 public key fingerprint as
//! specified in section 4 of RFC4716. For ED25519 key pairs, the key fingerprint is the
//! base64-encoded SHA-256 digest, which is the default for OpenSSH"
//! (`KeyPairInfo$KeyFingerprint`) — and the pair the import answers reads back through
//! `DescribeKeyPairs` under the same id, name, type and fingerprint.
//!
//! Every table row is a distinct input along what that generalises over — the key's
//! algorithm, its length, the form it was armoured in, whether it carries a comment — so the
//! one key a recording happens to carry is one row among many rather than the specification.
//! The expected RSA fingerprint is computed in this file from the DER the User Guide's own
//! verification command prints (`openssl rsa -in key -pubout -outform DER | openssl md5
//! -c`), written by hand here, so the value is cross-checked against the standard rather
//! than against the handler's own encoder.
//!
//! `CreateKeyPair`'s own suite is [`super::key_pairs`], which also covers what the describe
//! narrows by; this file covers the IMPORT and what it refuses.

use ::base64::Engine as _;
use ::md5::Digest as _;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for. A key pair carries no Region member, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

// ===========================================================================
// Building the public key material a row imports
// ===========================================================================

/// One length-prefixed field of an SSH wire encoding: a 32-bit big-endian length and the
/// bytes (RFC 4251 §5).
fn ssh_field(bytes: &[u8]) -> Vec<u8> {
    let mut field = u32::try_from(bytes.len())
        .expect("a test field shorter than 4 GiB")
        .to_be_bytes()
        .to_vec();
    field.extend_from_slice(bytes);
    field
}

/// The binary public key blob of `algorithm` with `parts` as its key fields — the bytes an
/// OpenSSH public key line carries base64-encoded (RFC 4253 §6.6).
fn ssh_blob(algorithm: &str, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut blob = ssh_field(algorithm.as_bytes());
    for part in parts {
        blob.extend(ssh_field(part));
    }
    blob
}

/// `value`, a big-endian unsigned integer, as an SSH `mpint`: two's complement, so a value
/// whose top bit is set carries a leading zero byte, and no other leading zero (RFC 4251
/// §5).
fn ssh_mpint(value: &[u8]) -> Vec<u8> {
    let trimmed = &value[value.iter().position(|byte| *byte != 0).unwrap_or(value.len())..];
    let mut mpint = Vec::with_capacity(trimmed.len() + 1);
    if trimmed.first().is_some_and(|byte| byte & 0x80 != 0) {
        mpint.push(0);
    }
    mpint.extend_from_slice(trimmed);
    mpint
}

/// The single-line OpenSSH public key form of `blob`, with `comment` when one is given:
/// `key-type key-body [comment]`, the form `ssh-keygen` writes into
/// `~/.ssh/authorized_keys`.
fn openssh_line(algorithm: &str, blob: &[u8], comment: &str) -> String {
    let body = ::base64::engine::general_purpose::STANDARD.encode(blob);
    if comment.is_empty() {
        format!("{algorithm} {body}")
    } else {
        format!("{algorithm} {body} {comment}")
    }
}

/// `blob` as the armoured public key file RFC 4716 defines, with a `Comment` header — the
/// other public-key form the import requirements name.
fn rfc4716_file(blob: &[u8]) -> String {
    let body = ::base64::engine::general_purpose::STANDARD.encode(blob);
    let mut file = String::from("---- BEGIN SSH2 PUBLIC KEY ----\nComment: \"imported\"\n");
    for line in body.as_bytes().chunks(70) {
        file.push_str(&String::from_utf8_lossy(line));
        file.push('\n');
    }
    file.push_str("---- END SSH2 PUBLIC KEY ----\n");
    file
}

/// The public exponent every RSA row uses: 65537, the exponent `ssh-keygen` writes.
const RSA_PUBLIC_EXPONENT: [u8; 3] = [0x01, 0x00, 0x01];

/// An RSA modulus of `bits` bits whose bytes depend on `seed`, so two rows that ask for
/// different seeds are different keys. The top bit is set, so the value really is `bits`
/// bits wide, and the low bit is set, as the product of two odd primes is.
fn rsa_modulus(bits: usize, seed: u8) -> Vec<u8> {
    let mut modulus: Vec<u8> = (0..bits / 8)
        .map(|index| seed.wrapping_mul(31).wrapping_add(index as u8) | 0x01)
        .collect();
    if let Some(first) = modulus.first_mut() {
        *first |= 0x80;
    }
    if let Some(last) = modulus.last_mut() {
        *last |= 0x01;
    }
    modulus
}

/// The blob of an `ssh-rsa` public key of `bits` bits, seeded by `seed`.
fn rsa_blob(bits: usize, seed: u8) -> Vec<u8> {
    ssh_blob(
        "ssh-rsa",
        &[
            ssh_mpint(&RSA_PUBLIC_EXPONENT),
            ssh_mpint(&rsa_modulus(bits, seed)),
        ],
    )
}

/// The blob of an `ssh-ed25519` public key seeded by `seed`: the 32 bytes an Ed25519 public
/// key is.
fn ed25519_blob(seed: u8) -> Vec<u8> {
    let key: Vec<u8> = (0..32u8).map(|index| seed.wrapping_add(index)).collect();
    ssh_blob("ssh-ed25519", &[key])
}

// ===========================================================================
// The fingerprints the model documents, computed independently of the handler
// ===========================================================================

/// One DER type-length-value: the tag, the length in DER's definite form, and the body.
fn der_tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut encoded = vec![tag];
    if body.len() < 0x80 {
        encoded.push(body.len() as u8);
    } else {
        let length = body.len().to_be_bytes();
        let significant = &length[length.iter().position(|byte| *byte != 0).unwrap_or(0)..];
        encoded.push(0x80 | significant.len() as u8);
        encoded.extend_from_slice(significant);
    }
    encoded.extend_from_slice(body);
    encoded
}

/// `value`, a big-endian unsigned integer, as a DER `INTEGER` — signed, so a value whose
/// top bit is set carries a leading zero byte.
fn der_integer(value: &[u8]) -> Vec<u8> {
    let trimmed = &value[value.iter().position(|byte| *byte != 0).unwrap_or(value.len())..];
    let mut body = Vec::with_capacity(trimmed.len() + 1);
    if trimmed.first().is_some_and(|byte| byte & 0x80 != 0) {
        body.push(0);
    }
    body.extend_from_slice(trimmed);
    der_tlv(0x02, &body)
}

/// The DER body of the `rsaEncryption` object identifier, 1.2.840.113549.1.1.1 (RFC 8017
/// appendix C).
const RSA_ENCRYPTION_OID: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/// The DER `SubjectPublicKeyInfo` (RFC 5280 §4.1) of the RSA key `modulus`/`exponent` — the
/// bytes `openssl rsa -pubout -outform DER` writes, which the Amazon EC2 User Guide pipes
/// into `openssl md5 -c` to verify an imported RSA key pair's fingerprint.
fn rsa_subject_public_key_info(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    let mut pkcs1 = der_integer(modulus);
    pkcs1.extend(der_integer(exponent));
    let pkcs1 = der_tlv(0x30, &pkcs1);

    let mut algorithm = der_tlv(0x06, &RSA_ENCRYPTION_OID);
    algorithm.extend(der_tlv(0x05, &[]));
    let mut info = der_tlv(0x30, &algorithm);

    let mut bit_string = vec![0u8];
    bit_string.extend_from_slice(&pkcs1);
    info.extend(der_tlv(0x03, &bit_string));
    der_tlv(0x30, &info)
}

/// The `KeyFingerprint` an imported RSA key pair of this modulus reports: the MD5 digest of
/// its DER `SubjectPublicKeyInfo`, as colon-separated lowercase hex byte pairs.
fn expected_rsa_fingerprint(bits: usize, seed: u8) -> String {
    let digest = ::md5::Md5::digest(rsa_subject_public_key_info(
        &rsa_modulus(bits, seed),
        &RSA_PUBLIC_EXPONENT,
    ));
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<String>>()
        .join(":")
}

/// The `KeyFingerprint` an imported ED25519 key pair reports: "the base64-encoded SHA-256
/// digest, which is the default for OpenSSH" — the `SHA256:<base64>` form `ssh-keygen -l`
/// prints, which is what that default is, over the key's own blob. The same rendering a
/// generated Ed25519 key pair's fingerprint takes, which [`super::key_pairs`] asserts.
fn expected_ed25519_fingerprint(blob: &[u8]) -> String {
    format!(
        "SHA256:{}",
        ::base64::engine::general_purpose::STANDARD_NO_PAD.encode(::sha2::Sha256::digest(blob))
    )
}

// ===========================================================================
// Calling the operation
// ===========================================================================

/// The form fields of one import: the name, the material (a blob, so base64 on the wire)
/// and its tags.
fn import_params(key_name: &str, material: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![
        param("KeyName", key_name),
        param(
            "PublicKeyMaterial",
            &::base64::engine::general_purpose::STANDARD.encode(material),
        ),
    ];
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "key-pair"));
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", position + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", position + 1),
                value,
            ));
        }
    }
    params
}

/// One import that has to succeed, and the body it answered.
async fn import(
    case: &str,
    router: &MultiServiceRouter,
    key_name: &str,
    material: &str,
    tags: &[(&str, &str)],
) -> String {
    ok(
        case,
        router,
        "ImportKeyPair",
        &import_params(key_name, material, tags),
    )
    .await
}

/// The one key pair a `DescribeKeyPairs` body reports, as its `<item>` element — or a panic
/// naming the case when it reported none or several.
fn described_pair<'a, 'i>(
    case: &str,
    parsed: &'a roxmltree::Document<'i>,
) -> roxmltree::Node<'a, 'i> {
    let set = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "keySet")
        .unwrap_or_else(|| panic!("case {case:?}: no <keySet> in {}", parsed.input_text()));
    let items: Vec<roxmltree::Node<'a, 'i>> =
        set.children().filter(roxmltree::Node::is_element).collect();
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: expected exactly one key pair, got {}: {}",
        items.len(),
        parsed.input_text()
    );
    items[0]
}

// ===========================================================================
// The tables
// ===========================================================================

/// One row of the import table: the case, the material the request carries, the `KeyType`
/// and `KeyFingerprint` the pair must report, and the public key material a describe that
/// asks for it must report.
struct ImportCase {
    case: &'static str,
    key_name: &'static str,
    material: String,
    key_type: &'static str,
    fingerprint: String,
    public_key: String,
}

/// The import table: the two algorithms an import takes, each published RSA length, both
/// public-key forms the requirements name, and a key line with no comment.
fn import_table() -> Vec<ImportCase> {
    let mut rows = Vec::new();
    // "Supported lengths: 1024, 2048, and 4096" (Amazon EC2 User Guide, "Create a key pair
    // and import the public key to Amazon EC2"), which `ec2::state::IMPORTABLE_RSA_KEY_BITS`
    // is this emulator's reading of.
    for (index, bits) in ec2::state::IMPORTABLE_RSA_KEY_BITS.into_iter().enumerate() {
        let seed = 7u8.wrapping_add(index as u8);
        let blob = rsa_blob(bits, seed);
        let line = openssh_line("ssh-rsa", &blob, "fixture@example.com");
        rows.push(ImportCase {
            case: match bits {
                1024 => "rsa-1024 openssh line",
                2048 => "rsa-2048 openssh line",
                _ => "rsa-4096 openssh line",
            },
            key_name: match bits {
                1024 => "fixture-rsa-1024",
                2048 => "fixture-rsa-2048",
                _ => "fixture-rsa-4096",
            },
            material: line.clone(),
            key_type: ec2::state::KEY_TYPE_RSA,
            fingerprint: expected_rsa_fingerprint(bits, seed),
            public_key: line,
        });
    }
    let ed25519 = ed25519_blob(3);
    let ed25519_line = openssh_line("ssh-ed25519", &ed25519, "fixture@example.com");
    rows.push(ImportCase {
        case: "ed25519 openssh line",
        key_name: "fixture-ed25519",
        material: ed25519_line.clone(),
        key_type: ec2::state::KEY_TYPE_ED25519,
        fingerprint: expected_ed25519_fingerprint(&ed25519),
        public_key: ed25519_line,
    });
    // The SAME key bytes as a `rsa-2048` row, armoured as RFC 4716 instead: a different
    // input carrying one key, so it must fingerprint by the same rule. The armour carries no
    // OpenSSH comment, so the stored line carries none either.
    let armoured_blob = rsa_blob(2048, 8);
    rows.push(ImportCase {
        case: "rsa-2048 rfc 4716 armour",
        key_name: "fixture-rsa-2048-armoured",
        material: rfc4716_file(&armoured_blob),
        key_type: ec2::state::KEY_TYPE_RSA,
        fingerprint: expected_rsa_fingerprint(2048, 8),
        public_key: openssh_line("ssh-rsa", &armoured_blob, ""),
    });
    let bare_blob = rsa_blob(2048, 40);
    let bare_line = openssh_line("ssh-rsa", &bare_blob, "");
    rows.push(ImportCase {
        case: "rsa-2048 with no comment",
        key_name: "fixture-rsa-2048-bare",
        material: bare_line.clone(),
        key_type: ec2::state::KEY_TYPE_RSA,
        fingerprint: expected_rsa_fingerprint(2048, 40),
        public_key: bare_line,
    });
    rows
}

#[tokio::test]
async fn an_imported_key_pair_is_the_public_key_the_request_carried() {
    let router = emulator();
    for row in import_table() {
        let case = row.case;
        let body = import(
            case,
            &router,
            row.key_name,
            &row.material,
            &[("Owner", "TeamA")],
        )
        .await;

        let key_pair_id = member(case, &body, "keyPairId");
        assert!(
            is_long_id(ec2::state::KEY_PAIR_ID_PREFIX, &key_pair_id),
            "case {case:?}: expected a key-<17 hex> id, got {key_pair_id:?}"
        );
        assert_eq!(
            member(case, &body, "keyName"),
            row.key_name,
            "case {case:?}: the import must echo the name it was given"
        );
        assert_eq!(
            member(case, &body, "keyFingerprint"),
            row.fingerprint,
            "case {case:?}: the fingerprint must be the one the key material decides"
        );
        assert_eq!(
            elements(&body, "key"),
            vec!["Owner".to_string()],
            "case {case:?}: the import must echo the tags it applied: {body}"
        );

        // The pair reads back through `DescribeKeyPairs`, which reports the public key
        // material only when asked: "if true, the public key material is included in the
        // response. Default: false" (`DescribeKeyPairsRequest$IncludePublicKey`).
        let described = ok(
            case,
            &router,
            "DescribeKeyPairs",
            &[
                param("KeyName.1", row.key_name),
                param("IncludePublicKey", "true"),
            ],
        )
        .await;
        let parsed = document(case, &described);
        let pair = described_pair(case, &parsed);
        for (element_name, expected) in [
            ("keyPairId", key_pair_id.clone()),
            ("keyName", row.key_name.to_string()),
            ("keyFingerprint", row.fingerprint.clone()),
            ("keyType", row.key_type.to_string()),
            ("publicKey", row.public_key.clone()),
        ] {
            let actual = child_text(pair, element_name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: the described pair's <{element_name}> must be the one the \
                 import answered, got {actual:?}"
            );
        }
        let create_time = child_text(pair, "createTime")
            .unwrap_or_else(|| panic!("case {case:?}: no <createTime>: {described}"));
        assert!(
            super::ec2_query::as_timestamp(&create_time).is_some(),
            "case {case:?}: <createTime> must be an ISO 8601 instant, got {create_time:?}"
        );
        let tags: Vec<(String, String)> = child(pair, "tagSet")
            .map(|set| {
                set.children()
                    .filter(roxmltree::Node::is_element)
                    .map(|item| {
                        (
                            child_text(item, "key").unwrap_or_default(),
                            child_text(item, "value").unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            tags,
            vec![("Owner".to_string(), "TeamA".to_string())],
            "case {case:?}: the described pair must carry the tag it was imported with: \
             {described}"
        );

        let without_key = ok(
            case,
            &router,
            "DescribeKeyPairs",
            &[param("KeyName.1", row.key_name)],
        )
        .await;
        let parsed = document(case, &without_key);
        let pair = described_pair(case, &parsed);
        assert_eq!(
            child_text(pair, "publicKey"),
            None,
            "case {case:?}: a describe that did not ask for the public key must not report \
             it: {without_key}"
        );
    }
}

#[tokio::test]
async fn one_key_is_one_fingerprint_however_many_names_it_is_imported_under() {
    let router = emulator();
    let blob = rsa_blob(2048, 11);
    let material = openssh_line("ssh-rsa", &blob, "shared@example.com");
    let expected = expected_rsa_fingerprint(2048, 11);

    // The same key under two names: two pairs, two ids, ONE fingerprint — it is derived
    // from the material and from nothing else.
    let mut ids = Vec::new();
    for name in ["twin-one", "twin-two"] {
        let body = import(name, &router, name, &material, &[]).await;
        assert_eq!(
            member(name, &body, "keyFingerprint"),
            expected,
            "case {name:?}: one key must fingerprint to one value whatever it is named"
        );
        ids.push(member(name, &body, "keyPairId"));
    }
    assert_ne!(
        ids[0], ids[1],
        "two imports of one key must be two pairs with two ids, got {ids:?}"
    );

    // A different key must fingerprint differently.
    let other = import(
        "other key",
        &router,
        "other-key",
        &openssh_line("ssh-rsa", &rsa_blob(2048, 12), ""),
        &[],
    )
    .await;
    assert_ne!(
        member("other key", &other, "keyFingerprint"),
        expected,
        "two different keys must not share a fingerprint"
    );

    // One key armoured two ways is one key: the RFC 4716 file of a key fingerprints exactly
    // as that key's OpenSSH line does.
    let armoured = import(
        "armoured",
        &router,
        "armoured-twin",
        &rfc4716_file(&blob),
        &[],
    )
    .await;
    assert_eq!(
        member("armoured", &armoured, "keyFingerprint"),
        expected,
        "the armoured form of one key must fingerprint as its OpenSSH line does"
    );
}

/// One row of the refusal table: the case, the form fields it sends, and the error code and
/// HTTP status EC2 answers it with.
type RefusalCase = (&'static str, Vec<Param>, &'static str, u16);

/// The refusals an import answers, each a distinct way the request cannot be served.
fn refusal_table() -> Vec<RefusalCase> {
    let readable_key = openssh_line("ssh-rsa", &rsa_blob(2048, 21), "");
    let ecdsa = openssh_line(
        "ecdsa-sha2-nistp256",
        &ssh_blob(
            "ecdsa-sha2-nistp256",
            &[b"nistp256".to_vec(), {
                let mut point = vec![0x04u8];
                point.extend((0..64u8).map(|index| index | 0x11));
                point
            }],
        ),
        "",
    );
    vec![
        (
            "material that is not a key at all",
            import_params("not-a-key", "this is not a public key", &[]),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "material above the size the import reads at all",
            import_params(
                "oversize",
                &format!(
                    "{readable_key}{padding}",
                    padding = "p".repeat(ec2::state::MAX_PUBLIC_KEY_MATERIAL_BYTES)
                ),
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "rfc 4716 armour with no end line",
            import_params(
                "armour-unterminated",
                rfc4716_file(&rsa_blob(2048, 25))
                    .trim_end()
                    .trim_end_matches("---- END SSH2 PUBLIC KEY ----"),
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "rfc 4716 armour whose body is not base64",
            import_params(
                "armour-unreadable",
                "---- BEGIN SSH2 PUBLIC KEY ----\nComment: \"imported\"\n{not base64}\n\
                 ---- END SSH2 PUBLIC KEY ----\n",
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "rfc 4716 armour carrying headers and no body",
            import_params(
                "armour-headerless",
                "---- BEGIN SSH2 PUBLIC KEY ----\nComment: \"imported\"\n\
                 ---- END SSH2 PUBLIC KEY ----\n",
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "an openssh line whose body is truncated",
            import_params("truncated", &readable_key[..readable_key.len() - 20], &[]),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "an rsa key shorter than every supported length",
            import_params(
                "rsa-512",
                &openssh_line("ssh-rsa", &rsa_blob(512, 22), ""),
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "an rsa key between two supported lengths",
            import_params(
                "rsa-3072",
                &openssh_line("ssh-rsa", &rsa_blob(3072, 23), ""),
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "an rsa key longer than every supported length",
            import_params(
                "rsa-8192",
                &openssh_line("ssh-rsa", &rsa_blob(8192, 24), ""),
                &[],
            ),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "a readable key of an algorithm an import does not take",
            import_params("ecdsa", &ecdsa, &[]),
            ec2::state::INVALID_KEY_PAIR_FORMAT,
            400,
        ),
        (
            "no public key material at all",
            vec![param("KeyName", "no-material")],
            "MalformedInput",
            400,
        ),
        (
            "an empty public key material",
            vec![
                param("KeyName", "empty-material"),
                param("PublicKeyMaterial", ""),
            ],
            "MissingParameter",
            400,
        ),
        (
            "an empty key name",
            vec![
                param("KeyName", ""),
                param(
                    "PublicKeyMaterial",
                    &::base64::engine::general_purpose::STANDARD.encode(&readable_key),
                ),
            ],
            "MissingParameter",
            400,
        ),
        (
            "a key name one character past the published maximum",
            import_params(
                &"n".repeat(ec2::state::MAX_KEY_PAIR_NAME_CHARS + 1),
                &readable_key,
                &[],
            ),
            "InvalidParameterValue",
            400,
        ),
        (
            "a key name of legal length carrying a character outside ASCII",
            import_params("fixture-\u{e9}", &readable_key, &[]),
            "InvalidParameterValue",
            400,
        ),
        (
            "a tag specification naming another resource type",
            {
                let mut params = import_params("mistyped-tags", &readable_key, &[]);
                params.push(param("TagSpecification.1.ResourceType", "instance"));
                params.push(param("TagSpecification.1.Tag.1.Key", "Owner"));
                params.push(param("TagSpecification.1.Tag.1.Value", "TeamA"));
                params
            },
            "InvalidParameterValue",
            400,
        ),
        (
            "a dry run of a request that would have succeeded",
            {
                let mut params = import_params("dry-run", &readable_key, &[]);
                params.push(param("DryRun", "true"));
                params
            },
            "DryRunOperation",
            412,
        ),
    ]
}

#[tokio::test]
async fn an_import_refuses_what_it_cannot_store() {
    let router = emulator();
    for (case, params, expected_code, expected_status) in refusal_table() {
        let (status, body) =
            super::ec2_query::call(&router, REGION, "ImportKeyPair", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
    }

    // Nothing above was stored: the account holds no key pair at all.
    let described = ok("nothing stored", &router, "DescribeKeyPairs", &[]).await;
    assert!(
        elements(&described, "keyPairId").is_empty(),
        "a refused import must store nothing, got {described}"
    );

    // A name AT the published maximum is accepted, which is what makes the row above a
    // bound rather than a blanket refusal.
    let longest = "n".repeat(ec2::state::MAX_KEY_PAIR_NAME_CHARS);
    let readable_key = openssh_line("ssh-rsa", &rsa_blob(2048, 31), "");
    let body = import("the longest name", &router, &longest, &readable_key, &[]).await;
    assert_eq!(
        member("the longest name", &body, "keyName"),
        longest,
        "a name of exactly the published maximum must be accepted"
    );

    // "A unique name for the key pair": the second import of that name is refused, with the
    // sentence EC2 words this error class with.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "ImportKeyPair",
        &import_params(
            &longest,
            &openssh_line("ssh-rsa", &rsa_blob(2048, 32), ""),
            &[],
        ),
    )
    .await;
    assert_eq!(status, 400, "a duplicate name must be refused, got {body}");
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_KEY_PAIR_DUPLICATE,
        "a duplicate name must be refused as a duplicate, got {body}"
    );
    assert_eq!(
        error_message(&body),
        ec2::state::DUPLICATE_KEY_PAIR_MESSAGE,
        "the duplicate refusal must carry the sentence EC2 words it with, got {}",
        error_message(&body)
    );

    // A name a CREATE already holds is a duplicate for an import too: one name, one key
    // pair, whichever call stored it.
    let created = ok(
        "created first",
        &router,
        "CreateKeyPair",
        &[param("KeyName", "created-elsewhere")],
    )
    .await;
    assert!(
        !member("created first", &created, "keyPairId").is_empty(),
        "the create must mint a key pair: {created}"
    );
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "ImportKeyPair",
        &import_params("created-elsewhere", &readable_key, &[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a name a create already holds must be refused, got {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_KEY_PAIR_DUPLICATE,
        "a name a create already holds must be refused as a duplicate, got {body}"
    );
}
