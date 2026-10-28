// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreatePlatformApplication`, the call that registers a
//! mobile-push platform application object of the calling account.
//!
//! The rule under test: a create REGISTERS one platform application per
//! (platform, name) pair of the calling account's region and answers the ARN that pair
//! has — `app/<platform>/<name>` — persisting the attributes the request carried
//! together with the delivery flag an application SNS has just registered carries, so
//! the application reads back through the attribute read with the write-only credential
//! material withheld; a second create naming the same pair addresses the SAME
//! application. The request is refused with the operation's one modelled client error
//! (HTTP 400 / `InvalidParameter`) when the NAME breaks the documented character or
//! length rule, when the PLATFORM names no push notification service SNS registers an
//! application with, when the ATTRIBUTES carry a name SNS does not define for an
//! application, or when the credential material is not the pair that platform's
//! documented credential type is — and a refused create persists nothing.
//!
//! The rows differ along what the rule generalises over: the push platform (every entry
//! of the documented catalog, and values outside it), the application name (both
//! published length bounds, one past the upper one, each punctuation character the rule
//! admits and characters it does not), the attribute set (none beyond the credentials,
//! one, several, each defined name, an undefined name), the form of the credential
//! material each platform takes, the request's region and the number of creates
//! addressed at one pair.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    attribute_flag, platform_application_arn, unknown_attribute_refusal,
    unsupported_platform_refusal, ATTRIBUTES_MEMBER, INVALID_PLATFORM_APPLICATION_NAME_REFUSAL,
    INVALID_PLATFORM_CREDENTIALS_REFUSAL, MAX_PLATFORM_APPLICATION_NAME_LEN,
    PLATFORM_APPLICATION_ATTRIBUTE_NAMES, PLATFORM_APPLICATION_ENABLED_ATTRIBUTE,
    PLATFORM_CREDENTIAL_ATTRIBUTE, PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES, PLATFORM_NAMES,
    PLATFORM_PRINCIPAL_ATTRIBUTE,
};

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// A private key as PEM carries one: the form the certificate and token credential
/// material of the Apple and MPNS push services is written in (RFC 7468's
/// `-----BEGIN <label>-----` encapsulation boundary).
const PEM_PRIVATE_KEY: &str =
    "-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBgkqhkiG9w0BAQEFAAS\n-----END PRIVATE KEY-----";

/// An API key in the form the GCM (Firebase Cloud Messaging) console issues one: a
/// 39-character URL-safe string beginning `AIza`.
const API_KEY: &str = "AIzaSy0123456789_abcdefghijklmnopq-rstu";

/// The length the key on [`API_KEY`] is issued with.
const API_KEY_LEN: usize = 39;

/// The JSON formatted private key file the GCM token credential type takes, which
/// `CreatePlatformApplication` documents as the other form of that platform's
/// credential.
const KEY_FILE: &str = r#"{"type":"service_account","project_id":"push-emulator",
"private_key":"-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBg\n-----END PRIVATE KEY-----",
"client_email":"push@push-emulator.example"}"#;

/// The credential pair `platform` documents, in the form that platform's credential
/// type names: a PEM-encoded key for the Apple services and MPNS, an API key for GCM
/// (which documents no principal), and an opaque secret for the services whose clause
/// names one (ADM's client secret, Baidu's and WNS's secret key).
///
/// `pub(crate)` so a sibling suite that needs a REGISTERED application to act on builds
/// one through the same credential material rather than re-deriving each platform's
/// documented pair.
pub(crate) fn credentials(platform: &str) -> Vec<(String, String)> {
    let mut pair = Vec::new();
    match platform {
        "APNS" | "APNS_SANDBOX" | "MPNS" => {
            pair.push((
                PLATFORM_CREDENTIAL_ATTRIBUTE.to_string(),
                PEM_PRIVATE_KEY.to_string(),
            ));
            pair.push((
                PLATFORM_PRINCIPAL_ATTRIBUTE.to_string(),
                "-----BEGIN CERTIFICATE-----\nMIIBkzCB/QIBADAN\n-----END CERTIFICATE-----"
                    .to_string(),
            ));
        }
        "GCM" => pair.push((
            PLATFORM_CREDENTIAL_ATTRIBUTE.to_string(),
            API_KEY.to_string(),
        )),
        _ => {
            pair.push((
                PLATFORM_CREDENTIAL_ATTRIBUTE.to_string(),
                "client-secret-0123456789".to_string(),
            ));
            pair.push((
                PLATFORM_PRINCIPAL_ATTRIBUTE.to_string(),
                "client-id-0123456789".to_string(),
            ));
        }
    }
    pair
}

/// The awsQuery form parameters carrying `attributes` as the input's attribute map.
///
/// `pub(crate)` so every suite that sends an attribute MAP over this service's awsQuery
/// form encodes it the same way rather than writing a second encoder.
pub(crate) fn attribute_params(attributes: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (name, value)) in attributes.iter().enumerate() {
        params.push((format!("Attributes.entry.{}.key", index + 1), name.clone()));
        params.push((
            format!("Attributes.entry.{}.value", index + 1),
            value.clone(),
        ));
    }
    params
}

/// One `CreatePlatformApplication` call in `region`, with `attributes` as its map.
async fn create_in_region(
    sns: &Sns,
    region: &str,
    name: &str,
    platform: &str,
    attributes: &[(String, String)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("Name".to_string(), name.to_string()),
        ("Platform".to_string(), platform.to_string()),
    ];
    params.extend(attribute_params(attributes));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call_in_region(region, "CreatePlatformApplication", &borrowed)
        .await
}

/// One `CreatePlatformApplication` call in the suite's own region.
async fn create(
    sns: &Sns,
    name: &str,
    platform: &str,
    attributes: &[(String, String)],
) -> Answer {
    create_in_region(sns, REGION, name, platform, attributes).await
}

/// A create carrying only the credential pair `platform` documents.
async fn create_with_credentials(sns: &Sns, name: &str, platform: &str) -> Answer {
    create(sns, name, platform, &credentials(platform)).await
}

/// A platform application of `platform` registered in `region`, with its ARN.
///
/// `pub(crate)` so a sibling suite acts on an application this service really holds —
/// registered through the wire call that mints one — rather than seeding a record.
pub(crate) async fn registered_application(
    sns: &Sns,
    region: &str,
    name: &str,
    platform: &str,
) -> String {
    let answer = create_in_region(sns, region, name, platform, &credentials(platform)).await;
    assert_eq!(
        answer.status, 200,
        "CreatePlatformApplication {name:?} on {platform:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.element("PlatformApplicationArn").unwrap_or_else(|| {
        panic!(
            "CreatePlatformApplication {name:?} on {platform:?}: no ARN in {}",
            answer.body
        )
    })
}

/// The attributes `GetPlatformApplicationAttributes` reports for `arn`.
async fn read(sns: &Sns, arn: &str) -> Answer {
    sns.call(
        "GetPlatformApplicationAttributes",
        &[("PlatformApplicationArn", arn)],
    )
    .await
}

/// Assert `answer` is the modelled client refusal carrying `message`, and that the
/// account holds `held` applications after it.
fn refused(case: &str, answer: &Answer, message: &str, held: usize, sns: &Sns) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "InvalidParameter",
        "case {case:?}: expected the InvalidParameter code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(message),
        "case {case:?}: expected the member's refusal sentence, got {}",
        answer.body
    );
    assert!(
        answer.element("PlatformApplicationArn").is_none(),
        "case {case:?}: a refused create must answer no ARN, got {}",
        answer.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        held,
        "case {case:?}: a refused create must persist nothing"
    );
}

/// Every platform of the documented catalog registers an application whose ARN carries
/// that platform and the name, and a value naming no push notification service is
/// refused with the member's sentence naming the value that was not one of them.
#[tokio::test]
async fn the_platform_is_the_documented_catalog_and_anything_else_is_refused() {
    let sns = Sns::new();

    // The catalog itself, from `CreatePlatformApplicationInput$Platform` and the
    // per-platform credential clauses of the `CreatePlatformApplication` doc — walked
    // entry by entry rather than on one cited platform.
    for (index, platform) in PLATFORM_NAMES.iter().enumerate() {
        let name = format!("catalog-{platform}");
        let answer = create_with_credentials(&sns, &name, platform).await;
        assert_eq!(
            answer.status, 200,
            "case {platform:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer
            .element("PlatformApplicationArn")
            .unwrap_or_else(|| panic!("case {platform:?}: no ARN in {}", answer.body));
        assert_eq!(
            arn,
            platform_application_arn(REGION, platform, &name),
            "case {platform:?}: expected the application's own ARN, got {arn:?}"
        );
        assert!(
            arn.ends_with(&format!(":app/{platform}/{name}")),
            "case {platform:?}: expected an app ARN of that platform and name, got {arn:?}"
        );
        assert_eq!(
            sns.state().lock().platform_applications.len(),
            index + 1,
            "case {platform:?}: expected the create to persist one application"
        );
    }

    let held = PLATFORM_NAMES.len();
    // A documented name in another case is the SAME push service: it resolves to the
    // catalog's spelling, so it addresses the application that service already has
    // rather than minting a second record under a second ARN.
    let documented = PLATFORM_NAMES[0];
    let lowered = documented.to_ascii_lowercase();
    let name = format!("catalog-{documented}");
    let answer = create(&sns, &name, &lowered, &credentials(documented)).await;
    assert_eq!(
        answer.status, 200,
        "case \"a documented name in lower case\": expected 200, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("PlatformApplicationArn").as_deref(),
        Some(platform_application_arn(REGION, documented, &name).as_str()),
        "case \"a documented name in lower case\": expected the ARN of the documented \
         spelling, got {}",
        answer.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        held,
        "case \"a documented name in lower case\": expected no second record for one \
         push service"
    );

    let unknown: &[(&str, &str)] = &[
        ("a platform nothing documents", "NOT_A_REAL_PLATFORM"),
        ("the service's other trade name", "FCM"),
        ("empty", ""),
        ("whitespace", " "),
        ("a documented name with a suffix", "GCM_SANDBOX"),
        ("a documented name as a substring", "XGCM"),
    ];
    for (case, platform) in unknown {
        let answer = create_with_credentials(&sns, "unknown-platform", platform).await;
        refused(
            case,
            &answer,
            &unsupported_platform_refusal(platform),
            held,
            &sns,
        );
    }
}

/// The application name is held to the rule the model documents: 1 to 256 characters of
/// ASCII letters, numbers, underscores, hyphens and periods.
#[tokio::test]
async fn the_application_name_rule_is_the_documented_one() {
    let sns = Sns::new();
    let at_bound = "a".repeat(MAX_PLATFORM_APPLICATION_NAME_LEN);
    let past_bound = "a".repeat(MAX_PLATFORM_APPLICATION_NAME_LEN + 1);
    let cases: &[(&str, &str, bool)] = &[
        ("one character, the lower bound", "a", true),
        ("letters digits underscore hyphen period", "Push_app-9.v2", true),
        ("every punctuation character the rule admits", "_-.", true),
        ("at the documented upper bound", &at_bound, true),
        ("one past the documented upper bound", &past_bound, false),
        ("empty", "", false),
        ("an embedded space", "two words", false),
        ("punctuation the rule does not admit", "push!", false),
        ("a slash, which would re-shape the ARN", "push/app", false),
        ("non-ascii", "push-ü", false),
    ];

    let mut held = 0;
    for (case, name, accepted) in cases {
        let answer = create_with_credentials(&sns, name, "GCM").await;
        if *accepted {
            held += 1;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer.element("PlatformApplicationArn").unwrap_or_default();
            assert!(
                arn.ends_with(&format!("/{name}")),
                "case {case:?}: expected an ARN ending in the application name, got {arn:?}"
            );
            assert_eq!(
                sns.state().lock().platform_applications.len(),
                held,
                "case {case:?}: expected the create to persist one application"
            );
        } else {
            refused(
                case,
                &answer,
                INVALID_PLATFORM_APPLICATION_NAME_REFUSAL,
                held,
                &sns,
            );
        }
    }
}

/// The attribute map is the closed set SNS defines for a platform application: every
/// defined name is written, and a name it does not define is refused with the member
/// and the key that was not recognised.
#[tokio::test]
async fn the_attribute_map_is_the_closed_set_the_service_defines() {
    let sns = Sns::new();
    let topic = "arn:aws:sns:us-east-1:000000000000:endpoint-events";

    // Each name of the defined set, walked one at a time beside the credential pair, so
    // every entry of the catalog is exercised rather than one cited name.
    let mut held = 0;
    for name in PLATFORM_APPLICATION_ATTRIBUTE_NAMES {
        let value = match *name {
            "SuccessFeedbackSampleRate" => "50".to_string(),
            "ApplePlatformTeamID" => "TEAM123456".to_string(),
            "ApplePlatformBundleID" => "com.example.push".to_string(),
            flag if flag == PLATFORM_APPLICATION_ENABLED_ATTRIBUTE => attribute_flag(false),
            credential if PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(&credential) => {
                continue;
            }
            _ => topic.to_string(),
        };
        let mut attributes = credentials("GCM");
        attributes.push(((*name).to_string(), value.clone()));
        let application = format!("defined-{name}");
        let answer = create(&sns, &application, "GCM", &attributes).await;
        held += 1;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer.element("PlatformApplicationArn").unwrap_or_default();
        let reported = attribute_map(&read(&sns, &arn).await);
        assert_eq!(
            reported.get(*name),
            Some(&value),
            "case {name:?}: expected the attribute to read back, got {reported:?}"
        );
    }

    let unknown: &[(&str, &str)] = &[
        ("a name of another resource's set", "CustomUserData"),
        ("a misspelled defined name", "EventEndpointCreate"),
        ("a name the service derives", "AuthenticationMethod"),
        ("another name the service derives", "AppleCertificateExpiryDate"),
        ("a name nothing defines", "Whatever"),
        ("a defined name in another case", "platformcredential"),
    ];
    for (case, name) in unknown {
        let mut attributes = credentials("GCM");
        attributes.push(((*name).to_string(), "x".to_string()));
        let answer = create(&sns, "unknown-attribute", "GCM", &attributes).await;
        refused(
            case,
            &answer,
            &unknown_attribute_refusal(ATTRIBUTES_MEMBER, name),
            held,
            &sns,
        );
    }
}

/// The credential pair each platform documents has to be there, and the credential has
/// to be the artifact that platform's credential type names: material no push service
/// could authenticate with is refused, whichever platform it was offered for.
#[tokio::test]
async fn the_credential_material_must_be_the_pair_the_platform_documents() {
    let sns = Sns::new();
    let credential = PLATFORM_CREDENTIAL_ATTRIBUTE.to_string();
    let principal = PLATFORM_PRINCIPAL_ATTRIBUTE.to_string();
    assert_eq!(
        API_KEY.len(),
        API_KEY_LEN,
        "the API key row must carry the length the console issues"
    );

    type Row = (&'static str, &'static str, Vec<(String, String)>, bool);
    let rows: Vec<Row> = vec![
        (
            "GCM key credentials: the API key its console issues",
            "GCM",
            vec![(credential.clone(), API_KEY.to_string())],
            true,
        ),
        (
            "GCM token credentials: the JSON formatted key file",
            "GCM",
            vec![(credential.clone(), KEY_FILE.to_string())],
            true,
        ),
        (
            "GCM: material that is neither an API key nor a key file",
            "GCM",
            vec![(credential.clone(), "not-a-real-credential".to_string())],
            false,
        ),
        (
            "GCM: an API key one character short of the issued length",
            "GCM",
            vec![(
                credential.clone(),
                API_KEY[..API_KEY_LEN - 1].to_string(),
            )],
            false,
        ),
        (
            "GCM: no credential at all",
            "GCM",
            vec![(principal.clone(), "unused".to_string())],
            false,
        ),
        (
            "GCM: a blank credential",
            "GCM",
            vec![(credential.clone(), "   ".to_string())],
            false,
        ),
        (
            "APNS certificate credentials: a PEM key beside the certificate",
            "APNS",
            credentials("APNS"),
            true,
        ),
        (
            "APNS token credentials: the signing key beside its id",
            "APNS",
            vec![
                (credential.clone(), PEM_PRIVATE_KEY.to_string()),
                (principal.clone(), "ABCD123456".to_string()),
            ],
            true,
        ),
        (
            "APNS: a key that is not PEM-encoded",
            "APNS",
            vec![
                (credential.clone(), "bogus-signing-key".to_string()),
                (principal.clone(), "ABCD123456".to_string()),
            ],
            false,
        ),
        (
            "APNS: the key with no principal, which its clauses require",
            "APNS",
            vec![(credential.clone(), PEM_PRIVATE_KEY.to_string())],
            false,
        ),
        (
            "APNS_SANDBOX: a key that is not PEM-encoded",
            "APNS_SANDBOX",
            vec![
                (credential.clone(), "sandbox-signing-key".to_string()),
                (principal.clone(), "ABCD123456".to_string()),
            ],
            false,
        ),
        (
            "MPNS: a PEM key beside the TLS certificate",
            "MPNS",
            credentials("MPNS"),
            true,
        ),
        (
            "ADM: the opaque client secret beside the client id",
            "ADM",
            credentials("ADM"),
            true,
        ),
        (
            "ADM: the client secret with no client id",
            "ADM",
            vec![(credential.clone(), "client-secret".to_string())],
            false,
        ),
        (
            "Baidu: the opaque secret key beside the API key",
            "Baidu",
            credentials("Baidu"),
            true,
        ),
        (
            "WNS: the opaque secret key beside the package identifier",
            "WNS",
            credentials("WNS"),
            true,
        ),
    ];

    let mut held = 0;
    for (index, (case, platform, attributes, accepted)) in rows.iter().enumerate() {
        let name = format!("credential-{index}");
        let answer = create(&sns, &name, platform, attributes).await;
        if *accepted {
            held += 1;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                sns.state().lock().platform_applications.len(),
                held,
                "case {case:?}: expected the create to persist one application"
            );
        } else {
            refused(
                case,
                &answer,
                INVALID_PLATFORM_CREDENTIALS_REFUSAL,
                held,
                &sns,
            );
        }
    }

    // The attribute map is a required member, so a request carrying none of it is
    // refused before the handler runs and registers nothing either.
    let answer = create(&sns, "no-attribute-map", "GCM", &[]).await;
    assert_eq!(
        answer.status, 400,
        "case \"no attribute map at all\": expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        held,
        "case \"no attribute map at all\": a refused create must persist nothing"
    );

    // The refusals state the member and the reason, never the material itself.
    let answer = create(
        &sns,
        "secret-withheld",
        "GCM",
        &[(credential, "not-a-real-credential".to_string())],
    )
    .await;
    assert!(
        !answer.body.contains("not-a-real-credential"),
        "a refusal must not echo the credential material back, got {}",
        answer.body
    );
}

/// A created application reads back through the attribute read — the attributes the
/// request carried plus the delivery flag — with the write-only credential material
/// withheld; a second create naming the same platform and name addresses the SAME
/// application, and the ARN carries the region the request was signed for.
#[tokio::test]
async fn a_created_application_reads_back_and_a_second_create_addresses_it() {
    let sns = Sns::new();
    let topic = "arn:aws:sns:us-east-1:000000000000:endpoint-created";
    let mut attributes = credentials("APNS");
    attributes.push((
        "EventEndpointCreated".to_string(),
        topic.to_string(),
    ));
    attributes.push(("ApplePlatformTeamID".to_string(), "TEAM123456".to_string()));

    let answer = create(&sns, "round-trip", "APNS", &attributes).await;
    assert_eq!(
        answer.status, 200,
        "the create: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer
        .element("PlatformApplicationArn")
        .unwrap_or_else(|| panic!("the create: no ARN in {}", answer.body));

    let read_back = read(&sns, &arn).await;
    assert_eq!(
        read_back.status, 200,
        "the read: expected 200, got {} / {}",
        read_back.status, read_back.body
    );
    let reported = attribute_map(&read_back);
    let expected: HashMap<String, String> = [
        (
            PLATFORM_APPLICATION_ENABLED_ATTRIBUTE.to_string(),
            attribute_flag(true),
        ),
        ("EventEndpointCreated".to_string(), topic.to_string()),
        ("ApplePlatformTeamID".to_string(), "TEAM123456".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        reported, expected,
        "the read: expected {expected:?}, got {reported:?}"
    );
    for (name, value) in &attributes {
        if !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(&name.as_str()) {
            continue;
        }
        assert!(
            !read_back.body.contains(value),
            "the read must withhold the {name} value, got {}",
            read_back.body
        );
    }

    // A second create of the same pair addresses the application that pair already has:
    // the same ARN, one record, and what the second request carried applied to it.
    let mut second = credentials("APNS");
    second.push((
        "EventEndpointDeleted".to_string(),
        "arn:aws:sns:us-east-1:000000000000:endpoint-deleted".to_string(),
    ));
    let again = create(&sns, "round-trip", "APNS", &second).await;
    assert_eq!(
        again.status, 200,
        "the second create: expected 200, got {} / {}",
        again.status, again.body
    );
    assert_eq!(
        again.element("PlatformApplicationArn").as_deref(),
        Some(arn.as_str()),
        "the second create: expected the same ARN, got {}",
        again.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        1,
        "the second create: expected one application under that pair"
    );
    let merged = attribute_map(&read(&sns, &arn).await);
    assert_eq!(
        merged.get("EventEndpointDeleted").map(String::as_str),
        Some("arn:aws:sns:us-east-1:000000000000:endpoint-deleted"),
        "the second create: expected its attribute applied, got {merged:?}"
    );
    assert_eq!(
        merged.get("EventEndpointCreated").map(String::as_str),
        Some(topic),
        "the second create: expected the held attributes kept, got {merged:?}"
    );

    // A regional service registers the application in the region the request addresses,
    // and the ARN it answers carries that region.
    let elsewhere = "us-west-2";
    let answer = create_in_region(
        &sns,
        elsewhere,
        "round-trip",
        "APNS",
        &credentials("APNS"),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the create in another region: expected 200, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("PlatformApplicationArn").as_deref(),
        Some(platform_application_arn(elsewhere, "APNS", "round-trip").as_str()),
        "the create in another region: expected an ARN of that region, got {}",
        answer.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        2,
        "the create in another region: expected a record of its own"
    );
}
