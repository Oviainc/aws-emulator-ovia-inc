// SPDX-License-Identifier: CC0-1.0

//! The emulator every suite in this crate drives, the encoding its structured
//! headers carry, and the bot its turns run against.
//!
//! Each test builds the service over one shared engine, registers it with the
//! real [`MultiServiceRouter`], and dispatches buffered HTTP requests — so what
//! a suite asserts is wire behaviour (status, headers, body) and not a handler
//! called directly.
//!
//! The Lex Models V2 control plane is mounted on the SAME engine, because the
//! bot a call is served against is its resource: this runtime refuses a call
//! addressed to a bot alias it cannot resolve or to a locale with no built
//! model, so a suite that drives one needs a bot that was built. A suite about
//! the bot drives that control plane over HTTP like any client
//! ([`Emulator::built_bot`] is that sequence: `CreateBot`, `CreateBotLocale`,
//! `CreateIntent`, `CreateSlot`, `UpdateIntent`, `BuildBotLocale`); a suite
//! about the SESSION, where the bot is only a fixture addressed by the fixed
//! labels its table fixes, asks for one through [`Emulator::built_locale`].

use bytes::Bytes;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2 as lexmodels;
use smithy_server_runtime::MultiServiceRouter;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService;

/// The bot version every locale created here belongs to: the working copy, which
/// is what the alias Amazon Lex keeps for testing a bot points at.
pub(crate) const DRAFT: &str = "DRAFT";

/// An IAM role ARN satisfying the control plane's modelled `RoleArn`
/// (len 32..2048, pattern `^arn:aws:iam::[0-9]{12}:role/.*$`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/lex-demo";

/// The alias Amazon Lex V2 keeps for testing a bot, created with the bot — the
/// alias a bot created through the control plane is addressed through.
pub(crate) const TEST_ALIAS: &str = lexmodels::state::TEST_BOT_ALIAS_ID;

/// The intent [`Emulator::built_bot`] declares in each of its locales.
pub(crate) const INTENT: &str = "OrderPizza";

/// The one slot of [`INTENT`], and the prompt authored on it.
pub(crate) const SLOT: &str = "Size";
/// The message of the slot's `valueElicitationSetting.promptSpecification`.
pub(crate) const SLOT_PROMPT: &str = "What size pizza would you like?";
/// The message of the intent's `intentConfirmationSetting.promptSpecification`.
pub(crate) const CONFIRMATION_PROMPT: &str = "Shall I place the order?";
/// The message of the intent's `intentClosingSetting.closingResponse`.
pub(crate) const CLOSING_MESSAGE: &str = "Your order is on its way";

/// A bot the control plane created, with the locales it built for it.
#[derive(Debug, Clone)]
pub(crate) struct Bot {
    /// The `botId` the control plane minted.
    pub(crate) bot_id: String,
    /// The alias Amazon Lex keeps for testing the bot, created with it.
    pub(crate) alias_id: String,
    /// The locales whose model was built, in the order they were created.
    pub(crate) locales: Vec<String>,
}

impl Bot {
    /// The session URI for `session_id` in `locale_id`.
    pub(crate) fn session_path_in(&self, locale_id: &str, session_id: &str) -> String {
        format!(
            "/bots/{}/botAliases/{}/botLocales/{locale_id}/sessions/{session_id}",
            self.bot_id, self.alias_id
        )
    }

    /// The session URI for `session_id` in this bot's first built locale.
    pub(crate) fn session_path(&self, session_id: &str) -> String {
        self.session_path_in(self.locale(), session_id)
    }

    /// This bot's first built locale.
    pub(crate) fn locale(&self) -> &str {
        self.locales
            .first()
            .map(String::as_str)
            .expect("the fixture bot has at least one locale")
    }
}

/// The emulator under test: one shared engine, the runtime and the control plane
/// that holds its bots, driven through the router every front door uses.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the framework's business, not this suite's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(lexmodels::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The emulator plus one bot whose `en_US` locale the control plane built,
    /// declaring [`INTENT`] and its prompted slot — what a suite that needs a
    /// bot with content to talk to asks for.
    pub(crate) async fn with_bot() -> (Self, Bot) {
        let emulator = Self::new();
        let bot = emulator.built_bot("PizzaBot", &["en_US"]).await;
        (emulator, bot)
    }

    /// Give the bot, alias and locale those labels address a BUILT locale that
    /// declares no intent of its own, as a fixture for the suites whose subject
    /// is the session rather than the bot.
    ///
    /// The record is written into the control plane's own store, because that is
    /// where a bot lives: a locale `CreateBot` / `CreateBotLocale` /
    /// `BuildBotLocale` produced carries a MINTED bot id, and these suites
    /// address bots by the fixed labels their tables fix. A locale with no
    /// intents is also what keeps them independent of any bot's content:
    /// nothing can be matched in it, so every turn is the fallback intent,
    /// which is the case the real service answers the same way.
    pub(crate) fn built_locale(&self, bot_id: &str, bot_alias_id: &str, locale_id: &str) {
        let now = SystemTime::now();
        let control_plane = lexmodels::state::service_state(&*self.engine)
            .expect("the control plane's store is reachable through the shared engine");
        let mut control_plane = control_plane.lock();
        control_plane.bot_aliases.insert(
            lexmodels::state::bot_alias_key(bot_id, bot_alias_id),
            lexmodels::state::BotAlias {
                bot_id: bot_id.to_string(),
                bot_alias_id: bot_alias_id.to_string(),
                bot_alias_name: lexmodels::state::TEST_BOT_ALIAS_NAME.to_string(),
                description: None,
                bot_version: lexmodels::state::DRAFT_BOT_VERSION.to_string(),
                bot_alias_status: lexmodels::state::BOT_ALIAS_STATUS_AVAILABLE.to_string(),
                creation_date_time: now,
                last_updated_date_time: now,
            },
        );
        control_plane.bot_locales.insert(
            lexmodels::state::bot_locale_key(bot_id, lexmodels::state::DRAFT_BOT_VERSION, locale_id),
            lexmodels::state::BotLocale {
                bot_id: bot_id.to_string(),
                bot_version: lexmodels::state::DRAFT_BOT_VERSION.to_string(),
                locale_id: locale_id.to_string(),
                locale_name: lexmodels::state::locale_name(locale_id)
                    .unwrap_or_default()
                    .to_string(),
                description: None,
                // Inside the modelled `ConfidenceThreshold` range 0..1. The
                // locale declares no intent, so nothing is scored against it.
                nlu_intent_confidence_threshold: 0.4,
                voice_settings: None,
                unified_speech_settings: None,
                audio_filler_settings: None,
                speech_recognition_settings: None,
                generative_ai_settings: None,
                speech_detection_sensitivity: None,
                speaker_diarization_settings: None,
                bot_locale_status: lexmodels::state::BOT_LOCALE_STATUS_BUILT.to_string(),
                creation_date_time: now,
                last_updated_date_time: now,
                last_build_submitted_date_time: Some(now),
                failure_reasons: Vec::new(),
                pending_failure_reasons: Vec::new(),
                pending_status: None,
            },
        );
    }

    pub(crate) async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.send_with_headers(method, path, body, &[]).await
    }

    /// [`Self::send`] with extra request headers, so a member the model binds
    /// with `@httpHeader` is exercised through the binding an SDK client uses
    /// rather than through the body.
    pub(crate) async fn send_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        self.send_raw(
            method,
            path,
            Bytes::from(payload),
            "application/json",
            headers,
        )
        .await
    }

    /// Dispatch a request whose body is NOT JSON: the `@httpPayload` blob an
    /// operation like `RecognizeUtterance` binds, under the content type the
    /// request declares for it.
    pub(crate) async fn send_raw(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        content_type: &str,
        headers: &[(&str, &str)],
    ) -> Reply {
        self.dispatch(
            method,
            path,
            body,
            content_type,
            headers,
            "runtime-v2-lex.us-east-1.amazonaws.com",
        )
        .await
    }

    /// Dispatch one request to the Lex Models V2 control plane, which answers on
    /// its own endpoint host.
    pub(crate) async fn send_control_plane(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        self.dispatch(
            method,
            path,
            Bytes::from(payload),
            "application/json",
            &[],
            "models-v2-lex.us-east-1.amazonaws.com",
        )
        .await
    }

    async fn dispatch(
        &self,
        method: &str,
        path: &str,
        body: Bytes,
        content_type: &str,
        headers: &[(&str, &str)],
        host: &str,
    ) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            // The routing key: the SigV4 credential scope names the `lex`
            // signing name, which is how a request reaches this service.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260102/us-east-1/lex/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            )
            .header("host", host)
            .header("content-type", content_type);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(body)
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.into_body();
        Reply {
            status,
            headers,
            body,
        }
    }

    // --- the control plane, driven as a client drives it ----------------------

    /// Create a bot named `name` and answer the `botId` the control plane
    /// minted.
    pub(crate) async fn create_bot(&self, name: &str) -> String {
        let reply = self
            .send_control_plane(
                "PUT",
                "/bots",
                Some(json!({
                    "botName": name,
                    "roleArn": ROLE_ARN,
                    "dataPrivacy": {"childDirected": false},
                    "idleSessionTTLInSeconds": 300,
                })),
            )
            .await;
        assert_eq!(
            reply.status,
            202,
            "CreateBot {name:?}: expected 202, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        reply
            .json()
            .get("botId")
            .and_then(Value::as_str)
            .expect("CreateBot reports the botId it minted")
            .to_string()
    }

    /// Add a locale to `bot_id`'s working copy, WITHOUT building it.
    pub(crate) async fn create_locale(&self, bot_id: &str, locale_id: &str) {
        let reply = self
            .send_control_plane(
                "PUT",
                &format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales"),
                Some(json!({
                    "localeId": locale_id,
                    "nluIntentConfidenceThreshold": 0.40,
                })),
            )
            .await;
        assert_eq!(
            reply.status,
            202,
            "CreateBotLocale {locale_id:?}: expected 202, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    /// Declare [`INTENT`] in that locale, with one required, prompted slot
    /// ([`SLOT`]), a confirmation prompt and a closing response — a locale
    /// Amazon Lex can build.
    pub(crate) async fn create_intent_with_slot(&self, bot_id: &str, locale_id: &str) {
        let intents = format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{locale_id}/intents");
        let authored = json!({
            "intentName": INTENT,
            "sampleUtterances": [{"utterance": "I want to order a pizza"}],
            "intentConfirmationSetting": {
                "promptSpecification": {
                    "messageGroups": [{
                        "message": {"plainTextMessage": {"value": CONFIRMATION_PROMPT}},
                    }],
                    "maxRetries": 2,
                },
            },
            "intentClosingSetting": {
                "closingResponse": {
                    "messageGroups": [{
                        "message": {"plainTextMessage": {"value": CLOSING_MESSAGE}},
                    }],
                },
            },
        });
        let created = self
            .send_control_plane("PUT", &intents, Some(authored.clone()))
            .await;
        assert_eq!(
            created.status,
            200,
            "CreateIntent in {locale_id:?}: expected 200, got {} {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let intent_id = created
            .json()
            .get("intentId")
            .and_then(Value::as_str)
            .expect("CreateIntent reports the intentId it minted")
            .to_string();

        let slot = self
            .send_control_plane(
                "PUT",
                &format!("{intents}/{intent_id}/slots"),
                Some(json!({
                    "slotName": SLOT,
                    // A built-in slot type, so the fixture needs no slot type of
                    // its own to elicit values against.
                    "slotTypeId": "AMAZON.AlphaNumeric",
                    "valueElicitationSetting": {
                        "slotConstraint": "Required",
                        "promptSpecification": {
                            "messageGroups": [{
                                "message": {"plainTextMessage": {"value": SLOT_PROMPT}},
                            }],
                            "maxRetries": 2,
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            slot.status,
            200,
            "CreateSlot in {locale_id:?}: expected 200, got {} {:?}",
            slot.status,
            String::from_utf8_lossy(&slot.body)
        );
        let slot_id = slot
            .json()
            .get("slotId")
            .and_then(Value::as_str)
            .expect("CreateSlot reports the slotId it minted")
            .to_string();

        // Every slot of an intent has to be ordered by that intent's
        // `slotPriorities` for the locale to build, and ordering them is
        // `UpdateIntent`'s — which replaces the intent, so everything authored
        // above is sent again with the priority.
        let mut update = authored;
        update["slotPriorities"] = json!([{"priority": 1, "slotId": slot_id}]);
        let updated = self
            .send_control_plane("PUT", &format!("{intents}/{intent_id}"), Some(update))
            .await;
        assert_eq!(
            updated.status,
            200,
            "UpdateIntent in {locale_id:?}: expected 200, got {} {:?}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
    }

    /// Submit a build for that locale and let the transition the clock owes fall
    /// due, so the locale is in its built state when the next read settles it.
    pub(crate) async fn build_locale(&self, bot_id: &str, locale_id: &str) {
        let reply = self
            .send_control_plane(
                "POST",
                &format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{locale_id}"),
                None,
            )
            .await;
        assert_eq!(
            reply.status,
            202,
            "BuildBotLocale {locale_id:?}: expected 202, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        self.settle_control_plane();
    }

    /// Move every status transition the control plane has scheduled to due NOW.
    ///
    /// The ONE thing written into the control plane's store by hand here: the
    /// clock. A created bot settles and a submitted build settles after a span
    /// that service chooses, and no operation exposes that instant, so a suite
    /// that would otherwise sleep for it moves it instead.
    pub(crate) fn settle_control_plane(&self) {
        let state = lexmodels::state::service_state(&*self.engine)
            .expect("the control plane's store is reachable through the shared engine");
        let mut store = state.lock();
        let now = Instant::now();
        for bot in store.bots.values_mut() {
            if let Some((status, _)) = bot.pending_status.take() {
                bot.pending_status = Some((status, now));
            }
            bot.settle();
        }
        for locale in store.bot_locales.values_mut() {
            if let Some((status, _)) = locale.pending_status.take() {
                locale.pending_status = Some((status, now));
            }
            locale.settle();
        }
    }

    /// Create a bot with a BUILT locale per entry of `locales`, each declaring
    /// [`INTENT`] and its prompted slot — the whole sequence a client drives.
    pub(crate) async fn built_bot(&self, name: &str, locales: &[&str]) -> Bot {
        let bot_id = self.create_bot(name).await;
        for locale_id in locales {
            self.create_locale(&bot_id, locale_id).await;
            self.create_intent_with_slot(&bot_id, locale_id).await;
            self.build_locale(&bot_id, locale_id).await;
        }
        Bot {
            bot_id,
            alias_id: TEST_ALIAS.to_string(),
            locales: locales.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    /// Add a second alias of `bot`, pointing at the same working copy, and
    /// answer the bot as that alias addresses it.
    ///
    /// Seeded into the control plane's own store because its `CreateBotAlias`
    /// is still a 501 scaffold: the record is the one `BotAlias` that service
    /// builds for an alias of a bot, so what the runtime resolves is the control
    /// plane's own state either way.
    pub(crate) fn second_alias(&self, bot: &Bot, alias_id: &str) -> Bot {
        let state = lexmodels::state::service_state(&*self.engine)
            .expect("the control plane's store is reachable through the shared engine");
        let mut alias = lexmodels::state::BotAlias::test_alias(&bot.bot_id, SystemTime::now());
        alias.bot_alias_id = alias_id.to_string();
        alias.bot_alias_name = format!("{alias_id}Name");
        state
            .lock()
            .bot_aliases
            .insert(lexmodels::state::bot_alias_key(&bot.bot_id, alias_id), alias);
        Bot {
            bot_id: bot.bot_id.clone(),
            alias_id: alias_id.to_string(),
            locales: bot.locales.clone(),
        }
    }

    /// The build state the control plane reports for that locale, as
    /// `DescribeBotLocale` answers it.
    pub(crate) async fn bot_locale_status(&self, bot_id: &str, locale_id: &str) -> String {
        let reply = self
            .send_control_plane(
                "GET",
                &format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{locale_id}"),
                None,
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "DescribeBotLocale {locale_id:?}: expected 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        reply
            .json()
            .get("botLocaleStatus")
            .and_then(Value::as_str)
            .expect("DescribeBotLocale reports the locale's status")
            .to_string()
    }
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: Bytes,
}

impl Reply {
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| v.to_str().expect("header is ASCII").to_string())
    }

    /// The error shape a failed call names, from the restJson1 `__type`.
    pub(crate) fn error_type(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string()
    }

    /// The `Message` a failed call carries, as restJson1 renders an error's
    /// `@error` member.
    pub(crate) fn error_message(&self) -> String {
        let body = self.json();
        ["message", "Message"]
            .iter()
            .find_map(|name| body.get(*name).and_then(Value::as_str))
            .unwrap_or("<no message>")
            .to_string()
    }

    /// Decode one `x-amz-lex-*` header carrying a structured payload: the
    /// documented "compressed with gzip and then base64 encoded" form of the
    /// value's JSON.
    pub(crate) fn lex_header(&self, name: &str) -> Option<Value> {
        let payload = self.lex_header_text(name)?;
        Some(
            serde_json::from_str(&payload)
                .unwrap_or_else(|e| panic!("header {name} is not JSON ({e}): {payload}")),
        )
    }

    /// Decode one `x-amz-lex-*` header to the text it carries, under the same
    /// encoding ([`un_gzip_base64`]).
    pub(crate) fn lex_header_text(&self, name: &str) -> Option<String> {
        let encoded = self.header(name)?;
        Some(un_gzip_base64(name, &encoded))
    }
}

/// gzip then base64, the encoding this service's structured headers carry on
/// the REQUEST side too ("the `sessionState` field must be compressed using gzip
/// and then base64 encoded before sending to Amazon Lex V2").
pub(crate) fn gzip_base64(payload: &str) -> String {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(payload.as_bytes())
        .expect("writing to a vector cannot fail");
    let compressed = encoder.finish().expect("finishing a vector cannot fail");
    aws_smithy_types::base64::encode(compressed)
}

/// base64 then gunzip: the inverse of [`gzip_base64`], panicking with the
/// header's name when the value is not that encoding.
pub(crate) fn un_gzip_base64(name: &str, encoded: &str) -> String {
    let compressed = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|e| panic!("header {name} is not base64 ({e}): {encoded}"));
    let mut payload = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut payload)
        .unwrap_or_else(|e| panic!("header {name} is not gzipped ({e}): {encoded}"));
    String::from_utf8(payload)
        .unwrap_or_else(|e| panic!("header {name} does not decode to UTF-8 ({e}): {encoded}"))
}
