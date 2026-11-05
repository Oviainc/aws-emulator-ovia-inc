// SPDX-License-Identifier: CC0-1.0

//! The GUI (Amazon DCV) session of a virtual computer: its status, the access
//! details a start hands out, and what a stop leaves behind.

use super::*;

/// The modelled `Status` values a virtual computer's GUI session reports, from
/// the `Status` enum (`crate::ops::types::STATUS_VALUES`).
///
/// `notStarted` is reported when no session is up and nothing is bringing one
/// up: a running virtual computer whose session nobody has opened yet — opening
/// one is `StartGUISession`'s business ("Use StartGUISession to open the
/// session", the modelled `CreateGUISessionAccessDetails` documentation) — and
/// every state of a computer that is not running, because an Amazon DCV session
/// runs ON that computer.
pub const GUI_SESSION_STATUS_NOT_STARTED: &str = "notStarted";
/// The virtual computer the session would run on is still being set up, so the
/// session cannot be brought up yet.
pub const GUI_SESSION_STATUS_SETTING_UP_INSTANCE: &str = "settingUpInstance";
/// The session's access details exist and the session is coming up.
pub const GUI_SESSION_STATUS_STARTING: &str = "starting";
/// The session is serving: its two URLs reach a session that is up.
pub const GUI_SESSION_STATUS_STARTED: &str = "started";
/// The session is being terminated: the close a `StopGUISession` call asked for
/// is still in flight, so the session is on its way down.
pub const GUI_SESSION_STATUS_STOPPING: &str = "stopping";
/// The session has been terminated — "The session will close and any unsaved
/// data will be lost" (the modelled `StopGUISession` documentation) — so the
/// URLs it was published under reach nothing and only a start brings a session
/// back on that virtual computer.
pub const GUI_SESSION_STATUS_STOPPED: &str = "stopped";
/// The session's active window ([`GUI_SESSION_ACTIVE_MILLIS`]) has run out. The
/// caller resumes it with `StartGUISession` — "Use this action to resume the
/// session after it expires" (the modelled `StartGUISession` documentation) —
/// which is why creating access details does NOT silently start another hour:
/// that call reports a session, it does not open one.
pub const GUI_SESSION_STATUS_START_EXPIRED: &str = "startExpired";

/// How long a GUI session stays active once it has started.
///
/// AWS DOCUMENTED, not chosen: "The session will be active for 1 hour" — the
/// modelled `StartGUISession` documentation — so the window keeps that value
/// rather than being shortened to something a test can wait out.
pub const GUI_SESSION_ACTIVE_MILLIS: i64 = 60 * 60 * 1_000;

/// The name of the PRIMARY session of the pair this service creates: "The
/// primary URL initiates a web-based Amazon DCV session to the virtual
/// computer's application" (the modelled `CreateGUISessionAccessDetails`
/// documentation), so the primary session is the APPLICATION one.
pub const GUI_SESSION_NAME_APPLICATION: &str = "application";

/// The name of the secondary session: "The secondary URL initiates a web-based
/// Amazon DCV session to the virtual computer's operating session" — the
/// operating system of the virtual computer, from the same documentation.
pub const GUI_SESSION_NAME_OPERATING_SYSTEM: &str = "operating-system";

/// The path space a virtual computer's GUI session URLs are published under,
/// beneath the endpoint the emulator is reached on.
///
/// `Session$url` is "The session URL", which AWS publishes in its own Amazon DCV
/// zone. This emulator owns no DNS zone, and a literal AWS hostname would send
/// the next client out of the emulator entirely (see [`request_endpoint`]), so
/// the address names the endpoint THIS request arrived at and carries the
/// session it addresses in the path — the same convention a container service's
/// public URL follows (see [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`]). The prefix
/// names this service and this resource, so it cannot collide with another
/// service's modelled URI space.
///
/// What the address identifies is real — the session this store created, under
/// the endpoint the caller reached — but the Amazon DCV session PROTOCOL is not
/// emulated: nothing here serves a remote desktop, so following the URL is not
/// answered by this service. The URL is the control-plane value the operation
/// models, not a promise of a GUI.
pub const GUI_SESSION_PATH_PREFIX: &str = "/lightsail-gui-session";

/// The URL of the `session_name` session of the virtual computer
/// `resource_name`, whose access details carry `token`, as announced to a caller
/// that reached this emulator at `endpoint` (its `Host`/`:authority`).
///
/// The token is in the address because the URL addresses ONE session of the
/// pair this call created, not the virtual computer: a URL created for an
/// earlier session is a different address.
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn gui_session_url(
    endpoint: &str,
    resource_name: &str,
    token: &str,
    session_name: &str,
) -> String {
    format!("https://{endpoint}{GUI_SESSION_PATH_PREFIX}/{resource_name}/{token}/{session_name}")
}

/// How far through the span `start_ms`..`end_ms` the instant `now_ms` is, as a
/// whole percentage in `0..=100`.
///
/// This is what a `percentageComplete` member reports: a span that has not begun
/// is 0, one that is over is 100, and a span of zero or negative length is
/// complete (there is nothing left to wait for).
pub fn elapsed_percentage(start_ms: i64, end_ms: i64, now_ms: i64) -> i32 {
    let span = end_ms.saturating_sub(start_ms);
    if span <= 0 {
        return 100;
    }
    let elapsed = now_ms.saturating_sub(start_ms).clamp(0, span);
    i32::try_from(elapsed.saturating_mul(100) / span).unwrap_or(100)
}

/// The termination of one GUI session a caller asked for: when the close was
/// asked for, and the instant the clock owes the session its `stopped` status.
///
/// A close is not instantaneous — `stopping` is a modelled `Status` member, so a
/// session on its way down is a state AWS reports — and it takes
/// [`STATE_TRANSITION_MILLIS`], the same span every other transition in this
/// store takes, so a caller that reads straight after the call still observes it.
#[derive(Debug, Clone, Copy)]
pub struct GuiSessionStop {
    /// The instant the close was asked for, which the progress of the close is
    /// measured from.
    pub requested_at_ms: i64,
    /// The instant the clock owes the session its `stopped` status.
    pub at_ms: i64,
}

impl GuiSessionStop {
    /// A close asked for at `now_ms`, due [`STATE_TRANSITION_MILLIS`] later.
    pub fn after(now_ms: i64) -> Self {
        Self {
            requested_at_ms: now_ms,
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
        }
    }

    /// Whether the session this close belongs to has reached `stopped` by
    /// `now_ms`.
    pub fn is_complete_at(&self, now_ms: i64) -> bool {
        self.at_ms <= now_ms
    }

    /// How far through this close the instant `now_ms` is, as a whole
    /// percentage — what `percentageComplete` reports while a session is on its
    /// way down, and 100 once it is down.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        elapsed_percentage(self.requested_at_ms, self.at_ms, now_ms)
    }
}

/// The GUI (Amazon DCV) session access this store has created for one virtual
/// computer: the token its URLs are addressed by, and the instants that decide
/// the session's status — when it finishes coming up, when its active window runs
/// out, and, once a caller has terminated it, when it finishes closing.
///
/// The URLs themselves are NOT stored: they are built per request from the
/// endpoint that request arrived at (see [`gui_session_url`]), so a caller that
/// reaches the emulator on a different address is told an address it can reach.
#[derive(Debug, Clone)]
pub struct StoredGuiSession {
    /// The token both of this session pair's URLs carry, minted by the store's
    /// own allocator ([`ComAmazonawsLightsailLightsail20161128State::mint_uuid`]).
    pub token: String,
    /// When the access details were created.
    pub created_at_ms: i64,
    /// The instant the clock owes the session its `started` status.
    pub starts_at_ms: i64,
    /// The close a caller asked for on this session, once one has been asked
    /// for: `None` for a session nothing has terminated.
    pub stop: Option<GuiSessionStop>,
}

impl StoredGuiSession {
    /// Access details minted at `now_ms` carrying `token`, for a session that is
    /// still coming up.
    ///
    /// The session is `starting` for [`STATE_TRANSITION_MILLIS`] — a non-zero
    /// span, so a caller that reads straight after the call still observes the
    /// transient status — and `started` after it.
    pub fn starting(token: String, now_ms: i64) -> Self {
        Self::starting_once_ready(token, now_ms, now_ms)
    }

    /// Access details minted at `now_ms` carrying `token`, for a session on a
    /// virtual computer that is itself serving from `ready_at_ms`.
    ///
    /// A session runs ON the computer, so it cannot be up before the computer
    /// is: the session comes up [`STATE_TRANSITION_MILLIS`] after the LATER of
    /// the two instants, which is `now_ms` for a computer that is already
    /// running and the instant the clock owes that computer its running state
    /// for one that is still coming up.
    pub fn starting_once_ready(token: String, now_ms: i64, ready_at_ms: i64) -> Self {
        Self {
            token,
            created_at_ms: now_ms,
            starts_at_ms: ready_at_ms
                .max(now_ms)
                .saturating_add(STATE_TRANSITION_MILLIS),
            stop: None,
        }
    }

    /// The instant this session's active window runs out: an hour after it
    /// started (see [`GUI_SESSION_ACTIVE_MILLIS`]).
    pub fn expires_at_ms(&self) -> i64 {
        self.starts_at_ms.saturating_add(GUI_SESSION_ACTIVE_MILLIS)
    }

    /// Whether this session is serving, or still on its way to serving, at
    /// `now_ms`.
    ///
    /// A session a caller has terminated is NOT live however much of its active
    /// window is left: the close is what ended it, so the next start resumes the
    /// computer's session as a new one rather than handing back this address.
    pub fn is_live_at(&self, now_ms: i64) -> bool {
        self.stop.is_none() && now_ms < self.expires_at_ms()
    }

    /// The modelled `Status` this session is in at `now_ms`: `starting` until it
    /// has come up, `started` for its active hour, and `startExpired` once that
    /// hour has run out — or, for a session a caller has terminated, `stopping`
    /// until the close is through and `stopped` after it, because the close is
    /// what the session's status is about from the instant it was asked for.
    pub fn status_at(&self, now_ms: i64) -> &'static str {
        if let Some(stop) = self.stop {
            if stop.is_complete_at(now_ms) {
                GUI_SESSION_STATUS_STOPPED
            } else {
                GUI_SESSION_STATUS_STOPPING
            }
        } else if now_ms < self.starts_at_ms {
            GUI_SESSION_STATUS_STARTING
        } else if now_ms < self.expires_at_ms() {
            GUI_SESSION_STATUS_STARTED
        } else {
            GUI_SESSION_STATUS_START_EXPIRED
        }
    }

    /// `percentageComplete` for this session at `now_ms`: how far it is through
    /// coming up, and 100 once it is up — the start it reports the completion of
    /// is over, whether the session is still serving or its hour has run out. For
    /// a session a caller has terminated it is how far that close has got, which
    /// is the work the call reporting it started.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        if let Some(stop) = self.stop {
            stop.percentage_complete_at(now_ms)
        } else if now_ms < self.starts_at_ms {
            elapsed_percentage(self.created_at_ms, self.starts_at_ms, now_ms)
        } else {
            100
        }
    }

    /// The pair of modelled `Session`s this access carries for the virtual
    /// computer `resource_name`: the primary (application) session first, then
    /// the secondary (operating system) one.
    ///
    /// A request that names no host at all names no endpoint to announce, so
    /// `url` is omitted rather than filled with an address nothing serves — the
    /// same rule a container service's `url` follows.
    pub fn views(&self, resource_name: &str, endpoint: Option<&str>) -> Vec<crate::ops::types::Session> {
        [
            (GUI_SESSION_NAME_APPLICATION, true),
            (GUI_SESSION_NAME_OPERATING_SYSTEM, false),
        ]
        .into_iter()
        .map(|(name, is_primary)| crate::ops::types::Session {
            name: Some(name.to_string()),
            url: endpoint
                .map(|endpoint| gui_session_url(endpoint, resource_name, &self.token, name)),
            is_primary: Some(is_primary),
        })
        .collect()
    }
}
