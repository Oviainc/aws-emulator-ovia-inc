// SPDX-License-Identifier: CC0-1.0

//! What `GetMFADevice` answers, end to end over the awsQuery wire.
//!
//! RULE: this read serves ONE kind of MFA device and only that kind — "for this API, we
//! only accept FIDO security key ARNs"
//! (`smithy_query(query="doc",shape="GetMFADeviceRequest",member="SerialNumber")`) — so a
//! `SerialNumber` naming no FIDO security key of the account is a serial this API cannot
//! serve and is refused HTTP 400 with the error code `ValidationError` and AWS's fixed
//! sentence for it, before anything about the addressed user or device is reported: the
//! refusal is the same whether the named user exists, is named at all or exists under
//! another name. A serial that DOES name one is reported as the account holds it, and
//! asking for that device under another user's name is the modelled `NoSuchEntity` 404,
//! because the device is not that user's.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the form
//! of the serial (a FIDO/U2F device ARN, a virtual-MFA ARN, an ARN of another account or
//! partition, a bare hardware token serial, a value that is no ARN at all), both published
//! length bounds of `serialNumberType`, whether the request names a user and whether that
//! user exists — and, for the devices the account holds, which user holds one and how many
//! certifications it carries.
//!
//! A FIDO security key is enrolled by the key itself through the browser's WebAuthn
//! ceremony and IAM's API binds no operation that registers one, so the rows about a HELD
//! device seed it into the store the handler reads — the same way
//! `tests/all/organizations_access_report_generation.rs` records the organization AWS
//! Organizations, not IAM, puts an account in.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share, so a request is built and read exactly one way.

use std::collections::BTreeMap;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// The sentence IAM words a `SerialNumber` this API cannot serve with — entirely fixed
/// text, naming no part of the request.
const UNSUPPORTED: &str = "The MFA device serial number sent in the request is invalid or \
     not supported by this API. Currently, only hardware FIDO security keys are supported. \
     Please use the compatible device or API.";

/// The error code IAM answers a value it cannot accept with on the query protocol.
const VALIDATION_ERROR: &str = "ValidationError";

/// An instant a seeded device was enabled at, fixed so what the read reports is the stored
/// value rather than the clock.
const ENABLED_MILLIS: i64 = 1_738_368_000_000;

/// One `GetMFADevice` call: the serial it addresses, and the user it names when it names
/// one.
async fn get_mfa_device(
    router: &MultiServiceRouter,
    serial_number: &str,
    user_name: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("SerialNumber", serial_number)];
    if let Some(user_name) = user_name {
        params.push(("UserName", user_name));
    }
    call(router, "GetMFADevice", &params).await
}

/// Create the IAM user `user_name`, so a row can name a user the account really holds.
async fn create_user(router: &MultiServiceRouter, case: &str, user_name: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
}

/// Record a FIDO security key the account holds, in the service's OWN store.
///
/// No operation of this service enrolls one (see the module docs), so the fact is written
/// into the store the handler reads rather than into a second copy of it.
fn record_fido_security_key(
    engine: &engine::InMemoryEngine,
    serial_number: &str,
    user_name: &str,
    certifications: &[(&str, &str)],
) {
    let store = state::service_state(engine).expect("the IAM store");
    store.lock().fido_security_keys.insert(
        serial_number.to_string(),
        state::FidoSecurityKey {
            serial_number: serial_number.to_string(),
            user_key: state::name_key(user_name),
            enable_date: DateTime::from_millis(ENABLED_MILLIS),
            certifications: certifications
                .iter()
                .map(|(name, level)| ((*name).to_string(), (*level).to_string()))
                .collect::<BTreeMap<String, String>>(),
        },
    );
}

/// One row of the refusal table: the case it is, the serial it addresses and the user it
/// names.
struct Case {
    case: &'static str,
    serial_number: String,
    /// The `UserName` the request carries, if any.
    user_name: Option<&'static str>,
    /// Whether the fixture creates that user first, so a row can show the refusal does
    /// not depend on the user existing.
    user_exists: bool,
}

/// The rows. The account holds no FIDO security key at all, so every serial below names
/// none of them whatever its form.
fn refused_cases() -> Vec<Case> {
    let account = account_id();
    vec![
        // The ARN form AWS's ARN reference spells a U2F/FIDO device of an account with:
        // ARN-shaped, of this account, and naming no key the account holds.
        Case {
            case: "a U2F device ARN of this account, for a user that does not exist",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/ghost/key-1"),
            user_name: None,
            user_exists: false,
        },
        // The same serial with the user NAMED and really held: the refusal is about the
        // serial, so the user existing changes nothing.
        Case {
            case: "a U2F device ARN of this account, for a user that exists",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/holder/key-1"),
            user_name: Some("holder"),
            user_exists: true,
        },
        // And with the user named but absent, which is the third combination.
        Case {
            case: "a U2F device ARN naming a user the account does not hold",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/ghost/key-2"),
            user_name: Some("ghost"),
            user_exists: false,
        },
        // A VIRTUAL MFA device's ARN: a device of another kind, which this API does not
        // serve either.
        Case {
            case: "a virtual MFA device ARN",
            serial_number: format!("arn:aws:iam::{account}:mfa/holder-phone"),
            user_name: Some("holder"),
            user_exists: true,
        },
        // An ARN of ANOTHER account: no device of this account, whatever it names there.
        Case {
            case: "a U2F device ARN of another account",
            serial_number: "arn:aws:iam::111122223333:u2f/user/holder/key-1".to_string(),
            user_name: None,
            user_exists: false,
        },
        // And of another partition.
        Case {
            case: "a U2F device ARN in another partition",
            serial_number: format!("arn:aws-cn:iam::{account}:u2f/user/holder/key-1"),
            user_name: None,
            user_exists: false,
        },
        // A bare hardware token serial, the form a TOTP token carries: no ARN at all.
        Case {
            case: "a bare hardware token serial",
            serial_number: "GAHT12345678".to_string(),
            user_name: None,
            user_exists: false,
        },
        // The PUBLISHED MINIMUM length of `serialNumberType` (9), spelled in the member's
        // own alphabet (`^[\w+=/:,.@-]+$`).
        Case {
            case: "the shortest serial the member admits",
            serial_number: "A+=/:,.@-".to_string(),
            user_name: None,
            user_exists: false,
        },
        // The PUBLISHED MAXIMUM length of `serialNumberType` (256).
        Case {
            case: "the longest serial the member admits",
            serial_number: "s".repeat(256),
            user_name: None,
            user_exists: false,
        },
    ]
}

/// RULE: a `SerialNumber` naming no FIDO security key of the account is refused HTTP 400
/// `ValidationError` with AWS's fixed sentence, before anything about the addressed user
/// or device is reported.
#[tokio::test]
async fn a_serial_naming_no_fido_security_key_is_refused_as_unsupported() {
    let router = router();
    // The users some rows name, created once: a row shows the refusal does not depend on
    // the user, so the same user is named by more than one of them.
    let mut created: Vec<&str> = Vec::new();

    for case in refused_cases() {
        if case.user_exists {
            let user_name = case.user_name.expect("a row whose user the fixture creates");
            if !created.contains(&user_name) {
                create_user(&router, case.case, user_name).await;
                created.push(user_name);
            }
        }

        let (status, body) = get_mfa_device(&router, &case.serial_number, case.user_name).await;

        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status}: {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_ERROR,
            "case {:?}: expected the error code {VALIDATION_ERROR}, got {:?}",
            case.case,
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            UNSUPPORTED,
            "case {:?}: expected the fixed refusal sentence, got {:?}",
            case.case,
            element(&body, "Message")
        );
        // Nothing about the device is reported beside the refusal.
        for member in ["SerialNumber", "EnableDate", "UserName"] {
            assert!(
                elements(&body, member).is_empty(),
                "case {:?}: a refused read reports no {member}, got {:?}",
                case.case,
                elements(&body, member)
            );
        }
    }
}

/// One row of the held-device table: the serial the account holds it under, the user it is
/// enabled for, and the certifications it carries.
struct Held {
    case: &'static str,
    serial_number: String,
    user_name: &'static str,
    /// Rows differ in certification COUNT (0, 1, 2), which is what the modelled map
    /// generalises over.
    certifications: Vec<(&'static str, &'static str)>,
}

/// The devices the rows seed: two users, three certification counts, and two spellings of
/// the same serial form.
fn held_devices() -> Vec<Held> {
    let account = account_id();
    vec![
        Held {
            case: "a key carrying no certification",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/alice/key-1"),
            user_name: "alice",
            certifications: Vec::new(),
        },
        Held {
            case: "a key carrying one certification",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/alice/key-2"),
            user_name: "alice",
            // The certification levels the member's own documentation names: "we
            // currently provide FIPS-140-2, FIPS-140-3, and FIDO certification levels"
            // (`GetMFADeviceResponse$Certifications`).
            certifications: vec![("FIDO", "L1")],
        },
        Held {
            case: "a key carrying two certifications",
            serial_number: format!("arn:aws:iam::{account}:u2f/user/Bob/key-1"),
            user_name: "Bob",
            certifications: vec![("FIPS-140-2", "L2"), ("FIPS-140-3", "L1")],
        },
    ]
}

/// RULE: a serial that names a FIDO security key the account holds is reported as the
/// account holds it — the serial, the user it is enabled for, when it was enabled and its
/// certifications — whether or not the request names that user; a request naming any
/// other user is the modelled `NoSuchEntity` 404.
#[tokio::test]
async fn a_held_fido_security_key_is_reported_for_the_user_it_is_enabled_for() {
    for held in held_devices() {
        let (router, engine) = router_with_engine();
        create_user(&router, held.case, held.user_name).await;
        create_user(&router, held.case, "someone-else").await;
        record_fido_security_key(
            &engine,
            &held.serial_number,
            held.user_name,
            &held.certifications,
        );

        // Named or not, the device is the same record: the user is resolved from the
        // device, not from the request.
        for named in [None, Some(held.user_name)] {
            let case = format!("{} named {named:?}", held.case);
            let (status, body) = get_mfa_device(&router, &held.serial_number, named).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the held device to be reported, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "SerialNumber"),
                held.serial_number,
                "case {case:?}: expected the stored serial, got {:?}",
                element(&body, "SerialNumber")
            );
            assert_eq!(
                element(&body, "UserName"),
                held.user_name,
                "case {case:?}: expected the user the device is enabled for, got {:?}",
                element(&body, "UserName")
            );
            let enabled = DateTime::from_str(&element(&body, "EnableDate"), Format::DateTime)
                .unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: expected an ISO-8601 EnableDate, got {:?}: {error}",
                        element(&body, "EnableDate")
                    )
                });
            assert_eq!(
                enabled.to_millis().expect("a representable instant"),
                ENABLED_MILLIS,
                "case {case:?}: expected the stored EnableDate, got {:?}",
                element(&body, "EnableDate")
            );
            // `CertificationMapType` is a wrapped map, so each pair arrives as a
            // `<key>`/`<value>` element run inside `<entry>`, and that map is the only one
            // in this response shape. What a map reports is the SET of pairs it carries
            // rather than an order, so the comparison is made over the set.
            let reported: BTreeMap<String, String> = elements(&body, "key")
                .into_iter()
                .zip(elements(&body, "value"))
                .collect();
            let expected: BTreeMap<String, String> = held
                .certifications
                .iter()
                .map(|(name, level)| ((*name).to_string(), (*level).to_string()))
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the stored certifications, got {reported:?}"
            );
        }

        // The same device asked for under ANOTHER user's name: that user does not have it.
        let (status, body) =
            get_mfa_device(&router, &held.serial_number, Some("someone-else")).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 for another user's name, got {status}: {body}",
            held.case
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected the error code NoSuchEntity, got {:?}",
            held.case,
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!(
                "MFA Device {} cannot be found for user someone-else.",
                held.serial_number
            ),
            "case {:?}: expected the refusal to describe the device asked about, got {:?}",
            held.case,
            element(&body, "Message")
        );

        // IAM names are "not distinguished by case", so the user the device is enabled
        // for is the same user however the request spells its name.
        let (status, body) = get_mfa_device(
            &router,
            &held.serial_number,
            Some(&held.user_name.to_ascii_uppercase()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the held device to be reported for the same name in \
             another case, got {status}: {body}",
            held.case
        );
        assert_eq!(
            element(&body, "UserName"),
            held.user_name,
            "case {:?}: expected the stored spelling of the user name, got {:?}",
            held.case,
            element(&body, "UserName")
        );

        // Reading a device neither enrolled nor dropped one.
        let store = state::service_state(&*engine).expect("the IAM store");
        let serials: Vec<String> = store.lock().fido_security_keys.keys().cloned().collect();
        assert_eq!(
            serials,
            vec![held.serial_number.clone()],
            "case {:?}: a read persists nothing, got {serials:?}",
            held.case
        );
    }
}
