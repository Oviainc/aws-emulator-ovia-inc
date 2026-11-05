// SPDX-License-Identifier: CC0-1.0

//! Which STAGE refuses a request of this service, and what that stage states — driven over
//! the wire through the same router the emulator boots.
//!
//! Two rules, neither about one operation or one member:
//!
//! * A member this service requires because ANOTHER member's value makes it necessary — the
//!   S3 bucket an enabled `CrlConfiguration` publishes its CRLs to — is refused by the
//!   request-validation stage, which answers the generic `ValidationException` naming that
//!   member's WIRE name and the constraint it failed, ahead of the semantic argument checks.
//!   A combination of members that gets past that stage and still cannot hold together is
//!   the service's own `InvalidArgsException`, so the two classes stay distinguishable: a
//!   caller branching on the code learns whether the request's SHAPE or its MEANING was
//!   wrong, and a schema fault names the member to fill in.
//! * The admissible set a refusal states for an enumerated type is the set of values this
//!   front door OFFERS, and a type it offers only SOME of the declared values of is judged
//!   one layer deeper than the request-validation stage — the model declares the whole set,
//!   so the stage passes a value outside the offered subset, and the service's own check
//!   states its single clause in its own words: the member path and the values it offers, in
//!   the order it filtered them, with no value it does not offer advertised and no count
//!   envelope the stage never reached. A type nothing is withheld from is stated by the stage
//!   exactly as before (`crate::enum_value_set_order` owns that order).
//!
//! The rows differ along what the rules generalise over: which operation carried the
//! configuration (the create and the update), what else the configuration carried beside the
//! missing member, which value was outside the enumerated type, and — for the stated set —
//! every value the set lists, each of which a create then has to accept and store.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::KEY_ALGORITHM_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::service::UNOFFERED_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::CRL_BUCKET_MEMBER;

use crate::private_ca_lifecycle::{active_ca, blob, root_ca_request, string, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// The error code the request-validation stage answers with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this service's own semantic argument fault answers with.
const INVALID_ARGS_EXCEPTION: &str = "InvalidArgsException";

/// What the request-validation stage states for a member it found no value for: the count of
/// violations and, per violation, the member's wire name and the requirement it failed.
///
/// The template is fixed text; its variable parts are the count and the member name, which is
/// taken from the request's own payload (`CRL_BUCKET_MEMBER` is the wire name this service
/// holds for the CRL bucket, so the listing here cannot drift from the member the handler
/// names).
fn missing_member_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value null or empty at '{member}' failed to satisfy \
         constraint: Member must not be null or empty."
    )
}

/// The admissible set this front door states for `KeyAlgorithm`.
///
/// The values and their order come from the type's declaration
/// (`smithy_query(query="shape", shape="KeyAlgorithm")` — RSA_2048, RSA_3072, RSA_4096,
/// EC_prime256v1, EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, ML_DSA_87, SM2), minus the
/// values this front door does not offer (`service::UNOFFERED_VALUES`). The membership is
/// checked against the model-derived list in `ops::types` below, so this listing cannot drift
/// from the type the model declares.
const OFFERED_KEY_ALGORITHM_SET: &str = "RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, \
                                         EC_secp384r1, EC_secp521r1, ML_DSA_44, ML_DSA_65, \
                                         ML_DSA_87";

/// `ValidityPeriodType`, a type nothing is withheld from, stated in the order the set's own
/// hash table is walked in (`crate::enum_value_set_order`).
const VALIDITY_PERIOD_TYPE_SET: &str = "END_DATE, MONTHS, YEARS, DAYS, ABSOLUTE";

/// The clause the SERVICE's own check states for a value outside an enumerated type it
/// filters: the member path and the set it offers, with no count envelope, because the
/// request-validation stage passed the value and never counted it.
fn offered_value_set_message(path: &str, value_set: &str) -> String {
    format!(
        "Value at '{path}'{PREAMBLE}Member must satisfy enum value set: [{value_set}]"
    )
}

/// The clause the request-validation STAGE states for a value outside an enumerated type it
/// judges, under the count of how many values failed.
fn enum_value_set_message(path: &str, value_set: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}'{PREAMBLE}Member must satisfy enum \
         value set: [{value_set}]"
    )
}

/// Where every clause states the requirement the value failed.
const PREAMBLE: &str = " failed to satisfy constraint: ";

/// The wire error code an answer carries.
fn error_code(body: &Value, case: &str) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no error code in {body:?}"))
        .to_string()
}

/// The ARNs of the CAs this emulator holds.
async fn listed_cas(emulator: &Emulator, case: &str) -> Vec<String> {
    let answer = emulator
        .ok(case, "ListCertificateAuthorities", &json!({}))
        .await;
    answer
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|authority| string(authority, "Arn", case))
        .collect()
}

/// The `CrlConfiguration` a CA reports.
async fn reported_crl(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let described = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    described
        .get("CertificateAuthority")
        .and_then(|authority| authority.get("RevocationConfiguration"))
        .and_then(|revocation| revocation.get("CrlConfiguration"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Revocation configurations whose enabled CRL names no bucket: the member is missing however
/// much else the configuration carries, and whichever call carries it.
fn configurations_missing_the_bucket() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an enabled CRL with only the expiry",
            json!({"CrlConfiguration": {"Enabled": true, "ExpirationInDays": 7}}),
        ),
        (
            "an enabled CRL at the expiry range floor",
            json!({"CrlConfiguration": {"Enabled": true, "ExpirationInDays": 1}}),
        ),
        (
            "an enabled CRL at the expiry range ceiling, with a custom name",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 5000,
                    "CustomCname": "crl.example.com",
                }
            }),
        ),
        (
            "an enabled CRL carrying every member but the bucket",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "ExpirationInDays": 30,
                    "S3ObjectAcl": "PUBLIC_READ",
                    "CrlType": "COMPLETE",
                    "CustomPath": "crl/partitions",
                }
            }),
        ),
        (
            "an enabled CRL with no bucket beside an enabled OCSP responder",
            json!({
                "CrlConfiguration": {"Enabled": true, "ExpirationInDays": 7},
                "OcspConfiguration": {"Enabled": true},
            }),
        ),
    ]
}

/// A create whose enabled CRL configuration names no bucket is refused by the
/// request-validation stage, which names the member, and nothing is created.
#[tokio::test]
async fn a_create_missing_the_crl_bucket_is_refused_naming_the_member() {
    let emulator = Emulator::new();
    let expected = missing_member_message(CRL_BUCKET_MEMBER);

    for (index, (case, revocation)) in configurations_missing_the_bucket()
        .into_iter()
        .enumerate()
    {
        let mut body = root_ca_request(&format!("no-bucket-{index}"));
        body["RevocationConfiguration"] = revocation.clone();
        let (status, answer) = emulator.call("CreateCertificateAuthority", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} {answer:?}"
        );
        let code = error_code(&answer, case);
        assert!(
            code.contains(VALIDATION_EXCEPTION) && !code.contains(INVALID_ARGS_EXCEPTION),
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {code:?} ({answer:?})"
        );
        let message = error_message(&answer, case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            answer.get("CertificateAuthorityArn").is_none(),
            "case {case:?}: a refused create answers no ARN, got {answer:?}"
        );
    }

    // A bucket that is PRESENT but blank fails the member's own modelled pattern: the same
    // stage refuses it, naming the same member under a clause of its own, and still no CA.
    let case = "an enabled CRL whose bucket is blank";
    let mut body = root_ca_request("blank-bucket");
    body["RevocationConfiguration"] = json!({
        "CrlConfiguration": {"Enabled": true, "ExpirationInDays": 7, "S3BucketName": "   "},
    });
    let answer = emulator
        .refused(case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
        .await;
    let message = error_message(&answer, case);
    assert!(
        message.starts_with("1 validation error detected: ") && message.contains(CRL_BUCKET_MEMBER),
        "case {case:?}: expected one clause naming {CRL_BUCKET_MEMBER}, got {message:?}"
    );

    // Every one of those creates registered nothing.
    let held = listed_cas(&emulator, "the CAs the emulator holds").await;
    assert!(
        held.is_empty(),
        "a refused create must register no CA, got {held:?}"
    );
}

/// The same rule on the UPDATE, which carries the same member under the same rules: the
/// refusal is the request-validation stage's, and the CA keeps the configuration it had.
#[tokio::test]
async fn an_update_missing_the_crl_bucket_is_refused_naming_the_member() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "update-no-bucket").await;
    let before = reported_crl(&emulator, &arn, "the CA before any update").await;
    let expected = missing_member_message(CRL_BUCKET_MEMBER);

    for (case, revocation) in configurations_missing_the_bucket() {
        let (status, answer) = emulator
            .call(
                "UpdateCertificateAuthority",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "RevocationConfiguration": revocation,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} {answer:?}"
        );
        let code = error_code(&answer, case);
        assert!(
            code.contains(VALIDATION_EXCEPTION) && !code.contains(INVALID_ARGS_EXCEPTION),
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {code:?} ({answer:?})"
        );
        let message = error_message(&answer, case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        let after = reported_crl(&emulator, &arn, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, expected {before:?}, got {after:?}"
        );
    }
}

/// The stage boundary: a configuration whose members are all there but cannot hold together
/// got PAST the request-validation stage, so it is the service's own argument fault — the
/// same configurations are not reworded into the schema class. A custom path that designates
/// no place UNDER the CRL bucket (a `.` or `..` segment) is one of them: the member is there
/// and well formed, and what it names is not a folder the CA can publish into.
#[tokio::test]
async fn a_configuration_that_cannot_hold_together_stays_an_argument_fault() {
    let emulator = Emulator::new();

    let rows: [(&str, Value); 6] = [
        (
            "an enabled CRL with a bucket and no expiry",
            json!({"CrlConfiguration": {"Enabled": true, "S3BucketName": "crl-bucket"}}),
        ),
        (
            "an enabled CRL whose custom path climbs out of its folder",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "S3BucketName": "crl-bucket",
                    "ExpirationInDays": 7,
                    "CustomPath": "crls/../../elsewhere",
                }
            }),
        ),
        (
            "an enabled CRL whose custom path names the folder itself",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "S3BucketName": "crl-bucket",
                    "ExpirationInDays": 7,
                    "CustomPath": "./crls",
                }
            }),
        ),
        (
            "a disabled CRL carrying a bucket",
            json!({"CrlConfiguration": {"Enabled": false, "S3BucketName": "crl-bucket"}}),
        ),
        (
            "a disabled CRL carrying an expiry",
            json!({"CrlConfiguration": {"Enabled": false, "ExpirationInDays": 7}}),
        ),
        (
            "a disabled OCSP responder carrying a custom name",
            json!({"OcspConfiguration": {"Enabled": false, "OcspCustomCname": "ocsp.example.com"}}),
        ),
    ];

    for (index, (case, revocation)) in rows.iter().enumerate() {
        let mut body = root_ca_request(&format!("argument-fault-{index}"));
        body["RevocationConfiguration"] = revocation.clone();
        let answer = emulator
            .refused(case, "CreateCertificateAuthority", &body, INVALID_ARGS_EXCEPTION)
            .await;
        let message = error_message(&answer, case);
        assert!(
            !message.contains("validation error detected"),
            "case {case:?}: an argument fault is not stated as a schema fault, got {message:?}"
        );
    }
}

/// A key algorithm outside the type is refused with the set this front door OFFERS: the
/// values the model declares, minus the ones it does not serve, and in the order it filtered
/// them — stated as the service's own clause, since the request-validation stage judges that
/// member against the whole declared set and passes it.
#[tokio::test]
async fn the_stated_key_algorithm_set_lists_the_values_the_front_door_offers() {
    let emulator = Emulator::new();
    let expected = offered_value_set_message(
        "certificateAuthorityConfiguration.keyAlgorithm",
        OFFERED_KEY_ALGORITHM_SET,
    );

    // Values outside the type, differing along how they are outside it: another cipher's key
    // size, a key the service never offered, a signature algorithm in a key member, and a
    // listed value in the wrong case.
    for value in ["RSA_1024", "ED25519", "SHA256WITHRSA", "rsa_2048", "AES_256"] {
        let case = format!("a key algorithm of {value}");
        let mut body = root_ca_request("stated-key-algorithms");
        body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!(value);
        let answer = emulator
            .refused(&case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
            .await;
        let message = error_message(&answer, &case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The set STATED holds exactly the values the model declares that this front door offers:
    // a listing that dropped or invented one would pass the string above only by being wrong
    // in both places at once.
    let mut listed: Vec<&str> = OFFERED_KEY_ALGORITHM_SET.split(", ").collect();
    listed.sort_unstable();
    let mut offered: Vec<&str> = KEY_ALGORITHM_VALUES
        .iter()
        .copied()
        .filter(|value| !UNOFFERED_VALUES.contains(value))
        .collect();
    offered.sort_unstable();
    assert_eq!(
        listed, offered,
        "the stated set must be the declared values this front door offers: expected \
         {offered:?}, got {listed:?}"
    );
    assert!(
        !UNOFFERED_VALUES.is_empty(),
        "a value this front door does not offer is what the rule is about, got none"
    );
}

/// Every value the stated set lists is a value a create ACCEPTS and stores: the set is the
/// front door's own, so a caller repairing its request off it is repairing it correctly.
#[tokio::test]
async fn every_key_algorithm_the_stated_set_lists_is_accepted_and_stored() {
    let emulator = Emulator::new();

    for key_algorithm in OFFERED_KEY_ALGORITHM_SET.split(", ") {
        let case = format!("a CA keyed with {key_algorithm}");
        // A CA signs its own certification request with a signature of its key's family
        // ("The specified signing algorithm family (RSA or ECDSA) must match the algorithm
        // family of the CA's secret key", `IssueCertificate` docs).
        let signing_algorithm = if key_algorithm.starts_with("RSA_") {
            "SHA256WITHRSA"
        } else if key_algorithm.starts_with("EC_") {
            "SHA256WITHECDSA"
        } else {
            key_algorithm
        };
        let mut body = root_ca_request(&format!("offered-{key_algorithm}"));
        body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!(key_algorithm);
        body["CertificateAuthorityConfiguration"]["SigningAlgorithm"] = json!(signing_algorithm);
        let created = emulator
            .ok(&case, "CreateCertificateAuthority", &body)
            .await;
        let arn = string(&created, "CertificateAuthorityArn", &case);

        let described = emulator
            .ok(
                &case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let configuration = described
            .get("CertificateAuthority")
            .and_then(|authority| authority.get("CertificateAuthorityConfiguration"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            configuration.get("KeyAlgorithm").and_then(Value::as_str),
            Some(key_algorithm),
            "case {case:?}: expected the stored key algorithm {key_algorithm}, got \
             {configuration:?}"
        );
    }
}

/// A request that fails BOTH layers at once is refused by the request-validation stage: the
/// service's own check runs only on a request that stage passed, so the stage's count and
/// clauses are the whole of what is stated.
#[tokio::test]
async fn a_value_the_stage_refuses_is_stated_without_the_service_check() {
    let emulator = Emulator::new();
    let case = "a CA kind outside its type beside a key algorithm outside the offered set";
    let mut body = root_ca_request("both-layers");
    body["CertificateAuthorityType"] = json!("INTERMEDIATE");
    body["CertificateAuthorityConfiguration"]["KeyAlgorithm"] = json!("AES_256");
    let answer = emulator
        .refused(case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
        .await;
    let message = error_message(&answer, case);
    let expected = enum_value_set_message("certificateAuthorityType", "ROOT, SUBORDINATE");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// The other branch of the same rule: an enumerated type nothing is withheld from is stated
/// by the request-validation stage from the type's own value set, in the order that set is
/// walked in — filtering one type's values changes nothing about how any other type reads.
#[tokio::test]
async fn a_type_nothing_is_withheld_from_is_stated_from_its_whole_value_set() {
    let emulator = Emulator::new();
    let case = "a validity period type outside the type";
    let body = json!({
        // A well-formed CA ARN of this emulator's account and Region. No CA answers to it,
        // which is immaterial: a value outside its enumerated type is refused before any CA
        // is resolved.
        "CertificateAuthorityArn": format!(
            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
             6a1c2e44-9e4f-4b3a-9d91-0c7b5d2f8e13"
        ),
        "Csr": blob("a certification request"),
        "SigningAlgorithm": "SHA256WITHRSA",
        "Validity": {"Value": 1, "Type": "FORTNIGHTS"},
    });
    let answer = emulator
        .refused(case, "IssueCertificate", &body, VALIDATION_EXCEPTION)
        .await;
    let message = error_message(&answer, case);
    let expected = enum_value_set_message("validity.type", VALIDITY_PERIOD_TYPE_SET);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}
