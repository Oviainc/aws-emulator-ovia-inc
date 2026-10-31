// SPDX-License-Identifier: CC0-1.0

//! The account-enforced guardrail ACROSS THE TWO PLANES, end to end over HTTP: the
//! configuration this control plane installs, and the inference the Bedrock Runtime data
//! plane then evaluates against it, with both services mounted on the one shared engine
//! the emulator boots them on.
//!
//! # The rule
//!
//! An account-level enforced guardrail configuration is applied SERVER-SIDE to inference
//! the caller never opted in to: once a configuration is installed, an invocation of a
//! model its `modelEnforcement` covers is evaluated against the enforced guardrail
//! revision's policies even though the request names no guardrail at all, and the
//! invocation reports the action the guardrail took — the service-injected body member
//! `amazon-bedrock-guardrailAction`, `INTERVENED` for a request the guardrail blocked and
//! `NONE` for one it passed — answering a blocked request with the guardrail's
//! `blockedInputMessaging` instead of a completion. Deleting the configuration withdraws
//! that enforcement.
//!
//! # What the table below generalises over
//!
//! The rows differ along the `modelEnforcement` the put carried (none at all, the `ALL`
//! wildcard, the model named outright, another model named, the model excluded), the model
//! invoked, and whether the prompt carries the guardrail's blocked word. The recorded case
//! — a configuration naming the invoked model in `includedModels`, and a prompt carrying
//! the word — is ONE row.

use serde_json::{json, Value};

use crate::support::{emulator_with_inference, Emulator, Reply};

/// The member Amazon Bedrock injects into a guarded invocation's body, carrying the action
/// its guardrail took. The two values it carries are read from the data plane's own
/// publication rule, so this suite and that service cannot disagree about either.
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::enforced_guardrail::{
    ACTION_INTERVENED, ACTION_NONE, ALL_MODELS, GUARDRAIL_ACTION_FIELD,
};

/// The URI `PutEnforcedGuardrailConfiguration` and `ListEnforcedGuardrailsConfiguration`
/// are bound to.
const CONFIGURATION_URI: &str = "/enforcedGuardrailsConfiguration";

/// The word the fixture's guardrail blocks: a token nothing in a request or a served
/// completion carries otherwise.
const BANNED: &str = "zephyrine";

/// What the fixture's guardrail answers a blocked input with.
const BLOCKED_INPUT: &str = "This request was blocked by the account guardrail.";

/// What it answers a blocked response with.
const BLOCKED_OUTPUT: &str = "This response was blocked by the account guardrail.";

/// Two models the data plane's inventory serves and the control plane's catalog publishes,
/// so a row can enforce on one and invoke the other.
const GUARDED_MODEL: &str = "amazon.nova-lite-v1:0";
const OTHER_MODEL: &str = "amazon.nova-pro-v1:0";

/// One row: the `modelEnforcement` the put carries, the model the invocation names, whether
/// the prompt carries the blocked word, and the action the invocation must report (`None`
/// for an invocation the configuration does not cover).
struct Row {
    case: &'static str,
    model_enforcement: Option<Value>,
    /// The `selectiveContentGuarding` the put carries, where it carries one.
    selective_content_guarding: Option<Value>,
    model_id: &'static str,
    banned_prompt: bool,
    expected: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the invoked model named in includedModels, the word in the prompt",
            model_enforcement: Some(json!({
                "includedModels": [GUARDED_MODEL],
                "excludedModels": [],
            })),
            model_id: GUARDED_MODEL,
            banned_prompt: true,
            expected: Some(ACTION_INTERVENED),
            selective_content_guarding: None,
        },
        Row {
            case: "the same configuration and a benign prompt",
            model_enforcement: Some(json!({
                "includedModels": [GUARDED_MODEL],
                "excludedModels": [],
            })),
            model_id: GUARDED_MODEL,
            banned_prompt: false,
            expected: Some(ACTION_NONE),
            selective_content_guarding: None,
        },
        Row {
            case: "no modelEnforcement at all, so every model is enforced on",
            model_enforcement: None,
            model_id: OTHER_MODEL,
            banned_prompt: true,
            expected: Some(ACTION_INTERVENED),
            selective_content_guarding: None,
        },
        Row {
            case: "the ALL wildcard",
            model_enforcement: Some(json!({
                "includedModels": [ALL_MODELS],
                "excludedModels": [],
            })),
            model_id: OTHER_MODEL,
            banned_prompt: true,
            expected: Some(ACTION_INTERVENED),
            selective_content_guarding: None,
        },
        Row {
            case: "the wildcard, with the invoked model excluded",
            model_enforcement: Some(json!({
                "includedModels": [ALL_MODELS],
                "excludedModels": [GUARDED_MODEL],
            })),
            model_id: GUARDED_MODEL,
            banned_prompt: true,
            expected: None,
            selective_content_guarding: None,
        },
        Row {
            case: "another model named in includedModels",
            model_enforcement: Some(json!({
                "includedModels": [OTHER_MODEL],
                "excludedModels": [],
            })),
            model_id: GUARDED_MODEL,
            banned_prompt: true,
            expected: None,
            selective_content_guarding: None,
        },
        // The invoke surface carries no `guardContent` marking, so a selective
        // guarding mode has nothing to narrow to there and the prompt is evaluated
        // whole: carrying the member cannot turn the account's control off.
        Row {
            case: "selective guarding of the messages, which this surface marks none of",
            model_enforcement: None,
            model_id: GUARDED_MODEL,
            banned_prompt: true,
            expected: Some(ACTION_INTERVENED),
            selective_content_guarding: Some(json!({ "messages": "SELECTIVE" })),
        },
        Row {
            case: "both guarding members, one mode each",
            model_enforcement: Some(json!({
                "includedModels": [ALL_MODELS],
                "excludedModels": [],
            })),
            model_id: OTHER_MODEL,
            banned_prompt: true,
            expected: Some(ACTION_INTERVENED),
            selective_content_guarding: Some(json!({
                "system": "SELECTIVE",
                "messages": "COMPREHENSIVE",
            })),
        },
    ]
}

/// A guardrail whose word policy blocks [`BANNED`], with one numbered version: the id and
/// the version an enforced configuration names.
async fn guardrail_blocking_the_word(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator
        .create(json!({
            "name": name,
            "blockedInputMessaging": BLOCKED_INPUT,
            "blockedOutputsMessaging": BLOCKED_OUTPUT,
            "wordPolicyConfig": { "wordsConfig": [{ "text": BANNED }] },
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case {name}: CreateGuardrail must answer 202, got {} {:?}",
        created.status, created.body
    );
    let id = created.text("guardrailId");
    let cut = emulator.create_version(&id, json!({})).await;
    assert_eq!(
        cut.status, 202,
        "case {name}: CreateGuardrailVersion must answer 202, got {} {:?}",
        cut.status, cut.body
    );
    (id, cut.text("version"))
}

/// `PUT /enforcedGuardrailsConfiguration` for `id` at `version`, with the row's model
/// enforcement and selective-guarding members; answers the `configId` it installed.
async fn install(
    emulator: &Emulator,
    case: &str,
    id: &str,
    version: &str,
    model_enforcement: Option<&Value>,
    selective_content_guarding: Option<&Value>,
) -> String {
    let mut config = json!({
        "guardrailIdentifier": id,
        "guardrailVersion": version,
    });
    if let Some(enforcement) = model_enforcement {
        config["modelEnforcement"] = enforcement.clone();
    }
    if let Some(guarding) = selective_content_guarding {
        config["selectiveContentGuarding"] = guarding.clone();
    }
    let installed = emulator
        .call(
            "PUT",
            CONFIGURATION_URI,
            Some(json!({ "guardrailInferenceConfig": config })),
        )
        .await;
    assert_eq!(
        installed.status, 200,
        "case {case:?}: the put must answer 200, got {} {:?}",
        installed.status, installed.body
    );
    let config_id = installed.text("configId");

    // The configuration is the one the account holds, which is what the listing reports.
    let listed = emulator.call("GET", CONFIGURATION_URI, None).await;
    let reported: Vec<Value> = listed
        .get("guardrailsConfig")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        reported
            .iter()
            .any(|row| row["configId"].as_str() == Some(config_id.as_str())),
        "case {case:?}: the installed configId must appear in the listing, got {:?}",
        listed.body
    );
    config_id
}

/// One `InvokeModel` call on `model_id`, carrying NO guardrail of its own.
async fn invoke(emulator: &Emulator, case: &str, model_id: &str, prompt: &str) -> Reply {
    let reply = emulator
        .call(
            "POST",
            &format!("/model/{model_id}/invoke"),
            Some(json!({
                "messages": [{ "role": "user", "content": [{ "text": prompt }] }],
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the invocation must be served, got {} {:?}",
        reply.status, reply.body
    );
    reply
}

/// The generated text an Amazon Nova response envelope carries.
fn served_text(case: &str, reply: &Reply) -> String {
    reply
        .body
        .pointer("/output/message/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the response carried no text: {:?}", reply.body))
        .to_string()
}

/// RULE: a configuration this control plane installed is applied by the data plane to an
/// invocation that names no guardrail, on the models the configuration covers and on no
/// others.
#[tokio::test]
async fn an_installed_configuration_is_applied_to_an_invocation_that_names_no_guardrail() {
    for (index, row) in rows().into_iter().enumerate() {
        // One account per row: an account holds a single configuration, so each row
        // installs its own rather than replacing the row before it.
        let emulator = emulator_with_inference();
        // `GuardrailName` is `[0-9a-zA-Z-_]+`, so the fixture names the guardrail by the
        // row's ordinal; the row's own prose stays in the assertion messages.
        let (id, version) =
            guardrail_blocking_the_word(&emulator, &format!("enforced-inference-{index}")).await;
        install(
            &emulator,
            row.case,
            &id,
            &version,
            row.model_enforcement.as_ref(),
            row.selective_content_guarding.as_ref(),
        )
        .await;

        let prompt = if row.banned_prompt {
            format!("Tell me about {BANNED} in one line.")
        } else {
            "Name two planets.".to_string()
        };
        let reply = invoke(&emulator, row.case, row.model_id, &prompt).await;
        let action = reply
            .body
            .get(GUARDRAIL_ACTION_FIELD)
            .and_then(Value::as_str);
        assert_eq!(
            action, row.expected,
            "case {:?}: expected the guardrail action {:?}, got {action:?} in {:?}",
            row.case, row.expected, reply.body
        );

        let text = served_text(row.case, &reply);
        if row.expected == Some(ACTION_INTERVENED) {
            assert_eq!(
                text, BLOCKED_INPUT,
                "case {:?}: a blocked invocation must answer the guardrail's \
                 blockedInputMessaging, got {text:?}",
                row.case
            );
        } else {
            assert!(
                text != BLOCKED_INPUT && !text.contains(BANNED),
                "case {:?}: an invocation the guardrail did not block must answer the model's \
                 own turn, got {text:?}",
                row.case
            );
        }
    }
}

/// RULE: the enforcement stands as long as the configuration does — deleting the
/// configuration withdraws it, and a further put replaces it with the guardrail the new
/// configuration names.
#[tokio::test]
async fn deleting_the_configuration_withdraws_the_enforcement() {
    let emulator = emulator_with_inference();
    let (blocking, blocking_version) =
        guardrail_blocking_the_word(&emulator, "withdrawn-blocking").await;
    // A second guardrail with NO word policy at all: enforcing it blocks nothing, which is
    // what makes the replacement observable.
    let permissive = emulator.create_named("withdrawn-permissive").await;
    let permissive_version = emulator
        .create_version(&permissive, json!({}))
        .await
        .text("version");

    let prompt = format!("Tell me about {BANNED} in one line.");
    let config_id = install(
        &emulator,
        "installed",
        &blocking,
        &blocking_version,
        None,
        None,
    )
    .await;
    let guarded = invoke(&emulator, "installed", GUARDED_MODEL, &prompt).await;
    assert_eq!(
        guarded
            .body
            .get(GUARDRAIL_ACTION_FIELD)
            .and_then(Value::as_str),
        Some(ACTION_INTERVENED),
        "case installed: the configuration must be in force, got {:?}",
        guarded.body
    );

    // A further put REPLACES the configuration, so what is enforced is the guardrail the
    // last admitted put named.
    install(
        &emulator,
        "replaced",
        &permissive,
        &permissive_version,
        None,
        None,
    )
    .await;
    let replaced = invoke(&emulator, "replaced", GUARDED_MODEL, &prompt).await;
    assert_eq!(
        replaced
            .body
            .get(GUARDRAIL_ACTION_FIELD)
            .and_then(Value::as_str),
        Some(ACTION_NONE),
        "case replaced: the guardrail now enforced blocks nothing, so the invocation reports \
         NONE, got {:?}",
        replaced.body
    );

    // The delete addresses the installed configuration by its id; an id the account does
    // not hold addresses nothing.
    let unknown = emulator
        .call(
            "DELETE",
            &format!("{CONFIGURATION_URI}/{config_id}0"),
            None,
        )
        .await;
    assert_eq!(
        unknown.status, 404,
        "case unknown configId: the delete must be refused 404, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case unknown configId: a configuration the account does not hold is a \
         ResourceNotFoundException, got {:?}",
        unknown.error
    );

    let deleted = emulator
        .call("DELETE", &format!("{CONFIGURATION_URI}/{config_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case deleted: the delete must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let listed = emulator.call("GET", CONFIGURATION_URI, None).await;
    assert_eq!(
        listed.get("guardrailsConfig"),
        json!([]),
        "case deleted: the account holds no configuration, so the listing is empty, got {:?}",
        listed.body
    );
    let withdrawn = invoke(&emulator, "deleted", GUARDED_MODEL, &prompt).await;
    assert_eq!(
        withdrawn.body.get(GUARDRAIL_ACTION_FIELD),
        None,
        "case deleted: an account that enforces nothing carries no action member, got {:?}",
        withdrawn.body
    );

    // And the delete is answered the same way when it is repeated: the configuration it
    // named is no longer there.
    let again = emulator
        .call("DELETE", &format!("{CONFIGURATION_URI}/{config_id}"), None)
        .await;
    assert_eq!(
        again.status, 404,
        "case deleted twice: the configuration is gone, so the delete is refused 404, got {} {:?}",
        again.status, again.body
    );
}
