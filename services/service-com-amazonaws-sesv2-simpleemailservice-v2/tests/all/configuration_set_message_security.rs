// SPDX-License-Identifier: CC0-1.0

//! The message-level security options of a configuration set, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: a create persists the message-security configuration the
//! request carried and a read of that name reports exactly it (and no member at all for a
//! set that configured none), while a value outside the modelled value set of an
//! enum-valued member is refused with the operation's input error and stores nothing — the
//! same rule the sibling suite pins for this input's other enum-valued members, here
//! reaching the one nested inside the `SigningScheme` union.
//!
//! The table below spans what the rule generalises over in this member: which arm of the
//! union the request names (`DefaultScheme`, `SmimeScheme`), whether the arm's own member
//! is carried at all, and the value in it — the whole declared value set of
//! `SignatureFormat`
//! (`smithy_query(query="shape",shape="SigningScheme,SmimeSigningScheme,SignatureFormat")`),
//! beside values the model declares nowhere.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// Every value `SignatureFormat` declares, from the model's own enum
/// (`smithy_query(query="shape",shape="SignatureFormat")`): the format of the S/MIME
/// signature SES applies to messages sent with the configuration set.
const SIGNATURE_FORMATS: [&str; 1] = ["DETACHED"];

/// One row of the round-trip table: the `MessageSecurityOptions` the create carries, and
/// what a read of the stored set must report for that member.
struct SecurityCase {
    case: &'static str,
    name: &'static str,
    /// The request's `MessageSecurityOptions`, or `Value::Null` for a create that carries
    /// the member not at all.
    sent: Value,
    /// The member a read must report, or `Value::Null` where the set holds none.
    reported: Value,
}

/// The rows of the round-trip table.
fn round_trip_cases() -> Vec<SecurityCase> {
    let mut cases = vec![
        SecurityCase {
            case: "no message-security options at all",
            name: "security-absent",
            sent: Value::Null,
            reported: Value::Null,
        },
        SecurityCase {
            case: "the default scheme, which adds no S/MIME signature",
            name: "security-default-scheme",
            sent: json!({"SigningScheme": {"DefaultScheme": {}}}),
            reported: json!({"SigningScheme": {"DefaultScheme": {}}}),
        },
        SecurityCase {
            case: "the S/MIME scheme with no signature format chosen",
            name: "security_smime_unformatted",
            sent: json!({"SigningScheme": {"SmimeScheme": {}}}),
            reported: json!({"SigningScheme": {"SmimeScheme": {}}}),
        },
        SecurityCase {
            case: "the options object present but naming no scheme",
            name: "security-no-scheme",
            sent: json!({}),
            reported: json!({}),
        },
    ];
    // One row per declared `SignatureFormat`, so the whole value set round-trips rather
    // than one chosen value.
    for format in SIGNATURE_FORMATS {
        cases.push(SecurityCase {
            case: "the S/MIME scheme with a declared signature format",
            name: "security-smime-declared",
            sent: json!({"SigningScheme": {"SmimeScheme": {"SignatureFormat": format}}}),
            reported: json!({"SigningScheme": {"SmimeScheme": {"SignatureFormat": format}}}),
        });
    }
    cases
}

#[tokio::test]
async fn create_persists_the_message_security_options_and_get_reads_them_back() {
    let ses = Ses::new();

    for case in round_trip_cases() {
        let mut body = json!({"ConfigurationSetName": case.name});
        if !case.sent.is_null() {
            body["MessageSecurityOptions"] = case.sent.clone();
        }

        let created = ses.create_set(body).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateConfigurationSet, got {} ({})",
            case.case,
            created.status,
            created.message()
        );
        assert!(
            created.is_empty_body(),
            "case {:?}: CreateConfigurationSetResponse declares no members, got {:?}",
            case.case,
            created.member("MessageSecurityOptions")
        );

        let read = ses.get_set(case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetConfigurationSet, got {} ({})",
            case.case,
            read.status,
            read.message()
        );
        assert_eq!(
            read.member("MessageSecurityOptions"),
            case.reported,
            "case {:?}: expected MessageSecurityOptions {:?}, got {:?}",
            case.case,
            case.reported,
            read.member("MessageSecurityOptions")
        );

        // A read reports the stored set without changing it, so a second read answers the
        // same configuration.
        let again = ses.get_set(case.name).await;
        assert_eq!(
            again.member("MessageSecurityOptions"),
            case.reported,
            "case {:?}: a read must not change what a later reader sees, got {:?}",
            case.case,
            again.member("MessageSecurityOptions")
        );
    }
}

/// One row of the refusal table: a `SignatureFormat` the model declares nowhere, and the
/// name the create that carries it asks for.
struct RefusedFormatCase {
    case: &'static str,
    name: &'static str,
    format: Value,
}

/// The rows of the refusal table: a value whose spelling only resembles a declared one, a
/// case-shifted declared value (enum values are compared exactly), the empty string and a
/// format no S/MIME signature has.
fn refused_format_cases() -> Vec<RefusedFormatCase> {
    vec![
        RefusedFormatCase {
            case: "a format the model declares nowhere",
            name: "security-bad-attached",
            format: json!("ATTACHED"),
        },
        RefusedFormatCase {
            case: "a declared value in the wrong case",
            name: "security-bad-case",
            format: json!("Detached"),
        },
        RefusedFormatCase {
            case: "the empty string is no declared value",
            name: "security-bad-empty",
            format: json!(""),
        },
        RefusedFormatCase {
            case: "a declared value with trailing whitespace",
            name: "security_bad_padded",
            format: json!("DETACHED "),
        },
    ]
}

#[tokio::test]
async fn a_signature_format_outside_the_modelled_value_set_is_refused() {
    let ses = Ses::new();

    for case in refused_format_cases() {
        let created = ses
            .create_set(json!({
                "ConfigurationSetName": case.name,
                "MessageSecurityOptions": {
                    "SigningScheme": {"SmimeScheme": {"SignatureFormat": case.format}},
                },
            }))
            .await;
        assert_eq!(
            (created.status, created.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException for format {:?}, got {} ({})",
            case.case,
            case.format,
            created.status,
            created.message()
        );
        assert_refusal_states_the_value_set(case.case, &created);

        // A refused create stores nothing: the name it asked for is still free.
        let read = ses.get_set(case.name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: a refused create must store nothing, got {} ({})",
            case.case,
            read.status,
            read.message()
        );
    }
}

/// Assert the refusal says which member was wrong and which values it accepts, so a client
/// can correct the request from the message alone.
fn assert_refusal_states_the_value_set(case: &str, refused: &Reply) {
    let message = refused.message();
    assert!(
        message.contains("SignatureFormat") || message.contains("signatureFormat"),
        "case {case:?}: the refusal must name the member, got {message:?}"
    );
    for format in SIGNATURE_FORMATS {
        assert!(
            message.contains(format),
            "case {case:?}: the refusal must state the accepted value {format:?}, \
             got {message:?}"
        );
    }
}
