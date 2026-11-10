// SPDX-License-Identifier: CC0-1.0

//! Downloading the Lightsail default key pair — `DownloadDefaultKeyPair` —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test:
//!
//! * The call reports the halves of the ONE key pair the account holds under the
//!   default key pair name, minting that pair — a real RSA key pair, in the two
//!   encodings the modelled output documents — when the account holds none, and
//!   reporting the same halves and the same creation instant on every later
//!   call. The minted pair is a key pair of the account like any other: it is
//!   read back by `GetKeyPair`, listed by `GetKeyPairs` only when the request
//!   asks for the default pair, and its creation is the one entry the operation
//!   log gains.
//! * The halves belong to the pair the account HOLDS: deleting that pair drops
//!   the material with it, so the next download mints a fresh pair, and a key
//!   pair held under the default name that the service minted no material for is
//!   refused rather than answered with another key's halves.
//!
//! The request carries no member at all (`DownloadDefaultKeyPairRequest` is an
//! empty structure), so the rows below differ along what the rule does
//! generalise over — the Region the call is made in, including the one a request
//! carrying no credential scope is served in — and the cases differ along what
//! the account already holds when the call is made.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, DEFAULT_REGION, DEFAULT_SSH_KEY_NAME,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
// The published fingerprint rule and the DER its digest is taken over are the
// sibling create suite's own, so both suites check ONE rule rather than two
// renderings of it.
use crate::key_pair_create::{is_md5_colon_hex, public_key_der};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// Every member the modelled `DownloadDefaultKeyPairResult` carries, which is
/// the whole of what this call reports:
/// `smithy_query(query="shape",shape="DownloadDefaultKeyPairResult")`.
const DOWNLOAD_MEMBERS: &[&str] = &["createdAt", "privateKeyBase64", "publicKeyBase64"];

/// The algorithm name the modelled output fixes for the public half: "A
/// base64-encoded public key of the `ssh-rsa` type"
/// (`DownloadDefaultKeyPairResult$publicKeyBase64`).
const SSH_RSA: &str = "ssh-rsa";

/// The PEM header and footer the modelled output fixes for the private half: "A
/// base64-encoded RSA private key"
/// (`DownloadDefaultKeyPairResult$privateKeyBase64`), the PKCS#1 labels of
/// RFC 7468.
const RSA_PRIVATE_KEY_BEGIN: &str = "-----BEGIN RSA PRIVATE KEY-----";
const RSA_PRIVATE_KEY_END: &str = "-----END RSA PRIVATE KEY-----";

/// One call of this operation, and the Region it is made in.
struct Row {
    case: &'static str,
    /// The Region of the call's credential scope, or `None` for a request that
    /// carries none, which is served in [`DEFAULT_REGION`].
    region: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "no credential scope at all",
        region: None,
    },
    Row {
        case: "us-west-2",
        region: Some("us-west-2"),
    },
    Row {
        case: "eu-central-1",
        region: Some("eu-central-1"),
    },
    Row {
        case: "ap-northeast-1",
        region: Some("ap-northeast-1"),
    },
];

impl Row {
    /// The Region this row's call is served in — its credential scope's, or the
    /// Region a request carrying none is served in.
    fn region(&self) -> &str {
        self.region.unwrap_or(DEFAULT_REGION)
    }
}

/// Download the account's default key pair from `region`'s endpoint, answering
/// the status and the parsed body.
async fn download(emulator: &Emulator, region: Option<&str>) -> (u16, Value) {
    emulator
        .call_in(region, "DownloadDefaultKeyPair", json!({}))
        .await
}

/// Read one key pair by name, answering the status and the parsed body.
async fn read(emulator: &Emulator, region: Option<&str>, name: &str) -> (u16, Value) {
    emulator
        .call_in(region, "GetKeyPair", json!({"keyPairName": name}))
        .await
}

/// Every key pair name the store holds.
fn held_names(emulator: &Emulator) -> Vec<String> {
    state::with_state(emulator.engine(), |store| {
        store.key_pairs.keys().cloned().collect()
    })
    .expect("the shared engine holds this service's store")
}

/// The key pair names `GetKeyPairs` reports, asking for the default pair only
/// when `include_default` says so — absent when it is `None`, so a case
/// exercises the member's absence too.
async fn listed_names(emulator: &Emulator, include_default: Option<bool>) -> Vec<String> {
    let mut body = json!({});
    if let Some(include_default) = include_default {
        body["includeDefaultKeyPair"] = json!(include_default);
    }
    let (status, listing) = emulator.call("GetKeyPairs", body).await;
    assert_eq!(status, 200, "GetKeyPairs must succeed, got {listing:?}");
    listing["keyPairs"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a keyPairs list, got {listing:?}"))
        .iter()
        .map(|key_pair| key_pair["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every operation-log entry held for the resource `name`.
async fn operations_for(emulator: &Emulator, name: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    assert_eq!(
        status, 200,
        "GetOperationsForResource must succeed, got {body:?}"
    );
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an operations list, got {body:?}"))
        .clone()
}

/// The `operationType` of every entry held for `name`, sorted, so a case states
/// WHICH calls the log recorded without pinning the order the log walks in.
async fn operation_types(emulator: &Emulator, name: &str) -> Vec<String> {
    let mut types: Vec<String> = operations_for(emulator, name)
        .await
        .iter()
        .map(|operation| {
            operation["operationType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    types.sort();
    types
}

/// Assert `body` carries exactly the modelled members of this operation's
/// output, so nothing is reported that the model does not declare and nothing it
/// declares is left out.
fn assert_download_members(case: &str, body: &Value) {
    let members: BTreeSet<&str> = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a response object, got {body:?}"))
        .keys()
        .map(String::as_str)
        .collect();
    let modelled: BTreeSet<&str> = DOWNLOAD_MEMBERS.iter().copied().collect();
    assert_eq!(
        members, modelled,
        "case {case}: expected exactly the modelled result members, got {body:?}"
    );
}

/// Assert the two halves `body` answered are one real RSA key pair of the types
/// the modelled output fixes, and answer the fingerprint the published rule — the
/// MD5 digest of that pair's DER-encoded public key, in colon-hex — makes of it.
///
/// The material is re-read with the crates that implement each published
/// encoding, so this asserts a real key pair rather than a string shaped like
/// one.
fn assert_real_key_pair(case: &str, body: &Value) -> String {
    use ::md5::Digest;

    let public_key = body["publicKeyBase64"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected publicKeyBase64, got {body:?}"));
    let private_key = body["privateKeyBase64"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected privateKeyBase64, got {body:?}"));

    // The public half: the `ssh-rsa` OpenSSH line the model documents.
    assert!(
        public_key.starts_with(&format!("{SSH_RSA} ")),
        "case {case}: expected an {SSH_RSA} public key line, got {public_key:?}"
    );
    let public = ::ssh_key::PublicKey::from_openssh(public_key).unwrap_or_else(|error| {
        panic!("case {case}: expected an OpenSSH public key, got {public_key:?}: {error}")
    });
    assert_eq!(
        public.algorithm().as_str(),
        SSH_RSA,
        "case {case}: expected the {SSH_RSA} algorithm, got {public_key:?}"
    );

    // The private half: the PKCS#1 `RSA PRIVATE KEY` PEM the model documents,
    // holding the same modulus — so the two halves are ONE pair.
    assert!(
        private_key.starts_with(RSA_PRIVATE_KEY_BEGIN)
            && private_key.trim_end().ends_with(RSA_PRIVATE_KEY_END),
        "case {case}: expected an {RSA_PRIVATE_KEY_BEGIN} PEM, got {:?}",
        private_key.lines().next()
    );
    let document = ::der::Document::from_pem(private_key)
        .unwrap_or_else(|error| panic!("case {case}: expected a PEM private half: {error}"))
        .1;
    let parsed = <::pkcs1::RsaPrivateKey as ::der::Decode>::from_der(document.as_bytes())
        .unwrap_or_else(|error| panic!("case {case}: expected a PKCS#1 RSA private key: {error}"));
    let reported = public
        .key_data()
        .rsa()
        .unwrap_or_else(|| panic!("case {case}: expected an RSA public key"));
    assert_eq!(
        parsed.modulus.as_bytes(),
        reported.n.as_positive_bytes().unwrap_or_default(),
        "case {case}: expected the private half to carry the public half's modulus"
    );

    let digest = ::md5::Md5::digest(public_key_der(case, public.key_data()));
    let fingerprint: Vec<String> = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let fingerprint = fingerprint.join(":");
    assert!(
        is_md5_colon_hex(&fingerprint),
        "case {case}: expected sixteen colon-separated hex octets, got {fingerprint:?}"
    );
    fingerprint
}

/// Assert the `keyPair` the account holds under the default name is the record
/// the download reported: the fingerprint of the halves it answered, the Region
/// its call was made in, and the server-assigned members built from one minted
/// resource id.
fn assert_held_record(case: &str, row: &Row, key_pair: &Value, body: &Value, fingerprint: &str) {
    assert_eq!(
        key_pair["name"].as_str(),
        Some(DEFAULT_SSH_KEY_NAME),
        "case {case}: expected the default key pair name, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["fingerprint"].as_str(),
        Some(fingerprint),
        "case {case}: expected the fingerprint of the downloaded halves, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["resourceType"].as_str(),
        Some("KeyPair"),
        "case {case}: expected the KeyPair resource type, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["tags"],
        json!([]),
        "case {case}: nothing asks for a tag on the default pair, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["location"]["regionName"].as_str(),
        Some(row.region()),
        "case {case}: expected the Region the call was made in, got {:?}",
        key_pair["location"]
    );
    assert!(
        key_pair["location"]["availabilityZone"].is_null(),
        "case {case}: a key pair is not zonal, so no availability zone may be reported, got {:?}",
        key_pair["location"]
    );
    assert_eq!(
        key_pair["createdAt"], body["createdAt"],
        "case {case}: expected the record's creation instant to be the one reported, got \
         {key_pair:?}"
    );
    let arn = key_pair["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an arn, got {key_pair:?}"));
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {case}: expected an arn with a resource id, got {arn:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped resource id in the arn, got {id:?}"
    );
    assert_eq!(
        arn,
        state::key_pair_arn(row.region(), id),
        "case {case}: expected the key pair ARN of the call's Region, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["supportCode"].as_str(),
        Some(format!("{ACCOUNT}/{id}").as_str()),
        "case {case}: expected the support code of the same resource id, got {key_pair:?}"
    );
}

/// Assert `operation` is the terminal entry the creation of the default key pair
/// in `row`'s Region wrote.
fn assert_creation_logged(case: &str, row: &Row, operation: &Value) {
    assert_eq!(
        operation["operationType"].as_str(),
        Some("DownloadDefaultKeyPair"),
        "case {case}: expected the entry to name the call that created the pair, got \
         {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("KeyPair"),
        "case {case}: expected the KeyPair resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(DEFAULT_SSH_KEY_NAME),
        "case {case}: expected the entry to name the default key pair, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(row.region()),
        "case {case}: expected the pair's own Region on the log entry, got {:?}",
        operation["location"]
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the pair exists as the call returns, so expected a Succeeded operation, \
         got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

// ---------------------------------------------------------------------------
// The download table
// ---------------------------------------------------------------------------

/// RULE: the call mints the account's default key pair when it holds none —
/// a real RSA key pair, held under the default key pair name in the Region the
/// call was made in — and reports its halves and its creation instant on that
/// call and on every later one, writing the one operation-log entry the creation
/// is audited by.
#[tokio::test]
async fn the_first_download_mints_the_default_key_pair_and_every_later_one_reports_it() {
    let mut public_keys: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        // One emulator per row: each row is a FIRST download, which is what
        // mints the pair.
        let emulator = Emulator::new();
        assert!(
            held_names(&emulator).is_empty(),
            "case {case}: a fresh account holds no key pair"
        );

        let (status, first) = download(&emulator, row.region).await;
        assert_eq!(
            status, 200,
            "case {case}: expected a served download, got {first:?}"
        );
        assert_download_members(case, &first);
        let fingerprint = assert_real_key_pair(case, &first);
        public_keys.insert(first["publicKeyBase64"].as_str().unwrap_or_default().to_string());
        assert!(
            first["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected the creation instant, got {first:?}"
        );

        // The minted pair is a key pair of the account: a read by name answers
        // the record the download just stored.
        let (status, held) = read(&emulator, row.region, DEFAULT_SSH_KEY_NAME).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetKeyPair to serve the minted pair, got {held:?}"
        );
        assert_held_record(case, row, &held["keyPair"], &first, &fingerprint);

        // A later download reports that same pair — the halves and the creation
        // instant — rather than minting another one.
        let (status, again) = download(&emulator, row.region).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the repeated download to be served, got {again:?}"
        );
        assert_eq!(
            again, first,
            "case {case}: expected the held default key pair again, got {again:?}"
        );
        assert_eq!(
            held_names(&emulator),
            vec![DEFAULT_SSH_KEY_NAME.to_string()],
            "case {case}: expected exactly one stored key pair"
        );

        // The creation is the ONE entry the log gained: the download that
        // minted nothing wrote none.
        let operations = operations_for(&emulator, DEFAULT_SSH_KEY_NAME).await;
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected the creation alone in the log, got {operations:?}"
        );
        assert_creation_logged(case, row, &operations[0]);

        // The listing reports the default pair only when the request asks for
        // it.
        let asked = listed_names(&emulator, Some(true)).await;
        assert_eq!(
            asked,
            vec![DEFAULT_SSH_KEY_NAME.to_string()],
            "case {case}: expected the default pair to be listed when asked for, got {asked:?}"
        );
        for include_default in [None, Some(false)] {
            let listed = listed_names(&emulator, include_default).await;
            assert!(
                listed.is_empty(),
                "case {case}: includeDefaultKeyPair {include_default:?} must not report the \
                 default pair, got {listed:?}"
            );
        }
    }
    assert_eq!(
        public_keys.len(),
        ROWS.len(),
        "expected one freshly minted public key per account, got {} distinct keys for {} \
         accounts",
        public_keys.len(),
        ROWS.len()
    );
}

/// RULE: the halves reported belong to the key pair the account HOLDS, so
/// deleting that pair drops its material too and the next download mints a fresh
/// pair — while the delete that is refused leaves the held pair downloadable
/// unchanged.
#[tokio::test]
async fn deleting_the_default_key_pair_makes_the_next_download_mint_another() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let (status, first) = download(&emulator, row.region).await;
        assert_eq!(
            status, 200,
            "case {case}: expected a served download, got {first:?}"
        );
        let first_fingerprint = assert_real_key_pair(case, &first);

        // Deleting the DEFAULT pair without the fingerprint it expects is
        // refused, and the pair is still downloadable exactly as before.
        let (status, refusal) = emulator
            .call_in(
                row.region,
                "DeleteKeyPair",
                json!({"keyPairName": DEFAULT_SSH_KEY_NAME}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the unqualified delete of the default pair to be refused, \
             got {refusal:?}"
        );
        assert_eq!(
            error_code(&refusal),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {refusal:?}"
        );
        let (status, unchanged) = download(&emulator, row.region).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the held pair to still be downloadable, got {unchanged:?}"
        );
        assert_eq!(
            unchanged, first,
            "case {case}: a refused delete must change nothing, got {unchanged:?}"
        );

        let (status, deleted) = emulator
            .call_in(
                row.region,
                "DeleteKeyPair",
                json!({
                    "keyPairName": DEFAULT_SSH_KEY_NAME,
                    "expectedFingerprint": first_fingerprint.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the qualified delete to be served, got {deleted:?}"
        );
        let (status, missing) = read(&emulator, row.region, DEFAULT_SSH_KEY_NAME).await;
        assert_ne!(
            status, 200,
            "case {case}: the deleted pair must not be readable, got {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case {case}: expected the service's DoesNotExist envelope, got {missing:?}"
        );

        // The material went with the record: the next download mints a pair of
        // its own rather than handing out the deleted pair's halves.
        let (status, second) = download(&emulator, row.region).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the next download to mint a pair, got {second:?}"
        );
        let second_fingerprint = assert_real_key_pair(case, &second);
        assert_ne!(
            second_fingerprint, first_fingerprint,
            "case {case}: expected freshly minted material after the delete"
        );
        assert_ne!(
            second["publicKeyBase64"], first["publicKeyBase64"],
            "case {case}: expected a fresh public half after the delete"
        );
        assert_ne!(
            second["privateKeyBase64"], first["privateKeyBase64"],
            "case {case}: expected a fresh private half after the delete"
        );
        assert!(
            second["createdAt"].as_f64().unwrap_or_default()
                >= first["createdAt"].as_f64().unwrap_or_default(),
            "case {case}: expected the new pair's own creation instant, got {second:?}"
        );
        let (status, held) = read(&emulator, row.region, DEFAULT_SSH_KEY_NAME).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the fresh pair to be held, got {held:?}"
        );
        assert_held_record(case, row, &held["keyPair"], &second, &second_fingerprint);

        // The log holds what ran: two creations and the delete between them.
        let types = operation_types(&emulator, DEFAULT_SSH_KEY_NAME).await;
        assert_eq!(
            types,
            vec![
                "DeleteKeyPair".to_string(),
                "DownloadDefaultKeyPair".to_string(),
                "DownloadDefaultKeyPair".to_string(),
            ],
            "case {case}: expected the two creations and the delete in the log, got {types:?}"
        );
    }
}

/// RULE: the default pair is one key pair of the account among the others, so a
/// download neither reports nor disturbs a key pair the caller made itself.
#[tokio::test]
async fn a_download_leaves_the_other_key_pairs_of_the_account_alone() {
    /// The key pairs the caller makes itself before the download, and the Region
    /// each is created in.
    const OWN: &[(&str, Option<&str>)] = &[
        ("kp-alpha", Some("us-west-2")),
        ("kp_beta_2", None),
        ("KP3", Some("eu-west-1")),
    ];

    let emulator = Emulator::new();
    let mut created: Vec<Value> = Vec::new();
    for (name, region) in OWN {
        let (status, body) = emulator
            .call_in(*region, "CreateKeyPair", json!({"keyPairName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: expected the create to be served, got {body:?}"
        );
        created.push(body);
    }

    let (status, downloaded) = download(&emulator, Some("ap-southeast-1")).await;
    assert_eq!(
        status, 200,
        "expected a served download, got {downloaded:?}"
    );
    assert_download_members("with other key pairs held", &downloaded);
    assert_real_key_pair("with other key pairs held", &downloaded);

    for ((name, region), body) in OWN.iter().zip(&created) {
        let (status, after) = read(&emulator, *region, name).await;
        assert_eq!(
            status, 200,
            "case {name}: the caller's own pair must still be held, got {after:?}"
        );
        assert_eq!(
            after["keyPair"], body["keyPair"],
            "case {name}: a download must leave another key pair exactly as it was, got \
             {after:?}"
        );
        assert_ne!(
            downloaded["publicKeyBase64"], body["publicKeyBase64"],
            "case {name}: the default pair's public half must be its own"
        );
        assert_ne!(
            downloaded["privateKeyBase64"], body["privateKeyBase64"],
            "case {name}: the default pair's private half must be its own"
        );
    }

    let mut wanted: Vec<String> = OWN.iter().map(|(name, _)| name.to_string()).collect();
    wanted.push(DEFAULT_SSH_KEY_NAME.to_string());
    wanted.sort();
    assert_eq!(
        held_names(&emulator),
        wanted,
        "expected the account to hold its own key pairs and the default one"
    );
    let mut listed = listed_names(&emulator, Some(true)).await;
    listed.sort();
    assert_eq!(
        listed, wanted,
        "expected every key pair to be listed when the default one is asked for, got {listed:?}"
    );
    let mut without_default = listed_names(&emulator, None).await;
    without_default.sort();
    let mut own: Vec<String> = OWN.iter().map(|(name, _)| name.to_string()).collect();
    own.sort();
    assert_eq!(
        without_default, own,
        "expected only the caller's own key pairs without the switch, got {without_default:?}"
    );
}

/// RULE: the halves answered are the held pair's, so a key pair held under the
/// default name that this service minted no material for is refused with the
/// modelled `OperationFailureException` and changes nothing — and once that pair
/// is gone the download mints the service's own.
#[tokio::test]
async fn the_default_name_held_without_service_material_cannot_be_downloaded() {
    let case = "a key pair the caller created under the default name";
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_in(
            Some("us-west-2"),
            "CreateKeyPair",
            json!({"keyPairName": DEFAULT_SSH_KEY_NAME}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected the create to be served, got {created:?}"
    );

    let (status, refusal) = download(&emulator, Some("us-west-2")).await;
    assert_eq!(
        status, 400,
        "case {case}: expected the download to be refused, got {refusal:?}"
    );
    assert_eq!(
        error_code(&refusal),
        "OperationFailureException",
        "case {case}: expected the modelled OperationFailureException, got {refusal:?}"
    );
    assert!(
        refusal["message"]
            .as_str()
            .unwrap_or_default()
            .contains(DEFAULT_SSH_KEY_NAME),
        "case {case}: expected the refusal to name the key pair, got {refusal:?}"
    );

    let (status, after) = read(&emulator, Some("us-west-2"), DEFAULT_SSH_KEY_NAME).await;
    assert_eq!(
        status, 200,
        "case {case}: the held pair must be untouched, got {after:?}"
    );
    assert_eq!(
        after["keyPair"], created["keyPair"],
        "case {case}: a refused download must change nothing, got {after:?}"
    );
    assert_eq!(
        held_names(&emulator),
        vec![DEFAULT_SSH_KEY_NAME.to_string()],
        "case {case}: a refused download must hold no second record"
    );
    let types = operation_types(&emulator, DEFAULT_SSH_KEY_NAME).await;
    assert_eq!(
        types,
        vec!["CreateKeyPair".to_string()],
        "case {case}: a refused download must write no log entry, got {types:?}"
    );

    // The positive control: the refusal is about the material the service holds
    // for the held record, so once that record is gone the download mints a pair
    // of its own.
    let fingerprint = created["keyPair"]["fingerprint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected the created pair's fingerprint"));
    let (status, deleted) = emulator
        .call_in(
            Some("us-west-2"),
            "DeleteKeyPair",
            json!({
                "keyPairName": DEFAULT_SSH_KEY_NAME,
                "expectedFingerprint": fingerprint,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected the delete to be served, got {deleted:?}"
    );
    let (status, minted) = download(&emulator, Some("us-west-2")).await;
    assert_eq!(
        status, 200,
        "case {case}: expected the download to mint the service's own pair, got {minted:?}"
    );
    let minted_fingerprint = assert_real_key_pair(case, &minted);
    assert_ne!(
        minted_fingerprint, fingerprint,
        "case {case}: expected freshly minted material"
    );
}

/// RULE: the default pair is held under ONE name and the check and the insert run
/// under one exclusive hold on the store, so however many first downloads race,
/// every one of them reports the SAME key pair and the account holds exactly one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_downloads_all_report_one_default_key_pair() {
    // The counts differ along what the rule generalises over: the NUMBER of
    // callers racing for the first download, from the smallest race there is
    // upwards.
    const CALLER_COUNTS: &[usize] = &[2, 3, 6];

    for callers in CALLER_COUNTS {
        let emulator = Arc::new(Emulator::new());
        let mut racing = Vec::new();
        for _ in 0..*callers {
            let emulator = Arc::clone(&emulator);
            racing.push(tokio::spawn(async move {
                emulator.call("DownloadDefaultKeyPair", json!({})).await
            }));
        }

        let mut answers: Vec<Value> = Vec::new();
        for racer in racing {
            let (status, body) = racer.await.expect("the racing call completed");
            assert_eq!(
                status, 200,
                "case {callers} callers: expected every download to be served, got {body:?}"
            );
            answers.push(body);
        }
        let first = answers[0].clone();
        assert_real_key_pair(&format!("{callers} callers"), &first);
        for answer in &answers {
            assert_eq!(
                *answer, first,
                "case {callers} callers: expected one default key pair for every caller, got \
                 {answer:?}"
            );
        }
        assert_eq!(
            held_names(&emulator),
            vec![DEFAULT_SSH_KEY_NAME.to_string()],
            "case {callers} callers: expected exactly one stored key pair"
        );
        let types = operation_types(&emulator, DEFAULT_SSH_KEY_NAME).await;
        assert_eq!(
            types,
            vec!["DownloadDefaultKeyPair".to_string()],
            "case {callers} callers: expected one creation in the log, got {types:?}"
        );
    }
}
