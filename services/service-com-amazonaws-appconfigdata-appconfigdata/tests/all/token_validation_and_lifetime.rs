// SPDX-License-Identifier: CC0-1.0

//! What a `GetLatestConfiguration` request has to supply for its token to be judged at
//! all, and how long the token it supplies stays good for — driven end to end over HTTP
//! through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine.
//!
//! Two rules, neither of them about one token value or one payload:
//!
//! * a request that supplies NO usable value for a `@required` input member is refused
//!   as the validation failure it is, naming that member and where its value comes from,
//!   before anything is looked up in state — so the refusal carries no verdict about a
//!   value the caller never sent. A verdict (the modelled
//!   `Details.InvalidParameters[…].Problem`) is reserved for a value that WAS supplied,
//!   which is what keeps "I sent no token" distinguishable from "the token I sent means
//!   nothing here";
//! * a configuration token is bounded by TIME and by nothing else. The model documents
//!   the bound on the member itself — "To support long poll use cases, the token is valid
//!   for up to 24 hours. If a `GetLatestConfiguration` call uses an expired token, the
//!   system returns `BadRequestException`" — so a token inside its lifetime is served
//!   however many polls it has already served, and only a token past its lifetime is
//!   refused `Expired`. "This token should only be used once" is guidance about which
//!   token a client should present NEXT, not a second expiry.
//!
//! The one input no operation takes is the passage of time, so a token's expiry instant
//! is moved in the session store rather than waited out: the answer the handler computes
//! from it is the same either way.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_appconfigdata_appconfigdata::state::DEFAULT_POLL_INTERVAL_IN_SECONDS;

use super::configuration_session_poll::{is_modelled_token, Emulator, Hierarchy, Reply};

/// The sentence AppConfigData answers a request that supplied no token with. It names
/// the input member and the operation that mints a token, and interpolates nothing from
/// the request.
const TOKEN_REQUIRED_MESSAGE: &str = "ConfigurationToken must be provided. \
     Use the StartConfigurationSession API to obtain an InitialConfigurationToken";

/// How the refusal is expected to read for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// No usable value was supplied for the member: the missing-parameter refusal, with
    /// no verdict attached.
    MissingParameter,
    /// A value was supplied and resolves to no session: the modelled `Corrupted` verdict
    /// on the token that was presented.
    CorruptedVerdict,
}

/// One `configuration_token` query parameter, as it travels on the wire.
struct TokenParameterCase {
    name: &'static str,
    /// The whole query string, or `None` for a request that carries no
    /// `configuration_token` parameter at all.
    query: Option<&'static str>,
    expected: Expected,
}

/// The rows differ along what the rule generalises over: whether the parameter is there
/// at all, and — when it is — whether its value supplies any character a token could be
/// made of. Each whitespace form is a distinct way of spelling "blank" on the wire
/// (`%20` a space, `%09` a tab, `%0A` a newline, `+` a space per
/// application/x-www-form-urlencoded), and none of them can satisfy the modelled `Token`
/// pattern `^\S{1,8192}$` (smithy_query query="shape" shape="Token"). The last rows are
/// the other side of the rule: a value that really was supplied earns a verdict.
const TOKEN_PARAMETER_CASES: &[TokenParameterCase] = &[
    TokenParameterCase {
        name: "no configuration_token parameter at all",
        query: None,
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "the parameter present with an empty value",
        query: Some("configuration_token="),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "a single space",
        query: Some("configuration_token=%20"),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "a tab",
        query: Some("configuration_token=%09"),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "spaces and tabs together",
        query: Some("configuration_token=%20%09%20"),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "a newline",
        query: Some("configuration_token=%0A"),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "a form-encoded space",
        query: Some("configuration_token=+"),
        expected: Expected::MissingParameter,
    },
    TokenParameterCase {
        name: "a value this emulator never minted",
        query: Some("configuration_token=not-a-real-token"),
        expected: Expected::CorruptedVerdict,
    },
    TokenParameterCase {
        name: "a hex-shaped value this emulator never minted",
        query: Some(
            "configuration_token=\
             0000000000000000000000000000000000000000000000000000000000000000",
        ),
        expected: Expected::CorruptedVerdict,
    },
    TokenParameterCase {
        name: "a value with a space inside it",
        query: Some("configuration_token=not%20a%20token"),
        expected: Expected::CorruptedVerdict,
    },
];

/// A request that supplies no usable `ConfigurationToken` is refused as a missing
/// parameter — the modelled `BadRequestException` (400) whose `Message` names the member
/// and the operation that mints a token, carrying NO `Reason` and no `Details` union,
/// because there is no presented token for a verdict to be about. A request that does
/// supply a value gets the verdict instead.
#[tokio::test]
async fn a_request_that_supplies_no_usable_token_is_refused_as_a_missing_parameter() {
    let emulator = Emulator::new();
    // A hierarchy with a deployed configuration exists throughout, so no row is refused
    // merely because there is nothing to serve.
    let hierarchy = emulator
        .create_hierarchy("MyApp", "Prod", "MyProfile")
        .await;
    emulator
        .deploy(
            &hierarchy,
            &hierarchy.profile_id,
            br#"{"feature":"on"}"#,
            "application/json",
            Some("v-one"),
        )
        .await;

    for case in TOKEN_PARAMETER_CASES {
        let name = case.name;
        let reply = emulator.get_configuration(case.query).await;
        assert_eq!(
            reply.status,
            400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.header("x-amzn-errortype"),
            Some("BadRequestException"),
            "case {name}: expected the error type header, got {:?}",
            reply.header("x-amzn-errortype")
        );
        assert_eq!(
            reply.header("next-poll-configuration-token"),
            None,
            "case {name}: a refusal rotates no token, got {:?}",
            reply.header("next-poll-configuration-token")
        );
        let body = reply.json();
        match case.expected {
            Expected::MissingParameter => {
                assert_eq!(
                    body["Message"].as_str(),
                    Some(TOKEN_REQUIRED_MESSAGE),
                    "case {name}: expected the missing-parameter sentence, got {}",
                    reply.text()
                );
                assert!(
                    body["Details"].is_null(),
                    "case {name}: a request that presented no token must carry no verdict \
                     about one, got {}",
                    reply.text()
                );
                assert!(
                    body["Reason"].is_null(),
                    "case {name}: a missing parameter is not a parameter the service could \
                     not use, so no Reason is reported, got {}",
                    reply.text()
                );
            }
            Expected::CorruptedVerdict => {
                assert_eq!(
                    body["Reason"].as_str(),
                    Some("InvalidParameters"),
                    "case {name}: expected the modelled Reason, got {}",
                    reply.text()
                );
                assert_eq!(
                    body["Details"]["InvalidParameters"]["ConfigurationToken"]["Problem"].as_str(),
                    Some("Corrupted"),
                    "case {name}: expected the verdict on the presented token, got {}",
                    reply.text()
                );
                assert_ne!(
                    body["Message"].as_str(),
                    Some(TOKEN_REQUIRED_MESSAGE),
                    "case {name}: a presented token must not be answered as an absent one, \
                     got {}",
                    reply.text()
                );
            }
        }
    }
}

/// One session, polled with a token it keeps re-presenting. The rows differ along the
/// poll floor the session negotiated (none, and each end of the modelled
/// `range 15..86400`) and along the form, size and content type of the configuration,
/// because none of that changes how long a token lasts.
struct LifetimeCase {
    name: &'static str,
    /// `RequiredMinimumPollIntervalInSeconds` the session asks for; `None` for a session
    /// that asks for no floor.
    requested: Option<i32>,
    /// The interval every served poll must report, which is also the one it enforces.
    reported: i32,
    content: &'static [u8],
    content_type: &'static str,
    version_label: Option<&'static str>,
}

const LIFETIME_CASES: &[LifetimeCase] = &[
    LifetimeCase {
        name: "no floor negotiated, json document",
        requested: None,
        reported: DEFAULT_POLL_INTERVAL_IN_SECONDS,
        content: br#"{"feature":"on"}"#,
        content_type: "application/json",
        version_label: Some("v-one"),
    },
    LifetimeCase {
        name: "the modelled minimum floor, plain text",
        requested: Some(15),
        reported: 15,
        content: b"on",
        content_type: "text/plain",
        version_label: None,
    },
    LifetimeCase {
        name: "a one-minute floor, yaml document",
        requested: Some(60),
        reported: 60,
        content: b"feature: on\nextra: 2\n",
        content_type: "application/x-yaml",
        version_label: Some("release-1"),
    },
    LifetimeCase {
        name: "the modelled maximum floor, non-utf8 blob",
        requested: Some(86_400),
        reported: 86_400,
        content: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        content_type: "application/octet-stream",
        version_label: Some("binary-1"),
    },
];

/// A token inside its lifetime is served however many polls it has already served: the
/// initial token of a session still answers 200 with the deployed configuration and a
/// freshly rotated successor after the poll that first used it, and again after that.
/// Only the lifetime ends it — one margin short of the expiry the token is still served,
/// and at it the poll is refused `Expired`.
#[tokio::test]
async fn a_token_inside_its_lifetime_is_served_however_often_it_has_been_presented() {
    for case in LIFETIME_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                case.content,
                case.content_type,
                case.version_label,
            )
            .await;
        let initial = start_session(&emulator, &hierarchy, case.requested).await;

        // Three presentations of the SAME token: the first poll, then two
        // re-presentations of a token the client still holds — the documented recovery
        // after a poll whose response it failed to persist. Each is served, each rotates
        // a fresh successor, and each reports the obligation the session negotiated.
        let mut rotated: Vec<String> = Vec::new();
        for presentation in [
            "the first poll",
            "the first re-presentation",
            "the second re-presentation",
        ] {
            let reply = emulator.get_latest_configuration(&initial).await;
            assert_eq!(
                reply.status,
                200,
                "case {name}: {presentation} expected 200, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.body.as_ref(),
                case.content,
                "case {name}: {presentation} must serve the deployed bytes, got {:?}",
                reply.body.as_ref()
            );
            assert_eq!(
                reply.header("content-type"),
                Some(case.content_type),
                "case {name}: {presentation} must report the deployed content type, got {:?}",
                reply.header("content-type")
            );
            assert_eq!(
                reply.header("version-label"),
                case.version_label,
                "case {name}: {presentation} must report the deployed version label, got {:?}",
                reply.header("version-label")
            );
            let reported = case.reported.to_string();
            assert_eq!(
                reply.header("next-poll-interval-in-seconds"),
                Some(reported.as_str()),
                "case {name}: {presentation} must report the negotiated interval, got {:?}",
                reply.header("next-poll-interval-in-seconds")
            );
            let next = reply.next_token();
            assert!(
                is_modelled_token(&next) && next != initial && !rotated.contains(&next),
                "case {name}: {presentation} must rotate a fresh, well-formed token, got \
                 {next:?}"
            );
            rotated.push(next);
        }

        // The successor the FIRST poll handed back is untouched by the re-presentations:
        // a session with no floor may present it at once, and it answers with an empty
        // payload because that token already holds the deployed version.
        if case.requested.is_none() {
            let successor = emulator.get_latest_configuration(&rotated[0]).await;
            assert_eq!(
                successor.status,
                200,
                "case {name}: the first successor must still be usable, got {} body {}",
                successor.status,
                successor.text()
            );
            assert!(
                successor.body.is_empty(),
                "case {name}: the first successor already holds the deployed version, so its \
                 payload is empty, got {:?}",
                successor.body.as_ref()
            );
        }

        // One margin short of its expiry the token is still inside its lifetime, so it is
        // still served.
        set_expiry(&emulator, &initial, SystemTime::now() + MARGIN);
        let inside = emulator.get_latest_configuration(&initial).await;
        assert_eq!(
            inside.status,
            200,
            "case {name}: a token one margin short of its expiry is still served, got {} \
             body {}",
            inside.status,
            inside.text()
        );

        // At its expiry it is refused, with the modelled verdict for a token past its
        // lifetime — and nothing is served.
        set_expiry(&emulator, &initial, SystemTime::now());
        let expired = emulator.get_latest_configuration(&initial).await;
        assert_expired(&expired, name);
    }
}

/// The whole refusal owed to a token past its lifetime: HTTP 400, the modelled code and
/// reason, the `Expired` discriminator under the input member, and no rotated token.
fn assert_expired(reply: &Reply, name: &str) {
    assert_eq!(
        reply.status,
        400,
        "case {name}: a token past its lifetime expected 400, got {} body {}",
        reply.status,
        reply.text()
    );
    assert_eq!(
        reply.error_code(),
        "BadRequestException",
        "case {name}: expected BadRequestException, got {}",
        reply.text()
    );
    let body = reply.json();
    assert_eq!(
        body["Reason"].as_str(),
        Some("InvalidParameters"),
        "case {name}: expected the modelled Reason, got {}",
        reply.text()
    );
    assert_eq!(
        body["Details"]["InvalidParameters"]["ConfigurationToken"]["Problem"].as_str(),
        Some("Expired"),
        "case {name}: expected the Expired verdict, got {}",
        reply.text()
    );
    assert_eq!(
        reply.header("next-poll-configuration-token"),
        None,
        "case {name}: a refusal rotates no token, got {:?}",
        reply.header("next-poll-configuration-token")
    );
}

/// How much of a token's lifetime is left when it is presented "just before" its expiry.
///
/// A request through the harness takes a noticeable fraction of a second and the clock
/// keeps running while it does, so the slice left has to be wider than one dispatch for
/// the poll to still be inside the lifetime when the handler reads the clock. It is a
/// property of the harness, not of the rule: the rule is about the boundary, which the
/// refused poll above sits exactly on.
const MARGIN: Duration = Duration::from_secs(3);

/// Set the instant `token` stops being usable, as if its lifetime were running out now.
///
/// The lifetime is a day long and the passage of time is the one input no operation
/// takes, so the stored instant is moved instead of waited for.
fn set_expiry(emulator: &Emulator, token: &str, expires_at: SystemTime) {
    let state = emulator.state();
    let mut store = state.lock();
    let session = store
        .sessions
        .get_mut(token)
        .expect("the session is still stored under the token the client holds");
    session.expires_at = expires_at;
}

/// Start a session over `hierarchy`, asking for `requested` as the minimum poll
/// interval, and answer its initial token.
async fn start_session(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    requested: Option<i32>,
) -> String {
    let minimum = requested
        .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
        .unwrap_or_default();
    let started = emulator
        .post_json(
            "/configurationsessions",
            &format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"{minimum}}}"#,
                application = hierarchy.application_name,
                environment = hierarchy.environment_name,
                profile = hierarchy.profile_name,
            ),
        )
        .await;
    assert_eq!(
        started.status,
        201,
        "StartConfigurationSession({requested:?}): expected 201, got {} body {}",
        started.status,
        started.text()
    );
    started.string("InitialConfigurationToken")
}
