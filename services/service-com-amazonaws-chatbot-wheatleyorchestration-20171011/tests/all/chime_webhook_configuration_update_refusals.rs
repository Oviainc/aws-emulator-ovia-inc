// SPDX-License-Identifier: CC0-1.0

//! How `UpdateChimeWebhookConfiguration` answers an update whose members do NOT satisfy
//! the constraints the model declares, over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: an update is applied only once its input
//! satisfies every modelled constraint — a member outside its published pattern or length
//! is refused with this service's modelled client-input error `InvalidRequestException`
//! (HTTP 400) and writes NOTHING, so the stored configuration a later describe reports is
//! exactly the one the refused update addressed; a member inside every published constraint
//! is not refused that way and is written through.
//!
//! The table spans what the rule generalises over rather than one recorded request: every
//! constrained member of the update (`ChatConfigurationArn`, `WebhookUrl`,
//! `WebhookDescription`, `LoggingLevel`, `IamRoleArn`, `SnsTopicArns`), both constraint
//! kinds (pattern and length), and for each length-bounded member the value one past the
//! published bound beside the value AT it.

use serde_json::{json, Value};

use crate::wire::Emulator;

const CREATE_PATH: &str = "/create-chime-webhook-configuration";
const UPDATE_PATH: &str = "/update-chime-webhook-configuration";
const DESCRIBE_PATH: &str = "/describe-chime-webhook-configurations";

/// The modelled client-input error a constraint violation is refused with
/// (`InvalidRequestException`, `@error("client") @httpError(400)`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";
const SNS_TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:alerts";

/// A `ChimeWebhookUrl` satisfying its published pattern (`smithy_query`
/// shape=ChimeWebhookUrl: `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`).
fn webhook_url(id: &str) -> String {
    format!("https://hooks.chime.aws/incomingwebhooks/{id}?token=tok-{id}")
}

/// A pattern-satisfying `ChimeWebhookUrl` of exactly `total` characters, so a row can sit
/// at the published length bound (1..255) and one past it without failing the pattern too.
/// The fixed text is 48 characters (`https://hooks.chime.aws/incomingwebhooks/` plus
/// `?token=`), and the webhook id and the token carry the rest.
fn webhook_url_of_length(total: usize) -> String {
    const FIXED: usize = "https://hooks.chime.aws/incomingwebhooks/?token=".len();
    const TOKEN: usize = 8;
    let id = "a".repeat(total - FIXED - TOKEN);
    let url = format!(
        "https://hooks.chime.aws/incomingwebhooks/{id}?token={}",
        "b".repeat(TOKEN)
    );
    assert_eq!(
        url.len(),
        total,
        "the built WebhookUrl must be {total} characters long, got {}",
        url.len()
    );
    url
}

/// The prefix of a `chatbot` chat-configuration ARN, up to the configuration name
/// (`state::chime_webhook_configuration_arn`).
const CHAT_CONFIGURATION_ARN_PREFIX: &str =
    "arn:aws:chatbot:us-east-1:000000000000:chat-configuration/chime-webhook/";

/// A `ChatConfigurationArn` whose resource segment — everything after the account id — is
/// `resource_segment` characters long. `ChatConfigurationArn`'s pattern admits a resource
/// segment of at most 1024 characters (`[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,1023}`), so this
/// is how a row sits at that published bound and one past it.
fn chat_configuration_arn_with_resource_segment(resource_segment: usize) -> String {
    let account_prefix = "arn:aws:chatbot:us-east-1:000000000000:";
    let name_prefix = CHAT_CONFIGURATION_ARN_PREFIX
        .strip_prefix(account_prefix)
        .expect("the ARN prefix carries the account id");
    let arn = format!(
        "{CHAT_CONFIGURATION_ARN_PREFIX}{}",
        "a".repeat(resource_segment - name_prefix.len())
    );
    assert_eq!(
        arn.len() - account_prefix.len(),
        resource_segment,
        "the built ARN's resource segment must be {resource_segment} characters long, got {}",
        arn.len() - account_prefix.len()
    );
    arn
}

/// The create every row starts from, carrying every member so a refused update is visible
/// as the stored record staying exactly as created.
fn baseline_create(configuration_name: &str) -> Value {
    json!({
        "ConfigurationName": configuration_name,
        "WebhookDescription": "Ops Room/Pager Webhook",
        "WebhookUrl": webhook_url(configuration_name),
        "SnsTopicArns": [SNS_TOPIC_ARN],
        "IamRoleArn": IAM_ROLE_ARN,
        "LoggingLevel": "INFO",
    })
}

/// Create the baseline configuration and answer `(arn, configuration)`.
async fn create_baseline(emulator: &Emulator, configuration_name: &str) -> (String, Value) {
    let created = emulator
        .call(CREATE_PATH, &baseline_create(configuration_name))
        .await;
    assert_eq!(
        created.status, 200,
        "creating {configuration_name:?} answers 200, got {} with {}",
        created.status, created.body
    );
    let configuration = created.body["WebhookConfiguration"].clone();
    let arn = configuration["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers an ARN, got {}", created.body))
        .to_string();
    (arn, configuration)
}

/// The configuration the store holds under `arn`, read back through Describe.
async fn described(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator
        .call(DESCRIBE_PATH, &json!({"ChatConfigurationArn": arn}))
        .await;
    assert_eq!(
        answer.status, 200,
        "describing {arn} answers 200, got {} with {}",
        answer.status, answer.body
    );
    let listed = answer.body["WebhookConfigurations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("describing {arn}: no WebhookConfigurations in {}", answer.body)
        })
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "describing {arn} by ARN must answer exactly one configuration, got {}",
        answer.body
    );
    listed[0].clone()
}

/// One refused row: a label and the members the update carries besides the ARN. A row that
/// carries `ChatConfigurationArn` itself overrides the baseline's ARN, which is how the
/// ARN's own constraints are exercised.
struct RefusedCase {
    label: &'static str,
    update: Value,
}

fn refused_cases() -> Vec<RefusedCase> {
    vec![
        // --- pattern ---
        RefusedCase {
            // ChimeWebhookUrl's pattern requires the `?token=` query string.
            label: "WebhookUrl without the token query string",
            update: json!({"WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/no-token"}),
        },
        RefusedCase {
            // ... and the Amazon Chime incoming-webhook host.
            label: "WebhookUrl on another host",
            update: json!({"WebhookUrl": "https://hooks.example.com/incomingwebhooks/x?token=y"}),
        },
        RefusedCase {
            // CustomerCwLogLevel's pattern `^(ERROR|INFO|NONE)$` is the whole published
            // catalogue of logging levels.
            label: "LoggingLevel outside the published catalogue",
            update: json!({"LoggingLevel": "DEBUG"}),
        },
        RefusedCase {
            // The catalogue is upper case, and a lower-case spelling of a published level
            // satisfies the length (4..5) and fails the pattern alone.
            label: "LoggingLevel in lower case",
            update: json!({"LoggingLevel": "info"}),
        },
        RefusedCase {
            // Arn's pattern `^arn:aws:…$`.
            label: "IamRoleArn that is not an ARN",
            update: json!({"IamRoleArn": "chatbot-role"}),
        },
        RefusedCase {
            // SnsTopicArnList's member targets the same `Arn` shape, so a list ENTRY is
            // constrained just as a scalar member is.
            label: "SnsTopicArns entry that is not an ARN",
            update: json!({"SnsTopicArns": [SNS_TOPIC_ARN, "alerts"]}),
        },
        RefusedCase {
            // ChatConfigurationArn's pattern admits only the `wheatley`/`chatbot` ARN
            // namespaces; this one is long enough and names another service.
            label: "ChatConfigurationArn in another service's namespace",
            update: json!({
                "ChatConfigurationArn":
                    "arn:aws:sns:us-east-1:000000000000:chat-configuration/chime-webhook/x",
            }),
        },
        // --- length ---
        RefusedCase {
            // ChimeWebhookDescription length 1..255: one below the minimum.
            label: "an empty WebhookDescription",
            update: json!({"WebhookDescription": ""}),
        },
        RefusedCase {
            // ... and one above the maximum.
            label: "a WebhookDescription one character past the length maximum",
            update: json!({"WebhookDescription": "d".repeat(256)}),
        },
        RefusedCase {
            // ChimeWebhookUrl length 1..255, with the pattern still satisfied.
            label: "a pattern-valid WebhookUrl one character past the length maximum",
            update: json!({"WebhookUrl": webhook_url_of_length(256)}),
        },
        RefusedCase {
            // ChatConfigurationArn's pattern admits a resource segment of at most 1024
            // characters; this one is a character past that bound.
            label: "a ChatConfigurationArn whose resource segment is past the pattern's bound",
            update: json!({
                "ChatConfigurationArn": chat_configuration_arn_with_resource_segment(1025),
            }),
        },
    ]
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (arn, created) = create_baseline(&emulator, "refused-updates").await;

    for case in refused_cases() {
        let mut request = json!({"ChatConfigurationArn": arn});
        for (member, value) in case.update.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }

        let answer = emulator.call(UPDATE_PATH, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {}",
            case.label, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?} in {}",
            case.label,
            answer.code(),
            answer.body
        );
        // restJson1 resolves the error SHAPE from this header, so it names the modelled
        // error too.
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_REQUEST_EXCEPTION),
            "case {:?}: expected x-amzn-errortype {INVALID_REQUEST_EXCEPTION}, got {:?}",
            case.label,
            answer.error_type
        );

        // The refused update wrote nothing: the store still holds the configuration as it
        // was created.
        let stored = described(&emulator, &arn).await;
        assert_eq!(
            stored, created,
            "case {:?}: a refused update must leave the stored configuration unchanged, got \
             {stored}",
            case.label
        );
    }

    // The counterpart AT that bound is no constraint violation at all: it is a well-formed
    // ARN the store holds no configuration under, so it is answered with the modelled 404
    // instead.
    let at_the_bound = emulator
        .call(
            UPDATE_PATH,
            &json!({
                "ChatConfigurationArn": chat_configuration_arn_with_resource_segment(1024),
                "WebhookDescription": "Never/Applied",
            }),
        )
        .await;
    assert_eq!(
        at_the_bound.status, 404,
        "an ARN at the longest resource segment the pattern admits is inside the constraint, \
         so it must be answered 404 rather than refused, got {} with {}",
        at_the_bound.status, at_the_bound.body
    );
    assert_eq!(
        at_the_bound.code(),
        "ResourceNotFoundException",
        "an ARN at the longest resource segment the pattern admits must answer the modelled \
         not-found, got {}",
        at_the_bound.body
    );

    // ... and it created no second configuration either.
    let all = emulator.call(DESCRIBE_PATH, &json!({})).await;
    assert_eq!(
        all.body["WebhookConfigurations"],
        json!([created]),
        "the refused updates must persist nothing, got {}",
        all.body
    );
}

/// One accepted row: a member AT a published bound, which must be written through rather
/// than refused. `persisted` names the member a later describe must report, if any — the
/// modelled `ChimeWebhookConfiguration` carries no `WebhookUrl`, so a URL row only claims
/// the update was accepted.
struct AcceptedCase {
    label: &'static str,
    update: Value,
    persisted: Option<(&'static str, Value)>,
}

fn accepted_cases() -> Vec<AcceptedCase> {
    vec![
        AcceptedCase {
            // ChimeWebhookUrl length 1..255, at the maximum.
            label: "a WebhookUrl at the length maximum",
            update: json!({"WebhookUrl": webhook_url_of_length(255)}),
            persisted: None,
        },
        AcceptedCase {
            // ChimeWebhookDescription length 1..255, at each bound.
            label: "a WebhookDescription at the length minimum",
            update: json!({"WebhookDescription": "a"}),
            persisted: Some(("WebhookDescription", json!("a"))),
        },
        AcceptedCase {
            label: "a WebhookDescription at the length maximum",
            update: json!({"WebhookDescription": "d".repeat(255)}),
            persisted: Some(("WebhookDescription", json!("d".repeat(255)))),
        },
        AcceptedCase {
            // Every value in the published LoggingLevel catalogue.
            label: "LoggingLevel ERROR",
            update: json!({"LoggingLevel": "ERROR"}),
            persisted: Some(("LoggingLevel", json!("ERROR"))),
        },
        AcceptedCase {
            label: "LoggingLevel INFO",
            update: json!({"LoggingLevel": "INFO"}),
            persisted: Some(("LoggingLevel", json!("INFO"))),
        },
        AcceptedCase {
            label: "LoggingLevel NONE",
            update: json!({"LoggingLevel": "NONE"}),
            persisted: Some(("LoggingLevel", json!("NONE"))),
        },
        AcceptedCase {
            // Arn length 12..1224: the shortest spelling the pattern admits.
            label: "an IamRoleArn at the shortest spelling the pattern admits",
            update: json!({"IamRoleArn": "arn:aws:iam:::r"}),
            persisted: Some(("IamRoleArn", json!("arn:aws:iam:::r"))),
        },
    ]
}

#[tokio::test]
async fn an_update_inside_every_modelled_constraint_is_accepted_and_written_through() {
    for case in accepted_cases() {
        // A fresh store per row, so each row updates the same baseline configuration.
        let emulator = Emulator::new();
        let (arn, _created) = create_baseline(&emulator, "accepted-updates").await;

        let mut request = json!({"ChatConfigurationArn": arn});
        for (member, value) in case.update.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }

        let answer = emulator.call(UPDATE_PATH, &request).await;
        assert_ne!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {:?}: input inside every modelled constraint must not be refused as a \
             constraint violation, got {} with {}",
            case.label,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with {}",
            case.label, answer.status, answer.body
        );

        let stored = described(&emulator, &arn).await;
        if let Some((member, expected)) = case.persisted {
            assert_eq!(
                stored.get(member),
                Some(&expected),
                "case {:?}: the describe must report the updated {member}, got {stored}",
                case.label
            );
        }
        assert_eq!(
            answer.body.get("WebhookConfiguration"),
            Some(&stored),
            "case {:?}: the update must answer the configuration the describe reports, got {}",
            case.label,
            answer.body
        );
    }
}
